# Petal — Goals Progress & Sequencing

This document tracks **what is actually built today** against the vision in
[PETAL_GOALS.md](PETAL_GOALS.md), and lays out the **sequencing** of upcoming
work. PETAL_GOALS.md is the North Star (the *why* and the *eventual where*);
this file is the honest *where we are now* and *what's next*.

Status legend:

- ✅ **Shipped** — built, exercised, and covered by tests/examples.
- 🟡 **Partial** — a usable subset exists; named gaps remain.
- 🔭 **Aspirational** — described in the vision, not yet started.
- ⚠️ **Needs hardening** — works, but has a known correctness/robustness risk
  that matters because it sits under a core promise.

Last reviewed: 2026-06-09.

---

## Snapshot

The **foundation is real**: a flat, SSA-style dataflow term graph with explicit
input edges (`rust/src/program.rs`), Phi joins for rebinding, reified control
flow, first-class `state` with temporal edges, a mark-sweep GC, and a step
evaluator (~12k lines of Rust, no `unimplemented!` stubs). The *introspection*
pillars built on top of it — provenance, slicing, `ExplainTerm`, and
state-preserving hot reload — are shipped and tested.

The gap is between this working foundation and the **headline research payoffs**:
reverse-mode back-propagation through programs, bidirectional projectional
editing, and cross-language abstract programming are still aspirational. The
active, validated momentum is in **live creative coding**, where Petal's
state + hot-reload + dataflow-legibility combination is genuinely differentiated.

---

## Goal 1: Dataflow-First Semantics

| Capability | Status | Notes |
|---|---|---|
| Dataflow IR term graph (explicit input edges, immutable, Phi joins) | ✅ Shipped | `program.rs`; control flow reified as terms with child blocks. The substrate everything else stands on. |
| Provenance — backward trace ("what influenced this?") | ✅ Shipped | `Program::trace_provenance`; `show-provenance` CLI; `provenance.test.ts`. |
| Forward dependents ("what does this influence?") | ✅ Shipped | `Program::trace_dependents`; `show-dependents` CLI. |
| `ExplainTerm` — backward causal walk with recorded values | ✅ Shipped | `trace.rs` ring buffer; `explain` CLI + MCP tool. Near-zero cost when tracing is off. |
| Forward-mode autodiff (dual numbers) | 🟡 Partial | `Value::Dual` propagates through `+ - * /` in the evaluator (`eval.rs`). **Gaps:** transcendentals (`sin`/`cos`/`sqrt`) operate on the primal only and drop the derivative. |
| Reverse-mode AD / back-propagation ("specify a target, suggest source-value changes") | 🔭 Aspirational | No gradient/adjoint code exists. This is the headline differentiable-programming goal and is the centerpiece of Phase 1 below. |

## Goal 2: First-Class State

| Capability | Status | Notes |
|---|---|---|
| `state` keyword; values persist across invocations | ✅ Shipped | Keyed by `RuntimeStateKey`; `stack.rs`, `eval.rs`. |
| Per-iteration / per-branch state (state inside loops & conditionals) | ✅ Shipped | Loop-index path in the state key; `bug-state-in-if.test.ts`. |
| Explicit state keys (`state(expr) name = ...`) for survival across reordering | ✅ Shipped | |
| Snapshot/restore; untouched-state sweeping on completion | ✅ Shipped | `env.rs`. Underpins hot reload (Goal 4). |
| State correctness under repeated reassignment / SSA `Copy` masking | ⚠️ Needs hardening | A second `state` reassignment per frame previously dropped silently (SSA `Copy` masked the `StateInit`); fixed, but the eval doc flags the whole category for a deeper audit. State trust is the product — see Phase 0. |

## Goal 3: Projectional Views

| Capability | Status | Notes |
|---|---|---|
| Program slicing (minimal subgraph for target terms) | ✅ Shipped | `Program::slice`, topological; `show-slice` CLI; `slicing.test.ts`. |
| Forward / backward static slices | ✅ Shipped | Same machinery as Goal 1 provenance/dependents. |
| Dynamic / scenario slices ("what was active for this run") | 🟡 Partial | The trace buffer captures execution; a polished "projected linear trace for one scenario" *view* is not yet a product surface. |
| Bidirectional / projectional editing (edit the projection, map edits back) | 🔭 Aspirational | No edit-mapping exists. Related to lenses / MPS. |
| Cross-language abstract programming (mount foreign programs through a Petal lens) | 🔭 Aspirational | A research-scale moonshot as originally framed. **Re-scoped** to the tractable inverse — *be a legible IR others emit into* (Phase 3, `idea-34b8348d`). |

## Goal 4: Live Editing

