# Panel apps — authoring guide

Every app under `examples/games/`, `examples/productivity/`, and
`examples/dashboards/` is a **pure-Petal Garden panel app**: a `.ptl` script
drawn by Garden's panel runtime, driven and inspected through Garden's headless
debug server. No Rust, no TypeScript.

Read this whole file before writing any code.

## Layout on disk

```
examples/<category>/<slug>/
  app.ptl        the app itself (a Garden panel script)
  layout.ptl     the launcher: layout(panel("<abs-or-cwd-relative>/app.ptl"))
  README.md      what it is, what it demonstrates, how to run it, controls
```

Multi-file apps may add modules next to `app.ptl` and import them.

## Running it

`layout(...)` is **required** — a bare `panel("...")` at top level silently
leaves you with an empty editor pane.

```bash
cd examples/<category>/<slug>
echo 'layout(panel("app.ptl"))' > layout.ptl        # path relative to cwd
/Users/andy/petal/garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
GPID=$!
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
```

Always `--headless --debug-port 0`. A windowed launch steals the user's focus;
a fixed port collides with the other agents running right now.

`GARDEN_HEADLESS_SIZE=WxH` sets the virtual viewport (default 1280x850). Pick a
size that suits the app and record it in the README.

**Your panel is smaller than the viewport you asked for.** The window chrome —
tab strip on top, status bar at the bottom, a 6px gutter each side — comes out
of it. Measured on a default single-pane layout:

| `GARDEN_HEADLESS_SIZE` | `panes[0].rect` |
|---|---|
| `1440x900` | `x:6 y:38 w:1428 h:828.4` |
| `1280x850` (default) | `x:6 y:38 w:1268 h:778.4` |

So the pane is roughly **`W-12` by `H-71.6`**. Never lay out against the number
you passed to `GARDEN_HEADLESS_SIZE`: use `screen_width()`/`screen_height()`
inside the script (they report the *pane*), and read `panes[0].rect` from
`/state` when you need the numbers outside it. The chrome sizes are not a
contract — they change with the layout and with future chrome — so read them,
don't hardcode them.

### Shutting down

**Kill by PID only** — `kill $GPID`. Never `pkill -f garden` or `killall`:
other agents have their own Garden processes running and you will kill them.

## Inspecting it

**Always curl `127.0.0.1`, never `localhost`.** Other agents are running their
own Garden right now; `localhost` can resolve to `::1` and land you on someone
else's process, and the symptom is an app that looks nothing like yours. Every
example below uses the literal `127.0.0.1` on purpose.

| What | How |
|---|---|
| Pixels | `curl -s 127.0.0.1:$PORT/screenshot -o shot.png` (PNG; frame number in the `X-Garden-Frame` header) |
| Draw calls | `curl -s 127.0.0.1:$PORT/scene` — every quad/text run with rect + color. Best for asserting layout numerically. |
| Logical state | `curl -s 127.0.0.1:$PORT/state \| jq '.panes[0].panel'` — `awake`, `frame`, `values` (every binding the last frame made), `input` |
| Filtered state | `curl -s "127.0.0.1:$PORT/state?values=sel,scroll"` — narrow `values` to the names you assert on (`?values_prefix=obs_`, `?values=none`) |
| Script `print` | the `script.output` array in `/state` (**draining** — each read consumes it) |

`panel.values` is the killer feature: a plain `let sel = 2` in your script is
readable as `sel`, and a binding inside `fn list_row` keys as `list_row.y`.
Write your assertions against those names rather than against pixels.

`/screenshot` and `/scene` settle panel frames before answering, so
**input-then-capture needs no sleep**.

### Driving it

#### Mouse coordinates: window in, pane-local out

**`POST /mouse` takes WINDOW coordinates. The script sees PANE-LOCAL ones.**
This is the single most expensive mistake in this harness — four separate
authors lost time to clicks that landed a few dozen pixels off and hit the
wrong row. `mouse_x()`/`mouse_y()`, and therefore `point_in`, `hovered` and
`clicked`, are all relative to the pane's top-left, exactly like every
coordinate you pass to `draw_*`. The `x`/`y` you POST are not.

The offset is the pane rect's origin — about `6,38` in a default single-pane
layout, but it changes with the layout and the chrome, so **read it, don't
hardcode it**:

