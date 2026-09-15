# Reactive rendering

Status (2026-09-15): **P0 and P1 are shipped; P2 and P3 are next.** Hosts
skip frames whose inputs did not change ([frame-gate.md](frame-gate.md)), and
within a frame that runs, user-function calls whose inputs did not change are
replayed ([memo-scopes.md](memo-scopes.md)). This file tracks the plan as a
whole: what is left of each layer, the layers not started, and the hazards
and measurements that should shape them. The shipped layers are documented
in their own files; this one does not repeat them.

The original proposal is *Reactive Rendering for Petal* (design
investigation, 2026-09-14).

## Goal

Keep the immediate-mode authoring model: a script reads top to bottom, polls
input (`mouse_x()`, `hovered(r)`, `time()`), keeps `state` where it lives, and
re-runs every frame. Make the runtime incremental underneath it, so that the
steady-state cost of a frame tracks **what changed**, not **how big the UI
is**. `goals.md` names whole-script re-runs as the ceiling on how large a
Petal app can get; the payoff is a 50,000-row table or a full IDE UI.

Non-goal: beating JIT-compiled JavaScript on a from-scratch run. An
interpreter cannot. The strategy is to make the interpreter's speed apply only
to the few scopes that re-run.

## The layers

The proposals run from coarse and cheap to fine and invasive. They compose:
P0 underlies everything, P2 and P3 make P1 cheaper.

| Layer | Unit | Language change | Status |
|---|---|---|---|
| P0 Gated frames, retained output | whole frame | none | 🟡 gate shipped in every host; segmented output and damage rects not started |
| P1 Memoized scopes | function call (loop body planned) | none | 🟡 function calls shipped; loop bodies, top-level body, hot-reload invalidation open |
| P2 Dependency classes | block, by input class | none | ⬜ next |
| P3 Keyed collections + hit-test index | collection element | `for … key` | ⬜ |
| P4 Signals | binding | new mental model | 🔭 internal representation only, if ever |
| P5 Self-adjusting term graph | IR term / block | none | 🔭 research |

### P0 — what is left

Shipped (c2032ad, d90bc35): the read-set gate (`rust/src/run_deps.rs`,
`Env::run_needed`) in `Headless`, Garden panels, petal-desktop-sdl and
petal-web-canvas; keyed DOM patching in petal-web-html in place of
`innerHTML`.

Open:

- **Segmented output.** The output buffer is still a flat `Vec` per symbol. The
  plan is a sequence of segments tagged with the emitting call path, so a host
  can diff segment by segment. Memo replay already appends a scope's cached
  tail, which is the in-VM half of this; the host-facing half is not built.
- **Damage rectangles for canvas hosts.** A frame that runs still repaints the
  whole surface in SDL and web-canvas. Segments are the input to this.
- **LIS moves in petal-web-html.** The keyed reconciler moves nodes naively;
  a longest-increasing-subsequence pass (Inferno, ivi) would cut moves for
  large reorders. The renderer's header comment notes it.
- **Probe early cutoff at the frame level.** The gate treats a moved pointer
  as a reason to run. The proposal had probe natives such as `hovered(r)`
  record their answer in the frame's read-set too, so a pointer move that
  flips no probe skips the frame. P1 does this per scope; the frame gate does
  not yet.

### P1 — what is left

Shipped (2d57269, 93da89b, a327d8c): every user-function call is a scope keyed
by call path, with probes, state/cell/host-data reads, child scopes, output,
observations and touched keys recorded; early cutoff for write-free children;
eviction of unvisited records; the on/off differential oracle over the
examples corpus. Design in [memo-scopes.md](memo-scopes.md).

Open:

- **Loop bodies as scopes.** Only calls are scopes. A top-level `for` that
  draws rows inline gets nothing; a list pays for its driving loop (~60 ns per
  iteration estimated) even when every row replays. Loop iterations already
  have `Index(i)` path parts, so they have addresses.
- **The top-level body.** The root frame has no record. Large apps are long
  top-level scripts, so this is where most of the remaining cost sits in the
  examples, and why the spreadsheet barely moved (1.32 → 1.15 ms).
