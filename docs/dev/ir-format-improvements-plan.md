# Intermediate Format Improvements Plan

An audit of Petal's four intermediate-format dumps — tokens, AST, IR term
graph, bytecode — each in text and `--json` form, with improvements ranked by
impact. Ran the tooling (`show-tokens` / `show-ast` / `show-ir` /
`show-bytecode`, plus the MCP `Show*` tools) over snippets covering functions,
closures, pipelines, string interpolation, if/else rebinding (phi), `for`
loops, `var`/`set` cells, `state`, and `match` with guards.

The language is pre-release, so format breaks are acceptable. The one format
with an external contract is the IR JSON (`docs/dev/ir-as-target.md` — the
foreign-front-end emit target, `run --ir`, and the golden fixtures in
`ts/test/fixtures/ir/`); changes there carry the largest blast radius and are
sequenced last.

## Current state, format by format

### Tokens (`show-tokens`, lexer.rs)

- **No source positions in either output**, even though the lexer already
  records a span for every token (`Lexer::token_spans`). Every mainstream
  token dump (clang, rustc) prints locations; ours can't answer "which `Ident`
  is this?".
- JSON mixes bare strings and single-key objects (`"Let"`, `{"Ident": "x"}`) —
  raw serde external tagging. Consumers need two code paths; a uniform
  `{kind, value?, span}` row is the industry norm.
- Text form (`3: Int(1)`) is serviceable but minimal.

### AST (`show-ast`, ast.rs)

- **Text form is raw Rust `{:#?}` debug output** — ~100 lines for
  `let x = 1 + 2.5`, most of it `SourcePosition { line: 1, column: 9, offset: 8 }`
  scaffolding. By far the least readable of the four dumps. Compare
  `swiftc -dump-ast` / `clang -ast-dump`: one node per line, key facts inline,
  compact `1:9-1:16` spans, children indented.
- JSON is fine structurally (documented in CLI.md) but noisy: every node
  carries `"resolved": null`, `"exported": false`, `"is_var": false`,
  `"is_config": false`, `"ty": null`, and a 10-line two-endpoint span object.
  Defaults should be omitted (`skip_serializing_if`), spans compacted.

### IR term graph (`show-ir`, ir_display.rs / serde on program.rs)

Text form has good bones — stable `t`/`r`/`c`/`fn`/`block` prefixes, `; name`
binding comments, `-> blockN` child links. Problems, in decreasing severity:

- **The prelude floods every dump.** `std.ptl` contributes ~20 functions and
  ~38 blocks, plus `Rect` machinery, to a hello-world listing. Phantom builtin
  `Copy` terms are already hidden (`is_phantom`), but prelude functions,
  blocks, and their constants are not. The JSON form is worse: it always
  includes the 117 phantom terms *and* the prelude — the MCP `ShowIR` tool
  returns ~2,000 lines for a 6-line program, ~95% noise. This directly
  undercuts the AI-legibility goal.
- **Hidden terms leave dangling references.** Block params and the self-ref
  binding are phantom `Copy`s, so the text form prints `t119 r2 = Copy [t117]`
  where `t117` appears nowhere. Nothing ties `params=["x"]` to `t117`/`r0`.
- **The phi mechanism is invisible.** `Block.phi_outs` — the entire
  branch-rebind / loop-carry story — is never printed. A reader sees
  `t120 = Phi [t116]` and rebindings in child blocks but not the edge that
  connects them.
- **Match arms are opaque.** `Match [t117] -> block1, block2, block4` shows
  neither patterns nor guards; the guard block (block3) prints as an unlabeled
  sibling. `Program.match_arms` never reaches the text form.
- **Constant cross-referencing.** `BuiltinCall(c3)` forces a lookup to learn
  it's `map`; same for `GetField(c2)`, `MethodCall(c5)`, `Constant(c0)`.
  The bytecode disassembler already solves this (`kconst` → `builtin "map"`);
  the IR printer should adopt the same convention.
- **State ops don't show their keys.** `StateWrite [t342]` doesn't say which
  state it writes or link to its `StateInit`.
- No source-line annotations anywhere in the text form.

JSON-specific issues (beyond phantom/prelude inclusion):

- **In-memory details leak into the interchange format.** `block_next` /
  `block_prev` is a hand-maintained doubly-linked list that foreign emitters
  must get exactly right, when an ordered per-block term array says the same
  thing declaratively. `register` / `register_count` are lowering outputs the
  loader can (and per the docs, may) recompute. The emit-target contract even
  requires foreign front-ends to emit phantom builtin `Copy` terms in
  registration order — an internal coupling (`builtins/mod.rs` "append only")
  exported to third parties.
