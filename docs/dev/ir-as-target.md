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
  derives; `rebuild_indexes` reconstructs the `#[serde(skip)]` indexes
  (`block_terms`, constant dedup) so a loaded `Program` is identical to a
  compiled one.
- **Validate** — `Program::validate` runs before evaluation and rejects invalid
  graphs with actionable messages (see [Validation invariants](#validation-invariants)).
- **Evaluate** on the bytecode VM, exactly as a compiled program.

**Round-trip guarantee:** `show-ir --json <src> | run --ir -` produces the same
result as `run <src>`, verified across the snippet matrix and the full
`examples/*.ptl` corpus in `ts/test/ir-roundtrip.test.ts`.

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
`print`) are emitted as leading phantom `Copy` terms *before* the block's
`entry`, where the evaluator binds them to their native functions; real terms
form the block's `block_next`/`block_prev` linked list; `let` bindings emit a
named `Copy` so the value stays legible in the dataflow graph.

`ts/test/calc-emitter.test.ts` runs the emitted IR through `run --ir`,
cross-checks its output against the real Petal compiler for the equivalent
source, and confirms the validator rejects a tampered graph. Golden IR fixtures
live in `ts/test/fixtures/ir/` (`print_arith`, `branch_phi`, `state_counter`).

## Schema

This is the import contract. It is derived from the live types in
`rust/src/program.rs`, `rust/src/constant_table.rs`, and `rust/src/ast.rs`, and
matches `petal show-ir --json` output (the serde derive). This document refers
to the current shape as schema v0 (v0.1 with the module-system additions below);
there is no `schema_version` field in the JSON — the loader neither requires nor
emits one, and unknown fields are ignored. The only addition a *loader*
introduces over the raw `show-ir` dump is the validation pass.

### Encoding conventions

- **IDs are bare integers.** `TermId`, `BlockId`, `ConstantId`, `FunctionId`
  are newtype wrappers over `u32` and serialize transparently as numbers;
  `StateKey` is a `u64`. A term's position in the `terms` array equals its
  `id` (`terms[i].id == i`); same for `blocks` and `functions`.
- **Ops use serde external tagging.** A unit variant is a bare string
  (`"op": "Add"`). A data-carrying variant is a single-key object:
  - newtype: `"op": {"Constant": 12}`, `"op": {"MethodCall": 5}`,
    `"op": {"MakeClosure": 0}`, `"op": {"MakeEnumVariant": 7}`
  - struct: `"op": {"AllocMap": {"fields": [3, 4]}}`,
    `"op": {"AllocElement": {"tag": 2, "prop_keys": [3]}}`,
    `"op": {"AllocMapSpread": {"entries": [{"Spread": 0}, {"Named": [4, 1]}]}}`
- **`inputs` are dataflow edges** — an ordered list of `TermId`s whose values
  feed this term.
- Fields omitted when empty/false: `in_loop` (term), `phi_outs` (block).

### Program

```
{
  "id": 0,
  "source": "...",                 // optional for imports; "" is fine
  "terms":   [ Term, ... ],
  "blocks":  [ Block, ... ],
  "root_block": 0,                 // BlockId of the entry block
  "constants": { "values": [ ConstantValue, ... ] },
  "functions": [ FunctionDef, ... ],
  "match_arms": { "<termId>": [ MatchArm, ... ] },
  "has_errors": false,             // must be false for a valid import
  "source_map": { ... }            // optional for imports
}
```

**Schema v0.1 (module system):** programs compiled from more than one file
carry a file table and file-tagged spans (see docs/module-system.md):

- `source_map.files`: `[{ "name": "main.ptl", "source": "...",
  "origin": "/abs/path.ptl"? }, ...]` — entry file at index 0, imported
  modules at 1..N. Omitted entirely for single-file programs, so their v0
  shape is unchanged.
- Every `SourceSpan` gains an optional `"file": <index>` field (omitted when
  0/entry). Line/column stay local to that file's source — each module is
  lexed independently.

Both additions are `#[serde(default)]`, so v0 documents load unchanged.

### Term

```
{
  "id": 7,
  "op": <Op>,
  "inputs": [TermId, ...],
  "block_id": 0,
  "block_next": 8 | null,          // intra-block linked list
  "block_prev": 6 | null,
  "name": "x" | null,              // user-visible binding name, if any
  "register": 4,                   // optional for imports — loader can reassign
  "state_key": 1234 | null,        // required for State* ops, else null
  "child_blocks": [BlockId, ...],
  "in_loop": false                 // omitted when false
}
```

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
| `Copy` | 1 | 0 | — | identity / variable reference. **Special case:** a `Copy` with `inputs=[]` and `name` set is a *phantom builtin* binding (e.g. `print`, `range`) |
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
| `MethodCall` | ≥1 | 0 | `ConstantId` (method name) | `inputs=[object, arg0, ...]` |
| `StateInit` | 1 | 0 | — | `inputs=[init_value]`, `state_key` required |
| `StateRead` | 0 | 0 | — | `state_key` required |
| `StateWrite` | 1 | 0 | — | `inputs=[value]`, `state_key` required |
| `AllocList` | ≥0 | 0 | — | inputs are elements |
| `AllocMap` | = `fields.len()` | 0 | `{fields: [ConstantId]}` | inputs are field values, aligned to `fields` |
| `AllocMapSpread` | varies | 0 | `{entries: [Spread(i) \| Named([cid, i])]}` | entries index into `inputs`; spreads then named values |
| `GetField` | 1 | 0 | `ConstantId` (field) | `inputs=[object]` |
| `SetField` | 2 | 0 | `ConstantId` (field) | `inputs=[object, value]` |
| `GetIndex` | 2 | 0 | — | `inputs=[object, index]` |
| `SetIndex` | 3 | 0 | — | `inputs=[object, index, value]` |
| `AllocElement` | = `prop_keys.len()` + #children | 0 | `{tag: ConstantId, prop_keys: [ConstantId]}` | first `prop_keys.len()` inputs are prop values, the rest are children |
| `MakeEnumVariant` | ≥0 | 0 | `ConstantId` (variant name) | inputs are field values |
| `Match` | 1 | = #arms | — | `inputs=[subject]`, `child_blocks` are arm body blocks; arm metadata in `match_arms[termId]` |

### Block

```
{
  "id": 0,
  "parent_term_id": 5 | null,      // the control-flow term owning this block; null for root
  "entry": 0 | null,               // first TermId in the block; null if empty
  "param_names": ["x", ...],       // for fn bodies and for-loop bodies
  "register_count": 6,             // optional for imports — loader can recompute
  "phi_outs": [ {"src_term": 9, "dest_term": 4}, ... ]  // omitted when empty
}
```

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
  "id": 0, "name": "adder" | null, "params": ["x"],
  "body_block": 3, "capture_names": ["n"],
  "capture_registers": [2], "self_ref_register": 1 | null,
  "register_count": 4
}