| Capability | Status | Notes |
|---|---|---|
| Hot reload with state reconciliation (additions default, removals GC'd, modifications migrate) | ✅ Shipped | `hot_reload.rs`; the killer feature of `petal-sdl`. |
| Speculative / sandboxed run | ✅ Shipped | `env.rs`. |
| Incremental dataflow update (recompute only affected nodes) | 🟡 Partial | The integrations re-run the whole program each frame; true incremental graph diffing is not implemented. Fine for current frame sizes, relevant if/when sketches scale. |
| Live editing flowing through back-prop paths | 🔭 Aspirational | Depends on reverse-mode AD (Goal 1). Realized as the Phase 1 drag-to-edit demo. |

## Cross-cutting

| Area | Status | Notes |
|---|---|---|
| Surface syntax — "Stem" (keyword + `end`; `{}` is *always* a record) | ✅ Shipped | `74c1152`. Removed the record-vs-block parser heuristic; one-sentence grammar rule. Watch the `when` keyword for field-name collisions. |
| Triple-quoted raw strings (for embedding source) | ✅ Shipped | `1b53925`. Enables `Program.parse("""...""")` metaprogramming sketches. |
| Type system | 🔭 None (intentional) | Dynamically typed; runtime tags only. Future direction: **types as a projection** — inferred shapes surfaced to tooling/agents, never enforced (consistent with "low floor, forgiving types"). |
| Modules / imports | 🔭 Missing | A ceiling on the "high ceiling" promise; not yet scoped. |
| Performance | 🟡 Partial | Introspection-first interpreter, not a fast VM. Heavy creative-coding sketches run ~11–17fps; the inner-loop boxing tax is the bottleneck. Unblocked by Phase 0 perf tickets. |
| AI-legibility (MCP tools, headless agent protocol, structured JSON traces, `ExplainTerm`) | ✅ Shipped (de-facto differentiator) | An agent can ask "why does this output have this value," walk to a root constant, change it, and re-run headlessly. Proposed for promotion to a *named* goal (Phase 2). |
| Creative-coding integrations | ✅ Shipped | `petal-sdl` (most mature), `petal-web`, `petal-diagram-canvas`, `petal-web-canvas`, `petal-fps`. |

---

## Sequencing

The strategy: **commit to live, dataflow-legible creative coding as the
near-term wedge**, and make the research pillars pay rent *inside* that wedge
rather than pursuing them as separate moonshots. The foundation is shared, so
this is a question of headline and ordering, not a fork.

### Phase 0 — Foundation trust + perf enablers (now)

Make the foundation fast enough and trustworthy enough to carry the wedge.

- **State-correctness audit.** Add the invariant check (`StateWrite` count ==
  top-level reassignment count) and property-test the Phi/state machinery.
  State trust is existential for a language whose pitch is "state and
  provenance are reliable." (See ⚠️ in Goal 2.)
- **Typed numeric arrays** `FlatList<f64>` — `tk-18c881bd` *(high)*.
- **Non-allocating numeric-range iteration** — `tk-0838ac3a` *(medium)*.
- **Offscreen canvas / no-clear framebuffer** — `tk-51aa4a94` *(medium)*.
- **Filled polygon / triangle primitives** — `tk-d6e83427` *(medium)*.
- **Normalize `hsv()`/`hsl()` hue to `[0,1)`** — `tk-1075226b` *(low)*.

Exit criterion: Nature-of-Code-scale sketches run at interactive frame rates,
and the state machinery has invariant coverage.

### Phase 1 — The unifying wedge: differentiable direct-manipulation live coding

One demo that exercises Goals 1, 3, and 4 in a single gesture, and finally
makes reverse-mode AD worth building by *scoping it to a use case*.

- Implement **reverse-mode AD** over the dataflow graph (scoped to the
  drag-to-edit path, not general program optimization). Closes the Goal 1 gap.
- **Drag an output on the canvas** → back-prop to the source constants that
  influence it → **project the candidate slice** of editable constants → when
  the choice is ambiguous, show the slice and let the user pick (the
  "human-in-the-loop" resolution the vision already describes) → **live-edit
  the source numbers** with state preserved.
- **Scrubbable provenance** (companion): record input/frame history; let the
  user scrub past frames and query "why is this pixel this color"; on
  hot-reload, re-run the recorded history through the edited graph to show a
  change retroactively.

### Phase 2 — AI-legibility as a named goal

- Consolidate `ExplainTerm` / trace / headless protocol into a coherent,
  documented agent-facing surface.
- **Types as a projection**: infer shapes from the dataflow graph and surface
  them to tooling and agents (hover, structured output) without enforcement.

### Phase 3 — IR as a legible emit target *(the inverted cross-language pillar)*

`idea-34b8348d` — see [ir-as-target.md](ir-as-target.md). **In progress.**

Rather than mounting foreign programs *into* Petal (intractable), make Petal's
dataflow IR a **stable, documented target others compile into** — so any
front-end that emits valid IR gets provenance, slicing, `ExplainTerm`, and
state-preserving live editing for free.

- ✅ M1 — import format contract (Schema v0) + golden fixtures
- ✅ M2 — `petal run --ir <file|->` load-and-run; `show-ir --json | run --ir`
  round-trip verified across the snippet matrix and the examples corpus
- ✅ M3 — pre-eval validation pass with actionable errors
- ⏳ M4 — a reference external emitter (a tiny non-Petal front-end that emits
  Petal IR and inherits provenance/`explain` for free)

### North Star (not scheduled)

Tracked in [PETAL_GOALS.md](PETAL_GOALS.md), deliberately *not* on the roadmap:
full bidirectional projectional editing, general cross-language mounting, and
back-propagation as general-program optimization. These remain the compass;
the phases above are how we earn the right to attempt them.
