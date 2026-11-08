# Proposal: Input dispatch, one owner per event (ID) for petal-ui

Status: in progress, 2026-11-07. Phases 0 to 3 are done (see Phases).

## Summary

petal-ui's widgets read input *edges* (`mouse_pressed`, `key_pressed`,
`text_input`) directly, in whatever order the script happens to run them.
Who gets a press, a key or the keyboard is decided by code order, a
one-frame-old registry, and conventions every caller has to remember
(`fc` threading, `menu_blocking`, the menu's `at` frame skip, an app's own
`TYPING` flag). Most interaction regressions in petal-ui and the
petal-creative editors are one of these going wrong.

**Recommendation:** resolve input **once per frame, as data, against the
frame the user saw**. `hit::roll` already runs once per frame, before the
script, with last frame's complete registry. Extend it to give every event
exactly one owner and bind the result as an immutable `dispatch` record.
Widgets ask about their own id. They become pure functions of bindings,
and none of them can steal an event from another.

## Why: the regressions

These come from a survey of about 1,650 commits across `petal` and
`petal-creative`. Five causes recur.

1. **Edges are consumed in script order.** The first widget to read a
   press decides what happens. Examples:
   - The menu's opening press would also close it, so it got an `at` frame
     skip.
   - A same-frame trackpad tap lost its drag (petal-creative `6583875`).
   - A click landed on a panel that appeared mid-click (`b1e12b17`, fixed
     with a press snapshot).
   - A lower-layer widget that runs before a new cover gets one frame of
     hover. This is documented in reference.md.
   - The editors clear focus at the top of the script, before the field
     sees the press.
2. **Ownership is half last frame and half this frame.** `ui_floor` comes
   from last frame's registry, but the prelude's `_floor` rises during the
   run. It is only right if higher layers run later.
3. **Focus is a value threaded by hand.** If one panel forgets to return
   `fc` (`42755d6`), shortcuts fire while typing or input goes dead. The
   same happens when `TYPING` is computed before the panels change focus,
   as in all four editors.
4. **Skipping work drops input silently.** A native with a dishonest
   effect row is replayed from memo and never called again (`3574a0f0`).
   The registry also relies on "an empty buffer means the frame was
   skipped".
5. **Each host has its own input bugs**: key names, Retina scale, resize,
   lost key releases. Each was found only in the host that had it.

## The design

### The principles it follows

- **Dataflow over immutable values.** Input is one immutable record bound
  before the run. Nothing is consumed and nothing mutates during the run.
- **No callbacks.** The script still polls. It now polls an answer instead
  of raw edges.
- **Specs as data.** Shortcuts, focus requests and popovers are declared
  as data, like D2's drag specs. They can be printed, diffed, replayed and
  driven by an agent.
- **Honest effects.** Every new native is pure over bindings, or an
  emit, so the frame gate and memo stay exact with no special cases.
- **Introspection.** "Why didn't my click work" gets an answer from data
  that is already on the timeline.

### The dispatch record

`hit::roll` builds this from the input events, last frame's registry, the
press snapshot and the focus:

```
dispatch = {
  pointer: {owner: "layer-row-12", phase: "click", x, y, mods, count},
  keys:    {owner: "inspector.width", pressed: ["a"], chord: nil},
  text:    "a",
  focus:   "inspector.width",
  dismiss: nil,              // the popover a press outside or Escape closes
}
```

- **Pointer.** The owner is the topmost registered rect under the press
  point in **last frame's registry**, which is what was on screen when the
  user pressed. Modals and blocking rects apply as they do today. The
  phases are `hover`, `press`, `drag`, `click` and `release`.
  - A press and release in one frame is one `click` with one owner, not a
    weaker path.
  - A rect that appeared after the press never gets the click, because
    ownership is fixed at the press.
- **Keys.** They are routed in a fixed order: the focused widget gets the
  keys it declared it wants, then the topmost modal, then the declared
  shortcuts. A chord is claimed by a field only if the field declared it
  (Cmd+A in a text field), otherwise it goes to shortcuts.
