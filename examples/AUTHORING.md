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
  [petal-ui/docs/components.md](../petal-ui/docs/components.md), with
  [petal-ui/prelude/ui.ptl](../petal-ui/prelude/ui.ptl) as the source of truth.
- The `bloom` component library (buttons, menus, controls and overlays that
  animate by default, in pure Petal):
  [petal-libs/bloom/docs/components.md](../petal-libs/bloom/docs/components.md).
  Garden registers its modules, so a panel app can `import bloom` with no
  setup; outside Garden, add `-I petal-libs/bloom/src`.
- The panel host (draw surface, input, fonts, sleep/wake, persistence):
  [garden/docs/petal-graphical-panels.md](../garden/docs/petal-graphical-panels.md).
- The debug server (every endpoint):
  [garden/docs/debug-server.md](../garden/docs/debug-server.md).

## Layout on disk

```
examples/<category>/<slug>/
  app.ptl        the app itself (a Garden panel script)
  layout.ptl     the launcher: layout(panel("app.ptl"))
  launch.sh      starts Garden on layout.ptl; extra args are passed through
  README.md      what it is, what it demonstrates, how to run it, controls
```

Multi-file apps may add modules next to `app.ptl` and import them. Copy an
existing app's `launch.sh` (for example `games/pong/launch.sh`); it finds the
Garden binary at `garden/target/debug/garden`, or wherever `GARDEN_BIN` points,
and sets a default `GARDEN_HEADLESS_SIZE`.

`layout(...)` is required in `layout.ptl`. A bare `panel("...")` at top level
silently leaves you with an empty editor pane.

## Running it

```bash
cd examples/<category>/<slug>
./launch.sh --headless --debug-port 0 > log.txt 2>&1 &
GPID=$!
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
```

Use `--headless --debug-port 0` while developing. A windowed launch steals
focus, and a fixed port collides with any other Garden already running.

`GARDEN_HEADLESS_SIZE=WxH` sets the virtual viewport (default 1280x850). Pick a
size that suits the app and record it in the README.

**Your pane is smaller than the viewport.** The tab strip, status bar and side
gutters come out of it — roughly `W-12` by `H-72` for a single pane, but the
numbers are not a contract. Inside the script use `screen_width()` /
`screen_height()`, which report the pane. Outside it, read `panes[0].rect`
from `/state`.

To stop, `kill $GPID`. Never `pkill -f garden` or `killall`: other Garden
processes (someone else's session, an agent's harness) will die with yours.

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
followed by a capture needs no sleep.

### Driving it

```bash
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"left"}'
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"s","mods":["cmd"]}'
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"shift","op":"down"}'   # held until "op":"up"
curl -sX POST 127.0.0.1:$PORT/text  -d '{"text":"hello"}'
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":86,"y":68}'            # WINDOW coords
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":86,"y":68,"button":1}' # right click
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"drag","x":86,"y":68,"to":{"x":306,"y":128}}'
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"scroll","x":86,"y":68,"lines":3}'
```