```bash
read -r OX OY <<<"$(curl -s 127.0.0.1:$PORT/state | jq -r '.panes[0].rect | "\(.x) \(.y)"')"
click() {  # click(pane_x, pane_y)
  curl -sX POST 127.0.0.1:$PORT/mouse \
    -d "{\"op\":\"click\",\"x\":$(($1 + ${OX%.*})),\"y\":$(($2 + ${OY%.*}))}"
}
click 80 30      # the pixel the script calls (80, 30)
```

Sanity check before you trust a click script: post a `move` to a spot you know,
then read `panel.input.mouse` back out of `/state` — that is the coordinate the
script actually saw.

```bash
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"left"}'
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"s","mods":["cmd"]}'
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"shift","op":"down"}'   # held across frames
curl -sX POST 127.0.0.1:$PORT/key   -d '{"key":"shift","op":"up"}'
curl -sX POST 127.0.0.1:$PORT/text  -d '{"text":"hello"}'
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":86,"y":68}'            # WINDOW coords
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"click","x":86,"y":68,"button":1}' # right
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"drag","x":86,"y":68,"to":{"x":306,"y":128}}'
curl -sX POST 127.0.0.1:$PORT/mouse -d '{"op":"scroll","x":86,"y":68,"lines":3}'
```

Named keys: `enter`, `tab`, `space`, `backspace`, `delete`, `escape`,
`left`/`right`/`up`/`down`, `home`, `end`, `pageup`, `pagedown`. Mods:
`cmd`, `ctrl`, `shift`, `alt` — all four reach the script, on keys and on the
mouse alike.

**One-frame edges** (`keys_pressed`, `*_released`, `click_count`, `scroll`,
`text`) are cleared by the next idle tick (~200ms headless). A script that must
observe an edge across a later `GET /state` has to *count it into a `state`
var*, which is then observable under its own name in `panel.values`.

#### Stepping frames and resetting state

