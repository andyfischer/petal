# Petal IDE mode

**Petal IDE** is a live-coding layout for [Petal graphical panels](petal-graphical-panels.md):
your Petal source in a full editor on the left, its rendered canvas on the right,
recompiling **as you type** — no save round-trip. It turns Garden into a
playground for writing panel sketches, where every edit shows on the canvas the
moment you make it.

This doc is the user guide. For how the live binding is wired into the app core,
see the **live editor↔panel binding** entry in [`architecture.md`](architecture.md);
for the panel draw/input API you write in the left pane, see
[`petal-graphical-panels.md`](petal-graphical-panels.md).

## Quick start

```bash
cargo run -p garden-app -- petal-ide            # a persistent scratch sketch
cargo run -p garden-app -- petal-ide my.ptl     # your own file
# or, once installed:
garden petal-ide my.ptl
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

Start typing draw calls in the left pane and watch the right pane react. Try
changing a number in the seeded starter sketch — the canvas updates on the next
keystroke.

### What the file argument does

- **`petal-ide my.ptl`** opens that file (resolved against the current
  directory). If it does **not exist yet**, it is seeded with a runnable starter
  sketch, so the canvas is never blank on launch — like `vim newfile`, but
  non-empty. An existing file is opened as-is and never overwritten.
- **`petal-ide`** with no argument opens a persistent scratch at
  **`~/.garden/petal-ide/scratch.ptl`** (seeded on first use), so you always have
  a sandbox to doodle in that survives between sessions. The scratch is
  **save-protected**: `Cmd+S` (or `:w`) does not overwrite it — it opens a
  filename prompt (`:w ` pre-filled) so you save your sketch to a real file. Once
  saved, the editor and canvas re-point to that file and save there normally.

Either way both panes address the **same absolute path**, which is what pairs
them (see [How it works](#how-it-works)).

## The live binding

The right pane is a normal Petal [panel](petal-graphical-panels.md); the only
extra behaviour Petal IDE adds is that it is **driven by the left pane's live
buffer** instead of only the file on disk.

- **Edits apply immediately, unsaved.** As you type, the panel recompiles from
  the editor's current text on the next frame. You do not need to save first;
  the file on disk is only touched when you actually save.
- **`state` survives every recompile.** Petal `state` variables are transferred
  across each live reload (the same mechanism as disk hot-reload), so an
  animation clock, a counter, or a scroll position keeps its place while you edit
  the code around it. Only a genuine restart (re-opening the pane) resets state.
- **A syntax error keeps the last good frame.** While the buffer doesn't compile,
  the previously-working program keeps running and rendering, and a red **error
  banner** across the top of the canvas shows the message and line/column:

  ```
  ⚠ panel error: Missing closing ')' [line 41, column 28]
  ```

  Fix the code and the banner clears the instant it compiles again — the canvas
  never goes blank and the animation never stops.
- **`Cmd+S` still writes to disk** as usual, so when you're happy you save the
  file like any other buffer. (Saving also means an external tool or a plain
  `panel("my.ptl")` layout picks up your work.)

## The toolbar

Petal IDE draws a **toolbar** across the top (below the titlebar) with buttons for
the live-coding controls. It is IDE-mode chrome — like the titlebar and status
bar it is drawn by the app, not the canvas, and only appears for `garden
petal-ide`:

- **⏸ Pause / ▶ Play** — freeze or resume the canvas re-render. While paused the
  canvas holds its last frame and the IR panel stops refreshing, but the **editor
  stays fully live** — so you can freeze a moment, edit the code, and hit ▶ to see
  the accumulated change all at once (or step your own animation by pausing and
  resetting). [Direct manipulation](#direct-manipulation-point-at-a-shape-find-its-code)
  keeps working too: freezing a moving sketch so you can point at a shape is a
  large part of what pause is *for*. Pausing also drops Garden back to its idle
  poll cadence, so a paused sketch costs no CPU.
- **IR** — open or close the [IR inspector](#the-ir-inspector) pane. Lit while it's
  open.
- **State** — toggle the live-[state inspector](#debugging-the-live-state-inspector)
  overlay (the same as `:State`). Lit while it's on.
- **Reset** — restart the sketch from scratch, discarding Petal `state` (the
  animation clock, counters, …) so `state x = …` initializers re-run. Your unsaved
  editor edits are kept — only the runtime state restarts.

Every button also has a menu/`/menu` action (`TogglePlay`, `ToggleIr`,
`ToggleStateInspector`), so the controls are drivable headlessly (see
[Automation](#automation--headless)).

## The IR inspector

The **IR** button opens a pane showing what your program *compiles to*, updating
as you type — a window into Petal's compiler beside the canvas:

```
┌───────────────┬─────────────────────┬───────────────┐
│  editor       │  IR / Bytecode / AST │  canvas       │
│  (your source)│  (of your source,    │               │
│               │   live, selectable)  │               │
└───────────────┴─────────────────────┴───────────────┘
```

A row of tabs across the top is the **menu**; click one to switch representation:

- **IR** — Petal's term-graph IR (the primary view: blocks and per-term dataflow).
- **Bytecode** — the lowered VM instructions the program actually runs.
- **AST** — the parsed statement tree.

The text is a native selectable, scrollable region (copy with `Cmd-C`), and it
recompiles the editor's **live buffer** on every keystroke — so you watch the IR
change as you edit, and a syntax error shows the compiler's message in place
rather than blanking. Pausing (⏸) freezes the IR too.

The inspector is a normal Petal panel (`~/.garden/petal-ide/ir_view.ptl`, seeded
on first launch and editable like any drawer) fed by a host data provider; the
rendering comes from Petal's own `show-ir` / `show-bytecode` / `show-ast`
machinery, packaged upstream as `petal::inspect`. See the **Petal-IDE inspector**
note in [`architecture.md`](architecture.md).

## The editor side

The left pane is Garden's ordinary editor, so everything you already know works:
modal vim editing, syntax highlighting (Petal `.ptl` is a bundled grammar), the
line-number gutter, soft wrap, search (`/`, `*`), the fuzzy file finder
(`Cmd/Ctrl+P`), undo/redo, and system-clipboard copy/paste. See the editor
sections of the [README](../README.md).

You can also rearrange the panes at runtime like any Garden layout — `Ctrl-W`
navigation and splits all apply — though the editor|canvas split is the point.

## Writing the canvas

The file you edit is a Petal panel sketch: Garden runs it every frame and paints
what it draws. The vocabulary (documented fully in
[`petal-graphical-panels.md`](petal-graphical-panels.md)):

- **Draw:** `clear`, `draw_rect`, `draw_rect_outline`, `draw_line`,
  `draw_circle`, `fill_triangle`, `fill_poly`, `draw_text`.
- **Timing / size:** `dt()`, `frame_count()`, `screen_width()`, `screen_height()`.
- **Input:** `mouse_x()`/`mouse_y()`, `mouse_pressed()`, `key_pressed(name)`,
  `scroll_y()`, … — the standard `petal-ui` input contract, so the canvas can be
  interactive, not just animated.
- **`state x = …`** persists a variable across frames (and across live reloads).
- **`panel_theme()`** returns the host's current color scheme, so a sketch can
  paint in the editor's colors.

The seeded starter sketch is a small annotated example of most of these.

## Debugging: the live-state inspector

`:State` toggles a **state inspector** overlay on the canvas: a translucent card
in the top-left listing every value the last frame bound (name = value —
`state` variables among them, since a `state` var is a named term like any
other), plus the current frame count, all updated every frame. It's the quickest way to watch state evolve (or catch a variable
stuck at the wrong value) while live-coding, without adding print statements. The
same data is available headlessly at `GET /state` (`.panes[i].panel`).

## Automation / headless

The mode is frontend-independent — the live binding lives in the app core — so it
runs under the headless frontend for scripted testing and screenshotting:

```bash
PORT=8080
garden petal-ide /abs/path/my.ptl --headless --debug-port $PORT &
# inject edits into the editor (pane 0) …
curl -s -X POST 127.0.0.1:$PORT/key  -d '{"key":"G"}'
curl -s -X POST 127.0.0.1:$PORT/key  -d '{"key":"o"}'
curl -s -X POST 127.0.0.1:$PORT/text -d '{"text":"draw_circle(100, 100, 30, 240, 90, 80)"}'
# … then observe the panel it drives (pane 1):
curl -s 127.0.0.1:$PORT/state | jq '.panes[1].panel | {frame, error, values}'
curl -s 127.0.0.1:$PORT/screenshot -o canvas.png   # offscreen GPU render
```

`/state` exposes each panel's `frame` counter, every value its last good frame
bound (the `values` object, keyed by function-qualified source name), and its
current `error` (the banner text, or `null` when healthy) — enough to assert the canvas end-to-end without decoding
pixels. Full protocol: [`debug-server.md`](debug-server.md).

The toolbar controls are `/menu` actions, so a headless session drives them too:

```bash
curl -s -X POST 127.0.0.1:$PORT/menu -d '{"action":"ToggleIr"}'    # open the IR pane
curl -s -X POST 127.0.0.1:$PORT/menu -d '{"action":"TogglePlay"}'  # pause the canvas
# the IR pane names its selected stage + rendered size, so both are observed:
curl -s 127.0.0.1:$PORT/state | jq '.panes[] | select(.panel.script|test("ir_view"))
                                    | .panel.values'  # incl. stage, stage_count,
                                                      # body_len, has_error (a bool)
