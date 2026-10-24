# Petal IDE mode

Petal IDE is a live-coding layout for [Petal graphical
panels](petal-graphical-panels.md): your Petal source in a full editor on the
left, its rendered canvas on the right, recompiling as you type with no save
round-trip.

This is the user guide. The panel draw and input API you write in the left
pane is in [petal-graphical-panels.md](petal-graphical-panels.md); how the
binding is wired into the app core is in
[architecture.md](architecture.md#the-petal-ide-binding).

## Quick start

```bash
garden petal-ide            # a persistent scratch sketch
garden petal-ide my.ptl     # your own file
```

You get a two-pane window:

```
┌───────────────────────────┬───────────────────────────┐
│  editor: my.ptl           │  rendered canvas of my.ptl │
│  (full vim editing,       │  (updates as you type,     │
│   syntax highlight,       │   ~60fps while animating)  │
│   line numbers)           │                            │
└───────────────────────────┴───────────────────────────┘
```

Change a number in the seeded starter sketch and the canvas updates on the
next keystroke.

- `petal-ide my.ptl` opens that file (resolved against the current
  directory). A file that does not exist yet is seeded with a runnable
  starter sketch, so the canvas is never blank on launch. An existing file is
  opened as-is.
- `petal-ide` with no argument opens a persistent scratch at
  `~/.garden/petal-ide/scratch.ptl`. The scratch is save-protected: `Cmd+S`
  or `:w` opens a filename prompt (`:w ` pre-filled) so you save to a real
  file, after which both panes re-point to it.

Both panes address the same absolute path, which is what pairs them.

## The live binding

The right pane is a normal panel; the one extra behavior is that it is
driven by the left pane's live buffer instead of the file on disk.

- **Edits apply immediately, unsaved.** The panel recompiles from the
  editor's text on the next frame. The file on disk is touched only when you
  save.
- **`state` survives every recompile**, the same way it survives a disk hot
  reload, so an animation clock or a scroll position keeps its place while
  you edit around it. Only Reset (below) or reopening the pane resets it.
- **A syntax error keeps the last good frame.** The previous program keeps
  running and a red banner across the top of the canvas shows the message
  with line and column. Fix the code and the banner clears the instant it
  compiles again.
- **`Cmd+S` writes to disk** as usual, so a plain `panel("my.ptl")` layout or
  an external tool picks up your work.

## The toolbar

Petal IDE draws a toolbar below the titlebar. It is app chrome, not part of
the canvas, and appears only for `garden petal-ide`.

- **Pause / Play**: freeze or resume the canvas. While paused the canvas
  holds its last frame and the IR pane stops refreshing, but the editor stays
  live, so you can freeze a moment, edit, and hit Play to see the change all
  at once. [Direct manipulation](#direct-manipulation) keeps working on the
  frozen frame; freezing a moving sketch so you can point at a shape is much
  of what pause is for. A paused sketch costs no CPU.
- **IR**: open or close the [IR inspector](#the-ir-inspector). Lit while open.
- **State**: toggle the [state inspector](#the-state-inspector) overlay, the
  same as `:State`. Lit while on.
- **Reset**: restart the sketch from scratch, discarding Petal `state` so
  `state x = …` initializers re-run. Unsaved editor edits are kept.

Play, IR, and State are also `/menu` actions (`TogglePlay`, `ToggleIr`,
`ToggleStateInspector`), so a headless session can drive them (see
[Automation](#automation)).

## The IR inspector

The IR button opens a pane showing what your program compiles to, updating
as you type:

```
┌───────────────┬─────────────────────┬───────────────┐
│  editor       │  IR / Bytecode / AST │  canvas       │
│  (your source)│  (of your source,    │               │
│               │   live, selectable)  │               │
└───────────────┴─────────────────────┴───────────────┘
```

Tabs across the top switch representation: **IR** (Petal's term-graph IR,
the primary view), **Bytecode** (the lowered VM instructions), **AST** (the
parsed statement tree). The text is a native selectable, scrollable region
(copy with `Cmd-C`) and recompiles the editor's live buffer on every
keystroke; a syntax error shows the compiler's message in place. Pausing
freezes it too.

The inspector is itself a Petal panel (`~/.garden/petal-ide/ir_view.ptl`,
seeded on first launch and editable like any drawer) fed by a host data
provider; the rendering is Petal's own `show-ir` / `show-bytecode` /
`show-ast` machinery.

## The editor side

The left pane is Garden's ordinary editor: modal vim editing, syntax
highlighting (Petal is a bundled grammar), the line-number gutter, soft
wrap, search, the fuzzy file finder (`Cmd/Ctrl+P`), undo/redo, and
system-clipboard copy/paste. See [keybindings.md](keybindings.md). You can
rearrange the panes at runtime like any Garden layout.

## Writing the canvas

The file you edit is a panel sketch: Garden runs it every frame and paints
what it draws. The vocabulary, documented fully in
[petal-graphical-panels.md](petal-graphical-panels.md):

- **Draw:** `clear`, `draw_rect`, `draw_rect_outline`, `draw_line`,
  `draw_circle`, `fill_triangle`, `fill_polygon`, `draw_text`, and the rest.
- **Timing and size:** `dt()`, `frame_count()`, `screen_width()`,
  `screen_height()`.
- **Input:** `mouse_x()` / `mouse_y()`, `mouse_pressed()`,
  `key_pressed(name)`, `scroll_y()`, and the rest of the petal-ui input
  contract, so the canvas can be interactive as well as animated.
- **`state x = …`** persists a variable across frames and across live
  reloads.
- **`palette()`** returns the host's current colors, so a sketch can paint in
  the editor's scheme.

The seeded starter sketch is a small annotated example of most of these.

## The state inspector

`:State` (or the State button) toggles a translucent card in the canvas's
top-left listing every value the last frame bound (`state` variables among
them) plus the frame count, updated every frame. It is the quickest way to
watch state evolve without print statements. The same data is available
headlessly at `GET /state` under `.panes[i].panel.values`.

## Automation

The live binding lives in the app core, so the mode runs under the headless
frontend for scripted testing and screenshots:

```bash
PORT=8080
garden petal-ide /abs/path/my.ptl --headless --debug-port $PORT &
# inject edits into the editor (pane 0)
curl -s -X POST 127.0.0.1:$PORT/key  -d '{"key":"G"}'
curl -s -X POST 127.0.0.1:$PORT/key  -d '{"key":"o"}'
curl -s -X POST 127.0.0.1:$PORT/text -d '{"text":"draw_circle(100, 100, 30, 240, 90, 80)"}'
# then observe the panel it drives (pane 1)
curl -s 127.0.0.1:$PORT/state | jq '.panes[1].panel | {frame, error, values}'
curl -s 127.0.0.1:$PORT/screenshot -o canvas.png   # offscreen GPU render
```

`/state` exposes each panel's `frame` counter, the `values` its last good
frame bound (keyed by function-qualified name), and its current `error`
(the banner text, or `null`). Full protocol:
[debug-server.md](debug-server.md).

The toolbar controls are `/menu` actions:

```bash
curl -s -X POST 127.0.0.1:$PORT/menu -d '{"action":"ToggleIr"}'    # open the IR pane
curl -s -X POST 127.0.0.1:$PORT/menu -d '{"action":"TogglePlay"}'  # pause the canvas
# the IR pane names its selected stage and rendered size:
curl -s 127.0.0.1:$PORT/state | jq '.panes[] | select(.panel.script|test("ir_view"))
                                    | .panel.values'  # stage, stage_count, body_len, has_error
```

The canvas is a GPU panel, so it renders in the windowed frontend and in
headless `/screenshot`. `--term` rasterizes a panel poorly; use it for the
editor, not the canvas.

## How it works

`garden petal-ide` is a thin launcher: it builds the layout
`row([editor(file), panel(file)])` over one absolute path and hands it to
the usual frontend wiring. The live behavior is a general rule, not a
special case: `App::sync_editor_panels` recompiles any `panel(...)` pane
whose script path matches a live editor pane's file from that editor's
buffer, hash-gated so an unchanged buffer costs nothing.

So a hand-written layout gets the same as-you-type canvas by putting an
editor and a panel on the same file:

```petal
import garden_config

// init.ptl: a permanent Petal-IDE-style workspace
layout(
    row([ editor("sketch.ptl"), panel("sketch.ptl") ], [0.5, 0.5])
)
```

## Direct manipulation

Move the mouse over a shape on the canvas and the `draw_*` call that drew it
lights up in the editor. **Cmd-click** (Ctrl-click elsewhere) puts the cursor
on that call and focuses the editor: go-to-definition with a pixel as the
symbol. **Cmd-drag** it and the code changes instead
([Dragging a shape](#dragging-a-shape-to-edit-its-code)).

```bash
garden petal-ide examples/panels/direct-manipulation.ptl
```

That demo is annotated for the cases worth understanding:

- **Overlapping shapes resolve to the one you can see.** The shape under the
  pointer is the last one painted there, the same one your eye picks.
- **Many shapes, one call.** Bars drawn by a `draw_rect` inside a loop all
  highlight that single line, which is the code you would edit to change any
  bar.
- **Your helper functions are traced through.** A shape drawn inside `fn
  swatch(...)` highlights the `draw_rect` in the function body. The trace
  walks out of library code (every `draw_*` is itself a Petal function in the
  petal-ui prelude) and stops at the first frame in the file you are editing.
- **An outline is picked on its stroke.** Hovering the hollow middle of a
  `draw_rect_outline` reaches whatever is drawn inside it.

Two rules about when it answers:

- **A plain click still belongs to the sketch.** The jump is behind Cmd/Ctrl
  on purpose: `mouse_pressed()` is part of the panel input contract, and an
  interactive sketch must not lose its clicks to the editor.
- **A broken buffer highlights nothing.** While your code does not compile,
  the canvas shows the last good frame, whose spans describe the text that
  compiled; insert a line above and every span is off by one. Rather than
  band the wrong line, the highlight goes quiet until the buffer compiles. A
  runtime error is not this case; tracing keeps working.

Nothing in a sketch opts in: the attribution comes from the runtime (Petal's
lowerer stamps each instruction with its IR term, and a `draw_*` native
records the call chain that reached it as it emits), so any sketch is
traceable as written, and a panel with no paired editor records nothing.
Spans and argument values are derived lazily on the mouse move that asks.
The language-side how-to is
[docs/features/direct-manipulation.md](../../docs/features/direct-manipulation.md).

### Automation

The highlight is exposed on each editor pane's `/state`, so the mapping is
assertable without decoding pixels:

```bash
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"move","x":761,"y":182}'
curl -s 127.0.0.1:$PORT/state | jq '.panes[0].trace_highlight'
# { "start": { "line": 15, "col": 0 }, "end": { "line": 15, "col": 39 } }
```

Lines and columns are 0-based. A top-level `trace` carries the whole traced
call, `null` when the pointer is over no shape:

```bash
curl -s 127.0.0.1:$PORT/state | jq '.trace | {callee, line: .call.start.line, args}'
# {
#   "callee": "draw_rect",
#   "line": 53,
#   "args": [
#     { "index": 0, "source": "binding", "value": 300, "is_int": true,
#       "span":          {"start": {"line": 53, "col": 10}, …},   # `edge`, at the call
#       "editable_span": {"start": {"line": 51, "col": 11}, …} }, # `300`, at its definition
#     …
#   ]
# }
```

`source` is `literal`, `binding`, or `computed`; `editable_span` is the range
a rewrite must replace, which for a `binding` is at the definition.

The jump gesture takes the same `mods` array `/key` does:

```bash
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":761,"y":182,"mods":["cmd"]}'
curl -s 127.0.0.1:$PORT/state | jq '{focus, cursor: .panes[0].cursor}'
# { "focus": 0, "cursor": { "line": 15, "col": 0 } }
```

### Dragging a shape to edit its code

Cmd/Ctrl-drag a shape and the numbers that placed it are rewritten under
your pointer.

```bash
garden petal-ide examples/panels/drag-to-edit.ptl   # a sketch built to be dragged
```

Press with Cmd held and pull. Nothing is written until you move a few
pixels, so Cmd-click still jumps. The whole gesture is one undo step. The
edits go into the buffer, not the file: nothing touches disk until you save.

**What you are saying.** A drag states a goal, "this shape should be 25px to
the right", and Petal answers with the edit. Which numbers move is Petal's
answer, not yours, and it can be any number the shape's position depends on:

- **A literal in the call** (`draw_rect(46, 210, …)`) is rewritten in place,
  keeping its spelling: an integer stays an integer.
- **A binding** (`let edge = 300 … draw_rect(edge, …)`) is rewritten at its
  definition, so every shape reading it moves. The status bar says so.
- **Anything computed**, through any number of function calls: a helper
  that returns the position (`draw_circle(place(i), …)`), a record or list
  built elsewhere, a `map` over a list, a loop counter, trigonometry. Petal
  re-runs the frame with each candidate number nudged, measures how far the
  shape moves, solves for the drag, and re-runs the answer to check it lands
  where you let go.

There are two engines behind this, picked per drag:

- **Direct** (v1) is used when every position argument is a literal or a
  binding written in the call. It is exact and costs nothing, and the edit
  follows the pointer frame by frame.
- **Solved** (v2) is used for everything else (a computed position, a value
  returned by a function, any shape of a loop, the `rect`- and
  `center`-taking overloads of `draw_rect` / `draw_circle`), and for every
  drag while `:DragAllow` / `:DragDeny` is set. See
  [Petal's direct manipulation v2](../../docs/dev/direct-manipulation-v2-plan.md).

`:DragEngine v1`, `:DragEngine v2` and `:DragEngine auto` (the default)
force the choice; `:DragEngine` alone reports it.

**A solved drag, step by step.**

1. *Press.* The canvas keeps the frame on screen as the starting point
   (its *checkpoint*: the stack before that frame ran, so an animation
   re-runs to the position you see).
2. *First move past the slop.* Petal finds the numbers behind the shape and
   measures each one. This is the one pause in the gesture, capped at 400 ms
   (`garden_script::OPEN_WALL`); on a 300-shape sketch it takes about
   90 ms (31 re-runs of a 2.5 ms frame).
3. *While you pull.* Each move is a linear prediction from those
   measurements, with no re-runs. The editor shows the predicted numbers
   (marked `(preview)` in the status bar) and the grabbed shape follows the
   pointer. The rest of the canvas stays on the program you pressed on until
   you let go.
4. *Release.* Petal solves for where you let go, rounds to tidy numbers,
   re-runs the result to verify it, and ranks the alternatives. The best one
   is written; the canvas recompiles from it on the next frame.

**Choosing what moves.** A drag can usually be answered several ways
(widen the gap, or move the origin), so the answer comes ranked:

- **`config let` is the strong hint.** A binding declared `config let` (or a
  literal wrapped in `tunable(...)`) is preferred over the others, so a sketch
  that names its knobs gets those moved. Other numbers still move when no
  knob can do the job.
- **The status bar names the edit and the alternatives**: `set gap to 36
  (line 2) — moves 2 other shapes [1 of 3: :DragNext] — also movable: x0
  (:DragAllow)`.
- **`:DragNext`, `:DragPrev` and `:DragPick N`** write another alternative
  in place of the first, from the text as it was when you pressed, so they
  never stack. They work until you edit the code by hand.
- **`:DragAllow NAME …`** restricts every following drag to those numbers,
  exactly; **`:DragDeny NAME …`** rules numbers out. A selector is a binding
  name (`gap`), `line:N` (any number on that line, 1-based), `fn:NAME` (the
  numbers in that function's body), or `config`. With no argument each
  clears. While either is set, every drag uses the solved engine.
- Numbers in libraries (the `ui` prelude, packages) never move; nor do the
  ones that feed `random()`, a loop's bounds or a comparison, or that are
  wrapped in `rigid(...)`.

**When it declines.** A drag that cannot be answered honestly says so in the
status bar rather than rewriting something nearby, and lists what could move
when anything could: nothing the shape depends on is a number you may edit,
the frame cannot be re-run without input it depends on, or the shape is not a
position pair (a polygon).

**What can be grabbed.** Anything drawn with a position: `draw_rect` (every
overload), `draw_rect_rounded`, `draw_rect_outline`, `draw_circle`,
`draw_circle_outline`, `draw_ellipse`, `fill_arc`, `fill_fan`, `draw_text`,
`draw_image`, and `draw_line` (which moves both endpoints). Polygons,
triangles and the batch `draw_rects` are not draggable.

**Cost.** While a canvas is paired with an editor it keeps the checkpoint:
one copy of the sketch's heap per frame that runs (a frame the gate skips
costs nothing). A drag's speculative re-runs never print, never write the
panel store and never send `emit` events.

**Driving it headlessly** is three debug-server calls, and `/state`'s `dm`
reports the engine, the selectors and the last solved drag (its ranked
alternatives and every candidate number, with whether it could move and why
not):

```bash
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"down","x":735,"y":314,"mods":["cmd"]}'
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"move","x":775,"y":339}'
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"up"}'
curl -s 127.0.0.1:$PORT/buffer/0 | sed -n 24p     # the rewritten line
curl -s 127.0.0.1:$PORT/state | jq '.status_note, .dm.last.alternatives[].description'
curl -s 127.0.0.1:$PORT/state | jq '.dm.last.candidates[] | {label, eligible, moves, why_not}'
```

`tools/dm-drag-integration-test.ts` drives exactly this.

## Troubleshooting

- **A shape doesn't highlight anything.** Only a panel paired with a live
  editor pane on the same file traces. A panel opened on its own records
  nothing.
- **Highlighting stopped, and there's a red banner.** Expected while the
  buffer doesn't compile (above). Fix the error and it comes back.
- **Cmd-click did nothing.** It only jumps from a shape; a press on bare
  canvas falls through to the ordinary click. It also needs a paired editor
  and a compiling buffer.
- **Cmd-drag moved nothing, and the status bar said why.** See "When it
  declines" above.
- **Cmd-drag moved something I did not grab.** The number it changed is one
  other shapes read; the status bar says how many moved. Try `:DragNext` for
  the next alternative, or undo and `:DragAllow` the number you meant (the
  status bar lists the others it could have moved), or make it a
  `config let` knob.
- **Cmd-drag paused before the shape followed.** That is the solved engine
  measuring the numbers behind the shape (at most 400 ms, once per drag).
- **The canvas is blank or shows `could not load panel`.** The file must be a
  valid panel script. A brand-new file is seeded; an existing non-panel file
  shows its first compile error in the pane.
- **The canvas stopped animating.** A panel sleeps ~10 s after its last
  activity; any edit or input wakes it. A static sketch has nothing to
  re-animate.
- **My edit didn't show up.** Only the pane that shares the file is driven.
  Check both panes address the same path.
- **`--term` shows no graphics.** Expected: the canvas is a GPU panel.