Two endpoints exist specifically so you do not have to fake input to make time
pass (see [debug-server.md](../../garden/docs/debug-server.md#stepping-frames-and-resetting-panels)):

```bash
curl -sX POST 127.0.0.1:$PORT/tick        -d '{"n":60,"dt":0.016}'   # 60 frames of exactly 16ms
curl -sX POST 127.0.0.1:$PORT/panel/reset -d '{}'                    # restart panels, drop `state`
```

`POST /tick` ignores the sleep window and fabricates no input, so an animation
or game test is a deterministic frame count rather than a stream of phantom
keypresses. `--panel-wake` (bare = never sleep, or `--panel-wake 60`) is the
other half of that, for a panel that must keep running on its own.

## The drawing API

Panel-local logical pixels, `(0,0)` at the pane's top-left;
`screen_width()`/`screen_height()` give the current pane size. Colors are
integer RGB `0..255`.

```
clear(r,g,b)
draw_rect(x,y,w,h, r,g,b[,a])
draw_rect_rounded(x,y,w,h,radius, r,g,b[,a])
draw_rect_outline(x,y,w,h, r,g,b[,a[,width]])
draw_rect_rounded_outline(x,y,w,h,radius, r,g,b[,a[,width]])
draw_line(x1,y1,x2,y2, r,g,b[,a[,width]])
draw_polyline(points, r,g,b[,a[,width]])          // one non-overlapping stroke
draw_circle(cx,cy,radius, r,g,b[,a])
draw_circle_outline(cx,cy,radius, r,g,b[,a[,width]])
draw_ellipse(cx,cy,rx,ry, r,g,b[,a])
draw_ellipse_outline(cx,cy,rx,ry, r,g,b[,a[,width]])
fill_arc(cx,cy,r_in,r_out,a0,a1, r,g,b[,a])       // annular sector; r_in=0 = pie slice
fill_triangle(x1,y1,x2,y2,x3,y3, r,g,b[,a])
fill_poly(points, r,g,b[,a])            // convex fan; points = [[x,y], ...]
fill_polygon(points, r,g,b[,a])         // concave-correct (ear clipping)
fill_fan(cx,cy,points, r,g,b[,a])       // fan from an explicit center
draw_text(s, x, y, size, r,g,b[,a])
draw_image(source, x,y,w,h[,a])         // PNG path, relative to Garden's cwd
clip(x,y,w,h) / clip_none()
```

Reach for the right one: a translucent brush stroke is `draw_polyline`, not N
`draw_line`s or a circle per sample (overlapping pieces double-blend and the
stroke comes out mottled); a star is `fill_polygon`, not ten `fill_triangle`s
(`fill_poly` fans from vertex 0 and spills across reflex corners); a donut
segment is one `fill_arc`, not 200 `fill_poly`s; and a rounded border is
`draw_rect_rounded_outline`, not a rounded fill with a smaller one on top
(which is opaque and degenerates at radius 1). `garden/examples/panels/shapes.ptl`
draws every primitive on one screen.

**Rotated text is not supported** and will not be — rotate your shapes, keep
your labels upright (or place them per character along a path).

Reads: `dt()`, `frame_count()`, `time()`, `screen_width()`, `screen_height()`,
`mouse_x()`, `mouse_y()`, `mouse_down(btn)`, `mouse_pressed(btn)`,
`mouse_released(btn)`, `click_count()`, `drag_active()`,
`key_down(name)`, `key_pressed(name)`, `key_released(name)`,
`mod_shift()`, `mod_ctrl()`, `mod_alt()`, `mod_cmd()`,
`scroll_y()`, `scroll_x()`, `text_input()`, `text_width(s, size)`,
`panel_theme()`.

**Modifiers are `mod_shift()` / `mod_ctrl()` / `mod_alt()` / `mod_cmd()`.** They
are the *only* documented way to read a modifier — several authors tried
`key_down("shift")` first. (That works too now, as do `"ctrl"`, `"alt"` and
`"cmd"`, since modifiers are published as held keys; the `mod_*` calls are still
the ones to write.) `click_count()` is the real click chain: `2` on a double
click, `3` on a triple.

Garden owns the Cmd/Ctrl chords. If your app needs one — a spreadsheet's Cmd+C,
an editor's Cmd+Z — ask for it back with `claim_key("z", "cmd")`, stated
unconditionally near the top of every frame. A claimed chord arrives as
`key_pressed("z")` with `mod_cmd()` true, and produces no `text_input()`.

Persistence across a restart is `panel_store_get(key)` /
`panel_store_set(key, string)` — a string→string map scoped to your script's own
path, capped at 256 KiB per value. There is no file API; pair it with
`json_stringify`/`json_parse`.

### The full Petal builtin table is available

A panel is a normal Petal program: **every builtin in
[docs/Builtins.md](../../docs/Builtins.md) is callable**, on top of the panel
draw/input natives and the `ui` prelude. Nothing is subsetted. In particular the
ones authors kept hand-rolling are already there — `random(min,max)`,
`random_int(lo,hi)`, `choose(list)` for seed data; `clamp`/`min`/`max`/`round(x,
places)` (all int-preserving); `parse_int`/`parse_float` (nil on bad input,
rather than aborting the frame); `chars`/`char_len`/`char_at`/`char_slice`/`index_of`
for anything non-ASCII, since `len`/`slice` are byte-indexed;
`json_parse`/`json_stringify`; `sort_by`, `map`, `filter`.

### Alpha composites in LINEAR space — `a` is not what you expect

This surprises everyone. Colors are linearized before blending, so a low `a` is
far brighter than the nominal percentage suggests. Measured on a `#12161e`
ground with white:

| call | nominal | actually renders |
|---|---|---|
| `draw_rect(r, 255,255,255, 5)` | 2% white | `#2c2e32` |
| `draw_rect(r, 255,255,255, 10)` | 4% white | `#3c3d40` — a visible grey block |
| `draw_rect(r, 255,255,255, 20)` | 8% white | `#525355` |
| `draw_rect(r, 255,255,255, 51)` | 20% white | `#7d7d7e` |
| `draw_rect(r, 255,255,255, 128)` | 50% white | `#bcbdbd` |

So `a: 10` is **not** a whisper-soft hover tint; it is a mid-grey. For a barely
visible tint on a dark ground you want `a` in the low single digits — or, better,
skip alpha entirely and use `mix(bg, fg, t)` / `lerp_color` from the prelude to
compute an opaque color, which is predictable and does not double-blend where
shapes overlap. Measure a screenshot pixel before re-seeding your data because
"the highlight looks too strong".

### Draw order is call order

**Calls composite in the order you make them**, across every kind: a `draw_rect`
after a `draw_text` covers that text; a `draw_text` after a `draw_rect` sits on
top of it. That is what makes overlays — menus, modals, tooltips, dropdowns —
just work: draw the surface, then its contents, after everything they should
hide. (Older panels carry hand-rolled "suppress the text a menu is about to
cover" workarounds from when text always won. Those are no longer needed.)

`text_width` measures with the real font advances (JetBrains Mono), so
centering and right-alignment are exact. **`weight` now draws.** Only the
Regular face is embedded, so a run at `weight >= 600` is *synthetically*
emboldened (drawn twice at a sub-pixel offset) — visibly heavier, but a
thickening rather than a true Bold cut, and advances are untouched so
`text_width` and any column computed from it still hold. It reads as emphasis,
not as a whole second face: keep leaning on size, color and spacing for the
hierarchy.

### The `ui` prelude

`petal-ui/prelude/ui.ptl` is an implicit import — call these bare. Read that
file; it is the authoritative reference. Highlights:

- `rect(x,y,w,h)`, `point_in(px,py,r)`, `hovered(r)`, `clicked(r)`
- record overloads of every `draw_*` — `draw_rect(r, c)`, `draw_text(s, pos, style)`
  where `style` is `{size, color, weight, italic, spacing}`
- `draw_text_right`, `draw_text_center`, `ellipsize`, `truncate_tail`, `wrap`,
  `preview`, `fit_parts`
- `button(r, label[, style])`
- `list_state()`, `list_update(lst, count, visible_rows, r[, active])`,
  `list_row_rect`, `ensure_visible`, `ensure_visible_px`, `draw_scrollbar`
- `scroll_update(offset, total, visible, r[, active])`
- `focus_state()`, `focused`, `focus_set`, `focus_next/prev`, `focus_update`,
  `text_field_update(fc, id, r, buf)` (input, no pixels) +
  `draw_text_field(r, text, has_focus[, style])` (the stock look), `section_label`
- context menus: `menu_state`, `menu_item`, `menu_sep`, `menu_open_on_right_click`,
  `menu_blocking`, `menu_show`, `menu_close`, `context_menu(m, items)`,
  `menu_rect(m, items)` (the landing rect, also returned by `context_menu`)
- theming: `ui_theme()`, `theme_set({panel: …, text: …})` (a partial merge),
  `theme_reset()`, `theme_from_palette(palette())`; plus an optional trailing
  `style` record on `button`, `context_menu`, `text_field`, `draw_scrollbar`
  and `section_label` to override the theme for one call
- drag and drop: `drag_state()`, `drag_update(ds, id, rect)` →
  `{dragging, id, dx, dy, dropped}`, `drag_cancel`, `dragging(ds, id)`,
  `insertion_index(rects, y)` / `insertion_index_x`
- color helpers: `mix`/`lerp_color(a, b, t)`, `luma(c)`, `contrast_text(bg)`
- `theme` — a palette record

#### `context_menu` is a DRAW call — it must be your LAST one

The two menu calls sit at opposite ends of the frame, and this trips people:

- `menu_blocking(m)` is an **input** guard. It belongs at the top, before the
  panel's own click handling, so the widgets underneath stand down while a menu
  is open.
- `context_menu(m, items)` **paints the menu**. It must come after the very last
  drawing call the panel makes — literally the bottom of the script, not the
  bottom of the input section. Calling it early paints the menu and then paints
  your background over it, and the menu vanishes with no error at all.

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

`picked.index` counts **every** entry, separators included.

Worked examples to read before starting:
`garden/examples/panels/sketch.ptl` (draw surface),
`garden/gpp-apps/garden-diff/src/garden_diff.ptl` and
`garden/gpp-apps/git-viewers/src/git_panel.ptl` (real interactive panels:
focus registry, lists, scrolling, menus).

## The headless frame contract

**A headless panel is not a 60fps game loop.** Garden renders only dirty frames,
and headless has nothing making them dirty. What you actually get:

- **Roughly one frame per injected event** (measured: about two panel frames per
  `POST /key`), plus one per ~200ms idle poll while awake, plus a settle before
  every `/screenshot` and `/scene`.
- **`dt()` is wall-clock, and it is huge.** On an idle headless poll it measures
  ~0.1–0.2s, not 0.016. It is also spiky: the frame after a long pause carries
  the whole pause.
- **A panel sleeps 10s after the last activity** and then runs no frames at all
  until the next input. A simulation, a clock, a `time() >= next` poll — all
  stop. Keep any poll interval meaningfully under 10s.

Consequences for your app:

- Drive animation off `dt()`, never off a fixed per-frame delta.
- **Any physics needs its own clamp and sub-stepping.** `dt = min(dt(), 0.05)`
  and then integrate in fixed sub-steps; a raw 0.2s step tunnels a ball through
  a paddle, and the first frame after a pause teleports everything.
- Note the 10s sleep in your README so a reviewer does not read a stopped
  simulation as a hang.

For testing, do **not** fake input to make time pass. Use the frame-stepping
endpoint, which ignores the wake window entirely and gives every frame exactly
the `dt` you name:

```bash
curl -sX POST 127.0.0.1:$PORT/tick -d '{"n":60,"dt":0.016}'    # one deterministic second
```

and launch with `--panel-wake` (never sleep) or `--panel-wake 60` when the app
genuinely has to keep running on its own.

## `state` survives hot reload — and that will bite you

Editing the script hot-reloads it, but **Petal `state` is carried over**. The
consequence four authors hit: you change your seed-data generator, or a function
whose result is cached in `state`, save, and *nothing changes* — because the old
value is restored, not recomputed. It looks like the edit didn't take.

Don't restart the process. Reset the panel:

```bash
curl -sX POST 127.0.0.1:$PORT/panel/reset -d '{}'   # rebuild panels from source, drop state
```

The same rule is why `state` is right for what genuinely persists (selection,
scroll offset, the document) and wrong for anything you are still iterating on.

## Quality bar

These are showpieces, not smoke tests. Aim for:

- A real visual design: a considered palette, consistent spacing scale, a
  typographic hierarchy built from size/color/spacing, generous padding,
  alignment that holds at the declared viewport size.
- Actual interactivity — hover states, selection, keyboard *and* mouse where
  both make sense, transitions where they help legibility.
- Enough content to look alive (plausible seeded data, not `foo`/`bar`).
- Idiomatic Petal: `state` for what persists across frames, `let` for dataflow,
  `var`/`set` only where mutation is genuinely needed, functions to factor
  drawing, classes to name record shapes.
- No crashes and no script error at any point in the interaction script you
  exercise. There is now **one place to look**: `status_error` in `/state`
  reports a load failure, a hot reload that will not compile, *and* a frame that
  raised, and every new message is also printed to the launch log.
  `panes[].panel.error` carries the same thing per pane. A script that fails to
  load keeps a **panel** pane (running an empty stub that still watches the
  file), so fixing the file brings the canvas back on the next good save — you do
  not need to restart the process, and a `kind: "editor"` pane now really does
  mean a bad layout or a wrong path. `petal check <file.ptl>` is still the
  fastest way to confirm a script compiles before you launch anything.
- When a frame raises, `panel.values` is the last **good** frame — `values_frame`
  and `values_stale` say so, and `values_partial` carries how far the failing
  frame got. A key missing because the frame blew up is not the same as a branch
  that never ran.

## Hard rules for this run

1. **Do not modify anything under `rust/`, `garden/*/src/`, `petal-ui/src/`, or
   any `Cargo.toml`.** No `cargo build`, no `cargo run`. The prebuilt binaries
   at `garden/target/debug/garden` and `rust/target/debug/petal` are what you
   use. A build would serialize every other agent behind the cargo lock and can
   swap the binary out from under a running test.
2. **Do not touch files outside your own `examples/<category>/<slug>/`
   directory** (plus your one debrief file). Other agents are editing this
   checkout concurrently.
3. Never `pkill`/`killall`. Kill your Garden by PID.
4. Do not add `.ptl` files to `examples/console/` — that directory is a
   golden-tested corpus and a new file there breaks the suite. Subdirectories
   inside your own app directory are fine.
5. If you hit a language or host limitation, **work around it in Petal and
   record it in your debrief** — do not fix it in Rust. Language fixes are a
   separate batch.

## Committing

When the app works, commit only your own files:

```bash
git add examples/<category>/<slug> .temp/testbed-debriefs/<slug>.md
git commit -m "examples(<category>): <app name>"
```

Other agents commit concurrently, so `git commit` can fail on
`.git/index.lock`. Sleep a couple of seconds and retry, up to a few times.
Never `git add -A`, never `git checkout`/`reset`/`stash`, never rebase.

## Debrief

Write `.temp/testbed-debriefs/<slug>.md`:

```markdown
# <App name>

**Status:** complete | partial | blocked
**Viewport:** WxH
**What works:** ...
**What I could not do:** ...

## Blockers
Things that stopped you outright. Include the smallest reproducing snippet
and the exact error text.

## Issues
Friction that cost you time but that you worked around. Missing builtins,
confusing errors, surprising semantics, awkward API shapes, docs that were
wrong or missing. Be specific and include snippets — this list is the point
of the exercise.

## Praise
What was genuinely good to use.

## Feature requests
Concrete, prioritized.
```

Be honest and specific. A debrief that says "everything was fine" from an app
that took three workarounds to finish is worse than useless.
