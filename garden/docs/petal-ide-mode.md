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
[docs/direct-manipulation.md](../../docs/direct-manipulation.md).

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
edits go into the buffer, which is what makes the shape follow the pointer:
the live binding recompiles the canvas on the next frame. Nothing touches
disk until you save.

**What you are saying.** A drag states a goal, "this argument should
evaluate to 148", and the runtime answers with the edit
([Petal's `direct_manipulation`](../../docs/direct-manipulation.md)):

- **A literal in the call** (`draw_rect(46, 210, …)`) is rewritten in place,
  keeping its spelling: an integer stays an integer.
- **A binding** (`let edge = 300 … draw_rect(edge, …)`) is rewritten at its
  definition, so every shape reading it moves. The status bar says so: `set
  edge to 325 (line 13) — shared, other shapes read it`.
- **A computed position** (`x0 + i * spacing`) has no number to edit, so the
  runtime solves it, inverting the arithmetic against the values the traced
  run saw, and moves one of the variables that feed it.

**Telling it which variables are knobs.** When a position is computed, more
than one variable could satisfy the goal. Say which one means something with
`config let`:

```petal
config let spacing = 78
let x0 = 46
for i in range(0, 4) do
  draw_rect(x0 + i * spacing, 96, 44, 44, 70, 165, 150)
end
```

Declaring any `config let` makes config bindings the preferred targets and
pins every other named binding. Dragging a card here re-spaces the row
(`spacing` moves) instead of sliding it. Call-site literals stay editable
either way.

**When it declines.** A drag that cannot be answered honestly says so in the
status bar rather than rewriting something nearby:

- **Nothing to move**: every candidate is pinned, or the argument flows
  through a call with no literal behind it.
- **One of N shapes drawn by this call, and its position is computed**:
  solving inverts against the last value each term took, which is the last
  iteration's, so only the last shape a looping call drew has current
  numbers. Drag that one, or edit the loop by hand. Non-computed arguments
  have no such limit.

**What can be grabbed.** The position arguments of `draw_rect`,
`draw_rect_rounded`, `draw_rect_outline`, `draw_circle`, `draw_text`,
`draw_image`, and `draw_line` (which moves both endpoints as one batch of
goals). The `rect`-taking overloads and polygons are not draggable: one
argument carries both axes, so there is no pair of numbers to move.

**Driving it headlessly** is three debug-server calls:

```bash
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"down","x":735,"y":314,"mods":["cmd"]}'
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"move","x":775,"y":339}'
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"up"}'
curl -s 127.0.0.1:$PORT/buffer/0 | sed -n 24p     # the rewritten line
curl -s 127.0.0.1:$PORT/state | jq .status_note   # what the drag reported
```

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
- **Cmd-drag moved something I did not grab.** The position was a binding
  other shapes read; the status bar flags the edit as `shared`. Undo, then
  make the variable a `config let` knob or inline the number.
- **The canvas is blank or shows `could not load panel`.** The file must be a
  valid panel script. A brand-new file is seeded; an existing non-panel file
  shows its first compile error in the pane.
- **The canvas stopped animating.** A panel sleeps ~10 s after its last
  activity; any edit or input wakes it. A static sketch has nothing to
  re-animate.
- **My edit didn't show up.** Only the pane that shares the file is driven.
  Check both panes address the same path.
- **`--term` shows no graphics.** Expected: the canvas is a GPU panel.