- **Hot reload by function body hash.** A program transfer clears the whole
  table today. The plan: invalidate only the records of functions whose body
  hash changed, so editing one widget re-runs only that widget's scopes —
  incremental evaluation as a live-coding feature.
- **Per-row validation cost.** ~0.4 µs per row on the 5,000-row list, most of
  it re-evaluating `hovered`. P2 and P3 address this rather than P1.
- **Optional hints, only if needed:** `@nomemo` on a function; `key` on `for`
  (shared with P3).

### P2 — dependency classes and static hoisting

Inspired by Svelte 3/4 dirty bitmasks, Imba memoized DOM, Million block DOM.

An interprocedural pass over the term graph tags every term with a small
bitmask of the input classes it can depend on: constant, viewport size,
pointer, keyboard, clock, each `state var`, host data. This is
`trace_dependents` (`program_analysis.rs`) run forward from the input natives
and extended across calls. Three uses:

1. **Hoist the constant class** out of the frame: palettes, type scales and
   literal-derived values computed once per program version; viewport-only
   values cached until a resize. `float` was 58% of all builtin calls in the
   analytics dashboard's profile.
2. **Guard blocks by class:** `if frame_dirty & block_mask == 0 then reuse`, a
   few instructions, so a clock-driven spinner stops dragging a static table
   along with it. This is also a way to cover the top-level body without
   making it a single scope.
3. **Choose P1 boundaries and trim read-sets:** scopes whose classes are known
   statically need no runtime read recording; scopes too small to pay for
   their check are never memoized (today that is the runtime
   `MIN_SCOPE_INSTS` heuristic).

On its own it is coarse — every list row has `pointer` in its mask — so its
value is making P1's per-scope overhead nearly free. Tune it with P1's slot
statistics (`--memo-stats`). Estimated ~4–6 weeks; ~10–30% off a full re-run.

### P3 — keyed incremental collections and a hit-test index

Inspired by DBSP / differential dataflow, Jane Street `Incr_map`, Solid's
`mapArray`, ImGui list clipping.

```petal
for t in get(tasks) key t.id do
  task_row(t)
end
```

- **Persistent collections.** Lists and maps gain persistent representations
  (RRB vectors, HAMTs) with structural sharing, so two versions diff in
  O(changes) by skipping shared subtrees by pointer (Jane Street's
  `Map.symmetric_diff`). A keyed `for` compiles to a map over that diff:
  bodies run only for inserted or changed keys, removed keys drop their
  segments and state, moves reorder segments. Map and filter are their own
  incremental versions (DBSP). Ordering is the hard part; fall back to keyed
  LIS diffing when a diff is too large.
- **Cost:** persistent structures are ~1.5–3× slower than a flat `Vec` in tight
  numeric loops. Keep `f64_array` and the escape-analysis in-place path for
  compute code.
- **Hit-test index.** Probe natives register their rectangles in a spatial
  grid owned by the host. On a pointer move the runtime looks up which probes
  flipped and dirties only those scopes, leaving every other scope unvisited:
  hover goes from O(rows) validations to O(log n). This retires the per-row
  `hovered` re-evaluation that dominates P1's remaining list cost.

Estimated ~6–8 weeks.

### P4 and P5 — held as research

**P4, signals** (SolidJS, Svelte 5 runes, TC39 Signals, Reactively's
push-dirty/pull-recompute coloring): `derive` bindings, views as a setup pass,
each draw an effect bound to the signals it read. Best asymptotics (Solid and
Svelte 5 sit within ~5% of vanilla JS), but it abandons top-to-bottom reading,
polled input and positional state, and brings Solid's footguns (destructured
props lose reactivity, effects need owners). Elm had signals and removed them
in 0.17. If adopted at all, it is as the internal representation P1–P3
compile into, never user syntax — Svelte 5's and Vue Vapor's use.