- **Focus.** It is runtime-owned. It changes by request
  (`focus_request(id)`, an emit) or by a press on a widget that declared
  `wants: {text}`. Either applies at the next roll. A press anywhere else
  clears it, unless a higher layer took the press.

`interact(r, id)` returns the same `{hover, down, pressed, released, click}`
as today. It is computed from `dispatch` and `id`, not from the edges plus a
floor. `focused(id)`, `keys_for(id)` and `shortcut("cmd+z")` read the same
record.

### Declarations

Each of these is an emit, collected like `ui_register` rows:

| Call | Declares |
|---|---|
| `interact(r, id, wants?)` | A rect, its layer and clip, and what it accepts: `{text, keys: [...], drag}` |
| `shortcut(chord, id?)` | A chord the app answers when nothing focused claims it. Returns true on the frame it fires. |
| `focus_request(id)` | Give `id` the keyboard at the next roll. |
| `popover(id, anchor, items, style?)` | A menu or popover open under `anchor`. Its open state is `state` keyed by `id`. Dismissal (a press outside, Escape, opening another) is a dispatch rule, so only one is open at a time. |

`ui_block`, `ui_modal`, `with_layer` and `cached_layer` keep their meaning.

### What goes away

- the menu's `at: frame_count()` skip and `menu_blocking`
- `fc` in and `.focus` out on every field and panel. A shim keeps
  `focus_state()` working.