- **Null/default noise.** Every term prints `"name": null`,
  `"state_key": null`, `"block_next": null`, empty `inputs`/`child_blocks`
  arrays; `collect`/`path_pop`/`phi_outs` already skip-if-default, the rest
  should too.
- `source_map.term_spans` is keyed by stringified TermIds with two nested
  6-field endpoint objects per span — the bulkiest encoding available.
- `match_arms` embeds `ast::Pattern` — an AST type — inside the IR, the one
  place the "IR is independent of surface syntax" claim breaks.

### Bytecode (`show-bytecode`, backend/bytecode/disasm.rs)

The best of the four. Text form is compact, resolves constants inline, uses
infix rendering (`r4 = r0 * r0`), labels params/captures/self. Remaining gaps:

- **JSON form is not structured** — each instruction is
  `{"ip": 0, "text": "r117 = closure f0 caps=[]"}`, i.e. prerendered strings.
  Fine for reading, useless for tooling; there's no operand-level encoding.
- Cosmetics: `builtin "map"[r121, r117]` (missing space), jump targets are raw
  indices with no visual anchor on the target line.

### Cross-cutting

Four dumps, four conventions: tokens use raw serde enums with no spans, AST
uses `{kind, span}` wrappers with verbose spans, IR uses a full struct dump
with stringified-id maps, bytecode uses rendered text in JSON. Span encoding,
default-omission, and id rendering should be uniform and documented once.

## Ranked improvements

### 1. Hide the prelude (and phantoms) from IR dumps by default

*Impact: highest — every `show-ir` invocation, human or agent. Effort: low.
Risk: low (display-only).*

- Text: in addition to phantom `Copy`s, hide functions/blocks/terms/constants
  whose spans resolve to a non-entry file (`source_map` file index ≠ 0 —
  the prelude and imports are separate files already). `--all` restores
  everything, as today.
- MCP `ShowIR` currently shells `show-ir --json` and returns the full program.
  Switch it to the filtered form (filtered JSON or the text form) so an agent
  sees the ~15 relevant terms, not 2,000 lines. Add an `all` parameter for the
  rare full-graph question.
- `show-ir --json` on the CLI stays complete by default — it is the executable
  interchange (`run --ir` round-trip). Add `--user-only` there if wanted, but
  don't change its default.

### 2. Replace the text AST printer with a compact tree

*Impact: high — the current form is effectively unusable. Effort: low-medium
(one new printer, ~150 lines). Risk: none (debug output only).*

Clang/swiftc-style: one node per line, kind + key facts inline, children
indented, spans as `@1:9-1:16`:

```
FnDecl square (x: number) -> number @1:1-3:4
  Expr @2:3-2:8
    BinaryOp Mul @2:3-2:8
      Ident x @2:3
      Ident x @2:7
Let xs @5:1-5:34
  Call @5:20-5:34
    Ident map @5:23
    List @5:10-5:19
      Literal 1 @5:11 · Literal 2 @5:14 · Literal 3 @5:17
    Ident square @5:27
```

(Exact layout to taste; the point is one line per node and compact spans.)

### 3. Make the text IR self-contained

*Impact: high — removes all cross-referencing while reading. Effort: medium.
Risk: low (display-only, plus any snapshot tests on the text form).*

- Resolve constants inline, adopting disasm's convention:
  `Constant(1)`, `BuiltinCall("map")`, `GetField(.x)`, `MethodCall(.dist2)`,
  `MakeEnumVariant(Circle)`, `AllocMap{x: t3, y: t4}`.
- Print match-arm metadata under the `Match` term, and label guard blocks:

  ```
  t130 r118 = Match [t117] ; msg
       arm0: when 0            -> block1
       arm1: when n if block3  -> block2
       arm2: when _            -> block4
  ```

- Print `phi_outs` as a block footer, naming the target:
  `phi-out: t124 -> t120 (x)`.
- Show state identity on `StateInit`/`StateRead`/`StateWrite`
  (`StateWrite(count)` or `k=0x94ab…`), so writes link to their init.
- List block params/captures/self with their term ids and registers in the
  block header (`block1 params: x=t117:r0  self: square=t118:r1`) so no
  visible term references an invisible one.
- Optional `--lines` (or on-by-default) source annotation per term: `@5:11`.
- Consider ordering the block listing as a tree (children after their parent,
  indented one level, MLIR-region style) instead of flat by id. Function
  bodies stay top-level sections.

### 4. Slim and de-internalize the IR JSON (schema v0.2)

*Impact: high for the emit-target story and dump size; medium for day-to-day
reading (rank 1 already fixes the worst of it). Effort: high — touches
`ir_validate.rs`, `calc-to-ir.ts`, golden fixtures, `ir-roundtrip.test.ts`,
the lint byte-identity round-trip, CLI.md, and ir-as-target.md. Do as one
deliberate schema rev.*