**P5, a self-adjusting term graph** (Acar's SAC, Adapton): treat a frame's
execution trace as a dynamic dependency graph and re-execute only blocks
downstream of a changed input. The most Petal-native idea — `explain` always
current, the SDL timeline's "replay through an edit" re-executing only what
the edit touched, provenance reading the same graph. Also the most expensive:
the 5,000-row list retires ~368k instructions per frame, so even block-level
tracing is megabytes per frame, and SAC's documented from-scratch overhead is
2–10×. P1 is the practical block-granularity approximation; revisit finer
granularity when P1's slot table shows where it would pay.

Revisit either only if profiles of large apps still show validation overhead
dominating after P2 and P3.

## Hazards

The proposal's hazard list, with where each stands.

| Hazard | Status |
|---|---|
| **Hidden impurity** (random, print, host data, FFI handles in a scope) | Handled. Effectful scopes and their ancestors are not recorded; host data enters the read-set with a revision; natives declare themselves with `note_effect` / `note_host_read`. Any new native that reaches host state must do one or the other. |
| **State sweep deleting skipped slots** | Handled: touch captures (eac4023, [state-call-paths.md §3.3a](state-call-paths.md)). |
| **State writes in a scope** | Handled: writes are recorded and re-applied on replay; a writing child is never re-executed speculatively. |
| **Heap id reuse** | Handled: generational heap and closure ids (826a24a); memo records are GC roots (`MemoTable::gc_roots`). |
| **Freshly built arguments** (`Rect(0, y, 400, 20)` each frame) | Mitigated by bounded structural compare (`ARG_COMPARE_BUDGET`). A large structure freshly allocated every frame still compares unequal and re-runs. Longer term: intern small records, or make `Rect` inline like `Vec2`. |
| **Aliasing a replayed result** | Handled (a327d8c): a call whose result the caller mutates in place is not a scope. |
| **Memory growth** | Records of unvisited scopes are evicted each run; `MAX_SLOTS` caps the table; `MAX_SCOPE_DEPS` caps a record. No cap yet on retained output per scope. Persistent collections (P3) will add structure overhead. |
| **Tooling fidelity** (`explain`, `--observe`, provenance) | Observations are re-recorded on replay. Memoization is off while the `explain` trace is on. `PETAL_OPT=off` disables it for differential testing. |
| **Correctness: a missed dependency is a stale pixel** | Differential oracles (`petal-ui/tests/gating.rs`, `petal-ui/tests/memo.rs`) over the examples corpus. **Not yet run over Garden (`~/garden`, `~/.garden`) or worlds-fair (`~/worlds-fair/ui/ptl`) scripts**, which the proposal called for. Every new layer needs the same incremental-vs-full oracle before it lands. |
| **Nearly every widget reads the mouse** | Why `hovered` is a native probe with early cutoff. Any other per-widget input read written in Petal (two pointer reads instead of one probe) will make every row re-run on every move; check `--memo-stats` when adding one. |
| **Prelude reads that defeat gating** | Found twice (`_host_theme` cached on `frame_count()`, `approach` reading `dt()` after landing). Expect more in Garden/bloom code; `--gate-stats` shows run reasons. |

## Measurements

Release build, Apple Silicon, `petal-ui/examples/bench_panel`, 1200×800.

**Baseline before P0 (2026-09-14, min / p50, every frame runs):**

| Script | Draw cmds | Instr / frame | Calls / frame | min ms | p50 ms |
|---|---|---|---|---|---|
| examples/productivity/todo | 192 | 18k | 511 | 0.69 | 0.77 |
| examples/productivity/kanban | 179 | 24k | 529 | 0.90 | 0.94 |
| examples/productivity/crm-contact-manager | 274 | 23k | 1,117 | 0.95 | 1.06 |
| examples/dashboards/finance-dashboard | 502 | 30k | 1,141 | 1.26 | 1.44 |
| examples/productivity/notes | 163 | 43k | 449 | 1.29 | 1.42 |
| examples/productivity/spreadsheet | 268 | 85k | 2,840 | 1.96 | 2.17 |
| examples/dashboards/analytics-dashboard | 1,471 | 90k | 3,235 | 3.31 | 3.60 |
| synthetic list, 100 rows | 200 | 7.9k | 508 | 0.49 | 0.54 |
| synthetic list, 1,000 rows | 2,000 | 74k | 5,083 | 4.03 | 4.42 |
| synthetic list, 5,000 rows | 10,000 | 368k | 25,417 | 20.7 | 22.0 |

The synthetic row is a function drawing one rect and one label with a
`hovered(rect)` check. Cost was linear in rows: 74 instructions and 5 user
calls per row, ~4.4 µs per row, ~20 µs per 1k instructions.

**After P0:** a quiet frame of any example is the gate check, a few µs.
A frame that runs costs what it did.

**After P1** (`--wiggle`, p50 over 60 frames; the "every call runs" column is
the same binary with `--no-memo`, so it reflects other work landed since the
baseline): 5,000-row list 10.7 → 3.4 ms, 1,000-row list 2.2 → 0.7 ms,
analytics dashboard 2.3 → 0.64 ms, server monitoring 4.9 → 1.3 ms,
photo-adjust 3.3 → 0.68 ms, notes 0.88 → 0.26 ms, kanban 0.61 → 0.22 ms,
todo 0.49 → 0.38 ms, spreadsheet 1.32 → 1.15 ms. The proposal estimated
~2 ms for the list's hover frame; the gap is the un-scoped top-level loop and
the ~0.4 µs validation per row (estimated 250–350 ns).

**Targets for the rest** (5,000-row list, script time, estimates):

| Scenario | P0+P1 est. | + P2 | + P3 |
|---|---|---|---|
| Hover move | ~2 | ~1.2 | ~0.05 |
| Edit 1 row | ~2 | ~1 | ~0.05 |
| Append row | ~2 | ~1 | ~0.08 |
| One animated widget | ~2 | ~0.05 | ~0.05 |
| Theme change (everything invalid) | ~24 | ~18 | ~18 |

A change that invalidates everything stays about as expensive as a full run
plus bookkeeping; that is expected, not a regression to chase. Assumptions:
P2 block guards ~5 instructions each; P3 diff O(changes · log n), hit-test
lookup O(log n) touching ≤ 2 probes per pointer move. Host rasterization and
DOM time are not included — for DOM hosts they are usually the larger share,
and P0's damage rects and keyed patching address them separately.

## What the field says

Lessons that shaped the plan, and should keep shaping it:

- **Don't make users think in graphs.** Elm dropped signals because they were
  hard to teach and every app became one `foldp`; `Html.Lazy` with reference
  equality over immutable data gave most of the speed. Petal's immutable
  values make the same trick cheap.
- **Early cutoff is the cheapest validation** (Salsa's revision counter and
  backdating). Trace memory is the trap (Adapton, SAC).
- **Signals work best as a compiler target**, not user API (Svelte 5, Vue
  Vapor). Lists need a keyed `mapArray`.
- **Jetpack Compose is the closest match:** re-run functions, skip calls whose
  parameters are equal, store results in a slot table keyed by call position.
  Imba shows whole-render re-execution can be fast when static parts are
  cached per callsite.
- **The bar is Inferno and Solid, not React.** On js-framework-benchmark
  (Chrome 138) the top tier sits within 5–10% of vanilla JS; React 19 is ~8×
  slower on row swap. A well-built keyed virtual DOM (Inferno) matches signals.

## Sources

Salsa algorithm and durable incrementality; Hammer et al., *Adapton* (PLDI
2014); Acar, self-adjusting computation; Jane Street, *Self-adjusting DOM and
diffable data*, Incremental, Bonsai; Czaplicki, *A Farewell to FRP*; Elliott,
*Push-pull FRP*; Milo, *Super Charging Fine-Grained Reactive Performance*;
TC39 Signals; Svelte 5 runes; Imba memoized DOM; Million block DOM; Compose
strong skipping and positional memoization; SwiftUI AttributeGraph; Levien,
*Xilem*; Budiu et al., *DBSP* (VLDB 2023); js-framework-benchmark.