```

The canvas itself is a **GPU panel**, so the rich graphics only render in the
default windowed frontend (and in headless `/screenshot`, which uses the same
renderer offscreen). `--term` rasterizes a panel poorly — use it for the editor,
not the canvas.

## How it works

`garden petal-ide` is a thin launcher: it builds the fallback layout
`row([editor(file), panel(file)])` over one absolute path and hands it to the
usual frontend wiring (`resolve_petal_ide_subcommand` in `main.rs`). The live
behaviour is a **general rule**, not special-cased to the CLI:
`App::sync_editor_panels` (run at the top of every panel tick) recompiles any
`panel(...)` pane whose resolved script path matches a **live editor pane's
file** from that editor's buffer text — via `PanelView::reload_from_editor` →
`PanelHost::reload_source` (`compile_program` + `transfer_state`, preserving
`state`). Pairing is by resolved path, so unrelated panels are untouched, and the
recompile is hash-gated per panel so an unchanged buffer costs nothing.

Because it's a general rule, you can get the same as-you-type canvas from a
hand-written layout script — put an editor and a panel on the same file:

```petal
// init.ptl — a permanent Petal-IDE-style workspace
layout(
    row([ editor("sketch.ptl"), panel("sketch.ptl") ], [0.5, 0.5])
)
```

`petal-ide` is just the one-line way to get there without writing the layout.

## Direct manipulation: point at a shape, find its code

Move the mouse over a shape on the canvas and the `draw_*` call that drew it
**lights up in the editor**. **Cmd-click** (Ctrl-click elsewhere) puts your
cursor on that call and moves focus to the editor, so the shape you were
pointing at is the code you are now typing in — go-to-definition, with a pixel as
the symbol. **Cmd-drag** it and the code changes instead:
[Dragging a shape to edit its code](#dragging-a-shape-to-edit-its-code). It is
the live binding run backwards: instead of asking what your code draws, you point
at what is drawn and ask where it came from — or push it somewhere else and let
the file catch up.

```bash
garden petal-ide examples/panels/direct-manipulation.ptl
```

That demo is annotated for the cases worth understanding:

- **Overlapping shapes resolve to the one you can see.** A panel frame is a flat
  list of commands painted in order, so the shape under the pointer is the *last*
  one covering it — the same one your eye picks.
- **Many shapes, one call.** Bars drawn by a `draw_rect` inside a loop all
  highlight that single line. Which is right: there is one piece of code there,
  and it is what you would edit to change any bar.
- **Your helper functions are traced through.** A shape drawn inside
  `fn swatch(...)` highlights the `draw_rect` in the function body. The trace
  walks *out* of library code — every `draw_*` name is itself a Petal function in
  the `petal-ui` prelude — but stops at the first frame in the file you are
  editing, which is the code you can actually act on.
- **An outline is picked on its stroke.** Hovering the hollow middle of a
  `draw_rect_outline` reaches whatever is drawn inside it, the way a picture frame
  doesn't intercept the picture.

Two rules about *when* it answers:

- **A plain click still belongs to the sketch.** The jump is behind Cmd/Ctrl on
  purpose: `mouse_pressed()` is part of the panel input contract, and an
  interactive sketch must not lose its clicks — or the keyboard — to the editor.
  Hovering is implicit because it changes nothing; acting is not.
- **A broken buffer highlights nothing.** While your code doesn't compile the
  canvas keeps showing the last good frame, but that frame's spans describe the
  text that *compiled* — insert a line above and every one is off by one. Rather
  than band a line it no longer means, the highlight goes quiet until the buffer
  compiles again (the error banner is already telling you why). A *runtime* error
  is not this case: the program still matches the source on screen, so tracing
  keeps working.

Nothing in a sketch opts in: the attribution comes from the runtime, so any
sketch is traceable as written. Like the live binding, it is a **general rule**
rather than a `petal-ide` special case — a hand-written layout with an editor and
a panel on the same file gets direct manipulation too.

### How the trace works (and why it's cheap)

The obvious implementations are both worse than what Petal can do:

- *Run the program a second time with tracing bindings.* The second run has to
  reproduce the first exactly — same `random()`, same clock, same input — so it is
  only ever as sound as the sketch is deterministic.
- *Register extra callbacks per draw native in IDE mode.* Sound, but it makes the
  hot draw path pay dispatch for a feature almost no run uses.

Neither is needed, because **the runtime already knows the call site**. Petal's
bytecode lowerer stamps every instruction with the IR term it came from, and the
VM hands that term to each native it calls. So a `draw_*` native can record which
call drew each command as it emits it — one id, no extra work, nothing re-run.

Two details make that actually usable:

- **It records the call *chain*, not just the innermost call.** The leaf is
  usually a line of the `petal-ui` prelude; the reader picks the innermost frame
  belonging to the file it is showing (`petal::provenance::pick_frame`).
- **Everything richer is derived lazily.** Spans, argument positions, and literal
  values are resolved from the recorded ids on the mouse move that asks — not on
  the frame that drew. A 60fps canvas pays one short id list per shape and no
  more, and a panel with no paired editor records nothing at all.

Hit-testing needs no spatial index for the same reason: the command list is
rebuilt every frame anyway, so there is nothing to index across frames, and a
linear scan on one mouse move is not a cost worth a data structure.

The pieces: `ExecutionContext::trace_emit` + `emit_origins` (recording),
`petal::provenance` (resolution), `petal_ui::draw::take_draw_commands_traced`
(the drain), `garden_script::panel_trace` (hit-testing and the span conversion),
`PanelView::trace_at`, and `EditorView::trace_highlight` (the band you see).
The language-side how-to — for building your own host on this — is
[`../docs/direct-manipulation.md`](../../docs/direct-manipulation.md).

### Automation / headless

The highlight is exposed on each editor pane's `/state`, so the canvas→source
mapping is assertable without decoding pixels:

```bash
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"move","x":761,"y":182}'
curl -s 127.0.0.1:$PORT/state | jq '.panes[0].trace_highlight'
# { "start": { "line": 15, "col": 0 }, "end": { "line": 15, "col": 39 } }
```

Lines and columns are 0-based, matching the editor's own coordinates.

A top-level **`trace`** carries the *whole* traced call — the part a drag mode
would act on, not just the range the editor bands. There is one pointer, so
there is one of these; it is `null` when the pointer is over no shape:

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

`source` is `literal` / `binding` / `computed`, and `editable_span` is the range
a rewrite must replace — which for a `binding` is at the definition, so a test
can assert that a drag would have moved more than one shape.

The jump gesture is drivable too — `/mouse` takes the same `mods` array `/key`
does (`"shift": true` still works as the shorthand it always was):

```bash
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":761,"y":182,"mods":["cmd"]}'
curl -s 127.0.0.1:$PORT/state | jq '{focus, cursor: .panes[0].cursor}'
# { "focus": 0, "cursor": { "line": 15, "col": 0 } }
```

### Dragging a shape to edit its code

Cmd/Ctrl-**drag** a shape and the numbers that placed it are rewritten under your
pointer. The canvas is not a preview of the code any more; it is a handle on it.

```bash
garden petal-ide examples/panels/drag-to-edit.ptl   # a sketch built to be dragged
```

Press with Cmd held and pull. Nothing is written until you actually move (a few
pixels of slop), so Cmd-*click* still jumps to the code — one modifier, click to
go there, drag to change it. The whole gesture is **one undo step**: `u` puts the
shape back where it was, not a pixel at a time.

The edits go into the **buffer**, which is what makes the shape follow the
pointer: the live binding recompiles the canvas from the buffer on the next
frame. Nothing touches disk until you save.

#### What you are actually saying

A drag never says "change the text at line 12". It states a **goal** — *this
argument should evaluate to 148* — and the runtime answers with the edit
([Petal's `direct_manipulation`](../../docs/direct-manipulation.md)). What
that means in practice, in the order you'll meet it:

- **A literal in the call** (`draw_rect(46, 210, …)`) is rewritten in place, and
  keeps its spelling: an integer stays an integer.
- **A binding** (`let edge = 300 … draw_rect(edge, …)`) is rewritten at its
  *definition*, so every shape reading it moves. The status bar says so:
  `set edge to 325 (line 13) — shared, other shapes read it`.
- **A computed position** (`x0 + i * spacing`) has no number to edit at all — so
  the runtime **solves** it, inverting the arithmetic against the values the
  traced run actually saw, and moves one of the variables that feed it.

#### Telling it which variables are knobs

When a position is computed, more than one variable could be moved to satisfy the
goal, and only the author knows which one *means* something. Say it in the source
with `config let`:

```petal
config let spacing = 78
let x0 = 46
for i in range(0, 4) do
  draw_rect(x0 + i * spacing, 96, 44, 44, 70, 165, 150)