- **Omit defaults**: `name`/`state_key`/`block_next`/`block_prev` when null,
  empty `inputs`/`child_blocks`/`param_names`. Loaders already use
  `#[serde(default)]`-style tolerance; extend it uniformly. Cuts a typical
  dump roughly in half before any other change.
- **Replace the linked list in the wire format** with an ordered `terms` array
  per block (`Block.terms: [TermId]`), rebuilding `block_next`/`block_prev` on
  load (`rebuild_indexes` already exists for `block_terms`). Emitters stop
  maintaining a doubly-linked list by hand; the graph stays identical.
- **Synthesize builtin phantoms on load.** The runtime knows its registration
  table; the loader can emit the phantom `Copy` terms itself when absent.
  Foreign front-ends stop depending on builtin registration order, and
  `show-ir --json` can stop shipping 117 boilerplate terms (keep emitting them
  during a deprecation window if useful).
- **Registers fully optional on the wire**: already documented as
  recomputable; make `show-ir --json` omission-capable (`--bare`) and the
  recompute path the documented default for emitters.
- **Compact spans everywhere**: `source_map` entries as
  `[start_offset, end_offset]` (+ optional file index) or `"5:11-5:19"`;
  pick one encoding and share it with the AST/tokens dumps.
- Consider an explicit `"schema": "0.2"` field now that there are two shapes
  in the wild; the loader currently ignores unknown fields, so this is cheap.
- Longer-term (not this rev): define an IR-level pattern encoding for
  `match_arms` so the IR stops embedding `ast::Pattern`.

### 5. Token dumps: uniform rows with spans

*Impact: medium — tokens are the least-consulted dump, but this is nearly
free since `token_spans` already exists. Effort: low.*

- JSON: `[{"kind": "Let", "span": "1:1-1:4"}, {"kind": "Ident", "value": "x", "span": "1:5-1:6"}, …]`.
- Text: `0: Let @1:1`, `1: Ident "x" @1:5`.

### 6. AST JSON polish

*Impact: medium. Effort: low. Ride along with the rank-4 conventions.*

- Skip defaults: `exported: false`, `is_var: false`, `is_config: false`,
  `ty: null`, `resolved: null`, `class: null`.
- Same compact span encoding as ranks 4–5.
- Decide once, while private: keep externally-tagged `{"kind": {"BinaryOp": …}}`
  or move to ESTree-style internal tagging (`{"type": "BinaryOp", …}`).
  Internal tagging reads better and matches the JS-ecosystem norm, but costs
  restructuring the newtype variants (`Ident(String)` etc.). Fine to
  explicitly decide "not worth it" — but decide, and document the rationale
  in CLI.md.

### 7. Structured bytecode JSON

*Impact: low today (no consumer of the JSON form is known; the text form is
good). Effort: low-medium.*

Serialize the `Inst` enum properly — `{"op": "Mul", "dst": 4, "a": 0, "b": 0}`
— keeping the rendered `text` field alongside for readability. Do it before
any external tool starts parsing the `text` strings.

### 8. One conventions section in CLI.md

*Impact: low individually, but it keeps ranks 2–7 coherent. Effort: trivial.*

Document once, for all four dumps: the span encoding, the id-prefix
conventions (`t`/`r`/`c`/`fn`/`block`/`k`), the omit-defaults rule, and which
dumps are contracts (IR JSON) vs. debug views (everything else).

## Observations that are IR design, not format (out of scope here)

- **Copy-per-reference**: every variable use emits a `Copy` term, roughly
  doubling term count. It's load-bearing (each reference site is a provenance
  node with its own span), and the bytecode lowering's copy-propagation erases
  the cost — but a display-level fold ("inline single-use Copies") could be a
  future `show-ir` nicety.
- String interpolation compiles to a `Concat` chain ending in a concat with
  the constant `""` (and the lexer emits a trailing empty `String("")` token).
  Harmless, mildly confusing in dumps; a compiler tweak could drop the empty
  tail.
- Builtin identity is a *string in the constant table* (`BuiltinCall(c3)` →
  `"map"`), resolved by name at lower time, while phantom term ids must match
  table registration order. Rank 4's loader-side phantom synthesis removes the
  order coupling from the wire; unifying builtin identity further is a
  separate design question.

## Suggested sequencing

Quick wins, display-only, in one pass: **1, 3, 2, 5, 6-minus-tagging** —
no schema breaks, immediate quality-of-life for humans and agents.
Then **4** as a deliberate schema v0.2 rev with fixtures/docs updated
together. **7** and **8** ride along opportunistically.