MatchArm := { "pattern": Pattern, "guard_block": BlockId | null, "body_block": BlockId }

Pattern := "Wildcard"
         | {"Literal": <Literal>}
         | {"Variable": "x"}
         | {"Variant": {"name": "Circle", "fields": [Pattern, ...]}}
         | {"List": {"elements": [Pattern, ...], "rest": "tail" | null}}
         | {"Record": [["field", Pattern], ...]}
```

`Float` constants are stored as the `u64` bit pattern of the `f64`
(`f64::to_bits`), for hashable dedup. An emitter must bit-encode floats; a
reader must `from_bits` them.

### Validation invariants

A program is a valid import iff:

1. **Referential integrity** — every `TermId` in any `inputs`/`phi_outs`/
   `child_blocks`/`root_block`/`entry` references an existing term/block; every
   `ConstantId`/`FunctionId` resolves in its table.
2. **No errors** — `has_errors == false` and no `Error` terms.
3. **Arity** — each term satisfies its row in the op table (input count, child
   count, and the data shape `AllocMap`/`AllocElement`/`AllocMapSpread` imply).
4. **Acyclic dataflow** — `inputs` edges form a DAG. The *only* legal backward
   reference is loop-carry expressed through a `Phi` + the body block's
   `phi_outs`; a raw `inputs` cycle is rejected.
5. **Block consistency** — every term's `block_id` matches the block that lists
   it; `entry`/`block_next`/`block_prev` form one consistent linked list per
   block; a non-root block's `parent_term_id` points to a control-flow term
   whose `child_blocks` include this block.
6. **Phi placement** — every `Phi` sits in the parent block *before* its
   control-flow term; every `phi_outs.dest_term` is a `Phi` in the parent block.
7. **State integrity** — every `StateRead`/`StateWrite` `state_key` has a
   matching `StateInit` with the same key; `state_key` is non-null exactly for
   `State*` ops. (This is the same invariant the compiler-side state-correctness
   audit enforces.)
8. **Registers** (if provided) — every register index used in a block is
   `< register_count`. If omitted, the loader assigns registers itself.

`register`, `register_count`, `source`, and `source_map` are **optional** for
an importer — the loader can synthesize registers from the dataflow graph and
default the source metadata. Everything else is required.

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
