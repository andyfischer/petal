# Petal IR as an Emit Target

Petal's dataflow IR is a **stable, documented target that other front-ends can
compile into**. Any tool that emits valid Petal IR JSON gets a byte-for-byte
normal `Program`, and so inherits everything the IR was built to provide:

- **Provenance** — backward "what influenced this?" (`trace_provenance`)
- **Slicing / projection** — minimal subgraph for a target (`slice`)
- **`ExplainTerm`** — causal, value-annotated backward walk
- **State-preserving live editing** — hot reload via inline state keys
- **AI-legibility** — an agent can emit IR directly and reason over the
  resulting dataflow graph, or read provenance back out of one it didn't write

The shared substrate is *computational structure* (dataflow, control flow,
state), which the IR encodes independent of any surface syntax.

## Usage

```bash
petal show-ir --json -e 'print(1 + 2)' | petal run --ir -   # => 3
petal run --ir path/to/program.ir.json
petal run --ir -                                            # read IR JSON from stdin
```

`petal show-ir --json` serializes a compiled `Program` to JSON IR
(`rust/src/ir_serialize.rs`). `petal run --ir <file>` (or `-` for stdin) is the
inverse: it deserializes the JSON back into a `Program`, validates it, and
evaluates it on the bytecode VM.

## How loading works

`run --ir` goes JSON → `Program` → evaluate:

- **Deserialize** via `Program::from_json` (`rust/src/ir_validate.rs`) and
  `Env::load_program_ir` (`rust/src/env/mod.rs`). The IR types carry `Deserialize`
  derives; the loader then normalizes the wire form back into the in-memory
  one: it rebuilds the intra-block execution linked list from each block's
  ordered `terms` array, reconstructs the `#[serde(skip)]` indexes
  (`block_terms`, constant dedup), and — when the document omits registers —
  recomputes the whole register assignment, so a loaded `Program` is
  identical to a compiled one.
