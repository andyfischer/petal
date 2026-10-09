# Panel apps — authoring guide

Every app under `examples/games/`, `examples/productivity/`, and
`examples/dashboards/` is a pure-Petal Garden panel app: a `.ptl` script drawn
by Garden's panel runtime, driven and inspected through Garden's headless debug
server. No Rust, no TypeScript.

This guide covers the workflow and the traps specific to these apps. The
reference material lives elsewhere; link to it rather than guessing:

- Language: [docs/language-guide.md](../docs/language-guide.md) and
  [docs/writing-petal-guide.md](../docs/writing-petal-guide.md).
- Builtins: [docs/Builtins.md](../docs/Builtins.md).
- The `ui` prelude (widgets, theme, layout, text and color helpers):
  [core-libs/petal-ui/docs/components.md](../core-libs/petal-ui/docs/components.md), with
  [core-libs/petal-ui/prelude/ui.ptl](../core-libs/petal-ui/prelude/ui.ptl) as the source of truth.
- The `bloom` component library (buttons, menus, controls and overlays that
  animate by default, in pure Petal):
  [core-runtime/bloom/docs/components.md](../core-runtime/bloom/docs/components.md).
  Garden registers its modules, so a panel app can `import bloom` with no
  setup; outside Garden, add `-I core-runtime`.
- The panel host (draw surface, input, fonts, sleep/wake, persistence):
  [garden/docs/petal-graphical-panels.md](../garden/docs/petal-graphical-panels.md).
- The debug server (every endpoint):
  [garden/docs/debug-server.md](../garden/docs/debug-server.md).

## Layout on disk

```
examples/<category>/<slug>/
  app.ptl        the app itself (a Garden panel script)
  layout.ptl     the launcher: layout(panel("app.ptl")), under a
                 `// headless-size: WxH` comment
  README.md      what it is, what it demonstrates, how to run it, controls
```

Multi-file apps may add modules next to `app.ptl` and import them.

`tools/run-example.ts <slug>` starts Garden on an example's `layout.ptl`;
extra arguments are passed through to Garden. `<slug>` may also be
`<category>/<slug>` or a path to the directory, and may be left out when run
from inside it; `--list` shows every example. It finds the Garden binary at
`garden/target/debug/garden`, or wherever `GARDEN_BIN` points, and sets
`GARDEN_HEADLESS_SIZE` from the `// headless-size: WxH` comment in
`layout.ptl` (1280x850 without one) unless it is already set. It does not
rebuild, so build Garden first (`cd garden && cargo build`). It refuses to start
a binary that is behind the checkout: it prints a `STALE GARDEN BINARY` banner
and exits 3 without launching, so a headless launch into a log file leaves no
debug port to find. Rebuild, or set `GARDEN_ALLOW_STALE=1` to launch it anyway
(the banner still prints, and `/state` → `identity.freshness.stale` is true).
The check counts committed changes under `garden/`, `core-libs/petal-ui/` and `core/`,
not uncommitted edits.

`layout(...)` is required in `layout.ptl`. A bare `panel("...")` at top level
silently leaves you with an empty editor pane.

## Running it

```bash
(nohup tools/run-example.ts <slug> --headless --debug-port 0 > log.txt 2>&1 < /dev/null &)
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
GPID=$(lsof -ti tcp:$PORT -sTCP:LISTEN)    # the process holding your debug port
```

Use `--headless --debug-port 0` while developing. A windowed launch steals
focus, and a fixed port collides with any other Garden already running.

[`tools/panel-test.sh`](../tools/panel-test.sh) wraps this launch and the
`curl` calls in the rest of this guide, so a session does not write its own:

```bash
source tools/panel-test.sh    # or one command per call: tools/panel-test.sh click 80 30
panel_start <slug>            # the launch above; waits for the port, prints it
obs                           # obs_* values as JSON (obs score,lives / obs ui_ / obs all)
err                           # status_error, exit 1 if there is one
shot [file]                   # PNG of the pane (shot file window: the whole window)
click 80 30                   # pane-local; also rclick, dclick, move, drag, scroll
key s cmd; keydown left; keyup left; typetext "hello"
tick 60 0.016                 # 60 frames of exactly 16 ms
panel_reset 42                # restart on seed 42
find_text Save                # pane-local rect and center of matching text runs
panel_stop                    # kills the Garden on this port, and only that one
```