- the apps' `TYPING` flags and `if mouse_pressed(0) && fc.id != "" then
  focus_clear` at the top of each editor
- the run-time `_floor` raise, and with it the one-frame hover leak
- the same-frame-tap fallback in `roll_press`

`mouse_pressed`, `key_pressed` and friends stay for games and canvases
that want raw input. The widgets stop using them.

### The one-frame question

Owners come from last frame's registry, so a widget that first appears on
this frame cannot own this frame's press. That is already true of `click`
today, and it is correct: the user pressed on what they saw. Hover on a
just-appeared widget lags one frame, as it does now for widgets that run
before their cover. `speculate` is not needed.

## Tooling and checks

- **`ui why X,Y`** (debug port, petal-ui-run, Garden debug): the dispatch
  record for a frame, the registry rows under the point in layer order, and
  the rule that picked the owner. The timeline records `dispatch` per
  frame, so a recorded session answers the question after the fact.
- **Invariants**, checked in debug builds and by `monkey:<seed>`:
  - every event has at most one owner
  - an idle frame (no input) changes no state and draws nothing on the
    popup layer
  - Escape and then a press on empty space always ends with nothing
    captured and no popover open
  - `dispatch` is identical with the frame gate and memo on or off
- **One scenario format**, by target id, not coordinates. It runs against
  `Headless`, petal-ui-run, Garden and the petal-creative editors (their
  `--snapshot` scripted input), so a host's input bug shows up in CI.

## Phases

- **Phase 0 (done 2026-11-07): the bug that started this, and its class.**
  petal-design's toolbar opened its View menu every frame and swallowed
  every click. Here is how:
  - The kit's `dropdown(r, label, id, opts = {})` lost 3-argument calls to
    ui's `dropdown(r, options, current)`, because merging an imported
    overload set replaced only the full arity of a function with defaults.
  - ui's select widget returned `current`, which is truthy, so the menu
    "opened" every frame and re-armed its `at` skip, which kept it from
    ever going live.

  The fix:
  - The merge now replaces every count such a function takes.
  - Two plain imports whose counts overlap through a default are
    ambiguous, as an exact clash already was.
  - The kit's button is now `menu_button`.
  - The same bug had been sending studio's 4-argument `stepper` call to
    ui's `stepper`.
- **Phase 1 (done 2026-11-07): one owner for the pointer.**
  - What shipped: `hit::roll` binds the open rows under the pointer
    (`ui_under`) in registry order. The rect in front (the highest layer,
    then the one registered last) is the owner. Only the owner is hot.
  - Presses: `down` and `click` go to the front row of the press snapshot.
    So a button over a list row takes its click alone. Before, both fired,
    because same-layer rects did not block each other.
  - A rect missing from last frame's registry (new or moved) is answered
    as before. `hit::pointer_owner` and `Headless::pointer_owner` report
    the owner.
  - Not done yet: the whole `dispatch` record (keys, focus, dismissal) is
    Phase 2's. The same-frame tap keeps its fallback.
  - petal-design's canvas picks nodes by its own hit test, not by
    registered rects. A plain click on a gallery page now goes through
    `<section>` and `<phone_row>` to the phone or note in front, where it
    used to pick the whole section behind them.
- **Phase 2 (done 2026-11-07): keys and focus.** What shipped
  (`core-libs/petal-ui/src/focus.rs`):
  - `hit::roll` decides the focus before the script runs and binds it as
    `ui_focus`. The last `focus_request(id)` of the frame before applies
    first. Then a press whose owner declared `{text: true}` or
    `{focus: true}` focuses it on the press frame itself. A press elsewhere
    clears the focus unless it lands on a higher layer.
  - What the focused widget claims is bound as `ui_focus_claims`.
    `shortcut(chord)` is quiet on a claimed chord. Text claims every key
    without Cmd/Ctrl plus the editing chords. `{keys: [...]}` claims the
    listed chords. `shortcut(chord, id)` fires only while `id` has the
    keyboard.
  - Every text field and text area registers as text, and still declares
    its claims when it gets `pointer: false`.
  - The shim runs in the other direction from the one planned: `fc` stays
    the app's value, and `focus_set` / `focus_clear` also emit a request,
    so the runtime follows code that threads `fc`. `focus_state()` is
    unchanged.
  - New names avoid bloom's 1-argument `focused`, `focus_set` and
    `focus_clear`: `has_focus(id)`, `focus_owner()`. Two plain imports
    overlapping in count are ambiguous (Phase 0).
  - Not done yet: key routing to the topmost modal, and one `dispatch`
    record. Focus and claims are two bindings next to `ui_under` and the
    press snapshot, which answers the first open question for now.
- **Phase 3 (done 2026-11-07): `popover`.** What shipped
  (`core-libs/petal-ui/src/popover.rs`):
  - `hit::roll` keeps the one open popover as `ui_popover`. It applies the
    last frame's `popover_open` / `popover_close` requests in order, then
    closes the open one on Escape or a left or right press outside the rect
    it registered under its id. What it closed is `ui_popover_dismissed`
    for that frame.
  - `popover(id, anchor, items)` draws the menu while `id` is open.
    `context_menu` runs on the same state: each `menu_state()` record gets
    its own id, `menu_show` and `menu_close` make requests, and a menu
    takes input once the runtime has it open. So the `at` frame skip is
    gone. `dropdown` is built on `context_menu` and comes along unchanged.
  - While a popover is open, `shortcut` answers only for it, and an Escape
    that dismissed one is not the app's.
  - Found on the way: the press snapshot's settle step added the press
    frame's own rows. So once a dismissing press had closed a menu, the
    button under the menu got the release as a click. Settle now adds only
    rows at or above the press-time floor.
  - `menu_blocking` still says whether a menu is open. Apps no longer need
    it as a guard, and Phase 4 removes it from petal-design. The modal
    dialog still has its own `at` skip.
  - petal-design's GUI scenarios all pass on this commit (39 of 39). An
    earlier run failed because the window was clamped below 1440x900 on a
    smaller screen, which moved the canvas off the scripted click points.
  - Not yet fixed: the SDL window host aborts on an event type the `sdl2`
    crate doesn't know (0x158, from SDL3 through sdl2-compat).
- **Phase 4: port petal-design.** Delete its `fc` threading and `TYPING`,
  and keep its GUI scenarios green. As with D2's kanban rule: if one real
  editor does not port cleanly, stop and revise before Phase 5.
- **Phase 5: port the other editors** (images, movies, social) and Garden
  panels, and delete the shims.

## Open questions

- Should `dispatch` be one binding or several (`ui_pointer`, `ui_keys`)?
  Several gate more finely: a run that reads only keys is not re-run by
  pointer motion.
- Text input with an IME (marked text) needs an owner from the moment
  composition starts. The focus owner is the natural one.
- Garden's `claim_key` should become `shortcut` with a host scope, so that
  Garden's own chords and a pane's are routed by one rule.
