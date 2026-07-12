# Refactor-Verification Tooling — Plan

Status: **proposal / not started**. Captures an investigation into what Petal
tooling could verify that a source refactor is safe, and a staged plan to build
it.

Two goals motivate this:

1. **Prove a source change has no observable behavior change.**
2. **Describe the extent (blast radius) of an observable change when there is one.**

There is a `petal lint` subcommand (formatter/normalizer — see
[linter-plan.md](linter-plan.md)), but no notion of program-to-program
equivalence. What follows is the capability inventory that exists to build on,
the core difficulty, and a staged plan.

## The core difficulty (worked example)

The change that prompted this — rewriting a chain of sequential mutating `if`s
into an `if/elsif/else` — is a perfect illustration of why this is non-trivial.

```petal
// last-wins: evaluates all three conditions every call
let c = "CTX"
if kind == "hunk" then c = "HUNK" end
if kind == "add"  then c = "GREEN" end
if kind == "del"  then c = "RED" end

// first-wins: short-circuits on the first match
if kind == "hunk" then "HUNK"
elsif kind == "add" then "GREEN"
elsif kind == "del" then "RED"
else "CTX"
end
```

These compile to **genuinely different** IR/bytecode (≈50 vs ≈28 instructions:
one evaluates all conditions and threads a mutable local through phi joins, the
other short-circuits). They are behaviorally equivalent **only because** the
conditions are pure and mutually exclusive. On overlapping conditions they
diverge — last-wins vs first-wins (e.g. `if n>0 … if n>10 …` at `n=50` yields
`"big"` vs `"positive"`).

**Consequence:** a structural IR comparison is the wrong primitive. It is sound
but so incomplete it rejects most real refactors. The useful relation is
*observable* equivalence (same outputs/effects over the relevant input domain),
not *structural* identity.

## Capability inventory (what exists today)

Petal has strong static dataflow analysis, a per-term runtime trace, cost
metrics, and a mature differential/golden oracle at the whole-program-output
level. It has **no** IR normalization, subgraph fingerprinting, or
cross-program equivalence.

| Capability | Command / source | Relevance |
|---|---|---|
| Compile validation | `check` — `rust/src/cli/handlers.rs` | "still parses/compiles" — a floor, not behavior. No type check, no execution. |
| Backward slice (provenance) | `show-provenance` — `program_analysis.rs:58` (`trace_provenance`) | BFS over `term.inputs`, follows `Phi` rebinds. "What influenced this output." |
| Forward slice (dependents) | `show-dependents` — `program_analysis.rs:119` (`trace_dependents`) | **Blast-radius primitive.** Reverse index + forward BFS. Caveat: does **not** follow phi rebinds like provenance does, so loop/branch carries under-approximate — fix before trusting. |
| Minimal slice | `show-slice` — `program_analysis.rs:164` | Union of ancestors of target terms; isolates the region a change touches. |
| Dataflow visualization | `show-graph` — `rust/src/dot_graph.rs` | Graphviz DOT of the IR; visualization only. |
| Per-term value trace | `--trace` / `--record-trace` — `rust/src/trace.rs` | Ring buffer of `TraceEvent { sequence, term_id, inputs, result }` — every term's inputs and result in execution order. `to_json` serializes with source line/col. The most promising differential primitive. |
| Cost metrics | `--dup-stats` — `rust/src/stats.rs` | Value-duplication (`DupKind` List/Map/F64Array/Fork) and alloc churn. Perf-regression guard, not correctness — but a real "did the cost profile change" signal. Debug/`dup-stats`-feature only. |
| Output-identity oracle | `ts/bin/test-examples.ts`, `test/example-golden/` | Requires **byte-identical stdout+stderr** across opt levels and against a frozen golden corpus. This is the maintained "no observable change" equality relation — but only for one program across lowerings, never two source versions. |
| IR round-trip | `ts/test/ir-roundtrip.test.ts` | `show-ir --json | run --ir -` reproduces source-run output. |

### Why the IR isn't directly comparable

- **No IR-level optimization/normalization pass** exists (`rust/src/compiler/`):
  no CSE, constant folding, DCE, or algebraic normalization. The only
  "canonicalization" is syntactic desugaring (`@`-args → writebacks, compound
  assignments) — source-shape sugar, not graph normalization.
- **Opt flags don't change the IR.** `OptFlags` are consumed only at bytecode
  lowering; `--no-opt` vs opts produce identical IR and identical output. The
  graph is opt-invariant.
- **Term IDs are compile-order** (`program.rs`) and therefore **not stable
  across a source edit** — a blocker for any naive cross-run trace diff.
- Constants are deduplicated (`constant_table.rs`); that is the only structural
  sharing. No `fingerprint` / `equiv` / `normalize` anywhere in `rust/src/`.

## Plan — three tiers, in priority order

### Tier 1 — `verify-equiv <old> <new>`: differential behavioral checker (highest value, buildable now)

Reuse the existing output-identity oracle, but between **two source versions**
over an input corpus instead of one program across opt levels. Run both, compare
stdout / stderr / return value byte-for-byte; optionally enforce dup/alloc
ceilings for cost-invariance. Answers goal (1) empirically.

For pure functions (like the diff-line classifiers), drive it with an
enumerated/property-based input domain — the productized form of the hand-run
differential harness that certified the Garden `elsif` refactor. Report: number
of inputs checked, any diverging input with both outputs.

Limitation to state honestly: only as strong as the input corpus (incomplete,
not a proof). For mutually-exclusive/pure branch refactors an enumerated domain
is effectively exhaustive.

### Tier 2 — `blast-radius <span>`: extent-of-change reporter (answers goal 2)

Build on `trace_dependents` + `slice`. Map the changed source span to term(s)
via `source_map`, then report the forward-reachable **outputs** (draw commands,
`print`s, state writes) whose value could observably change. Output: "this
change can only affect these N outputs" — a scoped, reviewable blast radius.

**Prerequisite:** fix `trace_dependents` to follow phi rebinds (mirror
`trace_provenance`'s handling in `program_analysis.rs:70`), or forward reachability
leaks across branches/loops.

### Tier 3 — span-keyed trace diff (semantic middle ground)

The per-term trace already captures inputs+results; the blocker is unstable
term IDs across edits. Key trace events on **source location / a structural term
key** instead of raw `term_id`, then align two runs. A refactor that leaves the
observable dataflow identical produces an alignable, diff-clean trace even when
the IR shape changed (as the `elsif` example does). This is the closest thing to
"prove no observable change" without an SMT-grade normalizer, and the honest
ceiling of what is cheap to build.

### Tier 4 (not recommended yet) — static equivalence proof

A true static proof needs IR canonicalization + subgraph equivalence
(SMT-style) — research-grade machinery that does not exist. Don't build it
before tiers 1–3 earn their keep.

## Suggested sequencing

1. Fix the `trace_dependents` phi-rebind gap (prerequisite for Tier 2; small).
2. Tier 1 `verify-equiv` — most value per unit effort; immediately useful for
   reviewing refactors like the `elsif` fold.
3. Tier 2 `blast-radius`.
4. Tier 3 span-keyed trace diff.

## Key source files to build on

- `rust/src/program.rs` — term graph; `rust/src/program_analysis.rs` — `trace_provenance` / `trace_dependents` / `slice`.
- `rust/src/trace.rs` — per-term value capture.
- `rust/src/stats.rs` — cost metrics.
- `rust/src/source_map.rs` — term ↔ source span (needed to key on spans).
- `ts/bin/test-examples.ts`, `rust/tests/script_cases.rs` — existing equivalence oracles to reuse.
