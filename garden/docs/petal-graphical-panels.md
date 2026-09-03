# Petal graphical panels

A panel is a pane whose pixels are drawn by a Petal script. Where an
`editor(...)` pane shows a text buffer, a `panel("clock.ptl")` pane runs a
Petal script every frame and paints whatever it draws. The script calls
`draw_rect`, `draw_text`, and friends; Garden turns those calls into pixels,
in-process.

The same vocabulary is what a GPP app's pushed drawer uses: `:Git`
(`gpp-apps/git-viewers`) and the diff review (`gpp-apps/garden-diff`) are
panels whose data comes from a subprocess instead of local Rust. Everything
here applies to both. For the subprocess side see
[writing-gpp-apps.md](writing-gpp-apps.md) and [gpp.md](gpp.md).

Panels are built on the shared [petal-ui](../../petal-ui/README.md) crate,
which every Petal embedder uses for input and draw natives and the `ui`
widget prelude. Garden adds a handful of natives of its own (`emit`, `mutate`,
`navigate`, `text_view`, `edit_view`, `palette`, `claim_key`,
`request_frame`, `panel_store_*`), all described below. Example scripts live
in `examples/panels/`.

## The model

A panel is one Petal VM per pane. Each awake frame Garden binds the frame's
input, timing, and pane size into the script's environment, runs the whole
script from the top, collects the draw commands it issued, and translates
them into render primitives offset into the pane's rect and clipped to it.
There is no retained scene: what you draw this frame is what shows.

