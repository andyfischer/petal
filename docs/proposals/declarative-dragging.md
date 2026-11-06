# Proposal: Declarative dragging (D2) for Petal

Status: proposed, 2026-11-05

Source paper: Horowitz, Blinn, Heer, *Declarative Dragging* (UIST '26),
`~/home/papers/D2.pdf`. Its library is Dragology (TypeScript/React/SVG).

## Summary

D2 builds drag-and-drop from two ideas:

1. **Model-driven dragging.** The author says which *model states* a drag can
   reach. The engine renders each one speculatively, finds the dragged
   element in each rendering, and builds the interaction from those
   positions.
2. **Composable drag behaviours.** A small set of operators (`closest`,
   `between`, `vary`, `withFloating`, `withSnapRadius`, `whenFar`,
   `dropTarget`, `onDrop`, chaining) are combined into a drag spec. The
   operators handle snapping, floating, interpolation and animation.

The paper names three things the idea needs. Petal already has all three:

- Views are functions of state that can be re-run.
- Speculative execution is safe: values are immutable, and effects are
  declared on each native.
- There is a vocabulary for "solve for the input that puts this output
  there", from direct manipulation (DM) v2.

None of that machinery is pointed at the *running app* yet. Both DM v2 and
goal-based editing edit *source*.

**Recommendation: adopt, in phases, with one new runtime primitive.**

- **P0: a `drag` library in petal-ui, with no language change.** It works on
  views written as pure `model -> scene` functions. A prototype of `closest`
  plus floating, with layout animation, runs today
  ([prototype](declarative-dragging/proto.ptl), §Before and after).
- **P1: `speculate(f, ...args)`, a hypothetical call.** It runs `f` with state
  writes discarded, escaping effects suppressed, and its emits and
  placements captured and returned. It does not fork the heap. This is the
  one change to the language and VM. It lets ordinary immediate-mode petal-ui
  views, which draw as they go, be speculated without rewriting them as data.
  It also fixes an existing wart in `ui_measure`.
- **P2: `between` and `vary`.** `vary` uses the DM v2 solver with its notion
  of a lever generalized from "source literal" to "number at a path in a
  candidate model". Sensitivities come from duals where they survive and
  from finite differences elsewhere.
- **P3: making candidate states easy to construct.** The paper found this to
  be the hardest part. The fix is helpers (`moves`, `set_in`) and the
  already-wanted multi-generator `for`, not `amb`.
- **P4: connections to goal-based editing and tooling.** A drop on a model
  that is a literal (a `config let`) becomes `Goal::should_insert` /
  `should_remove` / `should_set_path`. That gives *structural* direct
  manipulation, which DM v2 cannot do, since it edits numbers only. The
  paper's visualizers come almost free because a spec is data.

The strongest argument against is in §Assessment → Ease of writing. Petal
apps are not written as `view(model)` today. In the paper's own study,
enumerating reachable states was the main point of friction. If P0 does not
get a real app ported (kanban is the test), P1 should not be built.

## Motivation

### Drag code in the corpus is large, hand-rolled and unanimated

A survey of `.ptl` files across `examples/`, `garden/`, the prelude and
bloom, `~/petal-creative`, `~/cheesecake/games`, `~/worlds-fair/ui/ptl` and
`~/biz/petal-lang.org` found the following:

| App | Drag logic | Drag modes | Snapping | Animated drop |
|---|---|---|---|---|
| `examples/productivity/video-timeline/app.ptl` (110, 352-376, 524-724) | ~200 lines | move, trim L/R, scrub, bin→track | edges, playhead, markers | no |
| `petal-creative/petal-design/packages/studio/canvas.ptl` (549, 690-868) | ~180 lines | 11 kinds | native `snap`, 15° rotation | no |
| `examples/productivity/music-sequencer/app.ptl` (184, 600-780) | ~180 lines | 11 kinds | to steps | no |
| `examples/productivity/node-editor/app.ptl` (557, 770-920) | ~150 lines | pan, wire, node, marquee, place | 16px grid with Shift | no |
| `examples/productivity/calendar/app.ptl` (302, 505-545, 985-1094) | ~130 lines | move, resize, create, all-day, month | 15 min | no |
| `examples/productivity/kanban/app.ptl` (105, 198-264, 375-414) | ~110 lines | reorder within and across columns | none | no |
| `petal-design/.../layers.ptl:58-81`, `petal-images/ui/editor.ptl:796-883` | ~40 lines each | tree reorder: above, below, into | bands | no |

Every one keeps a `state drag = {mode/kind, sx, sy, moved, …}` record and
does its own hit testing with `point_in`. Every one writes its own
press → slop → in-flight → release state machine. **None animates the drop
or the snap.** The only animated snap in the corpus is a stepped slider in
`examples/productivity/settings/widgets.ptl:224-265`. Every drag-heavy file
has 0 calls to `approach`, `spring` or `ease`.

The prelude's drag helper `drag_state`/`drag_update`/`insertion_index`
(`core-libs/petal-ui/prelude/ui.ptl:3904-4003`) has one user,
`petal-design` (`layers.ptl:94,203`, `assets.ptl:170-524`). It handles
press, slop and release only. It has no snapping, animation or drop
targets.

Several apps already write *the inverse of their own view* by hand:

- calendar's `plan()` (`calendar/app.ptl:510-545`) turns the pointer back
  into a preview event `pv` through `min_at(my)` and `col_at(n, mx)`;
- kanban's `insert_index` (`:198-210`) walks card midpoints;
- `layers.ptl`'s `drop_target` (`:58-81`) uses 25/50/75% bands.

That inverse is exactly what D2 removes: the engine finds the drop
positions by running the view forwards.

### Petal has the pieces, pointed elsewhere

| D2 needs | Petal has | Gap |
|---|---|---|
| A view that can run on many model states | Whole-script re-run each frame; pure functions; immutable values | Apps draw imperatively from top-level `state`; there is no `view(model)` separation and no way to run a view "as if" |
| Speculative rendering without side effects | `fork_execution` / `run_speculative` (`core/src/env/fork.rs:36`, `env/run.rs:609`); the DM v2 spec gate that suppresses escaping natives (`vm/native.rs:532-551`) | Host-only. Each run deep-copies the heap (`heap.rs:1275`), costing 0.9-30 ms per run (`direct-manipulation-v2-study.md:321-334`). Pathed state cannot be set in a fork (`state_json.rs:150-160`) |
| Finding "the same element" across renderings | Implicit call keys and `for … key` (`state-call-paths.md:75-102`, `retained-scene-output.md:606-730`); `ui_target(id, r)` (`ui.ptl:938`); DM `EmitKey` (`dm/observe.rs:214`) | `EmitKey` assumes unchanged control flow, which a reorder breaks. Draw commands carry no element id (`draw.rs:107-200`) |
| Interpolation and animation | `approach`, `ease_*` (`ui.ptl:4153-4198`); bloom `spring`, `lerp_rect` (`bloom/src/motion.ptl`); `color_lerp` | No keyed layout animation, so nothing animates "this element moved" |
| `vary`: solve numbers so an element tracks the pointer | DM v2 solver: `linear_step`, `goal_step`, `Bound`, Broyden updates (`dm/solve.rs`); zero-run `preview()` (`dm/api/mod.rs:934`); duals (`builtins/autodiff.rs`) | Levers are literal `TermId`s (`dm/spec.rs:149`). Draw natives refuse duals (`native_fn.rs:1332-1345`) |
| Composable operators | Values, records, enums, closures in records (`language-guide.md:1755`) | None needed |

## Design

### The model the engine assumes

The app keeps its model in `state`. It renders through a function of the
model, and marks each draggable element and its *placement* with a key:

```petal ignore
import ui
import ui/drag

state board = initial_board()

fn view(b)
  for col in b.cols key col.id do
    for card in col.cards key card.id do
      let r = place(card.id, card_rect(col, card))           // identity + position
      draggable(card.id, r, fn() -> closest(moves_nested(b, card.id)) |> floating())
      draw_card(r, card)
    end
  end
end

let d = drag_session(board, view)       // runs the engine; see "Per frame"
view(d.preview)
if d.dropped then board = d.drop end
```

The three engine calls:

- **`place(key, value) -> value`** is the only hook inside the view. In a
  normal run it records where `key` landed and returns the *displayed*
  value: animated, floating or blended. In a speculative run it records the
  value and returns it unchanged. `value` is usually a rect; it may be any
  number, vector or colour, which is how `between` interpolates colours.
  Keys are compared like `for … key` keys: by Map-key hash, with functions,
  cells, pending values and NaN refused
  ([state-call-paths.md](../dev/state-call-paths.md#22a)).
- **`draggable(key, rect, spec_fn)`** registers a hit target, using the
  same registry as `interact` (`ui.ptl:892`) with the usual one-frame lag.
  It also registers a closure that returns a **drag spec**. The closure is
  called once, at drag start, like `dragologyOnDrag`.
- **`drag_session(model, view)`** holds the engine's state in its own
  call-path `state`. It returns
  `{active, key, preview, drop, dropped, cancelled, spec}`. With no drag in
  flight, `preview` is `model`.

### Drag specs are data

A spec is a plain value, a tagged record. It is not a closure tree, so it
can be printed, diffed, stored in `state`, shown by a visualizer and
inspected by an agent. The constructors:

| Constructor | Meaning (D2 name) |
|---|---|
| `fixed(state)` | One target state (`state`). |
| `closest([spec, …], {stickiness?})` | The sub-behaviour whose target is nearest the pointer. |
| `between([spec, …])` | Interpolate inside the convex hull of the targets: linear for 2, barycentric over a Delaunay triangulation for 3 or more. |
| `vary(state, [path, …], {bounds?, constraint?})` | Continuously solve the numbers at `path`s so the element tracks the pointer. |
| `drop_target(key, state)` | Dropping over element `key` gives `state` (trash, toolbar). |
| `spec \|> floating()` | The dragged element follows the pointer, while others preview (`withFloating`). |
| `spec \|> snap_radius(px, {chain?})` | Pull toward the drop state when within `px` (`withSnapRadius`); `chain` keeps the drag going into the new state. |
| `spec \|> when_far(far_spec, {px?})` | Switch behaviour beyond a distance (`whenFar`). |
| `spec \|> on_drop(fn(state) -> state)` | Transform the drop state (`onDrop`). |
| `switch_to(state, key, spec_fn)` | Switch now and keep dragging a different element (`switchToStateAndFollow`). |

`|>` is ordinary function application, so every operator is a plain `pub fn`
in `ui/drag`. Users can write their own, as the paper argues.

### Per frame

**At drag start**, when a press lands on a registered `draggable`:

1. Call the spec closure. Its value is the *spec* (data).
2. For each distinct target state, speculatively run `view(state)`.
3. Read the `place` value recorded for the dragged key in each run. This is
   the **target table**: an array of positions.
4. Store the spec, the target table and the grab offset in the session's
   state. Every one of these is a plain value, and no closure is kept.

**Every frame while dragging**, the behaviour maps the pointer and the target
table to `{preview, drop, gap}`:

- `closest`, `between`, `snap_radius`, `when_far` and `drop_target` cost
  **zero speculative runs** per frame. They only read the table.
- `vary` costs `k+1` runs per frame (see §vary).

The app then renders `view(preview)`. Each `place` call returns:

- for the dragged key under `floating`: `pointer - grab`;
- under `between`: the blend of that key's recorded values across the
  active targets;
- otherwise: the target value, approached by a keyed spring.

The approach is per-key `state`, so **every element that changes position
between previews animates, with nothing extra in the app**. That is the
"others move out of the way" behaviour. The floating element is drawn in an
overlay layer (`with_layer`, `ui.ptl:810`) so it sits on top. The prototype
showed this needs handling: without it, the dragged card draws in list
order.

**On release** the session reports `dropped` with `drop`, and the app commits
`board = d.drop`. The dragged element's spring carries it from the pointer
into its slot. **Escape** cancels: `cancelled`, with no commit.

### P1: `speculate`, a hypothetical call

P0 needs `view` to be pure (`model -> scene`) or to touch nothing but
`place`. Real petal-ui views draw as they go, call widgets that keep
`state` (`_anim`, `slider`'s `state dragging`), and register hit targets.
Running one speculatively today would draw and register for real, and its
`state` writes would persist; `ui_measure`'s comment says exactly this
(`ui.ptl:962-1005`).

```petal ignore
let r = speculate(view, candidate)
// r.value   what view returned
// r.places  [{key, value}] recorded by place() during the call
// r.emits   the draw commands it emitted, captured rather than drawn
```

The semantics are a write overlay, not a fork:

- **State reads** see the live slot. A slot first reached inside the call,
  such as a new keyed card's widget state, is initialized in the overlay and
  read from there.
- **State writes** (`StateWrite`, `StateInit`) and **`set` on any cell** go
  to the overlay. The overlay is dropped when the call returns.
- **Natives** use the DM v2 spec rules (`native-effects.md:25,85`):
  - escaping effects are not called;
  - contained effects run and are discarded;
  - emits go to a buffer owned by the call;
  - `ui_register` and `place` rows go to that buffer;
  - RNG draws come from a stream forked for the call, so the live stream does
    not advance.
- **The heap is shared.** This is the key difference from
  `fork_execution`. Petal values are immutable
  ([time-travel.md](../features/time-travel.md#why-a-copy-is-exact)), so the
  only mutable things a call can touch are state slots and cells, and the
  overlay covers both. Values allocated inside the call become garbage when
  it returns, unless they are returned.
- **Nesting** is an error (`speculate` inside `speculate`) in the first
  version.
- **Memo**: records made inside the call are not stored. Replaying
  existing records is allowed, because memo validation reads the slots that
  the overlay answers. This is the one invariant that needs a test under the
  differential oracle.

The error messages:

```text
error: speculate() cannot call itself: a speculative call is already running
  --> board.ptl:41:11
   = help: compute the candidate scenes first, then compare them outside
```

```text
warning: speculate(): `save_file` was not called (its effect escapes the run)
  --> board.ptl:58:3      [shown once per callsite, in debug runs]
```

`ui_measure(body)` becomes `speculate(body).places`, which fixes its
persisting-state wart.

### P2: `vary`

`vary(state, ["meetings", i, "start"], {bounds: [0, end - 15]})`:

1. Each frame, solve for the numbers at the paths so that
   `place(key)` ≈ `pointer - grab`.
2. Reuse `dm/solve.rs`. It is already independent of what a lever is
   (`linear_step` at solve.rs:125, `goal_step` at :419, `Bound::project`,
   `GoalRow`). Make `SolveCtx` and `jacobian` generic over a `Lever` trait,
   with two implementations:
   - `LiteralLever`: today's `Param` / `TermId` lever;
   - `PathLever`: `(path, f64)` in a model value. It runs by
     `speculate(view, set_in(model, path, v))`.
3. **Sensitivities**: first try one speculative run with the model numbers
   replaced by `dual(v, 1)`. `place` is Petal code, so it records `value_of`
   and `deriv_of` before anything reaches a draw native. That sidesteps the
   dual refusal at `native_fn.rs:1332`. If the dual raises, falls into
   `int`/`vec2`, or comes back with derivative 0 where finite differences
   say otherwise, fall back to central differences (2 runs per lever).
4. Warm-start from last frame's solution, and take a single Gauss-Newton
   step per frame with a step-length backtrack. Drags change slowly, so one
   step per frame converges. Typical drags have 1-2 levers (paper §5).
5. **Constraints**:
   - `bounds` project the step, as `tunable` bounds already do
     (`solve.rs:229`);
   - `constraint: fn(state) -> bool` rejects a step and shortens it, the
     way `assert` acts as a barrier in DM v2
     (`direct-manipulation.md:218-260`).

### P3: constructing candidate states

The paper found this the hardest part (§8.3, §9). Petal's immutable updates
are already shorter than JavaScript's `structuredClone` + `splice`. These
were verified to run:

```petal
fn moves(xs, i)
  let x = xs[i]
  let rest = remove_at(xs, i)
  for j in range(0, len(xs)) do take(rest, j) ++ [x] ++ drop(rest, j) end
end
```

```petal ignore
let opts = for r in range(0, 4) do
  let s = st
  s.meetings[i].room = r      // path assignment on a local copy
  s
end
```

That second form has a trap. Without `let s = st`, the assignment carries
out of the loop and rebinds `st` (`var.md:449-463`). The proposal:

- **`ui/drag` ships the common sets:**
  - `moves(xs, i)`;
  - `moves_nested(lists, key)` (lists of lists, the kanban case);
  - `tree_moves(tree, path)` (above, below, into, without moving into self:
    `layers.ptl`'s case);
  - `choices(state, path, values)`.
- **One core builtin, `set_in(value, path, v)`**, the expression form of
  path assignment. It has no loop-carried rebinding trap, and `vary` needs
  it internally anyway. `list_insert`/`list_remove` already exist (kanban
  uses them).
- **Multi-generator `for`** (`for a in xs, b in ys do … end`). It is already
  on the wanted list (`docs/tasks/moses-debrief.md:79`). It is the honest
  Petal answer to `amb` and deserves its own proposal.
- **Not `amb`.** Petal deliberately has no continuations or coroutines
  (`pending-values-plan.md:36-46`). The frame loop is its retry mechanism.
  `amb` would be a new control-flow concept for one library.

### P4: connecting to goal-based editing, DM and tooling

**Structural direct manipulation.** When the model *is* a literal, a drop
can be written back to source. Examples: `config let STEPS = [...]`, a
Garden panel's layout list, a `.ptl` config file.

- The engine diffs `model` against `drop`. A reorder becomes
  `Goal::should_remove("STEPS[2]")` + `should_insert("STEPS[0]", v)`, and a
  scalar becomes `should_set_path`
  ([goal-based-editing.md](../features/goal-based-editing.md#path-goals)).
- Comments and formatting survive (`literal_edit.rs`). DM v2 cannot express
  "move this element of the list" at all, because its levers are numbers.
- In Garden's edit mode, a draggable over a `config let` model offers the
  same drag *in source*: dragging previews in the running panel, and
  releasing proposes the goal edit.

**One vocabulary for goals.** DM v2's range goals (`at_most`, `between`),
holds and `tunable` bounds are the same concepts as `vary`'s bounds and
constraints. Share the solver crate and name things the same way. Drag specs
and DM requests are both "the element should be here; these are the levers".
In D2 the levers are runtime numbers; in DM they are source numbers.

**The paper's visualizers, cheaply**, because specs and target tables are
data:

- **Overlay**: draw the target table and the pointer-to-target distances.
- **Drop zones**: sample the behaviour on a grid. For `closest` and
  `between` this costs zero speculative runs, since only the table is read.
- **Spec tree**: render the spec record, highlighting the active branch.

Ship them as `drag_debug(d)` in `ui/drag`, toggled by a key in petal-sdl and
Garden.

**Tests and agents.**

- `petal-ui-run` scenarios already drive drags
  (`docs/dev/headless-ui-run.md`). Add `"drag": {"target": "card:7", "to":
  "col:done"}` sugar, which resolves through the `place` registry, and a
  `--drag-trace` that prints the target table and active branch per frame.
- An MCP tool `DragExplain` returns the spec, the target table and the
  branch, for an agent tuning a drag.
- The paper's preliminary LLM experiment (Appendix C) had Dragology at 4 of
  5 one-shot successes versus 0-2 for other libraries. An `AGENTS.md`-style
  page for `ui/drag` should be part of P0.

## Before and after

The "before" snippets are quoted from the corpus. Every app named was run
headlessly
(`petal-ui-run <app> --frames 3`: kanban, calendar, node-editor and
video-timeline all exit 0). The "after" code is **proposed API** and does
not run, except where marked as the P0 prototype.

### 1. Reorderable list: the common case (prototype, runs today)

**Before:** the prelude's recommended pattern (`ui.ptl:3906-3919`). The app
fills in the move, the indicator and any animation itself:

```petal ignore
state dg = drag_state()
for i in range(0, len(cards)) do
  let r = card_rect(i)
  dg = drag_update(dg, cards[i].id, r)
  ...draw the card, offset by (dg.dx, dg.dy) when dg.dragging && dg.id == id
end
if dg.dropped then
  let to = insertion_index(card_rects, mouse_y())
  ...move dg.id to `to`...
end
```

**After:** the P0 prototype,
[`declarative-dragging/proto.ptl`](declarative-dragging/proto.ptl). The core
of it is below. It ran under `petal-ui-run proto.ptl --scenario drag.json`:
dragging A over three slots and releasing gave
`items = [B, C, D, A, E]`. B, C and D slid left one slot at a time while A
followed the pointer (frames 6-12), and A eased into slot 3 over about ten
frames after release.

```petal ignore
fn view(items)
  for i in range(0, len(items)) do
    {key: items[i], x: 40 + i * 70, y: 100, w: 60, h: 60, c: COLORS[items[i]]}
  end
end

fn start_drag(model, key, mx, my)
  let i = index_of(model, key)
  let cands = moves(model, i)
  let targets = for c in cands do find_key(view(c), key) end   // speculation = a call
  ...
end
// per frame: preview = drag.cands[closest(drag, mx - drag.gx, my - drag.gy)]
// draw:      for n in view(preview) key n.key do state ax = … approach(ax, n.x, 18.0) … end
```

With the library, the app's part becomes:

```petal ignore
let d = drag_session(items, view)
for n in view(d.preview) key n.key do
  draggable(n.key, n, fn() -> closest(moves(items, index_of(items, n.key))) |> floating())
  draw_rect(place(n.key, n), n.c)
end
if d.dropped then items = d.drop end
```

What changed for the author:

- No drag record, slop, hit test or insertion index to write.
- The animation comes free.
- The new concept is "list the states the drag can reach".
- A newly possible mistake: listing *all permutations* instead of the
  single-move set. The paper saw this (§8.3).

### 2. Kanban: the case the change is meant to fix

**Before:** `examples/productivity/kanban/app.ptl:375-414`, plus `plan()`
and `insert_index` at `:198-264`, about 110 lines in all:

```petal ignore
if mouse_pressed(0) && hit_c >= 0 then
  sel_id = cols[hit_c][hit_i].id
  drag = {active: true, col: hit_c, idx: hit_i, card: cols[hit_c][hit_i],
    ox: mx - hit_x, oy: my - hit_y, sx: mx, sy: my, moved: false}
end
...
if drag.active && !drag.moved && (abs(mx - drag.sx) + abs(my - drag.sy)) > 4 then
  drag.moved = true
end
if mouse_released(0) && drag.active then
  if drag.moved then
    let dst = P0.tcol
    let at = P0.tidx
    if dst == drag.col then
      cols[dst] = list_insert(list_remove(cols[dst], drag.idx), k: at, v: drag.card)
    else
      cols[drag.col] = list_remove(cols[drag.col], drag.idx)
      cols[dst] = list_insert(cols[dst], k: at, v: drag.card)
    end
    moves = moves + 1
  end
  drag = {active: false, col: 0, idx: 0, card: drag.card,
    ox: 0, oy: 0, sx: 0, sy: 0, moved: false}
end
```

**After (proposed, needs P1):** the board's existing drawing code stays as
it is, wrapped as `fn view(cols)`:

```petal ignore
let d = drag_session(cols, view)
view(d.preview)                     // inside: r = place(card.id, r); draggable(card.id, r,
                                    //   fn() -> closest(moves_nested(cols, card.id)) |> floating())
if d.dropped then
  cols = d.drop
  moves = moves + 1
end
```

The `plan()`/`insert_index`/ghost-slot code (`:198-264`, `:581-583`) goes
away, because the gap is the other cards springing apart.

P1 is required because kanban's view calls `hovered` and draws text and
rounded rects as it goes. Under P0 it would have to be rewritten to return
a scene.

Keyboard moves (`:430-470`) are unchanged. They commit a state directly.

### 3. Video timeline: snapping, where D2 is close but not free

**Before:** `video-timeline/app.ptl:352-376, 623-639`. `snap_best` scans
clip edges, the playhead and markers within `SNAP_F` frames, and draws a
snap line.

**After (proposed):**

```petal ignore
draggable(c.id, r, fn()
  closest(for t in tracks_that_fit(c) do
    vary(with_track(doc, c.id, t), ["clips", k, "s"], {bounds: [0, nil]})
      |> snap_radius(8, {to: for p in snap_points(doc, playhead, c.id) do set_in(doc, ["clips", k, "s"], p) end})
  end)
end)
```

The track choice and the continuous move are declarative. Snapping is still
domain logic: the snap points are the app's to compute. `snap_radius` with a
list of snap *states* replaces `snap_best`'s loop and adds the animated pull.

Cost: `vary` runs the view about 2 times per frame for each visible track
candidate. With 6 tracks that is about 12 speculative calls per frame.
Large timelines need `when_far` to keep only the nearest track live. **This
is the site where D2 is weakest.** The hand-written inverse
(`fm - drag.off`) costs nothing, and the D2 version costs runs per frame.

### 4. Node editor grid snap: the old form is still better

**Before:** `node-editor/app.ptl:832-857`, a free move with
`round(nx / 16) * 16` under Shift.

**After:** `vary(...) |> on_drop(snap_to_grid)` is possible, but it is worse
here. A free 2D move over many selected nodes is a pure translation whose
inverse is `start + delta`. Speculation adds runs and buys no animation
that `place`'s spring would not already give the hand-written version. **The
guide should say: for free translation, keep the direct form and use
`place` only for its animation.**

### 5. Slider: the `vary` primitive

**Before:** `ui.ptl:4339-4391`. `_slider_draw` inverts `mouse_x()` to a
fraction by hand (`fr = (mouse_x() - r.x - pad_x) / usable`).

**After:** `slider` would not change. Its inverse is one line, and it is
hot library code. `vary` earns its place on the paper's rotary dial or a
knob on a Bézier path, where the view is easy to write and the inverse is
not: `vary(st, ["angle"])` over a view that places the knob at
`(cos a, sin a) * R`. Duals give the exact Jacobian there in one run.

### 6. Interaction with state and hot reload

```petal ignore
state board = initial_board()
let d = drag_session(board, view)   // its state: {spec, targets, grab, active, …} — all values
```

- **Editing `view` mid-drag.** The session stores candidate *states* (data),
  not closures. On a reload it sees the program generation change and
  re-runs speculation for the stored candidates against the new `view`.
  The targets move to the new layout and the drag continues. This is the
  live-coding behaviour we want: adjust the card spacing while holding a
  card.
- **Editing the spec function mid-drag** takes effect at the next drag
  start. The spec was resolved to data at press, like Dragology.
- **Keyed widget state** inside the view (a card's hover animation) follows
  its card into the preview, because the loops are `for … key card.id`.
  Without keys, position-keyed state would jump to whichever card occupies
  the slot. That is the same caveat `slider` documents (`ui.ptl:4344`), and
  the lint should flag a `place` under an unkeyed loop.
- **Equivalence with a fresh compile.** The session's state is ordinary
  call-path `state` holding plain values, so `transfer_state` handles it like
  any other slot. Mid-drag, a reloaded program equals a fresh compile
  plus a re-speculation.

### Migration

- **Unchanged:** all existing code. `drag_state`/`drag_update` stay.
- **Manual and optional:** porting an app means wrapping its board drawing
  in `fn view(model)`, adding `place`/`draggable`, and deleting the drag
  state machine. There is no mechanical rewrite. Drag logic is too varied
  for one, and the corpus shows 11-kind drag modes that D2 would cover only
  partly.

## Assessment

These are findings (read or measured) followed by opinion. Most of the
perspectives were researched by parallel subagents, with the code and
corpus as evidence. The judgements are mine.

### Project goals

**Findings.**

- README goals: "Easy to use, supports live coding, good introspection &
  debuggability, and supports goal-based program manipulation" (README.md:7-8).
- Design decisions: "dataflow based computation with immutable values",
  "limited effects", "first-class persistent state", and "safe speculative
  execution" (README.md:14-18).

**Opinion.**

- This fits Petal better than it fits React. Dragology needs React keys,
  `structuredClone` and SVG transform attributes. Petal has keyed call paths,
  immutable values and declared effects.
- It advances "easy to use" directly, for a feature apps visibly avoid or
  under-build.
- It extends "goal-based manipulation" from source to the running app.
- P4 makes the two meet: a drag is a goal, and the levers are model states,
  runtime numbers or source literals.
- This is distinctive territory, not generic surface. The only generic part
  is the operator library, and it is a library.

### Ease of writing

**Findings.**

- The paper's study (n = 13) found the state-space shift "brain-twisting".
  It found a "sharp cliff" at the reorderable list, and enumerating states
  was the top friction (§8.3).
- The prototype needed a pure `model -> scene` view, which no app in the
  corpus has. Every app draws inline from top-level `state`.
- The helpers cover the corpus's set shapes: `moves`, `moves_nested` and
  `tree_moves` match kanban, petal-design's layers and petal-images.

**Opinion.**

- `draggable(key, r, fn() -> closest(moves(...)))` reads well six months
  later. It says what the drag *does*.
- There are now two ways to build a drag. The guide should say:
  - **D2** for anything that snaps, reorders or re-parents;
  - **direct pointer math** for free translation and pan (pair 4).
- A JS or Python newcomer will not guess "list the reachable states". The
  helpers hide that for the common shapes.
- **The strongest argument against the proposal:** restructuring an
  existing immediate-mode app into `view(model)` is real work. If kanban's
  port under P0/P1 is not clearly shorter and nicer, the idea does not
  transfer to how Petal apps are written.

### Bug-freedom

**Bugs it prevents.**

- The slop, cancel and release state machine bugs every app re-implements
  (kanban resets 9 fields by hand at `:411-413`).
- Drop positions that disagree with what is drawn. The targets *are* the
  rendering.
- Insert-index off-by-ones.

**Bugs it makes possible.**

- **Ambiguous targets.** Two candidates place the element at the same
  spot, and the paper throws an error for this. Proposed: a warning naming
  both states; `closest` takes the first.
- **Unkeyed `place`.** Identity breaks under a reorder. Lint: `place` or
  `draggable` under a loop without `key`.
- **Over-enumeration.** All permutations instead of single moves. Cost
  only; `--drag-trace` shows the count.
- **Impure views under P0.** The view writes state or draws. P1 fixes this;
  until then, document it.

**Testing.**

- `speculate` must keep optimizer-on equal to `--no-opt` under the
  differential oracle. The memo-replay-inside-overlay invariant (§P1) is the
  part most likely to hide a bug.
- Drags are scriptable in `petal-ui-run` scenarios, so goldens are
  straightforward.

### Performance

**Measured: P0 prototype**, debug build of `petal-ui-run`, wall clock, with
the 3-frame baseline subtracted. This is the cost of the drag-start frame
that runs N views of N items:

| N items | Speculated scene nodes | Drag-start overhead |
|---|---|---|
| 20 | 400 | below noise (<1 ms) |
| 50 | 2,500 | ~2-3 ms |
| 100 | 10,000 | ~4 ms |
| 200 | 40,000 | ~13 ms |

The cost grows quadratically, as the paper reports (§9, footnote 8). It is
paid once per drag, at press. Frames during a drag run no speculation for
`closest` or `between`.

**Comparison: the fork route.** `run_speculative` copies the heap on every
run. That costs 0.9 ms per run below 200 emits and 5.7 ms at 1-3k emits
(`direct-manipulation-v2-study.md:321-334`), so a kanban with 40 card
targets would pay 40-200 ms at press. `speculate`'s overlay avoids the copy.
**That is the reason P1 is a new primitive rather than a host feature built
on `fork_execution`.**

**`vary`** runs `k+1` (with duals) or `2k+1` (finite differences)
speculative calls per frame. For k = 1-2 on a small view this is under 1 ms.
On the video timeline it is the dominant cost (pair 3).

**When unused:** zero cost. `place` outside a drag is one registry push.

**Mitigations, in order:**

1. Spread drag-start speculation over frames. A target that has not been
   computed yet is a pending value, and pending values already absorb
   (`pending-values.md`).
2. Allow memo replay inside `speculate`, so unchanged keyed subtrees
   (other columns) replay.
3. `when_far` pruning for `vary`.

**Benchmarks to add:**

- `bench_panel` with a 50-card kanban drag-start;
- `petal bench` on `speculate` against a plain call, to bound the overlay's
  cost.

### Live coding and hot reload

See pair 6. The session holds only values, so a reload is a re-speculation,
and the drag survives an edit to the view. Closures in `state` are dropped on
reload (`hot-reload.md:61-73`); the design avoids storing any.

A view edit that removes the dragged key from every target cancels the drag
cleanly, with the reason: "dragged element `card:7` no longer placed".

### Introspection, debugging and program manipulation

- The spec and the target table are data. They show up in state dumps,
  `petal-ui-run` traces and time travel. A scrubbed frame shows its drag
  branch.
- `speculate`'s captured emits keep their origins, so provenance and
  `explain` work on a speculative rendering. That is useful for "why is
  this target here?".
- DM v2 still traces a dragged card's drawn rect to source. In edit mode,
  `place`'s spring is an extra function in the slice. It is not rigid, so DM
  v2 may propose editing the spring rate. Mark `place`'s internal literals
  as library code, which eligibility already excludes (`params.rs:302`).
- Goal-based editing gains a new client in P4.

### Maintainability of the implementation

| Phase | Stages touched | Size |
|---|---|---|
| P0 | petal-ui prelude (`ui/drag` module, about 400 lines of Petal); docs; scenario sugar | ~1 week |
| P1 | VM: a state/cell write overlay on `StateWrite`/`StateInit`/cell `set` (`backend/bytecode/vm`); reuse of the spec native gate outside DM (`vm/native.rs:532`); emit/registry capture; builtin registration; type and effect rows (`native-effects`); lint (nested, unkeyed `place`); docs; oracle tests | ~2-3 weeks |
| P2 | Extract a lever-generic solver from `dm/solve.rs` and `dm/jacobian.rs` (refactor, DM goldens must not move); `set_in` builtin; Delaunay/barycentric in Rust behind a native | ~2 weeks |
| P3 | Multi-generator `for` (parser, `cst_project.rs`, compiler; separate proposal) | separate |
| P4 | Diff→Goal mapping (`goal_based_editing.rs` client); Garden edit-mode hook; `drag_debug`; MCP `DragExplain` | ~2 weeks |

**New invariants:**

- `speculate` never leaks a state or cell write;
- inside `speculate`, memo replays but never records;
- `place` keys follow `for … key` equality.

**Removed special cases:** `ui_measure` stops being its own "draw nothing"
mode.

**C bridge:** none for P0-P2. The engine is Petal code on top of petal-ui,
so cheesecake and worlds-fair get it by re-vendoring the prelude.

### Domains

| Domain | Effect | Example |
|---|---|---|
| Productivity and UI apps | **Helps most** | kanban, calendar, video-timeline, file-browser `drop_at` (`file-browser/app.ptl:1119-1140`), petal-design layers |
| Creative tools | Helps | petal-design layer and page trees, petal-images layers; the canvas's 11 modes only partly (free transforms stay direct) |
| Explorables and teaching | Helps, and this is the paper's niche (§8.6) | Ring of beads, Hanoi, Animate Algebra map directly onto `ui/drag` |
| Games | Neutral, with niche help | Inventory and grid placement (cheesecake `moses/input.ptl:93,160-180` tile drags, Sokoban, Hanoi) fit. Per-frame physics drags (`network-graph`'s force simulation) do not |
| Dashboards | Neutral | Panel reorder, range brushes via `vary` |
| Web | `petal-web-canvas` works through petal-ui. `petal-web-html` needs a `place` equivalent on its keyed element tree (`renderer.ts:127-158`) | |
| Editor scripting and config | Helps through P4 | Reorder a config list by dragging its rendering in Garden |
| Embedding hosts | Neutral | Library-level; no ABI change |
| Graphics, shaders | Neutral | |

### Compatibility and migration

- Nothing breaks.
- `speculate`, `place`, `draggable`, `drag_session` and `set_in` are new
  names. `place` might shadow app functions, but prelude bindings are weak
  (petal-ui README, versioning section), so apps that define their own
  `place` keep working. A grep for collisions across the corpus is part of
  P0.
- `ui/drag` is a sub-module, not a prelude export, so the default namespace
  is unchanged.
- No future direction is closed. `speculate` is a general building block:
  feedforward previews ("what would this button do"), hover previews of
  undo, and `ui_measure`.

### Prior art

- **Dragology** (the paper) works on SVG trees and interpolates every
  attribute (`lerpViews`). This proposal interpolates only what `place`
  marks: less automatic, but host-independent and cheap. Automatic
  draw-list interpolation, matched by implicit call keys, is possible later.
- **Apparatus** and **g9** (paper refs 38, 44) use speculative rendering
  for continuous drags on diagrams. g9 optimizes numeric parameters, like
  `vary`.
- **Bret Victor's tools** and Petal's DM v2 apply the same principle to
  source rather than state.
- **React and FLIP layout animation**: `place`'s keyed spring is FLIP done
  in the language rather than in the DOM.
- **Immer and `amb`** (the paper's §9 mitigations): Petal values are already
  immutable. `amb` is rejected above for its control-flow cost.
- **SwiftUI and Compose drag APIs**: these are the "drag source / drop
  target" style that the paper argues tops out early.

## Alternatives considered

1. **Do nothing.** Keep `drag_state` and hand-rolled drags. Cost: the corpus
   numbers in §Motivation, and no animated drops anywhere.
2. **Host-side D2 on `fork_execution`, with no language change.** It has
   the right semantics and needs no `view(model)` separation, since the
   whole frame is the view. But it costs a heap copy per target (40-200 ms
   at press for a kanban). Pathed state cannot be set in a fork, and the app
   cannot express its spec without a callback the host calls between runs
   (`ffi.md:1106`). Rejected as the main route. It stays as the fallback for
   hosts that want D2 on a script that has no `view`.
3. **Library only (P0 alone).** It works today and is the smallest version
   that delivers most of the value for *new* code. It cannot speculate the
   existing imperative views. **This is the recommended first step and the
   gate for P1.**
4. **Automatic interpolation of draw commands instead of `place`.** Match
   commands across renderings by implicit `call_key` and lerp their numeric
   fields. It is closer to `lerpViews` and needs no hook in the view, but
   draw commands lack element ids (`draw.rs:107-200`) and keys are `None`
   in `map` callbacks (`retained-scene-output.md:720`). Deferred. `place`
   is explicit and enough for P0-P2.
5. **`amb` or relational state specs** (paper §9). Rejected for now; see
   §P3.

## Implementation sketch

1. **P0, `ui/drag` in Petal.**
   - Contents: `place`, `draggable`, `drag_session`, and the operators
     `fixed`, `closest`, `floating`, `snap_radius`, `when_far`,
     `drop_target`, `on_drop`, plus the `moves*` helpers and `drag_debug`.
   - Ports: the reorderable list, Hanoi, the ring of beads, and **kanban**
     (with its view made pure, or wait for P1).
   - Tests: `petal-ui-run` scenario goldens per example, and an
     `AGENTS.md`-style guide.
   - **Exit criterion:** kanban's port is shorter, animates, and passes its
     existing golden for non-drag interactions.
2. **P1, `speculate`.**
   - The VM overlay plus the reuse of the spec gate.
   - Tests:
     - oracle (optimizer on vs `--no-opt`);
     - a property test that no state or cell changes across any
       `speculate` call;
     - `ui_measure` rebuilt on it, with its tests unchanged.
   - Then: port kanban's real view; benchmark a 50-card drag-start.
3. **P2, `between` and `vary`.**
   - Extract the solver behind a `Lever` trait. DM v2 study numbers and DM
     goldens must be unchanged.
   - Dual-first sensitivities; Delaunay triangulation.
   - Examples: three-way switch, rotary dial, the scheduler from paper §4.
4. **P3.** The multi-generator `for` proposal, and `set_in`.
5. **P4.** Diff→Goal mapping and the Garden edit-mode drag; MCP
   `DragExplain`; scenario `drag` sugar; `--drag-trace`.

## Open questions

- **Name:** `speculate` vs `what_if` vs `hypothetically`. This is
  user-facing, so the language owner decides.
- **Where `place` lives:** petal-ui only, or core. Core would let
  `petal-web-html` and non-UI hosts use it. Decide before P1.
- **What a new state slot first reached inside `speculate` should
  start from:** its initializer, or an inherited value? This matters for
  widget animation state in speculative renderings. The proposal says the
  initializer. Confirm with the P1 prototype.
- **Ambiguous targets:** a warning and take the first (this proposal), or
  an error (Dragology)?
- **Multi-touch and two simultaneous drags.** The paper leaves this open.
  `drag_session` could hold one drag per pointer. Not designed here.
- **Ordering:** whether P4's structural DM should come *before* P2. It is
  the most distinctive capability and needs only P0.

## Implementation notes

- **P0:** `fixed` is named `fixed_state`. The core builtin `fixed(value, places?)` formats numbers, and a plain `import ui/drag` exporting `fixed` would take over every one-argument `fixed(x)` in an importing file without a warning (bundled packages do not raise the builtin-shadowing warning).
- **P0:** A spec record's tag field is `d2` (`{d2: "closest", options, stickiness}`), so a model state that happens to have a `tag` field is never mistaken for a spec; anywhere a spec is expected a bare state means `fixed_state(state)`, which is what lets `closest(moves(...))` take states directly.
- **P0:** `on_drop(spec, f)` is applied when the spec is built, to every leaf's `drop`, so the stored spec holds no closure (hot reload drops closures in `state`). For `between`/`vary`, whose drop state is only known per frame, it keeps `{d2: "on_drop", inner, f}` for P2 to apply at the drop; P2 must re-obtain `f` (and `vary`'s `constraint`) from the view's `draggable` after a reload.
- **P0:** `switch_to(state, key, spec_fn?)` calls `spec_fn` when the spec is built; without it, the follow spec is read from the `draggable(key, …)` the view registers when `state` is speculated. `snap_radius`'s `chain` uses the same route, so both need `draggable` inside the view. The switch happens on the first frame the option is chosen.
- **P0:** `snap_radius` is visual, as in Dragology's "pull the preview toward the drop state": within `px` the element stops floating and is drawn in its slot; further away the inner behaviour is unchanged (it does not drop onto nothing). "Nothing" outside a region is expressed with `when_far` and `drop_target`.
- **P0:** The floating element is put on top by a three-argument `place(key, value, body)`, which runs `body(v)` on `UI_LAYER_DRAG` (900) while the key is dragged or settling: a two-argument `place` returns a value and cannot move the caller's later draw calls to another layer.
- **P0:** The press is detected by the live `draggable` (an `interact` on `"drag:<key>"`), which calls `spec_fn()` there and leaves `{key, x, y, spec}` for `drag_session` on the next frame; the session that claims it is the one whose `view(model)` places the key. The speculative target table is built when the pointer passes the slop. `draggable` returns the `interact` record.
- **P0:** Speculation is `ui_measure(fn() … view(model) … end)` with the captured places read inside the body and returned as its value, so it works both with the old `ui_measure` (writes kept, then reset by hand) and with P1's `speculate`-based one (writes undone). A view that returns a list of `{key, …}` nodes is also searched by `key`, for the "place outside the view" form of §Before and after 1. Ambiguity is a printed warning, once per message.
- **P0:** Reload detection: there is no script-visible program generation, so the session reads a `memo(0)` token at the press and every frame; a reload that changes the program drops memo entries, the token is made again, and the stored states are re-speculated. A memo eviction gives a harmless extra re-speculation.
- **P0:** A release with no target reports `cancelled` with `reason: "no target"` rather than `dropped` with `drop == model`. The session also returns `target` (the chosen leaf's id), `gap` and `reason`. One drag at a time.

## Implementation notes

- **P1:** The overlay is an undo log, not a side table: a write inside the call goes to the live slot or cell after its old contents are logged, and the log is played back on return (an error included). Reads therefore see the live value until the call writes it, which is the overlay's semantics. Containers the optimizer edits in place are protected by an allocation epoch on the heap: while a call runs, an in-place edit of an object older than the call copies it (`Heap::begin_speculation`).
- **P1:** Memo replay inside `speculate` is off, not "allowed". Records are keyed by call path, and every path inside the call runs through the `speculate` call's own site, which no live record has, so a replay could never hit; the replay-under-overlay invariant does not arise. No scope records inside the call, and a scope around it is marked effectful (it never replays), since the call's reads are not recorded. Replaying unchanged keyed subtrees (mitigation 2 in §Performance) would need path-transparent speculation and is not built.
- **P1:** The `places` mechanism is two core natives: `speculating()` and `speculate_place(key, value)` (records `{key, value}` into the running call's `places`, identity outside; keys checked like `for … key` keys). The drag library's `place` is built on them.
- **P1:** `emits` is `[{channel, value, at}]`, in emit order per channel and channels in name order (the buffers are per channel, so there is no cross-channel order); `at` is `{file, line, column}` of the first non-prelude frame that emitted.
- **P1:** Handle methods are not called inside the call (they answer nil and warn), like escaping natives. Resources a call starts loading and `expect` results are kept, not undone. `state` is allowed inside a `speculate` made from a higher-order builtin's callback. The "not called" warning is collected on every run, not only in debug runs (`Env::take_runtime_warnings`, printed by `petal run`).
- **P1:** The nested-`speculate` lint was not added; nesting is a runtime error with the proposal's message and help. `ui_measure` inside a script's own `speculate` call falls back to its old muted-span path, since a nested call is an error.
