# Direct Manipulation v2: complexity study

Status: M5 of [the v2 plan](direct-manipulation-v2-plan.md), 24 October
2026. The examples are in
[examples/direct-manipulation-v2/](../../examples/direct-manipulation-v2/), the
harness is `core-libs/petal-ui/tests/dm_v2_study.rs`, and the machine-readable
results are in [direct-manipulation-v2-study.json](direct-manipulation-v2-study.json).

The question: how complex can a program get while dragging one of its shapes
still finds the right source edit? The method: a ladder of 27 programs, from
a literal in a draw call to a 944-line dashboard, each with drags whose
intended edit is written down (105 goals); a sweep of generated programs that
grow one dimension at a time until something breaks; and 258 drags generated
automatically over every Petal app in the ecosystem that runs headless (45
apps).

Contents

1. [Summary](#1-summary)
2. [Method](#2-method)
3. [What breaks it, and why](#3-what-breaks-it-and-why)
4. [Engine changes this study made](#4-engine-changes-this-study-made)
5. [How complex a program can get](#5-how-complex-a-program-can-get)
6. [Results](#6-results) (generated)
7. [Running it](#7-running-it)

## 1. Summary

- **The ladder:** 105 goals over 27 examples. 91 succeed, which is every goal
  not marked as an expected failure. The top alternative is verified on 99
  (94%). The median is 20 speculative runs and 5 ms per drag.
- **The ecosystem:** 258 automatic drags over 45 runnable apps of 100 to
  2,700 lines and up to 6,400 emits. The top alternative is verified on 240
  (93%), and the edit holds after a hot reload on all 240 of those. The median is 56 runs
  and 117 ms. This clears the plan's bar for making v2 the default (§4.2:
  90% on the ladder, 70% external, a median of at most 60 runs).
- **What it takes:** reaching through function calls, loops, records,
  recursion, trig, `reduce` and `var` costs nothing extra. These need no
  understanding of the code, only re-runs. The cost of a run grows with the
  frame's work: about 0.2 ms for a small sketch, 10 ms for the 1,500-emit
  dashboard, 26 ms for a 100,000-step simulation and 43 ms for a
  12,000-emit tree.
- **What breaks it:** finding *a* verified edit is rarely the problem; picking
  the edit the user *means* is. Of the 14 expected failures on the ladder:
  - 5 are semantic: a cosmetic offset, a physics constant, a count, or an
    axis-rounding constant scores better than the intended knob, and only a
    hint or the client can tell them apart;
  - 6 are discontinuities: a branch that hides a literal until it flips, a
    step function flat at the probe size, an int-only path that has no exact
    answer;
  - 2 are hint ranges doing their job;
  - 1 is a tie between translating a tree and reshaping it.

  Outside the ladder there are two more limits. On big apps the run budget
  runs out on columns screening wrongly marked as moving. And apps whose
  geometry lives in `state` have nothing to edit at a live checkpoint.
- **Changes to the engine:** the study found that M3's ranking systematically
  preferred large absolute coordinates over the offsets, gaps and angles a
  drag means, and that many-literal programs exhausted the budget before
  solving. Six revisions fixed these (§4). They took the ladder from 82 to
  91 successes, and the external verified rate from 77% to 93%.

## 2. Method

**The examples.** Each example is a petal-ui script. Its header lists drags
as `dm-goal` lines:

- a target, written `circle#2.0 += 25`: the raw argument 0 of the third
  circle, before the native truncates it to an int;
- optionally a client filter (`allow=`/`deny=`);
- an `expect=` set: the literals the top alternative may change;
- for a known limit, an `xfail=` reason.

The format is in the
[examples README](../../examples/direct-manipulation-v2/README.md). Where the
plain version of an example is ambiguous, a second file adds `config let`,
`tunable` or `rigid` (l01b, l07b, l09c), or a goal adds a client filter. That
measures what each kind of help buys.

**One drag.** For each goal, the harness:

1. Loads the example in `Headless`, seeds its RNG (`set_seed(7)`; the
   default seed is the clock), runs `dm-frames` frames, and calls
   `propose_edits_v2` on the stack after the last one. That stack is a live
   checkpoint: its state is frozen.
2. Applies the top alternative three ways:
   - as overrides to the original program in a fresh session;
   - as a text edit to a copy on disk, hot-reloaded into a copy run to the
     same checkpoint (`Env::reload_program`);
   - as the same edit, restarted from scratch.
3. Counts the drag as a success when:
   - the top alternative is verified;
   - the hot-reloaded program draws exactly what the override run drew
     (every command, every number);
   - the goal is within tolerance there (±0.5 px by default);
   - the changed literals are in `expect=`.

   The restart is recorded but not required. It differs exactly when the
   edit changes how `state` evolves: `a02-state`'s `speed` is the only such
   case on the ladder.

Early versions of the harness reported seven verified edits as missing their
goal after the reload. Every one was a harness bug:
- four re-read the goal by its position in the draw list, which shifts when
  the edit changes how many shapes are drawn before it;
- two re-read it by `EmitKey`, which does not carry over to the edited
  program, whose terms are numbered anew;
- one overrode a negated literal (`-1`) with the wrong sign.

Compared draw for draw, all seven were correct. The lesson for a client that
re-locates a dragged shape after applying an edit (Garden): do it by
draw-list equivalence or by source position, not by `EmitKey`.

**The scaling sweep** generates programs with one growing dimension and one
drag each:

| Family | Grows | Drag |
|---|---|---|
| row | rects in a `for` row (5 to 1,600) | last rect right, first fixed |
| tree-depth | recursion depth of the fractal tree (2 to 12, up to 12,288 emits) | the last leaf right |
| sim-steps | Euler steps of a `var` projectile (10 to 100,000) | the landing point |
| shared-sum | literals summed into one emit (4 to 1,024) | that emit |
| map-items | items placed by a helper under `map` (10 to 1,000) | item 3, item 0 fixed |

**The external pass** walks every `app.ptl` of `examples/{games,productivity,
dashboards,custom-apps,ui}`, plus copies (on `/Volumes/bureau`, never in the
repo) of worlds-fair's `ui/ptl` screens, petal-creative's petal-images and
cheesecake's games. It runs each file that loads headless with Garden's panel
stubs. Each runnable app gets six drags, "move emit k's x by +10", over
circles, rects, lines and texts sampled across the draw list. These drags
have no `expect=`, so success there means verified and holding after a hot
reload.

## 3. What breaks it, and why

The expected failures (`xfail`), grouped by cause. Each one is a goal in an
example file, with the reason written next to it.

**Meaning that the numbers cannot see (needs a hint or the client).**

- *A cosmetic offset in the goal's own call.*
  `draw_circle(20 + r.x * scale, …)` is the projectile's landing marker. The
  `20` moves only the marker, so it has no collateral and wins (l09,
  l09b). This is the v1 behaviour and is right for most drags, but here it
  detaches the marker from the trajectory. With `rigid` on the drawing
  constants (l09c), the drag reaches `v0` or `angle` through 200 steps of
  simulation.
- *Physics and drawing constants.* "Higher arc, same landing" is solved by
  moving the ground line (`380`), or by changing the integration step
  `dt_sim`. Both are verified, and neither is what a designer means (l09).
  l09c fixes both with `rigid`.
- *Counts.* In the dashboard, KPI tile 2 can move with tile 1 fixed only if
  tile 1 widens. The cheapest edit is the `3` in
  `(right_w - 3 * tile_gap) / 4`, which is the number of gaps
  (big-server-monitoring kpi-gap). Discovery calls a literal structural only
  when it decides control flow; a count used in arithmetic is fair game. A
  `rigid(3)` hint fixes it. A "count-like literal" heuristic (an int that
  multiplies a gap or divides a width) could too.
- *Auto-scaled axes.* A bar's height relative to an axis computed from the
  data's maximum barely moves when its own value changes (48 → 50 gains
  8 px; 51 rescales the axis). The engine instead finds the `10` that
  `axis_max` rounds to, which rescales every bar (l10 tallest-up). With
  `deny=fn:axis_max` it moves the chart (`top`), which is the only other
  thing that can move that bar's top.
- *Translate or reshape.* Dragging a tree's leaf diagonally ties between
  reshaping it (`shrink`, `spread`) and translating it (the root's `380`).
  Reshaping moves the other leaves about as much as translating does (l07
  leaf-xy). With the client naming the shape knobs (`leaf-xy-client`), or
  with the hints of l07b, it is the shape.

**Discontinuities.**

- *A branch that hides a literal.* The puck stops at a wall:
  `if x > wall - r then wall - r else x`. At the baseline the puck is left of
  the wall, so `wall`'s column is zero and only `x` is measured. Past the flip
  `x` stops working, and the other literals are never re-measured. The best
  effort stops at the wall, unverified (a04 into-wall), even when the client
  names `wall`. The fix (not done) is to re-screen the zero columns at the
  first point where the fingerprint changed.
- *A step function, flat at the probe size.*
  `round(x / grid) * grid` does not move for a 0.1% nudge of `x`. Screening
  drops `x` as inert before the Jacobian's step escalation could find the
  next jump (a05 snap-next, even with only `x` allowed). The fix (not done)
  is to give inert-at-screening literals one 10% probe when nothing usable
  was found.
- *No exact answer.* A 4 px drag on a 20 px grid (a05 snap-small), or a 10 px
  drag of a bar whose int data moves it 4 px per unit (l10 bar2-up-exact),
  has no edit within ±0.5 px. The best effort is reported, unverified.
  `tol=2` (bar2-up-snap) accepts the snapped answer.

**Hint ranges doing their job.** `tunable(100, 50, 150)` stops a 180 px
drag at 150 (a07 x-out-of-range). l07b's `shrink` range keeps the tree from
growing past 0.8, so the engine falls back to the root (shrink-clamped).

**Outside the ladder: the external pass** (the 18 drags that did not succeed
out of 258):

- *The budget runs out on big apps* (6 drags: settings, video-timeline,
  solar-system, finance-dashboard, tower-defense). These apps have 160 to
  530 eligible literals per drag. Screening infers some literals as moving
  from their group without probing them alone, and on these apps that
  inference is wrong often enough that the 32 measured columns come out zero
  or unusable, and the 60% of the budget the Jacobian may use is gone. A
  larger budget, or probing the inferred singletons, would close most of
  these.
- *Geometry in `state`* (diagram-canvas `interactive.ptl`, 4 drags): every
  node position is a `state` initializer the live checkpoint already holds,
  so no literal moves the frame (§2.1.2). Editing a live app's *state* is a
  different operation (an inline-state edit) from editing its code.
- *Nothing editable reaches the goal* (crm-contact-manager, markdown-editor,
  paint, 4 drags): the shape's x is computed from text widths, from library
  layout code, or from structural literals only.
- *Int-only paths* (calculator, 2 drags): the only literals are ints whose
  steps miss ±0.5 px.
- *No alternative from a tiny candidate set* (flappy, pong, 1 drag each).

**Not runnable headless.** 152 of the 197 external entry files need natives
of another host and never got to a drag:

- 130 cheesecake game scripts (`import cheesecake`, 3D);
- 10 worlds-fair screens (`wf_model`, and `focus_state` from an older ui);
- the petal-fps and fantasy-NES carts;
- the side-scroller (SDL file I/O);
- the petal-images editor (its studio natives).

**What did not break it.**

- *`random()`.* Every draw is replayed by call site, and a star's drag edits
  `cx` or `spread`, never `random`'s arguments (a01, 4 of 4).
- *`var`.* a03 is l05b with a `var` accumulator, and gets the same answers.
  l09 (six `var`s) and its fold version l09b behave identically. A `var` on
  the path only raises the literal's cost weight (×1.5); for finite
  differences it is just more code to re-run.
- *`state` at a live checkpoint.* State is frozen, the per-frame `speed`
  moves the ball, and the hot reload agrees (a02). A restart does not,
  correctly.
- *Size and depth.* None of the following stopped it: 1,600 rects in a row,
  a tree of depth 12, 100,000 simulation steps, 1,024 literals summed into
  one number, 1,000 items under `map`, or the full dashboard (§5).

## 4. Engine changes this study made

The first run of the ladder against M3's engine succeeded on 82 of 105
goals. Most of the misses had one cause: M3 charged an edit by its relative
change `|Δ| / max(|p|, 1)`. That made `150 → 130` (a 13% change of a
coordinate) cheaper than `0 → -20` (the offset the drag means). It made
`30 → 40` (a size) cheaper than `12 → 22` (a gap). And it made a rate or an
angle, whose change is small in its own units, cheaper than either. The
revisions, recorded in [the plan's §9](direct-manipulation-v2-plan.md#9-status):

1. **Share-based edit cost** (`rank::edit_cost`). An edit costs
   `d_i · (share_i + 0.1 · min(rel_i, 10))`, where `share_i = ‖J_i Δ_i‖₁ / ‖r‖₁`
   is the part of the drag the literal carries. Every literal that does the
   drag alone costs its weight. The §2.5.2 weights and the measured
   collateral then decide between them, which is what they were for. The L1
   norm makes a pair where one literal undoes the other's side effect cost
   more than a decoupled pair (`cx`, `cy`).
2. **Occam count.** Each edit beyond the leanest verified alternative's
   costs `κ_n`, not each edit beyond the goal count. Three literals that do
   what one can pay for two extra edits.
3. **Lean subsets.**
   - Config-only tuples (a single knob before the spread over all of them).
   - Single literals for multi-goal drags (an hour hand's angle reaches both
     coordinates, which no straight-line prediction from the baseline sees).
   - Predictions are deduplicated by the literals they actually move, so a
     verified slot is not spent solving `(cy, len)` with `len` unchanged
     after `cy` alone.
4. **Budget shares.** Screening may use 30% of the run budget and screening
   with the Jacobian 60%. At most 32 columns are measured (the cheapest by
   weight, then the nearest). Past 8 columns, a column that does not move is
   escalated once instead of three times. Before this, a program with 64 or
   more relevant literals used the whole budget measuring and proposed
   nothing.
5. **More collateral rows** (8,192, was 512), including the numeric fields of
   rect and point records (`draw_rect(r, c)`). On the dashboard, every tile
   was past the 512th row, so every edit had zero collateral. With
   `SpecRun::read_all` indexing the emits once, this costs no time.
6. **Per-shape collateral** (`rank::collateral_shapes`, `Collateral::shapes`).
   The score charges `Σ_e min(1, move_e / ‖Δ goals‖)` over the other emits,
   compressed logarithmically past 1. Each other shape counts at most once.
   Once something else moves, moving more of it costs less than another edit.
   Before this, in a tree where every literal moves everything, a three-edit
   spread that partly cancelled itself outranked every single knob.
A seventh change was tried and reverted: a renumbering check that unverified
an edit when it changed the control flow and the goal's call drew a different
number of shapes. It rejected correct answers, and the two cases it was meant
for turned out to be correct (§2).

The before and after numbers, on the same examples and harness:

| | M3 ranking | M5 ranking |
|---|---:|---:|
| Ladder: goals that succeed | 82 / 105 | 91 / 105 |
| Ladder: top verified | 99 / 105 | 99 / 105 |
| External: top verified | 198 / 258 (77%) | 240 / 258 (93%) |
| External: median runs | 56 | 56 |
| shared-sum 64 / 256 / 1,024 | no proposal (200 runs) | `6.5→16.5` (130 runs) |
| tree-depth 4 / 8 / 10 | two- and three-edit spreads that move the root | one knob (`shrink`) |
| tree-depth 12 (12,288 emits) | spread with the root | the root alone (still wrong: §5) |

The M3 tests were updated where they pinned scores or orders
(`core/tests/dm_v2_solve.rs`, 3 tests). The CLI (`cli_propose_edit_v2`,
12 tests) and Garden (`garden-script` `dm_v2_drag`, 13 tests) suites pass
unchanged.

## 5. How complex a program can get

Speculative re-runs make reach a non-issue: a literal 200 simulation steps,
five recursion levels or three helper calls away from the shape is found,
measured and solved for exactly like one in the call. What bounds v2 is:

- **Run cost.** It scales with the frame's work:

  | Emits per frame | Median ms per run |
  |---|---:|
  | under 200 | 0.9 |
  | 200 to 1,000 | 2.5 |
  | 1,000 to 3,000 | 5.7 |
  | 3,000 to 10,000 | 30 |

  These are from the external pass, including the fork. The dashboard's two
  baselines and discovery cost about 50 ms before the first speculative run.
  At 20 to 60 runs per drag, a frame up to about 3,000 emits answers within
  half a second. The particle sandbox (6,200 emits), the fractal explorer
  (3,200 emits, 2.8 s) and a 12,000-emit tree (1.3 s) need the run-cost
  work of plan §2.3.5a (forking less, rebuilding statics less) or a smaller
  budget to stay interactive.
- **Candidate count.** A drag on the big apps reaches 200 to 940 literals,
  of which up to 530 are eligible. Screening finds the few that matter in
  logarithmic runs when effects are additive. When they are not, the budget
  runs out (§3).
- **Ambiguity grows with sharing, not size.** The more shapes a literal
  feeds, the more equally good edits exist. A row (`gap` versus `size`), a
  keypad (`unit`), a tree (shape versus position) and an equal-width tile row
  (a count) all need either a second goal ("keep this one fixed", which
  resolves most of them on the ladder), a hint, or the client's choice. At
  12,288 emits the tree's rightmost-leaf drag picks the root: every literal
  moves nearly every shape, and the collateral term stops discriminating.
  Programs that name their knobs (`config let`, `tunable`) and mark their
  constants (`rigid`) are the modifiable end of the sliding scale.
  Avoiding `var` matters much less for v2 than for v1.
- **Discontinuity.** Branches, `round`/`floor`, int literals and int division
  make columns zero, stepped or one-sided. Verification keeps every answer
  honest (no unverified alternative is ranked above a verified one), but a
  literal hidden behind a branch, or flat at the probe size, is not found.

<!-- the tables below are generated -->

## 6. Results

<!-- study:begin -->
<!-- Generated by core-libs/petal-ui/tests/dm_v2_study.rs; do not edit by hand. -->

Build: release. Ladder: 105 goals over 27 examples; 91 succeed (87%), top alternative verified on 99 (94%), some alternative verified on 99. Goals not marked as expected failures: 91/91 succeed. Median 20.0 runs and 5 ms per goal, at most 66 runs.

#### Examples

| Example | Level | Lines | Fns | Loops | var | state | config | hints | Emits | Clock | RNG |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| `a01-rand` | A1 | 12 | 0 | 1 | 0 | 0 | 2 | 0 | 15 |  | yes |
| `a02-state` | A2 | 7 | 0 | 0 | 0 | 1 | 0 | 0 | 4 |  |  |
| `a02b-state-fresh` | A2 | 6 | 0 | 0 | 0 | 1 | 0 | 0 | 3 |  |  |
| `a03-var` | A3 | 10 | 0 | 1 | 1 | 0 | 0 | 0 | 6 |  |  |
| `a04-branch-flip` | A4 | 10 | 0 | 0 | 0 | 0 | 0 | 0 | 5 |  |  |
| `a05-integer` | A5 | 17 | 0 | 2 | 0 | 0 | 0 | 0 | 14 |  |  |
| `a06-shared-literals` | A6 | 15 | 1 | 1 | 0 | 0 | 0 | 0 | 26 |  |  |
| `a07-hints` | A7 | 7 | 0 | 0 | 0 | 0 | 0 | 3 | 3 |  |  |
| `big-server-monitoring` | BIG | 711 | 27 | 31 | 0 | 12 | 0 | 0 | 1503 | yes | yes |
| `l00-literal` | L0 | 3 | 0 | 0 | 0 | 0 | 0 | 0 | 3 |  |  |
| `l01-binding` | L1 | 6 | 0 | 0 | 0 | 0 | 0 | 0 | 4 |  |  |
| `l01b-binding-config` | L1 | 6 | 0 | 0 | 0 | 0 | 2 | 0 | 4 |  |  |
| `l02-helper` | L2 | 11 | 1 | 0 | 0 | 0 | 0 | 0 | 5 |  |  |
| `l03-nested` | L3 | 23 | 4 | 2 | 0 | 0 | 0 | 0 | 8 |  |  |
| `l04-clock-hand` | L4 | 17 | 2 | 0 | 0 | 0 | 0 | 0 | 5 |  |  |
| `l04b-orbit-time` | L4 | 10 | 0 | 0 | 0 | 0 | 0 | 0 | 4 | yes |  |
| `l05-loop-row` | L5 | 8 | 0 | 1 | 0 | 0 | 0 | 0 | 8 |  |  |
| `l05b-map-running-sum` | L5 | 11 | 0 | 1 | 0 | 0 | 0 | 0 | 6 |  |  |
| `l06-records` | L6 | 20 | 1 | 1 | 0 | 0 | 0 | 0 | 5 |  |  |
| `l07-fractal-tree` | L7 | 17 | 1 | 0 | 0 | 0 | 0 | 0 | 97 |  |  |
| `l07b-fractal-tree-hints` | L7 | 17 | 1 | 0 | 0 | 0 | 1 | 2 | 97 |  |  |
| `l08-flex-row` | L8 | 26 | 2 | 2 | 0 | 0 | 0 | 0 | 12 |  |  |
| `l09-projectile` | L9 | 36 | 1 | 2 | 6 | 0 | 0 | 0 | 13 |  |  |
| `l09b-projectile-pure` | L9 | 26 | 1 | 1 | 0 | 0 | 0 | 0 | 13 |  |  |
| `l09c-projectile-hints` | L9 | 37 | 1 | 2 | 6 | 0 | 0 | 5 | 13 |  |  |
| `l10-bar-chart` | L10 | 29 | 2 | 2 | 0 | 0 | 0 | 0 | 22 |  |  |
| `l10b-ui-panel` | L10 | 15 | 0 | 0 | 0 | 0 | 0 | 0 | 17 |  |  |

#### Goals

| Example | Goal | Status | Top edit | Runs | ms | Cands d/e/s/m | Alts (verified) | Aids | Error |
|---|---|---|---|---:|---:|---|---|---|---|
| `a01-rand` | star-right | ok | `160→183` | 23 | 5 | 3/3/3/3 | 3 (3) | config | 0.10 |
| `a01-rand` | center-marker | ok | `200→220` | 5 | 3 | 1/1/1/1 | 1 (1) | config | 0.00 |
| `a01-rand` | star-bigger | ok | `4→7` | 5 | 3 | 3/1/1/1 | 1 (1) | — | 0.00 |
| `a01-rand` | star-pair | ok | `160→183` | 20 | 5 | 3/3/3/3 | 3 (3) | config | 0.10 |
| `a02-state` | ball | ok | `2.0→8.0` | 5 | 4 | 2/1/1/1 | 1 (1) | — | 0.00 |
| `a02-state` | ball-y | ok | `100→120` | 4 | 4 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `a02-state` | shadow | ok | `4→(-1)` | 10 | 5 | 3/2/2/2 | 2 (2) | — | 0.00 |
| `a02b-state-fresh` | ball | ok | `40.0→46.0` | 11 | 3 | 2/2/2/2 | 2 (2) | — | 0.00 |
| `a03-var` | widen-box1 | ok | `60→80` | 8 | 3 | 4/4/4/1 | 1 (1) | — | 0.00 |
| `a03-var` | box3-gap | ok | `10→15` | 27 | 5 | 6/6/6/5 | 3 (3) | — | 0.00 |
| `a03-var` | first | ok | `20→30` | 10 | 3 | 6/6/6/1 | 1 (1) | — | 0.00 |
| `a04-branch-flip` | small | ok | `180→185` | 7 | 3 | 3/3/3/1 | 1 (1) | — | 0.00 |
| `a04-branch-flip` | into-wall | **xfail** (expected: at the baseline the puck is left of the wall, so `wall`'s column is zero and only `x` is measured; past the flip `x` stops working and the Jacobian is not re-measured for the other literals (best effort stops at the wall)) | `180→220` | 19 | 4 | 3/3/3/1 | 1 (0) | — | 2.00 |
| `a04-branch-flip` | into-wall-client | **xfail** (expected: same: the client names `wall`, but its column is still zero at the baseline) | `180→220` | 18 | 4 | 3/2/2/1 | 1 (0) | client | 2.00 |
| `a04-branch-flip` | bar-step | ok | `60→90` | 4 | 3 | 4/1/1/1 | 1 (1) | — | 0.00 |
| `a05-integer` | snap-next | **xfail** (expected: screening nudges `x` by 0.1% and sees nothing move (the grid is flat there), so `x` is dropped before the Jacobian could escalate its step; `grid` alone cannot reach 140) | `20→22` | 10 | 3 | 2/2/2/1 | 1 (0) | — | 8.00 |
| `a05-integer` | snap-next-client | **xfail** (expected: same: with only `x` allowed, screening still drops it as inert) | — | 0 | 2 | 0/0/0/0 | 0 (0) | client | — |
| `a05-integer` | snap-small | **xfail** (expected: a 4 px drag on a 20 px grid has no exact answer (best effort leaves it where it was)) | `20→21` | 5 | 3 | 2/2/2/1 | 1 (0) | — | 2.00 |
| `a05-integer` | tile-row | ok | `30→40` | 31 | 6 | 5/4/4/3 | 2 (1) | — | 0.00 |
| `a05-integer` | int-bar | ok | `30→34` | 21 | 5 | 8/7/7/3 | 3 (2) | — | 0.00 |
| `a06-shared-literals` | one-key | ok | `8→10` | 31 | 7 | 11/10/10/5 | 3 (2) | — | 0.00 |
| `a06-shared-literals` | spread | ok | `8→10` | 30 | 7 | 11/10/10/5 | 2 (1) | — | 0.00 |
| `a06-shared-literals` | spread-client | ok | `8→10` | 4 | 3 | 11/1/1/1 | 1 (1) | client | 0.00 |
| `a06-shared-literals` | key-size | ok | `5→6` | 29 | 6 | 11/10/10/3 | 2 (1) | — | 0.00 |
| `a07-hints` | x-in-range | ok | `100→130` | 4 | 2 | 3/1/1/1 | 1 (1) | config, hint | 0.00 |
| `a07-hints` | x-out-of-range | **xfail** (expected: the range stops x at 150: best effort ends 30 px short, not within tolerance) | `100→150` | 4 | 3 | 3/1/1/1 | 1 (0) | config, hint | 30.00 |
| `a07-hints` | radius-step | ok | `10→16` | 4 | 3 | 4/1/1/1 | 1 (1) | config, hint | 1.00 |
| `a07-hints` | y-rigid | ok | `0→10` | 4 | 3 | 2/1/1/1 | 1 (1) | — | 0.00 |
| `big-server-monitoring` | header-taller | ok | `76→86` | 4 | 88 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `big-server-monitoring` | fleet-wider | ok | `316→336` | 12 | 147 | 12/12/12/1 | 1 (1) | — | 0.00 |
| `big-server-monitoring` | kpi-taller | ok | `104→120` | 33 | 308 | 374/11/11/3 | 1 (1) | — | 0.00 |
| `big-server-monitoring` | kpi-gap | **xfail** (expected: the tiles split the row equally, so tile 2 moves with tile 1 fixed only if tile 1 widens: the cheapest edit is the gap count `3` in `(right_w - 3 * tile_gap) / 4` (3→0), a count no hint marks rigid) | `3→0` | 62 | 579 | 374/11/11/8 | 3 (3) | — | 0.00 |
| `big-server-monitoring` | chart-down | ok | `104→114` | 29 | 318 | 11/11/11/4 | 3 (3) | — | 0.00 |
| `big-server-monitoring` | host-dot | ok | `20.0→26.0` | 34 | 335 | 379/16/16/3 | 2 (2) | — | 0.00 |
| `big-server-monitoring` | hero-dot | ok | `4.5→6.5` | 9 | 125 | 5/5/5/1 | 1 (1) | — | 0.00 |
| `l00-literal` | right | ok | `120→150` | 4 | 3 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `l00-literal` | diagonal | ok | `120→135`, `80→100` | 10 | 3 | 2/2/2/2 | 3 (1) | — | 0.00 |
| `l00-literal` | radius | ok | `20→32` | 4 | 3 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `l01-binding` | circle-x | ok | `140→160` | 4 | 2 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `l01-binding` | circle-y | ok | `90→75` | 4 | 3 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `l01-binding` | bar-x | ok | `30→10` | 10 | 3 | 2/2/2/2 | 3 (3) | — | 0.00 |
| `l01-binding` | bar-both | ok | `140→160` | 9 | 3 | 2/2/2/2 | 2 (1) | — | 0.00 |
| `l01b-binding-config` | bar-x | ok | `140→160` | 10 | 3 | 2/2/2/2 | 3 (3) | config | 0.00 |
| `l01b-binding-config` | bar-x-strict | ok | `140→160` | 8 | 3 | 2/2/2/2 | 1 (1) | strict, config | 0.00 |
| `l01b-binding-config` | circle-y | ok | `90→75` | 4 | 3 | 1/1/1/1 | 1 (1) | config | 0.00 |
| `l02-helper` | right-circle | ok | `60→90` | 14 | 4 | 4/4/4/2 | 3 (3) | — | 0.00 |
| `l02-helper` | left-circle-up | ok | `0→-20` | 11 | 3 | 4/4/4/2 | 2 (2) | — | 0.00 |
| `l02-helper` | center-dot | ok | `200→225` | 6 | 3 | 2/2/2/1 | 1 (1) | — | 0.00 |
| `l02-helper` | all-three | ok | `200→225` | 18 | 4 | 6/6/6/3 | 3 (1) | — | 0.00 |
| `l03-nested` | one-dot | ok | `8→20` | 44 | 7 | 10/8/8/6 | 3 (2) | — | 0.00 |
| `l03-nested` | spread-row | ok | `8→16` | 39 | 6 | 10/8/8/6 | 3 (2) | — | 0.00 |
| `l03-nested` | top | ok | `30→40` | 30 | 5 | 10/8/8/4 | 1 (1) | — | 0.00 |
| `l03-nested` | radius | ok | `0.4→0.5` | 10 | 3 | 2/2/2/2 | 2 (2) | — | 0.00 |
| `l04-clock-hand` | to-4-oclock | ok | `2.0→4.0` | 57 | 7 | 7/7/7/7 | 3 (3) | — | 0.00 |
| `l04-clock-hand` | to-5-oclock | ok | `2.0→5.0` | 51 | 7 | 7/7/7/7 | 3 (3) | — | 0.00 |
| `l04-clock-hand` | longer | ok | `80→100` | 60 | 7 | 7/7/7/7 | 3 (3) | — | 0.02 |
| `l04-clock-hand` | tip-x-only | ok | `80→89` | 31 | 5 | 7/7/7/6 | 3 (3) | — | 0.21 |
| `l04-clock-hand` | to-3-oclock-client | ok | `2.0→3.0` | 6 | 3 | 7/1/1/1 | 1 (1) | client | 0.00 |
| `l04b-orbit-time` | along-orbit | ok | `0.0→0.41` | 42 | 10 | 5/5/5/5 | 3 (3) | — | 0.30 |
| `l04b-orbit-time` | outward | ok | `90→110` | 40 | 10 | 5/5/5/5 | 3 (3) | — | 0.25 |
| `l04b-orbit-time` | along-speed-only | ok | `0.8→1.08` | 6 | 4 | 5/1/1/1 | 1 (1) | client | 0.18 |
| `l05-loop-row` | one | ok | `12→22` | 24 | 5 | 5/4/4/4 | 3 (2) | — | 0.00 |
| `l05-loop-row` | first | ok | `40→55` | 11 | 3 | 5/4/4/2 | 1 (1) | — | 0.00 |
| `l05-loop-row` | spread-client | ok | `12→22` | 4 | 3 | 5/1/1/1 | 1 (1) | client | 0.00 |
| `l05-loop-row` | alone | ok | `12→22` | 28 | 5 | 5/4/4/4 | 3 (3) | — | 0.00 |
| `l05-loop-row` | height | ok | `30→40` | 4 | 3 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `l05b-map-running-sum` | widen-box1 | ok | `60→80` | 9 | 3 | 6/6/6/1 | 1 (1) | — | 0.00 |
| `l05b-map-running-sum` | box3-gap | ok | `30→45` | 26 | 5 | 6/6/6/5 | 3 (3) | — | 0.00 |
| `l05b-map-running-sum` | first | ok | `20→30` | 10 | 3 | 6/6/6/1 | 1 (1) | — | 0.00 |
| `l06-records` | move-rect | ok | `160→185` | 19 | 4 | 12/12/12/2 | 2 (2) | — | 0.00 |
| `l06-records` | move-second-circle | ok | `200→220` | 21 | 4 | 12/12/12/2 | 2 (2) | — | 0.00 |
| `l06-records` | grow-circle | ok | `20→30` | 12 | 3 | 10/10/10/1 | 1 (1) | — | 0.00 |
| `l06-records` | all | ok | `0→10` | 28 | 5 | 12/12/12/4 | 3 (2) | — | 0.00 |
| `l07-fractal-tree` | rightmost-out | ok | `0.7→0.734` | 38 | 15 | 5/5/5/5 | 3 (3) | — | 0.04 |
| `l07-fractal-tree` | taller | ok | `0.7→0.75` | 32 | 14 | 5/5/5/5 | 3 (3) | — | 0.09 |
| `l07-fractal-tree` | leaf-xy | **xfail** (expected: `shrink` with the root's `380` ties with the shape-only pairs: reshaping the tree moves the other leaves about as much as translating it does, so nothing in the score prefers the shape) | `0.7→0.724`, `380→392` | 55 | 20 | 6/6/6/6 | 3 (3) | — | 0.45 |
| `l07-fractal-tree` | leaf-xy-client | ok | `0.7→0.72`, `0.45→0.487` | 30 | 12 | 6/2/2/2 | 3 (1) | client | 0.20 |
| `l07b-fractal-tree-hints` | rightmost-out | ok | `0.7→0.734` | 49 | 18 | 7/4/4/4 | 3 (3) | config, hint | 0.04 |
| `l07b-fractal-tree-hints` | taller | ok | `0.7→0.75` | 30 | 14 | 7/4/4/4 | 3 (3) | config, hint | 0.09 |
| `l07b-fractal-tree-hints` | shrink-clamped | **xfail** (expected: the goal needs shrink > 0.8, past its tunable range, so the engine moves the tree's root (`380`, unhinted) instead) | `380→260` | 54 | 20 | 7/4/4/4 | 3 (1) | — | 0.00 |
| `l08-flex-row` | widen-first | ok | `8→13` | 27 | 22 | 9/6/6/4 | 3 (1) | — | 0.00 |
| `l08-flex-row` | last-right | ok | `6→11` | 33 | 21 | 9/6/6/6 | 3 (2) | — | 0.00 |
| `l08-flex-row` | last-right-client | ok | `6→11` | 29 | 21 | 9/5/5/5 | 3 (1) | — | 0.00 |
| `l08-flex-row` | label-x | ok | `6→12` | 30 | 16 | 9/6/6/6 | 3 (2) | — | 0.00 |
| `l08-flex-row` | bar-down | ok | `10→20` | 4 | 3 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `l09-projectile` | land-further | **xfail** (expected: the `20` in the marker's own draw call moves only the marker (no collateral), so it wins; see l09c for the hints that rule it out) | `20→60` | 38 | 10 | 8/8/8/6 | 3 (3) | — | 0.00 |
| `l09-projectile` | land-further-client | ok | `0.9→0.83` | 6 | 4 | 8/1/1/1 | 1 (1) | client | 0.27 |
| `l09-projectile` | higher-arc | **xfail** (expected: the physics and drawing constants (`dt_sim`, `ground`) are as editable as the launch and win (moving the ground line raises the whole arc), unless marked `rigid` (l09c)) | `380→350` | 66 | 15 | 9/9/9/9 | 3 (3) | — | 0.00 |
| `l09b-projectile-pure` | land-further | **xfail** (expected: as in l09: the marker's own `20` offset wins) | `20→60` | 36 | 11 | 13/8/8/6 | 3 (3) | — | 0.00 |
| `l09b-projectile-pure` | land-further-client | ok | `0.9→0.83` | 6 | 4 | 13/1/1/1 | 1 (1) | client | 0.27 |
| `l09c-projectile-hints` | land-further | ok | `0.9→0.83` | 22 | 7 | 8/4/4/3 | 3 (3) | — | 0.27 |
| `l09c-projectile-hints` | higher-arc | ok | `0.0→10.0` | 45 | 12 | 9/4/4/4 | 3 (3) | — | 0.00 |
| `l09c-projectile-hints` | land-closer | ok | `0.9→0.998` | 41 | 11 | 8/4/4/3 | 3 (3) | — | 0.20 |
| `l10-bar-chart` | bar2-up | ok | `22→26` | 39 | 7 | 14/14/14/5 | 3 (3) | — | 0.00 |
| `l10-bar-chart` | bar2-up-exact | **xfail** (expected: an int data literal moves the bar 4 px per unit, so 10 px is out of reach within ±0.5 px; the exact answer found instead rescales the axis (`10` → 9)) | `10→9` | 34 | 7 | 14/14/14/5 | 3 (3) | — | 0.22 |
| `l10-bar-chart` | bar2-up-snap | ok | `22→25` | 34 | 7 | 14/14/14/5 | 3 (3) | — | 2.00 |
| `l10-bar-chart` | wider-bars | ok | `36→42` | 25 | 5 | 14/14/14/2 | 1 (1) | — | 0.00 |
| `l10-bar-chart` | plot-right | ok | `50→70` | 4 | 3 | 1/1/1/1 | 1 (1) | — | 0.00 |
| `l10-bar-chart` | tallest-up | **xfail** (expected: the axis is scaled to the data, so the tallest bar's height relative to it barely moves: 48→50 gains 8 px and 51 rescales the axis. The engine finds the `10` that axis_max rounds to (it rescales every bar)) | `10→9` | 45 | 8 | 14/14/14/5 | 3 (3) | — | 1.33 |
| `l10-bar-chart` | tallest-up-deny | ok | `30→10` | 38 | 7 | 14/11/11/4 | 3 (2) | — | 0.00 |
| `l10b-ui-panel` | card-right | ok | `20→40` | 9 | 4 | 4/4/4/1 | 1 (1) | — | 0.00 |
| `l10b-ui-panel` | taller-header | ok | `28→38` | 25 | 8 | 14/8/8/4 | 3 (3) | — | 0.00 |
| `l10b-ui-panel` | more-padding | ok | `12→18` | 21 | 7 | 14/8/8/3 | 2 (1) | — | 0.00 |
| `l10b-ui-panel` | row-gap | ok | `240→272` | 43 | 11 | 15/8/8/5 | 3 (3) | — | 0.00 |
| `l10b-ui-panel` | face-x | ok | `12→22` | 22 | 11 | 51/8/8/3 | 3 (3) | — | 0.00 |

#### Scaling sweep

| Family | Size | Emits | Status | Top edit | Runs | ms | Cands d/e/s/m | Note |
|---|---:|---:|---|---|---:|---:|---|---|
| row | 5 | 6 | ok | `2.5→5.0` | 22 | 4 | 5/4/4/4 |  |
| row | 20 | 21 | ok | `2.5→3.03` | 19 | 4 | 5/4/4/4 |  |
| row | 100 | 101 | ok | `2.5→2.6` | 18 | 6 | 5/4/4/4 |  |
| row | 400 | 401 | ok | `2.5→2.524` | 18 | 14 | 5/4/4/4 |  |
| row | 1600 | 1601 | ok | `2.5→2.506` | 18 | 45 | 5/4/4/4 |  |
| tree-depth | 2 | 12 | ok | `0.72→0.88` | 33 | 6 | 5/5/5/5 |  |
| tree-depth | 4 | 48 | ok | `0.72→0.773` | 36 | 10 | 5/5/5/5 |  |
| tree-depth | 6 | 192 | ok | `0.72→0.756` | 38 | 26 | 5/5/5/5 |  |
| tree-depth | 8 | 768 | ok | `0.72→0.755` | 39 | 98 | 5/5/5/5 |  |
| tree-depth | 10 | 3072 | ok | `0.72→0.76` | 32 | 325 | 5/5/5/5 |  |
| tree-depth | 12 | 12288 | fail | `400→415` | 31 | 1302 | 5/5/5/5 | top alternative edits ["400→415"], expected ⊆ ["spread", "shrink", "trunk"] |
| sim-steps | 10 | 2 | ok | `0.9→0.79` | 40 | 6 | 8/7/7/5 |  |
| sim-steps | 100 | 2 | ok | `0.9→0.79` | 40 | 7 | 8/7/7/5 |  |
| sim-steps | 1000 | 2 | ok | `0.9→0.79` | 40 | 17 | 8/7/7/5 |  |
| sim-steps | 10000 | 2 | ok | `0.9→0.79` | 40 | 118 | 8/7/7/5 |  |
| sim-steps | 100000 | 2 | ok | `0.9→0.79` | 39 | 998 | 8/7/7/5 |  |
| shared-sum | 4 | 2 | ok | `3.5→13.5` | 25 | 4 | 5/4/4/4 |  |
| shared-sum | 16 | 2 | ok | `6.5→16.5` | 73 | 8 | 17/16/16/16 |  |
| shared-sum | 64 | 2 | ok | `6.5→16.5` | 130 | 13 | 65/64/64/32 |  |
| shared-sum | 256 | 2 | ok | `6.5→16.5` | 130 | 15 | 257/256/256/32 |  |
| shared-sum | 1024 | 2 | ok | `6.5→16.5` | 130 | 26 | 1025/1024/256/32 |  |
| map-items | 10 | 11 | ok | `7.5→9.5` | 13 | 4 | 7/5/5/2 |  |
| map-items | 100 | 101 | ok | `7.5→9.5` | 13 | 7 | 7/5/5/2 |  |
| map-items | 1000 | 1001 | ok | `7.5→9.5` | 13 | 37 | 7/5/5/2 |  |

#### External projects (auto-generated goals)

Directories: `examples/games`, `examples/productivity`, `examples/dashboards`, `examples/custom-apps`, `examples/ui`, `/Volumes/bureau/petal-dmv2-study/worlds-fair-ui`, `/Volumes/bureau/petal-dmv2-study/petal-images`, `/Volumes/bureau/petal-dmv2-study/cheesecake-games`.

197 entry files, 45 runnable headless. 258 auto goals ("move emit k's x by +10"): top alternative verified on 240 (93%), and the edit re-run within tolerance on 240; median 56.0 runs, 118 ms.

| Project | Lines | Emits | Goals | Verified | Re-run ok | Median runs | Median ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| `asteroids/app.ptl` | 612 | 228 | 6 | 6 | 6 | 45.0 | 51 |
| `breakout/app.ptl` | 681 | 410 | 6 | 6 | 6 | 83.0 | 133 |
| `flappy/app.ptl` | 458 | 186 | 6 | 5 | 5 | 32.0 | 27 |
| `fractal-explorer/app.ptl` | 1427 | 3227 | 6 | 6 | 6 | 52.0 | 2773 |
| `hopper/game.ptl` | 203 | 62 | 6 | 6 | 6 | 35.0 | 11 |
| `memory/app.ptl` | 1002 | 161 | 6 | 6 | 6 | 61.0 | 102 |
| `particle-sandbox/app.ptl` | 815 | 6213 | 6 | 6 | 6 | 77.0 | 2297 |
| `physics-playground/app.ptl` | 1925 | 282 | 6 | 6 | 6 | 48.0 | 70 |
| `pong/app.ptl` | 449 | 100 | 6 | 5 | 5 | 41.0 | 18 |
| `snake/app.ptl` | 433 | 134 | 6 | 6 | 6 | 44.0 | 23 |
| `solar-system/app.ptl` | 528 | 402 | 6 | 5 | 5 | 123.0 | 175 |
| `tower-defense/app.ptl` | 1199 | 386 | 6 | 5 | 5 | 50.0 | 72 |
| `calculator/app.ptl` | 605 | 116 | 6 | 4 | 4 | 44.0 | 28 |
| `calendar/app.ptl` | 1554 | 452 | 6 | 6 | 6 | 122.0 | 378 |
| `chat/app.ptl` | 1221 | 150 | 6 | 6 | 6 | 122.0 | 260 |
| `command-palette/app.ptl` | 938 | 102 | 6 | 6 | 6 | 50.0 | 27 |
| `crm-contact-manager/app.ptl` | 681 | 287 | 6 | 4 | 4 | 33.0 | 42 |
| `email-client/app.ptl` | 2213 | 244 | 6 | 6 | 6 | 39.0 | 112 |
| `file-browser/app.ptl` | 2688 | 255 | 6 | 6 | 6 | 126.0 | 289 |
| `kanban/app.ptl` | 501 | 180 | 6 | 6 | 6 | 121.0 | 150 |
| `markdown-editor/app.ptl` | 1515 | 309 | 6 | 5 | 5 | 122.0 | 4642 |
| `music-sequencer/app.ptl` | 1255 | 590 | 6 | 6 | 6 | 40.0 | 103 |
| `node-editor/app.ptl` | 1369 | 1216 | 6 | 6 | 6 | 88.0 | 420 |
| `notes/app.ptl` | 1157 | 166 | 6 | 6 | 6 | 56.0 | 93 |
| `paint/app.ptl` | 681 | 440 | 6 | 5 | 5 | 78.0 | 130 |
| `photo-adjust/app.ptl` | 826 | 6426 | 6 | 6 | 6 | 63.0 | 620 |
| `restaurant-pos/app.ptl` | 1814 | 438 | 6 | 6 | 6 | 121.0 | 342 |
| `settings/app.ptl` | 2032 | 380 | 6 | 4 | 4 | 122.0 | 444 |
| `spreadsheet/app.ptl` | 1060 | 272 | 6 | 6 | 6 | 96.0 | 254 |
| `stock-trading/app.ptl` | 1835 | 1014 | 6 | 6 | 6 | 125.0 | 731 |
| `todo/app.ptl` | 923 | 193 | 6 | 6 | 6 | 65.0 | 61 |
| `vector-editor/app.ptl` | 1368 | 1378 | 6 | 6 | 6 | 40.0 | 131 |
| `video-timeline/app.ptl` | 1606 | 2161 | 6 | 5 | 5 | 120.0 | 906 |
| `analytics-dashboard/app.ptl` | 790 | 1476 | 6 | 6 | 6 | 83.0 | 386 |
| `finance-dashboard/app.ptl` | 918 | 535 | 6 | 5 | 5 | 101.0 | 177 |
| `network-graph/app.ptl` | 1760 | 790 | 6 | 6 | 6 | 135.0 | 976 |
| `server-monitoring/app.ptl` | 711 | 1503 | 6 | 6 | 6 | 123.0 | 1002 |
| `timeline-explorer/app.ptl` | 1743 | 1060 | 6 | 6 | 6 | 153.0 | 753 |
| `diagram-canvas/examples/flowchart.ptl` | 95 | 48 | 6 | 6 | 6 | 16.0 | 6 |
| `diagram-canvas/examples/interactive.ptl` | 111 | 25 | 6 | 2 | 2 | 0.0 | 2 |
| `diagram-canvas/examples/org-chart.ptl` | 109 | 72 | 6 | 6 | 6 | 23.0 | 8 |
| `bloom-gallery/app.ptl` | 276 | 77 | 6 | 6 | 6 | 54.0 | 113 |
| `layers/app.ptl` | 101 | 40 | 6 | 6 | 6 | 53.0 | 51 |

Goals that did not succeed:

| Example | Goal | Status | Top edit | Runs | ms | Cands d/e/s/m | Alts (verified) | Aids | Error |
|---|---|---|---|---:|---:|---|---|---|---|
| `flappy/app.ptl` | auto3 | **no-proposal** (no alternatives) | — | 25 | 23 | 51/23/23/2 | 0 (0) | — | — |
| `pong/app.ptl` | auto4 | **no-proposal** (no alternatives) | — | 28 | 14 | 82/22/22/3 | 0 (0) | — | — |
| `solar-system/app.ptl` | auto1 | **no-proposal** (no alternatives) | — | 126 | 175 | 203/166/166/32 | 0 (0) | — | — |
| `tower-defense/app.ptl` | auto3 | **fail** (best effort only, top alternative not verified (error 8.0 τ)) | `24→30` | 129 | 152 | 60/59/59/31 | 1 (0) | — | 4.00 |
| `calculator/app.ptl` | auto2 | **fail** (best effort only, top alternative not verified (error 4.0 τ)) | `4→2` | 43 | 27 | 19/19/19/6 | 3 (0) | — | 2.00 |
| `calculator/app.ptl` | auto3 | **fail** (best effort only, top alternative not verified (error 4.0 τ)) | `4→3` | 44 | 28 | 19/19/19/6 | 2 (0) | — | 2.00 |
| `crm-contact-manager/app.ptl` | auto2 | **no-proposal** (nothing can move the goals: no editable literal moves the goals (line 213: Zero; line 213: Zero; line 213: Zero; line 38: Zero; line 39: Zer…) | — | 0 | 34 | 0/0/0/0 | 0 (0) | — | — |
| `crm-contact-manager/app.ptl` | auto3 | **no-proposal** (nothing can move the goals: no editable literal moves the goals (line 213: Zero; line 213: Zero; line 213: Zero; line 38: Zero; line 39: Zer…) | — | 0 | 33 | 0/0/0/0 | 0 (0) | — | — |
| `markdown-editor/app.ptl` | auto0 | **no-proposal** (nothing can move the goals: no editable literal moves the goals (line 4754: library code (the prelude or a package) is never edited; line 47…) | — | 0 | 405 | 0/0/0/0 | 0 (0) | — | — |
| `paint/app.ptl` | auto2 | **no-proposal** (nothing can move the goals: no editable literal moves the goals (line 77: Zero; line 141: structural: a comparison, index, loop bound or key…) | — | 0 | 54 | 0/0/0/0 | 0 (0) | — | — |
| `settings/app.ptl` | auto0 | **no-proposal** (no alternatives) | — | 120 | 389 | 940/534/256/32 | 0 (0) | — | — |
| `settings/app.ptl` | auto4 | **no-proposal** (no alternatives) | — | 120 | 401 | 940/534/256/32 | 0 (0) | — | — |
| `video-timeline/app.ptl` | auto1 | **no-proposal** (no alternatives) | — | 120 | 860 | 481/255/255/32 | 0 (0) | — | — |
| `finance-dashboard/app.ptl` | auto1 | **fail** (best effort only, top alternative not verified (error 4.0 τ)) | `3→2` | 122 | 222 | 348/179/179/32 | 2 (0) | — | 2.00 |
| `diagram-canvas/examples/interactive.ptl` | auto1 | **no-proposal** (nothing can move the goals: no editable literal moves the goals (line 8: a `state` initializer whose state the checkpoint already holds; lin…) | — | 0 | 2 | 0/0/0/0 | 0 (0) | — | — |
| `diagram-canvas/examples/interactive.ptl` | auto2 | **no-proposal** (nothing can move the goals: no editable literal moves the goals (line 8: a `state` initializer whose state the checkpoint already holds; lin…) | — | 0 | 2 | 0/0/0/0 | 0 (0) | — | — |
| `diagram-canvas/examples/interactive.ptl` | auto3 | **no-proposal** (nothing can move the goals: no editable literal moves the goals (line 8: a `state` initializer whose state the checkpoint already holds; lin…) | — | 0 | 2 | 0/0/0/0 | 0 (0) | — | — |
| `diagram-canvas/examples/interactive.ptl` | auto5 | **no-proposal** (nothing can move the goals: no editable literal moves the goals (line 8: a `state` initializer whose state the checkpoint already holds; lin…) | — | 0 | 2 | 0/0/0/0 | 0 (0) | — | — |

Not runnable headless:

| Why | Files |
|---|---:|
| calls a host function the headless runtime lacks (focus_state) | 2 |
| calls a host function the headless runtime lacks (wf_model) | 8 |
| needs its embedder's modules (cheesecake) | 130 |
| needs native fns of another host (fps) | 5 |
| needs native fns of another host (nes/nes_host) | 4 |
| needs native fns of another host (sdl) | 2 |
| needs native fns of another host (studio) | 1 |

<!-- study:end -->

## 7. Running it

From `core-libs/petal-ui`, with release builds:

```sh
# The ladder (asserted; 6 s in release, 30 s in a debug build): every goal
# not marked xfail must succeed. It is part of petal-ui's `cargo test`.
cargo test --release --test dm_v2_study ladder -- --nocapture

# The whole study: ladder, scaling sweep and external pass, rewriting this
# file's generated section and the JSON (about 3 min).
R=$PWD/../../examples; S=/Volumes/bureau/petal-dmv2-study
DM_STUDY_WRITE=1 \
DM_STUDY_EXTRA=$R/games:$R/productivity:$R/dashboards:$R/custom-apps:$R/ui:$S/worlds-fair-ui:$S/petal-images:$S/cheesecake-games \
  cargo test --release --test dm_v2_study full_study -- --ignored --nocapture
```

The external copies are made with `rsync` of the `.ptl` files of
`~/worlds-fair/ui/ptl`, `~/petal-creative/petal-images` and
`~/cheesecake/games` into `$S`. Without `DM_STUDY_EXTRA` the external pass is
skipped.

Filters and diagnostics:

- `DM_STUDY_ONLY=l07,a0` runs only the examples whose names contain one of
  these;
- `DM_STUDY_GOAL=<name>` runs only that goal;
- `DM_STUDY_DUMP=1` prints the draw list;
- `DM_STUDY_VERBOSE=1` prints every alternative with its score parts, the
  candidate columns and the re-run comparison;
- `DM_STUDY_EXPLAIN=1` prints the solver's explain log;
- `DM_STUDY_DIR=<dir>` runs a ladder from another directory;
- `DM_STUDY_PROBE=<file>:<line>:<literal>:<value>` with the ignored
  `probe_override` test compares a slot override with the Recompile oracle.