Keep UI state in `state` variables; they persist across frames. See
[Persistence](#persistence-panel_store_get--panel_store_set) for state that
must outlive the process.

### Coordinates and color

A panel draws in panel-local logical pixels: `(0,0)` is the pane's top-left,
and `screen_width()` / `screen_height()` report the pane's current size,
rebound every frame. Colors are `0–255` integer RGB (`#rrggbb` literals, or
`{r, g, b, a}` records) and stay sRGB all the way to the pixel.

The pane is not the window: it is inset by the tab strip, the status bar, and
a small gutter. A script never needs those numbers, but a harness does: read
`panes[0].rect` from the debug server rather than assuming the window size.
`POST /mouse` takes window coordinates while `mouse_x()` / `mouse_y()` report
pane-local ones.

### Alpha composites in sRGB space, the way CSS does

`a` means what a design tool says it means. Blending happens on
gamma-encoded values, the same arithmetic CSS, Core Graphics, and Figma use.
Black over white at `a = 128` renders `#808080`; at `a = 26` (10%) it renders
`#e6e6e6`. A color picked in a design tool at 20% opacity lands on the pixel
the tool showed. Text follows the same rule, so a translucent label matches a
translucent rect of the same color.

Overlapping alpha is not idempotent: two 50% fills over one pixel read 75%.
For a tint that must survive being drawn twice, compute an opaque color with
the prelude's `mix` / `lerp_color`.

## Animation: sleep and wake

Garden renders only on dirty frames; there is no continuous loop. A panel
that animates needs ticking, but ticking forever would burn CPU on an idle
window. The rule:

- Any user input (key, click, mouse move, scroll) wakes every panel, as do
  spawning, hot-reloading, and resizing.
- A panel stays awake for 10 s after the last activity, ticking at ~60 fps.
- After that it sleeps: no run, no redraw, until the next input. Its last
  frame stays on screen.

A script that is mid-animation should say so with
[`request_frame()`](#request_frame-staying-awake-while-animating). For a
harness driving someone else's script, `garden --panel-wake` never sleeps,
`--panel-wake 60` sets the window in seconds, and `POST /tick {"n": 60,
"dt": 0.016}` runs frames on demand with a `dt` you choose (see
[debug-server.md](debug-server.md#stepping-frames-and-resetting-panels)).

A trap for any script that polls on a timer: a panel reads `time()` only on
a frame, and it only runs frames while awake, so a `time() >= next` check
stops firing once the panel sleeps. A poll survives only if something
re-stamps activity within each 10 s window (a query's answer landing does).
Keep the interval meaningfully under 10 s; `garden-diff` uses 9 s. A slower
poll needs a host-side timer, not a bigger constant.

The terminal and headless frontends tick panels on their ~200 ms poll loop,
so panels there animate at that rate.

### The headless contract

Headless is not a 60 fps loop. A script there gets roughly one frame per
injected event, one per ~200 ms idle poll while awake, and one settle before
every `/screenshot` or `/scene`. `dt()` is wall-clock, so it is large and
spiky (0.1–0.2 s on an idle poll; the frame after a pause carries the whole
pause). The 10 s sleep still applies, so a simulation with no input stops.

A panel that integrates anything physical must clamp and sub-step its own
`dt` (`let step = min(dt(), 0.05)`) rather than trust the delta it is handed.
Drive frames explicitly with `POST /tick` instead of faking input.

### `state` survives a hot reload

Editing a panel's script reloads it in place and carries `state` across the
reload, so a reload does not lose your selection or scroll position. The
consequence is the most common "my edit did nothing" report: a `state` value
is not recomputed when you change the code that computes it. `POST
/panel/reset` rebuilds every file-backed panel from source and drops `state`.

### Debug visibility

Every panel draws a tiny dot in its top-right corner, filled when awake and
hollow when asleep. `/state` reports each panel's `awake` flag and `frame`
count alongside its script path.

## Draw surface

The full petal-ui draw vocabulary is available, including the optional
per-primitive `a` (alpha), corner `radius`, and stroke `width` fields.
Rectangles and text are native primitives; the rest tessellate into meshes.

| Petal fn | Draws |
|---|---|
| `clear(r,g,b)` | a full-pane fill |
| `draw_rect(x,y,w,h,r,g,b[,a])` | a filled rect |
| `draw_rect_rounded(x,y,w,h,radius,r,g,b[,a])` | a filled rounded rect |
| `draw_rect_outline(x,y,w,h,r,g,b[,a[,width]])` | a `width`-px frame |
| `draw_rect_rounded_outline(x,y,w,h,radius,r,g,b[,a[,width]])` | a hollow rounded frame |
| `draw_line(x1,y1,x2,y2,r,g,b[,a[,width]])` | a `width`-px segment |
| `draw_polyline(points,r,g,b[,a[,width]])` | one stroked path with round joins and caps, non-overlapping (see below) |
| `draw_circle(cx,cy,radius,r,g,b[,a])` | a filled circle |
| `draw_circle_outline(cx,cy,radius,r,g,b[,a[,width]])` | a ring |
| `draw_ellipse(cx,cy,rx,ry,r,g,b[,a])` | a filled ellipse |
| `draw_ellipse_outline(cx,cy,rx,ry,r,g,b[,a[,width]])` | an elliptical ring |
| `fill_arc(cx,cy,r_in,r_out,a0,a1,r,g,b[,a])` | an annular sector; `r_in = 0` is a pie slice |
| `fill_triangle(x1,y1,x2,y2,x3,y3,r,g,b[,a])` | one triangle |
| `fill_poly(points,r,g,b[,a])` | a convex fill (fan from the first point) |
| `fill_polygon(points,r,g,b[,a])` | a concave-correct fill |
| `fill_fan(cx,cy,points,r,g,b[,a])` | a fan from an explicit center |
| `draw_text(s,x,y,size,r,g,b[,a])` | one text run at `size` logical px |
| `draw_image(source,x,y,w,h[,a[,radius]])` | a PNG scaled into the rect |
| `draw_rect_gradient(x,y,w,h,r0,g0,b0,a0,r1,g1,b1,a1,angle)` | a linear gradient along `angle` |
| `draw_rect_gradient_rounded(x,y,w,h,radius,…,angle)` | the same, rounded |
| `draw_circle_gradient(cx,cy,radius,r0,g0,b0,a0,r1,g1,b1,a1)` | a disc shading center to rim |
| `draw_shadow(x,y,w,h,radius,blur,spread,dx,dy,r,g,b[,a])` | a CSS box-shadow, as one mesh |
| `clip_push(x,y,w,h[,radius])` / `clip_pop()` | a nested clip and its restore |
| `create_canvas(w,h)` / `draw_to(id)` / `draw_to_screen()` | an offscreen canvas and the target switch |
| `draw_canvas(id,x,y[,a[,w,h]])` | the canvas composited at opacity `a` |
| `snapshot_to(id,x,y)` | copy the pixels under the canvas rect into it |
| `blur_canvas(id,radius)` | Gaussian-blur the canvas in place |

`examples/panels/shapes.ptl` draws every one of them on one screen.

The prelude adds record overloads over all of these (`draw_rect(rect, color)`
and so on), plus `linear_gradient(rect, stops, angle[, radius])` for three or
more stops and `draw_shadow(rect, {radius, blur, spread, dx, dy, color, a})`.

Three of these exist because composing the older calls gives the wrong
picture, not merely a slower one:

- **`draw_polyline`.** A stroke drawn as N `draw_line`s double-blends every
  join, and a translucent brush drawn as a circle per mouse sample comes out
  mottled. `draw_polyline` tessellates a stroke whose pieces do not overlap. A
  path that crosses its own stroke still overlaps itself.
- **`fill_polygon` vs `fill_poly`.** `fill_poly` fans from the first vertex,
  which fills a convex outline and spills across reflex corners.
  `fill_polygon` ear-clips, so a star is one call.
- **`draw_shadow`.** A soft shadow hand-rolled as concentric translucent rects
  double-composites every ring. `draw_shadow` is one mesh with a smooth alpha
  falloff.

Gradients are exact: a two-stop linear gradient is an affine function of
position, which is precisely what per-vertex color interpolation reproduces.

Relative image sources resolve from Garden's working directory. RGB and RGBA
PNGs are supported; a missing or invalid file is logged and skipped without
aborting the frame.

### Input, timing, and the `ui` prelude

Input and timing reads: `dt`, `time`, `frame_count`, `screen_width`,
`screen_height`, `mouse_x`, `mouse_y`, `mouse_down`, `mouse_pressed`,
`mouse_released`, `click_count`, `drag_active`, `key_down`, `key_pressed`,
`key_released`, `mod_shift`, `mod_ctrl`, `mod_alt`, `mod_cmd`, `scroll_x`,
`scroll_y`, `text_input`, `text_width`.

The `ui` prelude is an implicit import, so scripts call its widgets bare:
`rect` / `point_in` / `hovered`, `button`, `list_update`, `scroll_update`,
`truncate_tail`, `wrap`, `draw_text_right`, the `context_menu` family, RectCut
layout (`cut_left` / `cut_right` / `cut_top` / `cut_bottom`, `split_h` /
`split_v`, `pad`, `hstack` / `vstack`), and the component set (`checkbox`,
`toggle`, `radio_group`, `slider`, `tab_bar`, `splitter`, `table`, `modal`,
`tooltip`, `spinner`, `progress_bar`, `badge` / `pill`, `card`,
`empty_state`, `hint_bar`, the `load_state` family). The reference is
[petal-ui/docs/components.md](../../petal-ui/docs/components.md); the
showcase panel is `examples/panels/gallery.ptl`.

### `import bloom`

Garden also registers [bloom](../../petal-libs/bloom/README.md), the component
library written in Petal itself, as importable modules — buttons, menus,
controls and overlays that animate by default and that hold their own
animation state per callsite:

```petal
import bloom

if bloom.button(rect(20, 20, 120, 32), "Save", {variant: "primary", icon: "check"}) then
  bloom.toast("Saved", "success")
end
```

Unlike `ui` it is *not* an implicit import — a library that silently occupied a
hundred names would collide with panels that define their own `button` or
`switch` — so a drawer says `import bloom` (or `import bloom: button, dropdown`)
and pays nothing otherwise. The modules are registered in memory rather than
looked up on disk, so a panel-mode GPP drawer pushed as source can import them
too. Garden's own start screen (`gpp-apps/main-menu`) and the `screens-demo`
app use it; the full reference is
[petal-libs/bloom/docs/components.md](../../petal-libs/bloom/docs/components.md)
and the showcase is `examples/ui/bloom-gallery`.

A panel script on disk can also `import` a module sitting next to it: imports
resolve relative to the script's own directory, and editing an imported module
hot-reloads the panel exactly as editing the script does.

The whole Petal builtin table is available too. A panel is an ordinary Petal
program with extra natives, not a sandboxed subset: everything in
[docs/Builtins.md](../../docs/Builtins.md) is callable. Worth naming because
panel authors have hand-rolled them: `random`, `random_int`, `choose`;
`clamp` / `min` / `max` / `round(x, places)` (int-preserving); `parse_int` /
`parse_float` (nil on bad input rather than aborting the frame); `chars` /
`char_len` / `char_at` / `char_slice` / `index_of` for non-ASCII text (`len`
and `slice` are byte-indexed); and `json_parse` / `json_stringify`.

Three things the prelude also covers:

- **Theming.** `ui_theme()` is the live palette every widget paints with. In
  Garden it defaults to the host palette automatically, so prelude widgets
  paint in Garden's colors with no setup. `theme_set({panel: …, text: …})`
  merges explicit colors over that until `theme_reset()`. Every widget takes
  an optional trailing `style` record overriding the theme for that call.
- **Drag and drop.** `drag_state()` plus one `drag_update(ds, id, rect)` per
  draggable item yields `{dragging, id, dx, dy, dropped}`;
  `insertion_index(rects, y)` turns a drop position into a list index.
- **Text fields.** `text_field_update(fc, id, r, buf)` is the input half
  (click-to-focus, typing, backspace, Return) with no pixels of its own;
  `draw_text_field(r, text, has_focus[, style])` is the stock look.

Plus small color helpers: `mix` / `lerp_color(a, b, t)`, `luma(c)`,
`contrast_text(bg)`.

`garden --version --json` lists every prelude export as `name/arity`
(`prelude.exports`), so you can check whether a binary has a helper before
writing against it; an older binary fails at runtime with `Unknown builtin`.

### Context menus

A context menu is two calls, because an immediate-mode pass has to reconcile
z-order with input order: the menu must be drawn last to sit on top, but its
click must be claimed before the widgets underneath react to the same press.

- `menu_blocking(m)` is an input guard. Put it above the panel's own click
  handling.
- `context_menu(m, items)` is a draw call. It must come after the panel's
  last drawing call. Since [draw order is call order](#draw-order), calling
  it early paints the menu and then paints the panel's background over it, so
  the menu vanishes with no error.

```petal
state menu = menu_state()

// input, near the top, before the panel's own click handling:
if !menu_blocking(menu) && mouse_pressed(0) && point_in(mouse_x(), mouse_y(), row) then … end
menu = menu_open_on_right_click(menu, row, i)   // right-click arms it, tagged with `i`

…all of the panel's drawing…

// the LAST draw call in the frame:
let picked = context_menu(menu, [menu_item("Only this"), menu_sep(), menu_item("All", enabled)])
menu = picked.menu
if picked.index >= 0 then … picked.label, picked.tag … end
```

`picked.index` counts every entry, separators included. Escape and a click
outside dismiss without choosing; Up/Down skip separators and disabled rows
and Return takes the highlight; a menu that would run off the screen flips to
the other side of the pointer. `garden_diff.ptl`'s commit rows are the worked
example.

### Draw order

Calls composite in the order you make them, across shapes, text, and images
alike. A `draw_rect` after a `draw_text` covers that text. This is what makes
overlays work: a menu, a modal, a tooltip is "draw the surface, then its
contents, after everything it should hide".

### Clipping

`clip(x, y, w, h)` narrows every following call to that rect until
`clip_none()`. The clip is intersected with the pane, so a script can never
paint outside its own pane.

`clip` replaces whatever clip is active, which is wrong inside anything
reusable. `clip_push(x, y, w, h)` intersects with the clip in force and
`clip_pop()` restores it, so clips nest:

```
clip_push(list_rect)          -- the viewport
for row in rows do
  clip_push(row.badge_rect)   -- intersected with the viewport
  ...
  clip_pop()                  -- back to the viewport
end
clip_pop()                    -- back to the pane
```

The pane rect is the base of the stack and is never popped; an unmatched
`clip_pop()` means "back to the pane".

Both forms take an optional trailing `radius` to round the clip's corners.
Shapes and images are cut with an antialiased rounded mask; text is cut to
the clip's bounding rect (a degradation, never a dropped clip). Only the
innermost rounded mask is in force at a time, and a square `clip_push` inside
a rounded one keeps the rounding. The clip also cuts the text inside a
`text_view` / `edit_view` region declared while it is active.

An image can round itself with the trailing `radius` on `draw_image`, which
is the circular avatar in one call:

```petal
draw_image("avatar.png", rect(50, 210, 140, 140), 255, 70)   -- radius = w/2
```

Text is cut, not dropped: a run that straddles the clip's bottom edge renders
its top half, which is what a scrolling list wants at its viewport boundary.
A drawer does not need to cull half-visible rows itself; skipping rows
entirely outside the viewport is still cheap work avoidance. Each `/scene`
primitive carries a `visible` flag, so a headless test can tell a drawn row
from a clipped-away one.

### Layers and blur

The canvas ops give a script an offscreen buffer, which is what a group
opacity, a glow, or a frosted bar needs. The prelude wraps them so the common
cases are one call:

```petal
// Content first: the snapshot reads what is already there.
draw_list(...)
// A translucent bar it scrolls under: what is behind it, blurred, tinted.
draw_material(rect(0, 0, w, 52), {kind: "regular", hairline: "bottom"})

// A group at 50%: overlapping shapes as one object.
layer(card, {a: 128}, fn()
  draw_circle(rect(0, 0, 80, 80), c1)      // canvas-local: (0, 0) is card's top-left
  draw_circle(rect(40, 0, 80, 80), c2)
end)

// A glow: the shape, blurred.
layer(halo, {blur: 12}, fn() draw_circle(rect(0, 0, halo.w, halo.h), accent) end)
```

`layer(rect, body)` creates a canvas the size of `rect`, runs `body()` with
drawing redirected into it (coordinates are canvas-local and the clip stack
starts fresh at the canvas bounds), then composites it at `rect`. Layers
nest: `draw_to` returns the target it replaced. Canvas ids restart at 1 every
frame and the target starts at the pane, so a script that errors mid-layer
cannot strand the next frame in a canvas.

`/scene` reports the ops as `canvas`, `target`, `snapshot`, `blur`, and
`canvas_draw` entries; primitives between a `target` naming a canvas and the
one switching back are in canvas coordinates. `/hit` skips what is drawn into
a canvas and picks the composite as one rectangle. The terminal frontend
ignores the ops. `examples/panels/layers.ptl` draws the material bar, group
opacity, and glow.

### Text size and measurement

`draw_text`'s `size` is honored per run, so a script can build a real
typographic hierarchy (a 28 px heading over a 10 px caption). Line height
follows the size at 1.4×.

`text_width(s, size)` measures with the real advances of the font Garden
rasterizes with, so a rule drawn `text_width(s, size)` wide ends flush with
its text at every size; centering and right-alignment are exact. A face can
be named: `text_width(s, size, "mono")`.

### Any font on the machine

`font(name)` returns a face as a value, so its size and decorations travel
with it:

```petal
let body = font("Helvetica Neue", 15)
let title = font_size(font_bold(body), 28)

draw_text("Chapter One", {x: 20, y: 40}, title)
let w = text_width("Chapter One", title)      // measures that exact cut
```

`name` is a family name, one of the two role names (`mono`, `ui`), or a
CSS-style fallback list (`"Menlo, mono"`). Garden resolves it
case-insensitively against every family installed. A family the machine
lacks falls back to the monospace face for both measuring and drawing, so a
panel written on one machine still lays out sensibly on another.

A font object is a style record (next section), so it goes anywhere one does
and merges the same way: `{...title, color: palette().accent}`. `font_size`,
`font_weight`, `font_bold`, `font_italic`, `font_spacing`, and `font_color`
each return a new object. `fonts()` lists the families available.

Discovery and measurement are lazy and process-wide: font directories are
not scanned until a panel first calls `font()` or `fonts()`, and a family's
advances are measured once. A panel that cycles through hundreds of families
will feel it.

The two embedded roles are pinned to the cuts Garden ships (`mono` is
JetBrains Mono, `ui` is Inter Regular/Bold) even when the machine has a
same-named family installed, because the editor's column arithmetic depends
on the embedded advances.

### Styled text

`draw_text` also takes a style record, and `text_width` measures the same
record:

```petal
let BODY = {size: 15, color: palette().text}
draw_text("Merge pull request ", {x: 10, y: 20}, BODY)
draw_text("#482", {x: 170, y: 20}, {...BODY, weight: 700})
draw_text("s p a c e d", {x: 10, y: 40}, {...BODY, spacing: 2})
```

Fields are `size`, `color`, `font`, `weight`, `italic`, `spacing`; any
subset.

| Axis | In a Garden pane |
|---|---|
| `size` | honored per run |
| `font` | any installed family, plus `mono` (JetBrains Mono) and `ui` (Inter); fallback lists work; an unresolvable name degrades to JetBrains Mono |
| `italic` | resolves when a matching face is available; no italic cut is embedded, so on `mono` and `ui` it is upright |
| `weight` | real on a system family and on `ui` (Inter Bold is embedded, so `weight >= 600` shapes the Bold cut); synthetic on `mono` (drawn twice at a sub-pixel offset, so it measures regular) |
| `spacing` | honored; the pen matches `text_width` exactly |

Measure in the face you draw in. `text_width(s, 22)` sums monospace advances,
so measuring a `font: "ui"` run without the third argument lands visibly
wrong while the drawing looks fine. Passing the same font object to both
`draw_text` and `text_width` avoids this:

```petal
let TITLE = {size: 22, weight: 700, font: "ui", color: palette().text}
let w = text_width("Pick your handle", 22, "ui")   // not text_width(s, 22)
draw_text("Pick your handle", {x: (screen_width() - w) / 2, y: 40}, TITLE)
```

A text run that names no face is drawn and measured in the panel's default
face, which is Garden's monospace role. The full cross-host contract is in
[docs/text-and-fonts.md](../../docs/text-and-fonts.md).

### Rotated text is not supported

There is no `rotate(angle)` on a text draw. Text goes through glyphon, whose
placement API is `left` / `top` / `scale` only, so rotated labels would need
a new text path. Draw the label upright beside the rotated shape, or lay out
per-character positions along a path.

## Persistence: `panel_store_get` / `panel_store_set`

A panel's `state` lives and dies with the process. There is deliberately no
file API in the panel vocabulary, so a panel that must remember something
across a restart uses the store:

```petal
state todos = json_parse(panel_store_get("todos") ?? "[]")
# …after an edit…
panel_store_set("todos", json_stringify(todos))
```

- String to string, scoped to the script's own path (or, for a GPP drawer,
  the app's name). Two panels running different scripts cannot see each
  other's keys. `panel_store_set(key, nil)` deletes a key.
- One JSON file per script under `~/.garden/panel-store/`.
  `GARDEN_PANEL_STORE_DIR` overrides the directory, which is how a test gets
  a scratch store.
- Written atomically after any frame that changed it. A frame that changes
  nothing writes nothing.
- Capped at 256 KiB per value and 1024 keys; over either, `set` errors. It is
  a place for the kilobyte of state a panel owns, not a database.
- A write failure reaches the script's `print` output rather than failing the
  frame.

## Asking the host to act: `mutate`

`mutate(name, arg)` is the panel vocabulary's one effectful call. In a GPP
pane it is a request to the pane's subprocess (see [gpp.md](gpp.md)); the
reply becomes the status note, an error the status error.

An in-process `panel(...)` pane has no subprocess, so `mutate` is also how a
panel asks Garden itself to act. These names are answered by Garden before
any forwarding:

| `mutate(…)` | Effect |
|---|---|
| `mutate("open_path", { path: "…" })` | open that file in the focused pane, as `:e` does |
| `mutate("open_project", { path: "…" })` | record the directory as a project and browse it |
| `mutate("open_pr", { number: 42 })` | open the PR review (`:PR 42`) |
| `mutate("open_file_dialog", { mode: "file" })` | native picker, then open what was chosen (`mode: "folder"` opens a project) |

Every other name goes to the subprocess. A malformed argument is a status
error, and `open_file_dialog` is refused under `--term` / `--headless`.

`emit(event, arg)` only writes to a client's pipe, so in an in-process pane
it is silently dropped.

### Reading the outcome: the handle `mutate` returns

`mutate(...)` returns an integer handle, and `mutate_result(handle)` reads
back what the request resolved to: nil while in flight, otherwise a record:

```petal
{ ok: true,  value: "wrote 2 files", error: nil }
{ ok: false, value: nil,             error: "panel has no subprocess to handle 'apply'" }
```

The round trip does not block a redraw, so the frame that makes the request
never sees its own answer. Keep the handle in `state` and read it later:

```petal
state saving = 0
if key_down("ctrl") && key_pressed("s") then
  saving = mutate("save", { text: edit_view_text(1) })
end
let saved = mutate_result(saving)
let save_failed = saved?.ok == false
```

`saving` and `saved` are ordinary bindings, so they show up in
`panes[].panel.values`, which is how a headless test asserts that a save
happened. Ignoring the return value keeps fire-and-forget behavior.

## Navigating: `navigate(screen)` / `navigate(screen, arg)`

`navigate(screen)` pushes a new entry onto the pane's browser-style history;
`navigate_back()` / `navigate_forward()` walk it (as do `Ctrl+[` / `Ctrl+]`
and `:back` / `:forward`); `navigate_replace(screen)` swaps the current entry.
An in-process pane declares its screens with `panel(script, { screens:
[...] })`; a GPP app declares them on its `PanelUi`.

The two-argument form carries the subject the target screen is for, and the
target reads it back with `nav_arg()`:

```petal
// list.ptl
if clicked then
  navigate("detail.ptl", { id: rows[sel].id })
end

// detail.ptl
let id = nav_arg()?.id ?? 0
```

`arg` is any JSON-representable value. `nav_arg()` is nil for a screen
reached without one, including the panel's origin screen, so pair it with a
`??` fallback. The argument is stored on the history entry, so back and
forward return to a screen with the subject that visit was opened with.

For a subprocess panel, back and forward also re-issue the restored entry's
`navigate` request to the app, since the app holds the data the screen
draws. See [writing-gpp-apps.md](writing-gpp-apps.md#multi-screen-navigation).

## Input: what a focused panel receives

A focused panel reads input through the standard petal-ui contract listed
above. Garden feeds it from the same paths every frontend and the debug
server use, so a real window and `POST /key` / `POST /mouse` deliver the same
thing.

- A chord carries no text: `text_input()` is empty on any frame whose key was
  held with Cmd, Ctrl, or Alt. Shift is not a command modifier, so `Shift+a`
  still types `"A"`.
- Every modifier arrives on keys and mouse alike, so an alt-drag or a
  cmd-click sees its modifier.
- Modifiers are also held keys: `key_down("shift")` / `"ctrl"` / `"alt"` /
  `"cmd"` work as well as `mod_shift()` and friends.
- `click_count()` is the real chain: 2 on a double click, 3 on a triple.
- Return is spelled `"return"`, never `"enter"`.

### `request_frame`: staying awake while animating

A panel sleeps ten seconds after its last activity. That is right for a
still drawer and wrong for ambient motion: a spinner or a pulsing live dot
freezes and reads as a hang.

```petal
request_frame()   // this frame is part of an animation; keep ticking
animating()       // the same native, under a name that reads better in a loop
```

The call is declarative and per frame: it covers only the frame that makes
it, so a script asks while its motion runs and stops asking when it settles.
Calling it every frame is free.

```petal
if loading() then
  request_frame()
end
draw_spinner(rect(20, 20, 24, 24), time())
```

### `claim_key`: a panel's own command keyspace

Garden owns the Cmd/Ctrl chords; a panel never sees them. For a panel whose
bare letters are content (a spreadsheet, a console), `claim_key` asks for a
chord back:

```petal
claim_key("z", "cmd")          // Cmd+Z reaches this panel, not Garden's Undo
claim_key("s", "cmd+shift")    // one exact chord
claim_key("escape")            // this key under any modifier combination
```

A claim is declarative and per frame: state it unconditionally near the top
of the script, and it applies to the keys that arrive before the next frame.
The chord is then delivered like any other key: `key_pressed("z")` with
`mod_cmd()` true, and no `text_input()`.

The modifier argument is a spelling (`"shift"`, `"ctrl"` / `"control"`,
`"alt"` / `"option"`, `"cmd"` / `"super"` / `"meta"`, combined with `+`) or
the raw bitmask `1=shift 2=ctrl 4=alt 8=cmd`, the same encoding `/state`
reports as `panel.input.modifiers`. An unknown spelling is an error.

Cmd/Ctrl+Q cannot be claimed. Everything else, including the `:` command bar
and the `Ctrl+[` / `Ctrl+]` history chords, can be, so a panel that claims
them takes on the job of offering its own way out.
