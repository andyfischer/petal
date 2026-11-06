# Declarative dragging: `ui/drag`

`ui/drag` builds drag and drop from one idea: **say which model states a drag
can reach, and let the engine work out the rest.** The engine renders each
reachable state, finds the dragged element in each rendering, and from those
positions it gives you the reorder, the snapping, the floating element, drop
targets and the animation of everything that moves. You never write a hit
test, an insertion index or a drag state machine.

It is Petal's version of Dragology (Horowitz, Blinn and Heer, *Declarative
Dragging*, UIST '26). The design and its phases are in
[docs/proposals/declarative-dragging.md](../../../docs/proposals/declarative-dragging.md).
The one-line index of every name is in
[reference.md](reference.md#declarative-dragging-uidrag).

This page is written for people and for coding agents. If you are an agent
asked to "make this draggable", read [The rules](#the-rules) first.

## Contents

- [A reorderable list](#a-reorderable-list)
- [The three calls](#the-three-calls)
- [Specs](#specs)
- [Candidate states](#candidate-states)
- [Recipes](#recipes)
- [The rules](#the-rules)
- [Debugging and testing](#debugging-and-testing)
- [Hot reload](#hot-reload)
- [When not to use it](#when-not-to-use-it)
- [What is not here yet](#what-is-not-here-yet)

## A reorderable list

```petal ignore
import ui
import ui/drag

state items = ["A", "B", "C", "D", "E"]

fn view(items)
  for i in range(0, len(items)) key items[i] do
    let k = items[i]
    let r = place(k, rect(40, 60 + i * 52, 240, 44))
    draggable(k, r, fn() closest(moves(items, i)) |> floating() end)
    place(k, r, fn(p) draw_rect_rounded(p, 6, #64b5f6) end)
  end
  nil
end

let d = drag_session(items, view)
view(d.preview)
if d.dropped then items = d.drop end
```

Read the `draggable` line as a sentence: *dragging row `i` can reach every
list with that row moved to another index; take the closest one; the row
itself follows the pointer.* While you drag, the other rows slide out of the
way; when you let go, the row eases into its slot. Escape puts everything
back. A press that does not move is still a click.

The whole example, with a scenario that drags it headlessly, is
[`examples/ui/d2-list`](../../../examples/ui/d2-list/). Two more:
[`d2-hanoi`](../../../examples/ui/d2-hanoi/) (legal moves only, `closest` +
`floating`) and [`d2-beads`](../../../examples/ui/d2-beads/) (a ring, with
`on_drop` turning the ring after every drop).

## The three calls

### `place(key, value)` and `place(key, value, body)`

Marks where element `key` is in this rendering and returns the value to
draw. `value` is usually a rect; a point `{x, y}`, a number or a colour also
work (records and lists are eased field by field).

- Outside a drag, the returned value eases from where `key` was last frame
  toward `value`. **Any keyed element that changes position animates**,
  whether a drag moved it or not.
- For the dragged element under `floating`, it is the element's rect under
  the pointer.
- While the engine is speculating, it records `value` and returns it as is.

The three-argument form calls `body(v)` with the displayed value. Use it for
the drawing: while `key` is being dragged, or is settling after a drop, the
body runs on `UI_LAYER_DRAG` with the clip suspended (`ui_overlay`), so the
dragged element is drawn over the rows it passes and outside the scrolled
well it came from. Drawing with the two-argument form's value works, but a
floating element is then drawn in list order, under whatever comes after it.
While the engine is speculating, the body is **not run**: speculation only
needs the positions, and skipping the drawing halves the drag-start cost.
So do not `place` other elements inside a body.

Calling `place(k, r)` and then `place(k, r, body)` with the value the first
call returned (the usual pair) eases once: the second call sees the value it
already displayed this frame and hands it back.

### `place_origin(origin, body)`

Inside `body()`, `place` eases positions relative to `origin` (`{x, y}` in
screen coordinates) rather than to the screen. Wrap a scrolled container's
elements in it with the scroll offset:

```petal ignore
place_origin({x: 0, y: -scroll}, fn()
  for card in cards key card.id do
    let r = place(card.id, rect(x, top - scroll + card.off, w, card.h))
    ...
  end
end)
```

Scrolling then moves the elements at once, as a scroll should, while a
reorder inside the container still animates. Values given to `place` are
still screen positions; only the easing is relative.

`key` is compared like a `for … key` key (by content). Keys must be unique on
the screen and stable across states: an item's id, never its index.

### `drag_key()` and `is_dragging(key)`

`drag_key()` is the key of the element being dragged right now, or nil;
`is_dragging(key)` asks about one key. Use them for styling: a lifted card,
the column the dragged card would land in. Both are nil/false while the
engine is speculating, so a view's drop positions never depend on them.

### `draggable(key, r, spec_fn)`

Makes `r` grabbable. A press on `r` that then moves `DRAG_SLOP` (4) px with
the button held starts a drag of `key`. `spec_fn()` is called at the press
and returns the drag's [spec](#specs). The engine keeps the spec it
returns, which is data, and never the function.

`r` is registered for hit testing like `interact(r, "drag:<key>")`, so
layers, modals and `pointer_captured()` work as for any widget, and
`--targets` lists it. It returns the `interact` record (`hover`, `down`,
`pressed`, `released`, `click`).

### `drag_session(model, view, opts?)`

The engine. Call it once per frame **before** rendering, with the committed
model and the view function, then render `view(d.preview)`:

| Field | Meaning |
|---|---|
| `active` | A drag is in flight. |
| `key` | The dragged element's key; nil when idle. |
| `preview` | The state to render: the chosen target's, else `model`. |
| `drop` | What a release would commit now; nil when there is no target. |
| `dropped` | True on the frame a drag ended on a target. Commit `d.drop`. |
| `cancelled` | True on the frame a drag ended without one; `reason` is `"escape"`, `"no target"`, or the element no longer being placed. |
| `spec` | The drag's spec (data). |
| `target`, `gap` | The chosen target's number (spec order, from 1) and its distance in px. |

`opts.slop` overrides `DRAG_SLOP`. The session's own state is call-path
`state` holding only values, so it shows up in state dumps and time travel.

## Specs

A spec is a plain record whose `d2` field names its kind. It can be printed,
compared, stored and inspected. Build it with these functions, which compose
with `|>`:

| Spec | Meaning |
|---|---|
| `fixed_state(s)` | One target: state `s`. Anywhere a spec is expected, a bare state means this. |
| `closest([spec, …], {stickiness?})` | The option whose target is nearest the dragged element. `stickiness` (px) favours last frame's choice, so a pointer on a boundary does not flicker. |
| `spec \|> floating()` | The element follows the pointer; the rest of the view previews the chosen state. |
| `spec \|> snap_radius(px, {chain?})` | Within `px` of the target, the element snaps into its slot instead of floating. `chain: true` makes the snapped state the drag's new start, and the drag goes on with the spec the view's `draggable` gives there. |
| `spec \|> when_far(far, {px?})` | Use `far` once `spec`'s target is more than `px` (default 60) away. |
| `drop_target(key, s)` | A release with the pointer over the element placed as `key` gives `s`. For removals: the dragged element does not exist in `s`, so no rendering of `s` can show where it would go. |
| `spec \|> on_drop(f)` | Every target's drop state becomes `f(state)`. Applied when the spec is built: the spec still holds no function. |
| `switch_to(s, key, spec_fn?)` | Reaching this option switches the model to `s` at once and keeps dragging the element `key` (a palette that spawns a new item). The drag goes on with `spec_fn()`, or, without one, with the spec the view's `draggable(key, …)` gives in `s`. |

"Nearest" compares the centre of the dragged element (where it would be,
under the pointer) with the centre of the element in each target's
rendering.

With no target (outside every `drop_target`, or every option's state leaves
the key out), the preview is `model` and a release cancels.

Order matters only where a modifier reads its inner result:
`closest(xs) |> floating() |> snap_radius(8)` and
`closest(xs) |> snap_radius(8) |> floating()` both float until within 8 px.

## Candidate states

The work in using `ui/drag` is listing the states a drag can reach. These
helpers cover the common shapes:

| Helper | States |
|---|---|
| `moves(xs, i)` | Item `i` of list `xs` at every index (its own included). |
| `moves_nested(lists, key, field?)` | The item whose `id` (or `key`, or itself) is `key`, moved to every slot of every column. `lists` is a list of lists, or of records whose `field` is the list. |
| `tree_moves(tree, path, opts?)` | The node at `path` (indices) above, below or into (as the last child of) every other node, never into itself or its descendants; each distinct state once. `opts.children` names the child list (default `"children"`); `opts.into` is false, or `fn(node) -> bool`, to limit which nodes take children. |
| `choices(s, path, values)` | `s` with the value at `path` (a list of keys and indices, or one key) set to each of `values`. |

For anything else, write a function that returns the list. Leave illegal
states out and the drag cannot reach them; that is how Hanoi forbids a big
disk on a small one:

```petal ignore
fn hanoi_moves(pegs, p)
  let disk = top(pegs, p)
  let out = [pegs]                       // staying put is always allowed
  for q in range(0, 3) do
    let t = top(pegs, q)
    if q != p && (t == nil || t > disk) then
      let next = pegs
      next[p] = drop_last(pegs[p])
      next[q] = pegs[q] ++ [disk]
      out = out ++ [next]
    end
  end
  out
end
```

Note the `let next = pegs` before the path assignments: without it, the
assignment rebinds `pegs` itself for the rest of the loop.

## Recipes

**Kanban** (cards in columns; each column a list of cards, or a record whose
`cards` is that list). The whole port is
[`examples/productivity/kanban`](../../../examples/productivity/kanban/app.ptl):

```petal ignore
// columns are lists of cards
draggable(card.id, r, fn() closest(moves_nested(cs, card.id)) |> floating() end)
// columns are records holding the list in `cards`
draggable(card.id, r, fn() closest(moves_nested(cols, card.id, "cards")) |> floating() end)
```

**A trash can** beside the reorder:

```petal ignore
place("trash", trash_rect)                       // in the view, so the engine can find it
draggable(k, r, fn()
  closest(moves(items, i) ++ [drop_target("trash", remove_at(items, i))]) |> floating()
end)
```

**Snap into place near a slot, float elsewhere:**
`closest(moves(items, i)) |> floating() |> snap_radius(12)`.

**Only reorder near the list; far away, only the trash counts:**
`when_far(closest(moves(items, i)), closest([drop_target("trash", without)]), {px: 80}) |> floating()`.

**A palette that adds an item** and lets you place it:

```petal ignore
draggable("swatch", sw, fn() closest([switch_to(items ++ [new_item], new_item.id)]) end)
```

**Count or normalize on drop:** `... |> on_drop(fn(s) {...s, moves: s.moves + 1} end)`.

**A layers panel:** `closest(tree_moves(layers, path)) |> floating()`.

## The rules

1. **The view is a function of the model.** Write `fn view(model)` and call
   it as `view(d.preview)`. Everything a drag can change must come from the
   argument, not from top-level `state` read inside the view.
2. **Speculation runs your view.** The engine calls `view(candidate)` for
   every reachable state when a drag starts, each one as a core
   [`speculate`](../../../docs/features/speculate.md) call: nothing it draws
   or registers reaches the screen, and every `state` write, `set` and
   random draw it makes is put back. So the view can be your ordinary
   immediate-mode drawing code, widgets and all. An escaping native (a file
   save, a host handle method) is not called in there and answers nil.
3. **Key every loop that places**: `for c in cards key c.id do`. Keys are
   ids, unique on screen, the same in every state.
4. **Put `place` and `draggable` inside the view**, so speculation sees them.
   (`place` in a view that returns a list of `{key, …}` records also works:
   the engine finds the node by its `key` field.) `snap_radius`'s `chain` and
   `switch_to` without a `spec_fn` read the `draggable` registered *inside*
   the view.
5. **Draw with `place(key, r, fn(p) … end)`** so the floating element is on
   top.
6. **Call `drag_session` once, outside the view, before rendering**, and
   commit `d.drop` on `d.dropped`. Do not commit anything on `d.cancelled`.
7. **List single moves, not permutations.** `moves(xs, i)` is n states;
   every permutation of a list is n! and is slow, and the drag means
   something else.
8. **Two states that put the element on the same spot are ambiguous.** The
   engine prints one warning naming both, and `closest` takes the first.
   Usually it means two candidates are really the same move: drop one.
9. **Do not keep closures in the model or the spec.** `spec_fn` and
   `on_drop`'s `f` are called while the spec is built; what is kept is data.
10. **Keep expensive pure work out of the view.** A drag start runs the view
    once per reachable state, and memo does not record or replay inside a
    speculative call. Work that is the same in every state (text wrapping,
    measuring, anything keyed by the items rather than their order) belongs
    before the view, computed once a frame. Kanban's wrapped titles are the
    worked example: moving them out took its 50-card drag start from 63 ms to
    19 ms.
11. **Pass what the view reads.** A `fn view` defined at the top reads the
    `state` bindings it names as they were when it was defined, one run
    behind any later write (the compiler warns). Give the session a closure
    that passes them: `drag_session(cols, fn(cs) view(cs, scrolls, sel_id) end)`,
    and draw with `view(d.preview, scrolls, sel_id)`.

## Debugging and testing

- `d.spec` is the spec as data, `d.target` the chosen option and `d.gap` its
  distance: print them, or show them on screen.
- `petal-ui-run app.ptl --targets` lists each draggable as `drag:<key>`.
- A scenario drags like a user: press, move a few frames, release
  ([headless-ui-run.md](../../../docs/dev/headless-ui-run.md)):

  ```json
  {"size": [320, 340], "frames": 40, "events": [
    {"at": 2, "mouse_move": [100, 80]}, {"at": 3, "mouse_down": 0},
    {"at": 5, "mouse_move": [104, 110]}, {"at": 8, "mouse_move": [108, 170]},
    {"at": 11, "mouse_move": [110, 236]}, {"at": 14, "mouse_up": 0}]}
  ```

  The press is at frame 3; the drag starts on the first frame the pointer
  has moved past the slop, and the drop is the frame the button comes up.
  Check the committed state in the last record.
- The engine's cells appear in the trace's `state` under `ui/drag::`:
  `_anim` (every key's displayed value), `_press` (a press waiting to
  become a drag) and `drag_session/ui/drag::s` (the drag).

## Hot reload

The session keeps only values: the spec, the states it can reach and the
grab offset. An edit to the view while a card is held re-renders the stored
states against the new view, the targets move to the new layout, and the
drag goes on: you can change the row spacing with a card in your hand. An
edit to the spec function takes effect at the next press. If the new view no
longer places the dragged key, the drag is cancelled with that reason.

The engine notices a reload through `memo`: a reload that changes the
program drops every memo entry, so an entry made at the press being made
again means the program changed.

## Performance

A drag start speculates the view once per reachable state, so for a reorder
of n items over a view that draws all n, it costs about n renderings: it
grows with the square of the board. Frames during the drag run no
speculation for `closest`, `floating`, `snap_radius`, `when_far` and
`drop_target`; they only read the table built at the start.

Measured on the kanban example with its board replaced by n cards
(`cargo run --release --example bench_drag -- examples/productivity/kanban/app.ptl <n>`
in `core-libs/petal-ui`; minimum of 15 runs, frame gate off, Apple silicon):

| Cards | Idle frame | Drag start | Frame while dragging | Old hand-rolled app, while dragging |
|---|---|---|---|---|
| 12 | 0.30 ms | 2.1 ms | 0.38 ms | |
| 50 | 0.68 ms | 19 ms | 1.11 ms | 0.64 ms |
| 100 | 1.19 ms | 53 ms | 2.04 ms | |

So a 50-card board drops about one frame when a drag starts, and each frame
costs about 0.5 ms more than the hand-written drag (the per-key easing in
`place`). The first version of the port, which wrapped every title inside the
view, took 134 ms at 50 cards; skipping `place`'s body during speculation
and moving the wrapping out of the view (rule 10) are what brought it down.
Spreading the drag start over several frames is not built.

## When not to use it

For a **free translation** (moving nodes on a canvas, panning, a selection
of many items), the inverse is `start + delta` and the direct form is
simpler and cheaper. Keep the pointer math and use `place` alone for its
animation:

```petal ignore
let r = place(node.id, rect(node.x + dx, node.y + dy, node.w, node.h))
```

Use `ui/drag` for anything that **snaps, reorders or re-parents**.

## What is not here yet

- `between` and `vary` (interpolating between targets, and solving numbers
  so the element tracks the pointer) build spec data now, and a drag that
  reaches one stops with "not yet available". They arrive with P2.
- One drag at a time: a second pointer does not start a second drag.
- `drag_debug(d)` overlays and the scenario `drag` sugar are planned (P4).