Named keys: `enter`, `tab`, `space`, `backspace`, `delete`, `escape`,
`left`/`right`/`up`/`down`, `home`, `end`, `pageup`, `pagedown`. Mods: `cmd`,
`ctrl`, `shift`, `alt`. All four reach the script, on keys and on the mouse
alike. The full option list is in
[debug-server.md](../garden/docs/debug-server.md#input-injection).

#### Mouse coordinates: window in, pane-local out

`POST /mouse` takes window coordinates. The script sees pane-local ones. This
is the most expensive mistake in this harness: clicks land a few dozen pixels
off and hit the wrong row. `mouse_x()`/`mouse_y()`, and therefore `point_in`,
`hovered` and `clicked`, are relative to the pane's top-left, exactly like every
coordinate you pass to `draw_*`. The `x`/`y` you POST are not.

The offset is the pane rect's origin. Read it, do not hardcode it:

```bash
read -r OX OY <<<"$(curl -s 127.0.0.1:$PORT/state | jq -r '.panes[0].rect | "\(.x) \(.y)"')"
click() {  # click(pane_x, pane_y)
  curl -sX POST 127.0.0.1:$PORT/mouse \
    -d "{\"op\":\"click\",\"x\":$(($1 + ${OX%.*})),\"y\":$(($2 + ${OY%.*}))}"
}
click 80 30      # the pixel the script calls (80, 30)
```

Before trusting a click script, post a `move` to a known spot and read
`panel.input.mouse` back from `/state`. That is the coordinate the script saw.

**One-frame edges** (`key_pressed`, `*_released`, `click_count`, `scroll`,
`text_input`) are cleared by the next idle tick. A test that must observe an
edge across a later `GET /state` has to count it into a `state` var, which is
then visible under its own name in `panel.values`.

#### Stepping frames, reseeding, and resetting state

```bash
curl -sX POST 127.0.0.1:$PORT/tick        -d '{"n":60,"dt":0.016}'   # 60 frames of exactly 16ms
curl -sX POST 127.0.0.1:$PORT/seed        -d '{"seed":42}'           # fix random()
curl -sX POST 127.0.0.1:$PORT/panel/reset -d '{}'                    # restart panels, drop `state`
```

`POST /tick` runs frames on demand with the `dt` you name, ignores the sleep
window, fabricates no input, and puts the panel's `time()` on a virtual clock
that advances by exactly `dt` per frame. An animation or game test is a
deterministic frame count, not a stream of phantom keypresses. Reset first,
then seed, then tick, and a screenshot of a moving UI is byte-identical each
run. See [Stepping frames and resetting panels](../garden/docs/debug-server.md#stepping-frames-and-resetting-panels).

## The headless frame contract

A headless panel is not a 60fps loop. Garden renders only dirty frames, and
headless has nothing making them dirty. You get roughly one frame per injected
event, one per ~200ms idle poll while awake, and a settle before every capture.
`dt()` is wall-clock — 0.1–0.2s on an idle poll, and the frame after a pause
carries the whole pause. After 10s without activity the panel sleeps and runs
no frames at all until the next input. Details:
[The headless contract](../garden/docs/petal-graphical-panels.md#the-headless-contract).

For your app this means:

- Drive animation off `dt()`, never off a fixed per-frame delta.
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
and drops `state`.

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
stroke comes out mottled). A star is `fill_polygon`, not ten `fill_triangle`s
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
`panel_theme()`.

Garden owns the Cmd/Ctrl chords. If your app needs one — a spreadsheet's
Cmd+C, an editor's Cmd+Z — ask for it back with `claim_key("z", "cmd")`, stated
unconditionally near the top of every frame. A claimed chord arrives as
`key_pressed("z")` with `mod_cmd()` true, and produces no `text_input()`.

Persistence across a restart is `panel_store_get(key)` /
`panel_store_set(key, string)`: a string-to-string map scoped to your script's
path, capped at 256 KiB per value. There is no file API; pair it with
`json_stringify`/`json_parse`.

Builtins authors keep hand-rolling that already exist: `random(min,max)`,
`random_int(lo,hi)`, `choose(list)` for seed data; `clamp`/`min`/`max`/
`round(x, places)`; `parse_int`/`parse_float` (nil on bad input);
`chars`/`char_len`/`char_at`/`char_slice`/`index_of` for anything non-ASCII,
since `len`/`slice` are byte-indexed; `json_parse`/`json_stringify`;
`sort_by`, `map`, `filter`.

### Text and fonts

`draw_text` and `text_width` both take a style record — `{size, color, font,
weight, italic, spacing}` — or a `font(name, size)` object. Build the style
once and pass the same value to both, so what you measure is what you draw.
`font` is any family installed on the machine, or the embedded roles `mono`
(JetBrains Mono) and `ui` (Inter). `weight` is real on `ui` and on system
families; on `mono` only Regular is embedded, so bold there is synthetic. Text
cannot be rotated; `draw_text_along` and `draw_axis_labels` are the
workarounds. See
[Text size and measurement](../garden/docs/petal-graphical-panels.md#text-size-and-measurement)
and [docs/text-and-fonts.md](../docs/text-and-fonts.md).

### Alpha

Alpha blends in sRGB, the way CSS and design tools do: `a: 128` over white is
`#808080`. Overlapping translucent fills are still not idempotent — two 50%
fills read 75% — so for a tint that must survive being drawn twice, compute an
opaque color with `mix`/`lerp_color`, or use the prelude's `over`/`tint`/
`hairline` helpers. See
[Compositing flat tints](../petal-ui/docs/components.md#compositing-flat-tints).

### The `ui` prelude

`petal-ui/prelude/ui.ptl` is an implicit import; call its functions bare. The
catalogue is in [components.md](../petal-ui/docs/components.md): `rect`
(the built-in `Rect` under the prelude's name), `point_in`, `hovered`,
`clicked`, record overloads of every `draw_*`, alignment and ellipsizing
helpers, `button`, lists and scrolling, the focus registry and text fields,
context menus, drag and drop, tabs, modals, tables, splitters, RectCut layout,
and theming via `ui_theme()` / `theme_set({...})`.

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
  factor drawing, classes to name record shapes.
- No script error at any point in the interaction you exercise. `petal check
  app.ptl` confirms the script compiles before you launch anything;
  `status_error` in `/state` reports anything that breaks after.

## Rules of the road

- Do not add `.ptl` files to `examples/console/`. That directory is a
  golden-tested corpus; a new file there fails the suite unless a golden is
  generated for it. Subdirectories inside your own app directory are fine.
- Keep each app self-contained under `examples/<category>/<slug>/`.
- If you hit a language or host limitation, work around it in Petal and note
  it in the app's README rather than patching the host as a side effect of
  the app.