- **Validate** — `Program::validate` runs before evaluation and rejects invalid
  graphs with actionable messages (see [Validation invariants](#validation-invariants)).
- **Evaluate** on the bytecode VM, exactly as a compiled program.

**Round-trip guarantee:** `show-ir --json <src> | run --ir -` produces the same
result as `run <src>`, verified across the snippet matrix in
`ts/test/ir-roundtrip.test.ts`.

## Emitting IR from a foreign front-end

`ts/tools/calc-to-ir.ts` is a worked example: a self-contained front-end for a
toy "calc" language (`let`/`print` + integer arithmetic with precedence, parens,
and unary minus). It shares **no** code with Petal's lexer/parser/compiler — its
only contract is the schema below — and emits Petal IR JSON straight from its
own AST:

```bash
echo 'print 1 + 2 * 3' | tsx ts/tools/calc-to-ir.ts | petal run --ir -   # => 7
```

It demonstrates the key conventions a foreign front-end must honour: constants
go in the constant table and are referenced by `Constant` index; builtins (here
`print`) are called through `BuiltinCall` with the builtin's *name* as a
string constant — no phantom terms, no dependence on Petal's builtin
registration order; execution order is the block's declarative `terms` array;
registers are omitted entirely (the loader recomputes them); `let` bindings
emit a named `Copy` so the value stays legible in the dataflow graph.

`ts/test/calc-emitter.test.ts` runs the emitted IR through `run --ir`,
cross-checks its output against the real Petal compiler for the equivalent
source, and confirms the validator rejects a tampered graph. Golden IR fixtures
live in `ts/test/fixtures/ir/` (`print_arith`, `branch_phi`, `state_counter`).

## Schema

This is the import contract. It is derived from the live types in
`rust/src/program.rs`, `rust/src/constant_table.rs`, and `rust/src/ast.rs`, and
matches `petal show-ir --json` output (the serde derive). The current shape is
**schema 0.2**, declared by a top-level `"schema": "0.2"` field. The loader
accepts documents with no `schema` field at all — the pre-0.2 "v0"/"v0.1"
shapes, see [Legacy (v0) documents](#legacy-v0-documents) — and rejects any
other value. Unknown fields are ignored. The only addition a *loader*
introduces over the raw `show-ir` dump is the validation pass.

### Encoding conventions

- **IDs are bare integers.** `TermId`, `BlockId`, `ConstantId`, `FunctionId`
  are newtype wrappers over `u32` and serialize transparently as numbers;
  `StateKey` is a `u64`. A term's position in the `terms` array equals its
  `id` (`terms[i].id == i`); same for `blocks` and `functions`.
- **Ops use serde external tagging.** A unit variant is a bare string
  (`"op": "Add"`). A data-carrying variant is a single-key object:
  - newtype: `"op": {"Constant": 12}`, `"op": {"BuiltinCall": 6}`,
    `"op": {"MakeClosure": 0}`, `"op": {"MakeEnumVariant": 7}`
  - struct: `"op": {"MethodCall": {"name": 5}}`,
    `"op": {"AllocMap": {"fields": [3, 4]}}`,
    `"op": {"AllocElement": {"tag": 2, "prop_keys": [3]}}`,
    `"op": {"AllocMapSpread": {"entries": [{"Spread": 0}, {"Named": [4, 1]}]}}`
- **`inputs` are dataflow edges** — an ordered list of `TermId`s whose values
  feed this term.
- **Defaults are omitted on the wire.** Every field whose value is its default
  — `null` options (`name`, `state_key`, `parent_term_id`, `self_ref_register`,
  `hint`, …), empty arrays (`inputs`, `child_blocks`, `param_names`,
  `capture_names`, `functions`, …), `false` booleans (`has_errors`, `in_loop`,
  `collect`), empty strings/maps (`source`, `source_map`, `match_arms`) — is
  simply absent. Loaders must treat absence as the default; emitters may write
  the explicit default value, which loads identically.
- **Execution order is declarative**: each block carries an ordered `terms`
  array of `TermId`s. There is no linked list on the wire (the in-memory
  `entry`/`block_next`/`block_prev` links are rebuilt from `terms` on load).
- **Registers are optional.** If *any* term omits `register`, the loader
  recomputes the entire register assignment (and `register_count`,
  `capture_registers`, `self_ref_register`) from the graph. `show-ir --json`
  always emits them; a foreign emitter should just leave them out.
- **Spans are compact arrays** —
  `[startLine, startCol, startOffset, endLine, endCol, endOffset]`, with a
  seventh element (the file-table index) appended only when nonzero. Shared
  with the AST JSON dump. The verbose v0 object form
  (`{"start": {"line", "column", "offset"}, "end": {…}, "file"?}`) is still
  accepted on input.

### Program

```
{
  "schema": "0.2",
  "id": 0,
  "source": "...",                 // optional; omitted when empty
  "terms":   [ Term, ... ],
  "blocks":  [ Block, ... ],
  "root_block": 0,                 // BlockId of the entry block
  "constants": { "values": [ ConstantValue, ... ] },
  "functions": [ FunctionDef, ... ],          // omitted when empty
  "match_arms": { "<termId>": [ MatchArm, ... ] },  // omitted when empty
  "source_map": { ... }            // optional; omitted when empty
}
```

(`has_errors: true` marks a compile that failed; a valid import must omit it
or set it `false`.)

**Module system:** programs compiled from more than one file carry a file
table and file-tagged spans (see docs/module-system.md):

- `source_map.files`: `[{ "name": "main.ptl", "source": "...",
  "origin": "/abs/path.ptl"? }, ...]` — entry file at index 0, imported
  modules at 1..N. Omitted entirely for single-file programs.
- Every span's optional 7th element is the file index (omitted when 0/entry).
  Line/column stay local to that file's source — each module is lexed
  independently.

### Term

```
{
  "id": 7,
  "op": <Op>,
  "inputs": [TermId, ...],         // omitted when empty
  "block_id": 0,
  "name": "x",                     // user-visible binding name; omitted if none
  "register": 4,                   // optional — omit and the loader reassigns
  "state_key": 1234,               // required for State* ops, else omitted
  "child_blocks": [BlockId, ...],  // omitted when empty
  "in_loop": true                  // omitted when false
}
```

`block_id` is required and must agree with the block whose `terms` array
lists the term (the redundancy is deliberate: it makes single-term lookups
O(1) and lets the validator catch listing mistakes).

### Binding phantoms and builtins

A `Copy` term with **no inputs and a `name`** is a *binding phantom*: it
executes nothing and only names a register. Binding phantoms must **not**
appear in any block's `terms` array. They are used for:

- **Function params** — one phantom per `param_names` entry in the body
  block. Required (the loader seats them at registers `0..N-1` in parameter
  order when recomputing registers).
- **Captures and self-references** — one phantom per capture name, and one
  named after the function for recursion, in the body block. Required when
  the function has captures / recurses by name.
- **Builtins as first-class values** — a phantom whose name resolves in the
  native-function table (e.g. `"map"`) is seeded with that native's function
  value when the root frame is pushed. Matching is **by name**: emit one
  phantom per builtin you need as a *value* (to pass around, `Call` through a
  variable, …), at any term id and any register. There is no dependence on
  Petal's builtin registration order, and no phantom is needed at all for a
  direct call — use `BuiltinCall` with the builtin's name as a string
  constant instead.

`show-ir --json` on a compiled program still emits one leading phantom per
registered native (the compiler creates them for name resolution); a foreign
emitter should not imitate that.

### `TermOp` table (arities & child blocks)

`inputs` = required input count; `child` = required `child_blocks` count.
"data" = the value carried in the tagged op object.

| Op | inputs | child | data | Notes |
|---|---|---|---|---|
| `Constant` | 0 | 0 | `ConstantId` | literal from the constant table |
| `Error` | 0 | 0 | `ConstantId` | parse-error marker — **invalid in an import** (see `has_errors`) |
| `Add` `Sub` `Mul` `Div` `Mod` | 2 | 0 | — | binary arithmetic |
| `Neg` | 1 | 0 | — | unary negate |
| `Eq` `Ne` `Lt` `Le` `Gt` `Ge` | 2 | 0 | — | comparison |
| `Not` | 1 | 0 | — | logical not |
| `And` `Or` | 1 | 1 | — | short-circuit; `inputs=[left]`, `child_blocks=[rhs_block]` |
| `Concat` | ≥1 | 0 | — | string concat / interpolation parts |
| `Coalesce` | 1 | 1 | — | `??`; `inputs=[left]`, `child_blocks=[rhs_block]`, yields RHS when left is Nil/Pending |
| `Copy` | 1 | 0 | — | identity / variable reference. **Special case:** a `Copy` with no inputs and a `name` is a *binding phantom* — unlisted, see [Binding phantoms and builtins](#binding-phantoms-and-builtins) |
| `Phi` | 1 | 0 | — | join point; `inputs=[pre_control_flow_value]`. Must precede its control-flow term in the same block (see Phi rules) |
| `Branch` | 1 | 2 | — | `inputs=[cond]`, `child_blocks=[then, else]` |
| `ForLoop` | 1 | 1 | — | `inputs=[iterable]`, `child_blocks=[body]` |
| `NumericForLoop` | 2 | 1 | — | non-allocating integer range loop; `inputs=[start, end]` (both Int-valued), `child_blocks=[body]`. Iterates `start..end` (step 1) binding the loop var per iteration without materializing a list. Compiler emits this for `for x in range(a, b)` |
| `WhileLoop` | 0 | 2 | — | `child_blocks=[cond_block, body_block]` |
| `Break` `Continue` | 0 | 0 | — | loop control |
| `Return` | 0 or 1 | 0 | — | `inputs=[value]`, or empty for bare return |
| `MakeClosure` | = `capture_names.len()` | 0 | `FunctionId` | inputs are captured values, in capture order |
| `MakeOverloadSet` | ≥1 | 0 | — | inputs are closure terms, one per arity |
| `Call` | ≥1 | 0 | — | `inputs=[callable, arg0, ...]` |
| `MethodCall` | ≥1 | 0 | `{name: ConstantId, hint?: ConstantId}` | `inputs=[object, arg0, ...]`; `hint` (omitted when absent) names a class for live-edit dispatch |
| `BuiltinCall` | ≥0 | 0 | `ConstantId` (builtin name, a String constant) | direct builtin call, resolved by name at lower time; `inputs=[arg0, ...]` — **the way an emitter calls a builtin** |
| `StateInit` | 0 or 1 | 1 | — | `state_key` required; init expression in `child_blocks=[init_block]` (lazy); optional `inputs=[explicit_key]` for `state(expr) name` |
| `StateRead` | 0 | 0 | — | `state_key` required |
| `StateWrite` | 1 or 2 | 0 | — | `inputs=[value]` or `[value, explicit_key]`, `state_key` required |
| `CellNew` | 1 | 0 | — | allocate the cell behind a `var`; `inputs=[init]` |
| `CellRead` | 1 | 0 | — | dereference a cell; `inputs=[cell]` |
| `CellWrite` | 2 | 0 | — | write through a cell (`set x = …`); `inputs=[cell, value]` |
| `AllocList` | ≥0 | 0 | — | inputs are elements |
| `AllocMap` | = `fields.len()` | 0 | `{fields: [ConstantId], class?: ConstantId}` | inputs are field values, aligned to `fields`; `class` (omitted when absent) tags a class constructor's record |
| `AllocMapSpread` | varies | 0 | `{entries: [Spread(i) \| Named([cid, i])]}` | entries index into `inputs`; spreads then named values |
| `GetField` `GetFieldOpt` | 1 | 0 | `ConstantId` (field) | `inputs=[object]`; the `Opt` form yields Nil for a missing field/Nil object |
| `SetField` | 2 | 0 | `ConstantId` (field) | `inputs=[object, value]` |
| `GetIndex` `GetIndexOpt` | 2 | 0 | — | `inputs=[object, index]`; the `Opt` form yields Nil for a missing key/Nil object |
| `SetIndex` | 3 | 0 | — | `inputs=[object, index, value]` |
| `AllocElement` | = `prop_keys.len()` + #children | 0 | `{tag: ConstantId, prop_keys: [ConstantId]}` | first `prop_keys.len()` inputs are prop values, the rest are children |
| `MakeEnumVariant` | ≥0 | 0 | `ConstantId` (variant name) | inputs are field values |
| `Match` | 1 | = #arms | — | `inputs=[subject]`, `child_blocks` are arm body blocks; arm metadata in `match_arms[termId]` |

### Block

```
{
  "id": 0,
  "parent_term_id": 5,             // the control-flow term owning this block; omitted for root
  "terms": [TermId, ...],          // the block's terms in execution order; omitted when empty
  "param_names": ["x", ...],       // for fn bodies and for-loop bodies; omitted when empty
  "register_count": 6,             // optional — loader recomputes/fills it
  "phi_outs": [ {"src_term": 9, "dest_term": 4}, ... ]  // omitted when empty
}
```

`terms` is the wire form of intra-block execution order. Every executed term
appears in exactly one block's `terms` array, and its `block_id` must name
that block. Binding phantoms (params, captures, self-refs, builtins-as-values)
are deliberately *not* listed — they don't execute.

`phi_outs` is the loop-carry / branch-rebind mechanism: when this child block's
frame pops, each `src_term`'s value is copied into the parent frame at
`dest_term`'s register. `dest_term` must be a `Phi` in the parent block.

### Constants, functions, match arms, patterns

```
ConstantValue := "Nil"
               | {"Bool": true}
               | {"Int": 42}
               | {"Float": <u64 bits of the f64>}   // NB: raw IEEE-754 bits, not the number
               | {"String": "hi"}

FunctionDef := {
  "id": 0, "name": "adder",        // name omitted for lambdas
  "params": ["x"],                 // omitted when empty
  "body_block": 3,
  "capture_names": ["n"],          // omitted when empty
  "capture_registers": [2],        // register fields all optional — the loader
  "self_ref_register": 1,          //   re-derives them from the body block's
  "register_count": 4              //   binding phantoms when registers are omitted
}

MatchArm := { "pattern": Pattern, "guard_block": BlockId?, "body_block": BlockId }

Pattern := "Wildcard"
         | {"Literal": <Literal>}
         | {"Variable": "x"}
         | {"Variant": {"name": "Circle", "fields": [Pattern, ...]}}
         | {"List": {"elements": [Pattern, ...], "rest": "tail"}}
         | {"Record": [["field", Pattern], ...]}
```

`Variant.fields` and `List.rest` follow the omit-defaults rule: dumps skip an
empty `fields` array and an absent `rest`, and the loader accepts the explicit
spellings (`"fields": []`, `"rest": null`) as well.

`Float` constants are stored as the `u64` bit pattern of the `f64`
(`f64::to_bits`), for hashable dedup. An emitter must bit-encode floats; a
reader must `from_bits` them.

### Validation invariants

A program is a valid import iff:

1. **Referential integrity** — every `TermId` in any `inputs`/`phi_outs`/
   `child_blocks`/`root_block`/block `terms` references an existing
   term/block; every `ConstantId`/`FunctionId` resolves in its table.
2. **No errors** — `has_errors` absent/false and no `Error` terms.
3. **Arity** — each term satisfies its row in the op table (input count, child
   count, and the data shape `AllocMap`/`AllocElement`/`AllocMapSpread` imply).
4. **Acyclic dataflow** — `inputs` edges form a DAG. The *only* legal backward
   reference is loop-carry expressed through a `Phi` + the body block's
   `phi_outs`; a raw `inputs` cycle is rejected.
5. **Block consistency** — every term listed in a block's `terms` array has a
   matching `block_id`; no term is listed in more than one block (or twice); a
   non-root block's `parent_term_id` points to a control-flow term whose
   `child_blocks` include this block.
6. **Phantoms are unlisted** — a `Copy` on a block's `terms` list takes
   exactly one input; a no-input named `Copy` (binding phantom) must not be
   listed.
7. **Phi placement** — every `Phi` sits in the parent block *before* its
   control-flow term; every `phi_outs.dest_term` is a `Phi` in the parent block.
8. **State integrity** — every `StateRead`/`StateWrite` `state_key` has a
   matching `StateInit` with the same key; `state_key` is present exactly for
   `State*` ops. (This is the same invariant the compiler-side state-correctness
   audit enforces.)
9. **Registers** (if provided) — every term's register is
   `< register_count` of its block. If any register is omitted, the loader
   assigns the whole file itself — which requires the binding phantoms for
   every `param_names` entry, capture, and by-name self-reference to exist
   (that is how the recompute knows which registers must be seated first).

`register`, `register_count`, `capture_registers`, `self_ref_register`,
`source`, and `source_map` are **optional** for an importer — the loader
synthesizes registers from the graph and defaults the source metadata.
Everything else is required (modulo the omit-defaults rule above).

### Legacy (v0) documents

Documents produced before schema 0.2 carry no `schema` field, explicit
`null`/empty defaults, nested-object spans, an `entry`/`block_next`/
`block_prev` linked list instead of block `terms` arrays, and leading builtin
phantom terms in registration order. The loader still accepts all of that:

- Absence of `schema` selects legacy tolerance; `"schema": "0.2"` is the
  only other accepted value.
- A block with no `terms` array but an `entry` is walked via the terms'
  `block_next` links to reconstruct the array; both shapes load to the same
  in-memory `Program`. (When a `terms` array *is* present it wins, and the
  links are rebuilt from it.)
- Verbose span objects deserialize alongside compact arrays.
- Builtin phantoms are matched by name when the root frame is pushed, so
  their ids/positions/registration order no longer matter; a phantom whose
  name is not a registered native is simply left unseeded (it reads as Nil).

New emitters should target schema 0.2 and none of the legacy forms.

## Scope

The emit target is a load-and-run contract for *computational structure*, not a
language-interop layer. Deliberately outside it:

- **Importing arbitrary existing languages** (JS/C/Python). The `calc` emitter
  is only a reference; building real foreign front-ends is downstream work that
  lives against this contract, not inside it.
- **Bidirectional editing** — mapping edits on a projection back to foreign
  source. See Goal 3 in [goals.md](goals.md).
- **A binary/compact IR format.** The contract is JSON; a denser encoding would
  only be worth adding if a real emitter needs it.