It reads the port from `$PORT`, or from `./log.txt` (`$PANEL_LOG`), so it works
when every command runs in a fresh shell. `$PANE` picks the pane (default 0).
The file's header is the full reference.

Launch it with `nohup … < /dev/null` inside a subshell, as above. A plain
`… &` dies with the shell that started it — in an agent harness every tool
call is its own shell, so the second `curl` finds the process gone and the log
ending in `headless launcher exited; shutting down`. macOS has no `setsid`;
the subshell form is what survives.

If the app persists through `panel_store_*`, launch with
`GARDEN_PANEL_STORE_DIR=<scratch dir>` set, and say so in the README.
Otherwise every test run starts from whatever the last run saved, and
`POST /panel/reset` reloads the saved document rather than the seed data.

`GARDEN_HEADLESS_SIZE=WxH` sets the virtual viewport (default 1280x850). Pick a
size that suits the app and record it in `layout.ptl`'s `headless-size`
comment and in the README.

**Your pane is smaller than the viewport.** The tab strip, status bar and side
gutters come out of it — roughly `W-12` by `H-72` for a single pane, but the
numbers are not a contract. Inside the script use `screen_width()` /
`screen_height()`, which report the pane. Outside it, read `panes[0].rect`
from `/state`.

To stop, `kill $GPID`. Find the pid by the debug port, as above, rather than
by `pgrep -f`: `run-example.ts` execs Garden with a relative `layout.ptl`, so the
command line carries no path that tells your app from another one. Never
`pkill -f garden` or `killall`: other Garden processes (someone else's
session, an agent's harness) will die with yours.

## Inspecting it

Always curl `127.0.0.1`, never `localhost`. `localhost` can resolve to `::1`
and land on a different Garden; the symptom is an app that looks nothing like
yours.

| What | How |
|---|---|
| Pixels | `curl -s 127.0.0.1:$PORT/screenshot -o shot.png` (PNG; frame number in the `X-Garden-Frame` header) |
| Draw calls | `curl -s 127.0.0.1:$PORT/scene` — every quad and text run with rect and color. Best for asserting layout numerically |
| Logical state | `curl -s 127.0.0.1:$PORT/state \| jq '.panes[0].panel'` — `awake`, `frame`, `values` (every binding the last frame made), `input` |
| Filtered state | `curl -s "127.0.0.1:$PORT/state?values=sel,scroll"` — narrow `values` to the names you assert on (`?values_prefix=obs_`, `?values=none`) |
| Script `print` | the `script.output` array in `/state`. The default read drains it; `?output=all` does not |
| Errors | `status_error` in `/state` covers a load failure, a hot reload that will not compile, and a frame that raised. `panes[].panel.error` has the full message |

`panel.values` is the feature to build tests on. A plain `let sel = 2` in your
script is readable as `sel`; a binding inside `fn list_row` keys as
`list_row.y`. Assert against those names rather than against pixels.

When a frame raises, `panel.values` is the last good frame — `values_frame`
and `values_stale` say so, and `values_partial` carries how far the failing
frame got. A key missing because the frame blew up is not the same as a branch
that never ran.

`/screenshot` and `/scene` settle panel frames before answering, so input
followed by a capture needs no sleep. The settle does not advance time: a
transition the script eases under `request_frame()` is captured at its first
frame. Add `?settle=idle` to run it to rest first (`/screenshot?settle=idle`),
or `POST /tick` it yourself. `panel.frame_stats.last_ms` in `/state` is what
the last frame cost.

### Driving it

`petal check --strict --host garden app.ptl` resolves the `ui` prelude without
Garden, and the packages Garden registers for panels (`bloom`,
`text_layout`), with no `-I`. It
catches arity and type slips on known functions, a misspelled global
(`totally_bogus_fn(1)`), a native handed the wrong type (`sqrt("x")`), a call
whose argument count selects a prelude overload that cannot take its arguments,
captured-`state` reads and hoisting order. `--host garden` adds Garden's own
natives (`palette`, `emit`, `query`, …) to the petal-ui set the default knows.

```bash
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"left"}'
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"s","mods":["cmd"]}'
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"shift","op":"down"}'   # held until "op":"up"
curl -sX POST 127.0.0.1:$PORT/text  -d '{"text":"hello"}'
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":80,"y":30,"pane":0}'   # PANE-LOCAL coords
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":86,"y":68}'            # WINDOW coords
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":86,"y":68,"button":1}' # right click
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":86,"y":68,"clicks":2}' # double click
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"drag","x":86,"y":68,"to":{"x":306,"y":128}}'
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"scroll","x":86,"y":68,"lines":3}'
```

Named keys: `enter`, `tab`, `space`, `backspace`, `delete`, `escape`,
`left`/`right`/`up`/`down`, `home`, `end`, `pageup`, `pagedown`. Mods: `cmd`,
`ctrl`, `shift`, `alt`. All four reach the script, on keys and on the mouse
alike. The full option list is in
[debug-server.md](../garden/docs/debug-server.md#input-injection).

#### Mouse coordinates: say which pane

The script sees pane-local coordinates: `mouse_x()`/`mouse_y()`, and therefore
`point_in`, `hovered` and `clicked`, are relative to the pane's top-left,
exactly like every coordinate you pass to `draw_*`. `POST /mouse` takes window
coordinates by default, which is the most expensive mistake in this harness:
clicks land a few dozen pixels off and hit the wrong row.

Put `"pane": <n>` in the body and `x`/`y` (and a drag's `to`) are relative to
that pane's origin instead. Garden adds the offset, so there is nothing to
read or hardcode:

```bash
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":80,"y":30,"pane":0}'
click 80 30      # the same, with tools/panel-test.sh: the pixel the script calls (80, 30)
```

These are the coordinates `GET /scene?pane=0` reports, so a `center` from
`/scene?pane=0&find=text:Save` can be posted back as it is. Without `"pane"`
the offset is `panes[0].rect` in `/state`. (`"pane"` is feature
`debug.mouse-pane` in `GET /version`; the helpers fall back to adding the rect
on a Garden that lacks it.)

Before trusting a click script, post a `move` to a known spot and read
`panel.input.mouse` back from `/state`. That is the coordinate the script saw.

A `click`, `down` or `drag` presses at once, with no hover frame in between.
If your hit-testing relies on hover from the previous frame, add
`"hover_first": true` to the op: Garden moves the pointer and runs a frame
there before pressing.

**One-frame edges** (`key_pressed`, `*_released`, `click_count`, `scroll`,
`text_input`) are cleared by the next idle tick. A test that must observe an
edge across a later `GET /state` has to count it into a `state` var, which is
then visible under its own name in `panel.values`.

#### Stepping frames, reseeding, and resetting state

```bash
curl -sX POST 127.0.0.1:$PORT/tick        -d '{"n":60,"dt":0.016}'   # 60 frames of exactly 16ms
curl -sX POST 127.0.0.1:$PORT/seed        -d '{"seed":42}'           # fix random() from the next frame on
curl -sX POST 127.0.0.1:$PORT/panel/reset -d '{}'                    # restart panels, drop `state`
curl -sX POST 127.0.0.1:$PORT/panel/reset -d '{"seed":42}'           # both at once: restart on seed 42
```

`POST /tick` runs frames on demand with the `dt` you name, ignores the sleep
window, fabricates no input, and puts the panel's `time()` and `dt()` on a
virtual clock that advances by exactly `dt` per ticked frame and by nothing
otherwise. The frames Garden runs between your ticks (one per injected key or
click, the idle poll, the settle before a capture) see `dt() == 0` and an
unmoved `time()`, so a simulation stepped by `dt()` advances only when you
tick it, however much input you send in between and however long the test
pauses. The switch happens at a panel's first tick and lasts until the process
exits, across resets: a reset panel restarts at `time() == 0`. So start a test
with one warm-up `tick`, then the seeded reset, then inputs and ticks, and no
wall-clock frame is part of the run. An animation or game test is a
deterministic frame count, not a stream of phantom keypresses. Reset with a
seed (`{"seed":42}`), then tick, and a screenshot of a moving UI is
byte-identical each run. Put the seed in the reset body rather than sending
`/seed` after it: the restarted panel's first frame runs as soon as the reset
lands, so content generated on frame 1 would miss a later seed. See [Stepping frames and resetting panels](../garden/docs/debug-server.md#stepping-frames-and-resetting-panels).

## The headless frame contract

A headless panel is not a 60fps loop. Garden renders only dirty frames, and
headless has nothing making them dirty. You get roughly one frame per injected
event, one per ~200ms idle poll while awake, and a settle before every capture.
`dt()` is wall-clock — 0.1–0.2s on an idle poll, and the frame after a pause
carries the whole pause — until a test takes the clock over with `POST /tick`,
after which `dt()` is exactly what each tick names and 0 on every other frame
(see [Stepping frames](#stepping-frames-reseeding-and-resetting-state)). After
10s without activity the panel sleeps and runs
no frames at all until the next input. Details:
[The headless contract](../garden/docs/petal-graphical-panels.md#the-headless-contract).

For your app this means:

- Drive animation and simulation off `dt()`, never off a fixed per-frame
  delta. It is the right clock in both worlds: a measurement when a person is
  using the app, and the exact step a test asked for once it ticks. No
  `time()`-delta bookkeeping is needed to make a `dt()`-stepped app testable.
- Physics needs its own clamp and sub-stepping: `let step = min(dt(), 0.05)`,
  then integrate in fixed slices. A raw 0.2s step tunnels a ball through a
  paddle.
- Keep any `time() >= next` poll interval well under 10s, or the poll dies
  when the panel sleeps.
- A panel that is mid-animation can call `request_frame()` to stay awake; a
  harness can launch with `--panel-wake` (never sleep) or `--panel-wake 60`.
- Note the 10s sleep in your README so a reviewer does not read a stopped
  simulation as a hang.

## `state` survives hot reload

Editing the script hot-reloads it, but Petal `state` is carried over. So if
you change a seed-data generator, or a function whose result is cached in
`state`, and save, nothing changes on screen: the old value is restored, not
recomputed. It looks like the edit did not take.

Do not restart the process. `POST /panel/reset` rebuilds the panel from source
and drops `state`. It is also the way back from a frame that raised: the error
card stays up until a reset, even after the file is fixed and saved.

The same rule is why `state` is right for what genuinely persists (selection,
scroll offset, the document) and wrong for anything you are still iterating on.

## Drawing and input

A panel is a normal Petal program: every builtin in
[docs/Builtins.md](../docs/Builtins.md) is callable, plus the panel draw and
input natives and the `ui` prelude. Nothing is subsetted. Coordinates are
pane-local logical pixels with `(0,0)` at the top-left; colors are integer RGB
`0..255` with an optional `a`.

The full draw vocabulary — rects, rounded rects and outlines, lines and
polylines, circles, ellipses, arcs, triangles, convex and concave polygons,
text, images, clipping, gradients, shadows and offscreen layers — is tabulated
in [Supported draw surface](../garden/docs/petal-graphical-panels.md#supported-draw-surface).
`garden/examples/panels/shapes.ptl` draws every primitive on one screen.

Reach for the right primitive. A translucent brush stroke is one
`draw_polyline`, not N `draw_line`s (overlapping pieces double-blend and the
stroke comes out mottled); a closed outline is `draw_polygon_outline`, the
same call with the first point repeated for you. A star is `fill_polygon`, not ten `fill_triangle`s
(`fill_poly` fans from vertex 0 and spills across reflex corners). A donut
segment is one `fill_arc`. A rounded border is `draw_rect_rounded_outline`, not
a rounded fill with a smaller one on top.

Draw order is call order, across every kind: a `draw_rect` after a `draw_text`
covers that text. Overlays — menus, modals, tooltips — just work if you draw
them last.

Input reads: `dt()`, `frame_count()`, `time()`, `screen_width()`,
`screen_height()`, `mouse_x()`, `mouse_y()`, `mouse_down(btn)`,
`mouse_pressed(btn)`, `mouse_released(btn)`, `click_count()` (2 on a double
click, 3 on a triple), `drag_active()`, `key_down(name)`, `key_pressed(name)`,
`key_released(name)`, `mod_shift()`, `mod_ctrl()`, `mod_alt()`, `mod_cmd()`,
`scroll_y()`, `scroll_x()`, `text_input()`, `text_width(s, style)`,
`text_advance(s, style)`,
`panel_theme()`.

Garden owns the Cmd/Ctrl chords. If your app needs one — a spreadsheet's
Cmd+C, an editor's Cmd+Z — ask for it back with `claim_key("z", "cmd")`, stated
unconditionally near the top of every frame. A claimed chord arrives as
`key_pressed("z")` with `mod_cmd()` true, and produces no `text_input()`.

Every draw primitive has a **record form** next to its packed-int form —
`draw_rect(rect, color[, a])`, `draw_polyline(points, color[, a[, width]])`,
`fill_arc(center, r_in, r_out, a0, a1, color[, a])` and so on, where `color`
is a `{r, g, b}` record or a `#rrggbb` literal. Each also takes flat
coordinates with a colour record — `draw_line(x1, y1, x2, y2, color[, a[,
width]])`, `fill_triangle(x1, y1, x2, y2, x3, y3, color[, a])`,
`draw_circle_outline(cx, cy, r, color)` — with the same optional arguments. Both forms accept floats
(they are truncated to whole pixels), so world-space coordinates after a zoom
multiply need no `int(...)`.

Persistence across a restart is `panel_store_get(key)` /
`panel_store_set(key, string)`: a string-to-string map scoped to your script's
path, capped at 256 KiB per value. There is no file API; pair it with
`json_stringify`/`json_parse`.

Builtins authors keep hand-rolling that already exist: `random(min,max)`,
`random_int(lo,hi)`, `choose(list)` for seed data; `clamp`/`min`/`max`/
`round(x, places)`; `parse_int`/`parse_float` (nil on bad input);
`chars`/`index_of` and `s[i]` for scanning text (`len`, `slice` and `s[i]`
count characters, so non-ASCII text needs nothing special);
`json_parse`/`json_stringify`;
`sort_by`, `map`, `filter`.

### Text and fonts

`draw_text` and `text_width` both take a style record — `{size, color, font,
weight, italic, spacing}` — or a `font(name, size)` object. Build the style
once and pass the same value to both, so what you measure is what you draw.
`font` is any family installed on the machine, or the embedded roles `mono`
(JetBrains Mono) and `ui` (Inter). `weight` is real on `ui` and on system
families; on `mono` only Regular is embedded, so bold there is synthetic.
`italic` is upright on both embedded faces (no italic cut is embedded), so an
italic run needs a system family or a colour cue instead. A character the
face lacks (`⌘`, `⇧`, `—`, `·`, CJK) is drawn from an installed system face
that has it, so it shows up, but in that face's design, not Inter's or
JetBrains Mono's. Text cannot be rotated;
`draw_text_along` and `draw_axis_labels` are the workarounds.

`text_width` returns a rounded whole pixel. For monospace column math use
`text_advance(s, style)`, the same measurement as a float: one glyph of a
13 px face is 7.8, which `text_width` rounds to 8 and which drifts a full
column by column 30. `let CW = text_advance("m", style)`. See
[Text size and measurement](../garden/docs/petal-graphical-panels.md#text-size-and-measurement)
and [docs/text-and-fonts.md](../docs/text-and-fonts.md).

### Alpha

Alpha blends in sRGB, the way CSS and design tools do: `a: 128` over white is
`#808080`. Overlapping translucent fills are still not idempotent — two 50%
fills read 75% — so for a tint that must survive being drawn twice, compute an
opaque color with `mix`/`lerp_color`, or use the prelude's `over`/`tint`/
`hairline` helpers. See
[Compositing flat tints](../core-libs/petal-ui/docs/components.md#compositing-flat-tints).

### The `ui` prelude

`core-libs/petal-ui/prelude/ui.ptl` is an implicit import; call its functions bare. The
catalogue is in [components.md](../core-libs/petal-ui/docs/components.md): `rect`
(the built-in `Rect` under the prelude's name), `point_in`, `hovered`,
`clicked`, record overloads of every `draw_*` (each callable by name:
`draw_rect_outline(rect: r, c: red, width: 2)`), alignment and ellipsizing
helpers, `button`, lists and scrolling, the focus registry and text fields,
context menus, drag and drop, tabs, modals, tables, splitters, RectCut layout,
and theming via `ui_theme()` / `theme_set({...})`.

#### Text entry: use `text_field`, not your own caret

`text_field(fc, id, r, buf)` is a one-line field with a caret, a selection
(shift+arrows, shift+click, drag, double click for a word, Cmd+A), the
clipboard (Cmd+C/X/V) and undo (Cmd+Z, Cmd+Shift+Z). Do not write the editing
loop again. An app with its own look calls `text_field_update(fc, id, r, buf,
style)`, which is all of the behaviour and none of the pixels, and paints
from what it returns: `caret`, `sel_start`, `sel_end`, `scroll`. An app with
its own focus variable passes `{id: focus}` as `fc`.
`examples/productivity/email-client/app.ptl` does both for its search, To and
Subject fields.

```petal
state fc = focus_state()
state name = ""
claim_key("a", "cmd")                       // Garden keeps Cmd chords unless claimed
for k in ["c", "x", "v", "z"] do claim_key(k, "cmd") end
claim_key("z", "cmd+shift")

let res = text_field(fc, "name", rect(20, 20, 240, 28), name)
fc = res.focus
name = res.text
if res.submitted then … end
```

Every offset the field reports is a character offset, the unit `char_slice`
and `char_len` use. For anything wrapped or highlighted, use the range
helpers instead of measuring a growing prefix per character:
`text_wrap_rows(s, style, w)` returns `{text, start, end}` per row,
`text_row_of(rows, at)` finds a caret's row, and `text_range_rects(s, style,
a, b)` returns the rectangles covering a character range. `clipboard_get()` /
`clipboard_set(text)` are there for an app's own copy and paste. The
catalogue entries are in
[components.md](../core-libs/petal-ui/docs/components.md#text-field-caret-selection-clipboard-undo).

#### `context_menu` is a draw call — make it your last one

The two menu calls sit at opposite ends of the frame:

- `menu_blocking(m)` is an input guard. It belongs at the top, before the
  panel's own click handling, so the widgets underneath stand down while a
  menu is open.
- `context_menu(m, items)` paints the menu. It must come after the very last
  drawing call the panel makes — the bottom of the script, not the bottom of
  the input section. Calling it early paints the menu and then paints your
  background over it, and the menu vanishes with no error.

```petal
state menu = menu_state()

// top — input:
if !menu_blocking(menu) && mouse_pressed(0) && point_in(mouse_x(), mouse_y(), row) then … end
menu = menu_open_on_right_click(menu, row, i)

// …all of the panel's drawing…

// bottom — the last draw call in the frame:
let picked = context_menu(menu, [menu_item("Only this"), menu_sep(), menu_item("All")])
menu = picked.menu
if picked.index >= 0 then … end
```

`picked.index` counts every entry, separators included.

Worked examples to read before starting: `garden/examples/panels/sketch.ptl`
(draw surface), `garden/gpp-apps/garden-diff/src/garden_diff.ptl` and
`garden/gpp-apps/git-viewers/src/git_panel.ptl` (real interactive panels:
focus registry, lists, scrolling, menus).

## Quality bar

These are showpieces, not smoke tests. Aim for:

- A real visual design: a considered palette, a consistent spacing scale, a
  typographic hierarchy built from size, color and spacing, generous padding,
  and alignment that holds at the declared viewport size.
- Actual interactivity: hover states, selection, keyboard and mouse where both
  make sense, transitions where they help legibility.
- Enough content to look alive (plausible seeded data, not `foo`/`bar`).
- Idiomatic Petal: `state` for what persists across frames, `let` for
  dataflow, `var`/`set` only where mutation is genuinely needed, functions to
  factor drawing, classes to name record shapes. Every function reads a
  module-level `var` with `get` (`get level`), helpers called from the top
  level included; only module scope reads it bare. `petal check` flags the
  slip (see the [writing guide](../docs/writing-petal-guide.md)).
- No script error at any point in the interaction you exercise. `petal check
  --strict --host garden app.ptl` (see [Driving it](#driving-it)) confirms the
  script compiles and has no warnings before you launch anything;
  `status_error` in `/state` reports anything that breaks after.

## Rules of the road

- Do not add `.ptl` files to `examples/console/`. That directory is a
  golden-tested corpus; a new file there fails the suite unless a golden is
  generated for it. Subdirectories inside your own app directory are fine.
- Keep each app self-contained under `examples/<category>/<slug>/`.
- If you hit a language or host limitation, work around it in Petal and note
  it in the app's README rather than patching the host as a side effect of
  the app.
