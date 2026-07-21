# Experimental: IR-based editing

> **Status: experimental / unfinished.** Constructing and transforming a Petal
> program *as IR data* works for emit-load-run (there is a tested round-trip and
> a reference foreign emitter), but there is **no in-place IR rewrite API** in
> Rust, the graph-query passes are read-only, and none of this is wired into an
> agent- or user-facing editing workflow yet. It is documented here, separate
> from [program-modification.md](../program-modification.md), so the stable
> source-level and live-editing surfaces aren't confused with this early work.
>
> For the stable IR *import-format contract* used by external emitters (`run
> --ir`), see [ir-as-target.md](ir-as-target.md).

Petal's IR is a **documented, versioned, load-and-run emit target**. You can
construct or transform a program *as data* and run it directly, bypassing the
Petal front-end entirely.

## The term graph as a data structure

A `Program` ([`rust/src/program.rs`](../../rust/src/program.rs)) owns the whole
graph:

- `terms: Vec<Term>` — nodes, indexed so `terms[i].id == i`.
- `blocks: Vec<Block>` — scopes; `root_block`, `constants`, `functions`, `match_arms`.

Each `Term` is in **two graphs at once**: the **dataflow DAG** via
`inputs: SmallVec<[TermId;4]>` (ordered value edges) and the **intra-block
execution order** via `block_next`/`block_prev` (a linked list; `Block.entry` is
the head). Other fields: `op: TermOp`, `block_id`, `name` (binding label),
`register`, `state_key`, `child_blocks`, `in_loop`.

`TermOp` ([`program.rs`](../../rust/src/program.rs)) is the operation vocabulary:
arithmetic/comparison, `Copy`, `Phi`, `Branch`, `Return`, `Constant(id)`,
`MethodCall(id)`, `MakeClosure(fn)`, `AllocMap`, `AllocElement`,
`MakeEnumVariant`, etc. There is **no register-mutation op** — cross-block
rebinding goes through a `Phi` and `Block.phi_outs: Vec<PhiOut>`: on child-frame
pop, `src_term`'s value is written into the parent frame at `dest_term`'s
register (`dest_term` must be a `Phi`).

## Emit / load / run round-trip

Serialization is **JSON via serde derives** — the wire shape is the derived
serialization of `Program`, matching `show-ir --json` byte-for-byte.

- **Emit:** `petal show-ir --json` →
  [`cli/handlers.rs`](../../rust/src/cli/handlers.rs) (`serde_json::to_string_pretty`).
- **Load:** `Program::from_json` ([`ir_validate.rs`](../../rust/src/ir_validate.rs))
  → `rebuild_indexes()` (rebuilds the `block_terms` index + constant dedup) →
  `validate()` (structural invariants).
- **Run:** `petal run --ir <file|->` → `env.load_program_ir`
  ([`env/mod.rs`](../../rust/src/env/mod.rs)) → same bytecode-VM path as a
  compiled program. Guarantee: `show-ir --json | run --ir -` equals `run`.

So a program can be **built or rewritten as JSON, validated, and executed**
without touching source text.

## Reference emitter & transform passes

- **Foreign-language emitter (the canonical builder pattern):**
  [`ts/tools/calc-to-ir.ts`](../../ts/tools/calc-to-ir.ts) is a complete
  standalone front-end for a toy "calc" language that emits Petal IR JSON sharing
  **zero code** with Petal. Its `Emitter` class shows the mechanics: a deduped
  constant table (`constId`), phantom builtin `Copy` terms in leading slots
  (`addPhantom`), and `addListed` threading the `block_next`/`block_prev` linked
  list. This is the model for programmatic construction. Golden fixtures live in
  [`ts/test/fixtures/ir/`](../../ts/test/fixtures/ir/).
- **Read/rewrite passes over the graph (Rust):**
  [`rust/src/program_analysis.rs`](../../rust/src/program_analysis.rs) —
  `trace_provenance` (backward dataflow slice), `trace_dependents` (forward
  slice), `slice(targets)` (minimal connecting subgraph), `find_term`,
  `named_terms`. Exposed on the CLI as `show-provenance`, `show-dependents`,
  `show-slice`, `show-graph` (DOT). These are **read-only today** but define the
  graph queries a transformation would target (e.g. "slice the constants that
  influence this output, then rewrite them").

There is **no dedicated `IrBuilder` API in Rust** — the "builder" is either the
compiler (internal, [`rust/src/compiler/`](../../rust/src/compiler/)) or a foreign
emitter following the JSON contract.

## Capabilities

| Capability | Read | Write | Where |
|---|---|---|---|
| Inspect IR graph | ✅ | — | `show-ir`, `program_analysis.rs`, MCP `ShowIR` |
| Construct/transform IR as data | — | ✅ (via JSON) | `run --ir`, `Program::from_json`, `calc-to-ir.ts` |
| Provenance / dependents / slice | ✅ | — | `show-provenance/dependents/slice` |
| In-place IR rewrite API (Rust) | — | ✗ not built | transform by emitting new JSON instead |