end
```

Declaring any `config let` makes config bindings the preferred edit targets and
pins every other named binding. Dragging a card here re-spaces the row (`spacing`
moves) instead of sliding it (`x0` stays put), with no dialog and no guessing.
Call-site literals stay directly editable either way.

#### When it declines

A drag that cannot be answered honestly says so in the status bar rather than
rewriting something that happens to be nearby:

- **Nothing to move** — every candidate is pinned, or the argument flows through
  a call and there is no literal behind it.
- **One of N shapes drawn by this call, and its position is computed** — solving
  inverts against the last value each term took, which is the *last* iteration's.
  So for a looping call only the last shape it drew has current numbers; drag
  that one (or edit the loop by hand). Non-computed arguments have no such limit:
  a loop drawing at a shared `let` is draggable from any of its shapes.

#### What can be grabbed

The position arguments of `draw_rect`, `draw_rect_rounded`, `draw_rect_outline`,
`draw_circle`, `draw_text`, `draw_image`, and `draw_line` (which moves both
endpoints in one gesture — a batch of four goals, resolved together so they can't
contradict each other). The `Rect`-taking overloads (`draw_rect(rect, color)`)
and polygons are not draggable: one argument carries both axes, so there is no
pair of numbers to move.

#### Driving it headlessly

The gesture is three debug-server calls, which is how its tests are written:

```bash
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"down","x":735,"y":314,"mods":["cmd"]}'
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"move","x":775,"y":339}'
curl -s -X POST 127.0.0.1:$PORT/mouse -d '{"op":"up"}'
curl -s 127.0.0.1:$PORT/buffer/0 | sed -n 24p     # the rewritten line
curl -s 127.0.0.1:$PORT/state | jq .status_note   # what the drag reported
```

#### The argument detail behind it

The same `trace` on `/state` that drives the highlight is what the drag acts on.
For each argument of a traced call, `garden_script::DrawTrace` reports where it is
written, what literal it resolves to, and how safely it can be rewritten:
`literal` (the number is in the call, nothing else reads it), `binding` (the
reported `editable_span` is at the *definition*, which may feed other shapes), or
`computed` (no single number — solved, or refused). Literals also record how they
were spelled (`is_int`), so a rewrite puts back `12` rather than churning it into
`12.0`, and a negated literal's span covers its `-`.

## Troubleshooting

- **A shape doesn't highlight anything.** Only a panel paired with a *live editor
  pane on the same file* traces — the same pairing rule as the live binding. A
  panel opened on its own records nothing.
- **Highlighting stopped, and there's a red banner.** Expected: while the buffer
  doesn't compile, the spans of the frame on screen no longer describe the text
  you're looking at, so tracing goes quiet rather than banding the wrong line.
  Fix the error and it comes straight back.
- **Cmd-click did nothing.** It only jumps from a *shape* — a press on bare
  canvas falls through to the ordinary click. It also needs a paired editor pane,
  and won't jump while the buffer is broken, for the reason above.
- **Cmd-drag moved nothing, and the status bar said why.** That is the feature
  working: see [When it declines](#when-it-declines). The two common cases are a
  position built from pinned variables, and an earlier shape of a looping call
  whose position is computed.
- **Cmd-drag moved something I did not grab.** The position was a binding other
  shapes read; the status bar flags that edit as `shared`. Undo (one step for the
  whole drag), and make the variable a `config let` knob — or inline the number —
  if you want the shapes to move independently.
- **The canvas is blank / shows `could not load panel`.** The file must be a valid
  Petal panel script. A brand-new file is seeded for you; if you pointed at an
  existing non-panel file, its first compile error shows in the pane.
- **The canvas stopped animating.** A panel sleeps ~10s after its last activity
  to avoid burning CPU; any edit or input wakes it. A purely static sketch (no
  `dt()`/`frame_count()`) simply has nothing to re-animate.
- **My edit didn't show up.** Only the pane that shares the file is driven. If you
  opened a *different* file in the editor, or the panel names a different path,
  they aren't paired — check both address the same file.
- **`--term` shows no graphics.** Expected: the canvas is a GPU panel. Use the
  default windowed mode.

## See also

- [`petal-graphical-panels.md`](petal-graphical-panels.md) — the panel draw/input API you write.
- [`architecture.md`](architecture.md) — the live editor↔panel binding, `PanelHost::reload_source`.
- [`debug-server.md`](debug-server.md) — driving and screenshotting the mode from automation.
- The [README](../README.md) — the editor/vim reference and the full run-mode table.
