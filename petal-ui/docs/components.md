# The `ui` component library (prelude level 3)

The `ui` prelude module (`petal-ui/prelude/ui.ptl`) is the standard widget
library for Petal UIs. Embedders register it as an implicit import, so scripts
call everything here bare. This document is the reference for the level-3
surface — the component-library expansion — plus the conventions every widget
follows. The worked example showing all of it is
`garden/examples/panels/gallery.ptl`:

```
cd petal-ui && cargo run --bin petal-ui-run -- \
    ../garden/examples/panels/gallery.ptl --frames 60
```

Everything is **immediate mode**: the script re-runs top to bottom every
frame, widgets draw when called and return their new value. Retained state
lives in `state` (keyed per call path — two callsites of the same widget never
share internal state).

## Conventions

1. **Explicit state records.** Widgets with cross-frame state take and return
   a plain record you keep in `state`:
   `state files = list_state()` … `files = list_update(files, …)`.
   Multiple instances coexist because *you* hold the state.
2. **Update/draw split.** Interactive widgets come in three forms where it
   pays: `x_update(...)` (logic, no pixels), `draw_x(...)` (pixels, no logic),
   and `x(...)` (both). Keep the fiddly input half, bring your own paint.
3. **Trailing optional style record.** Every drawing widget takes a final
   `style` record; any key it omits falls back to `ui_theme()`. `{}` (or the
   shorter arity) is the fully themed widget.
4. **Values, not callbacks.** `checkbox(r, label, v)` returns the new bool;
   `button(r, label)` returns true on the frame it was clicked.

## Theme

`ui_theme()` is the live palette every widget paints with. Resolution order:

1. an explicit **`theme_set({...})`** — merges the given keys over the current
   resolved theme (leave a key out and it keeps its value); wins until
   `theme_reset()`;
2. the **host palette**, when the embedder published one via
   `petal_ui::input::bind_host_palette` (Garden binds its full `palette()`
   every frame, so prelude widgets paint in Garden's colors with zero
   script-side setup — no `theme_set(theme_from_palette(palette()))` needed);
3. the **built-in dark default** below.

`theme_from_palette(p)` projects a host-vocabulary palette record
(`window_bg`, `text_mut`, `sel`, `green`, …) onto these keys explicitly, for
apps that want to do the adoption themselves.

Semantic tokens (colors):

| token | role |
|---|---|
| `bg` | window / canvas floor |
| `panel`, `surface` | raised panel / card face (twins) |
| `overlay` | floating layer: tooltips, key caps, badges |
| `text`, `muted`, `dim` | primary / secondary / tertiary text |
| `accent` | interactive highlight |
| `success`, `warn`, `danger` | status hues |
| `outline`, `border` | hairlines (twins) |
| `border_strong` | emphasized border |
| `hover`, `selection` | interaction fills |

Scales (numbers, merged and overridden the same way):
`space_sm/space/space_lg` (4/8/16), `radius_sm/radius/radius_lg` (3/6/10),
`font_sm/font_md/font_lg/font_xl` (12/14/18/24).

Typography and elevation:

| token | role |
|---|---|
| `font` | the face widget text is set in — **`"ui"`**, the host's proportional interface face |
| `elevation_1/2/3` | the shadow scale, as `{blur, spread, dy, a}` records: resting / floating / lifted |

`font` is deliberately *not* the host's default face. Every host in this
ecosystem defaults to monospace, and a whole interface set in monospace is the
loudest possible tell that a panel was not designed. A drawer that genuinely
wants monospace chrome — a diff viewer, a data grid — opts back in with one
line, and every widget follows:

```
theme_set({font: "mono"})
```

Text a drawer draws itself with the bare `draw_text(s, pos, size, color)` form
is untouched: that still means "the host's own face", as it always has.

A style record and a theme record are interchangeable — `theme_set` carries
extra keys through, so an app can hang its own names on the theme.

## Layout — RectCut

No constraint solver; rects all the way down. `rect(x, y, w, h)` constructs a
`Rect` (the built-in class: `.center_x()`, `.inset(n)`, `.right()`, …).

- `cut_left(r, px)` / `cut_right` / `cut_top` / `cut_bottom` → `{cut, rest}` —
  slice a strip off one edge, keep the remainder.
- `split_h(r, frac, gap)` / `split_v` → `{a, b}` — two parts around a gap.
- `pad(r, n)` / `pad(r, x, y)` / `pad(r, l, t, r, b)` — inset.
- `hstack(r, n, gap)` / `vstack(r, n, gap)` → list of n equal cells.
- `row(r, widths, gap)` / `col(r, heights, gap)` → list of cells where an
  **int entry is fixed pixels and a float entry is a flex weight** over the
  leftover (weights are normalized against each other):
  `row(bar, [90, 0.6, 0.4, 120], 8)`.

## Motion

Deterministic under the headless harness (time/dt are fixed there).

- `approach(cur, target, rate)` — exponential smoothing, snaps within 0.001.
  Rate is 1/seconds; ~16 lands in roughly 120 ms.
- `ease_out(t)` / `ease_in_out(t)` — cubic easings, `t` clamped to [0, 1].
- Buttons, checkboxes, toggles, radios, tabs and sliders ease their
  hover/press/check transitions internally (~120 ms); at rest they paint the
  exact resting colors, so still frames are pixel-stable.

## Widgets

Signatures show the full arity; every `style` is optional.

### Button
`button(r, label, style)` → clicked? — `style: {bg, hover, fg, outline, size}`.

### List
`list_state()` → `{selected, scroll}`;
`list_update(lst, item_count, visible_rows, r, active)` handles
j/k/arrows/home/end/page keys (gated on `active`), click-select, hover-scoped
wheel; `list_row_rect(r, visible_rows, i, scroll)` for painting rows;
`ensure_visible(scroll, selected, visible_rows)` and the pixel/variable-height
counterpart `ensure_visible_px(scroll, row_off, row_h, view_h)`;
`draw_scrollbar(r, count, rows, scroll, style)`.

### Scroll region
`scroll_update(offset, total, visible, r, active)` → new offset (wheel is
hover-scoped; page keys gated on `active`).

### Focus registry
`focus_state()`, `focused(fc, id)`, `focus_set/clear`, `focus_next/prev(fc,
ids)`, `focus_update(fc, ids)` (Tab / Shift+Tab over the ring).
`section_label(label, x, y, active, style)` marks the focused region.

### Text field (with caret)
`text_field(fc, id, r, buf, style)` → `{focus, text, caret, submitted}`.
Click focuses and places the caret at the clicked character; left/right/
home/end move it; backspace deletes before it (alt/ctrl+backspace to word
start); delete removes after it; typing inserts at it; Return sets
`submitted`. Split forms: `text_field_update(fc, id, r, buf, style)` (logic
only — `style.size`/`inset` must match the draw half) and
`draw_text_field(r, text, has, caret, style)` (the 3- and 4-arg forms without
`caret` draw it at the end, as before level 3). No selection model.

### Spinner + progress
`spinner(cx, cy, radius, style)` — rotating arc off `time()`;
`spinner_glyph()` — the classic `| / - \` character, cycling on the frame
clock; `progress_bar(r, frac, style)` — `frac` in [0,1], negative for the
indeterminate sweep.

### Checkbox / toggle / radio / slider
`checkbox(r, label, v, style)` → new bool; `toggle(r, v, style)` → new bool
(animated knob); `radio_group(r, labels, selected, style)` → new index (one
row per label, `style.row_h`); `slider(r, value, lo, hi, style)` → new float
(press-to-jump, drag keeps tracking outside `r`).

### Tab bar
`tab_bar(r, labels, active, style)` → new active index; underlined active tab,
hover fade, bottom hairline across `r`.

### Tooltip
`tooltip(anchor, text, style)` — after the pointer rests over `anchor` for
`style.delay` (0.45 s), fades in near the pointer. Call late in the frame so
it draws on top.

### Modal / dialog
```
state md = modal_state()
if button(open_r, "Delete…") then md = modal_open(md) end
…guard your own input with modal_blocking(md)…
let res = modal(md, 380, 200, "Delete branch?")   // late in the frame
md = res.modal
if res.open then …draw into res.content; modal_close(md) on your buttons… end
```
Escape or a click outside closes. Split forms: `modal_update(m, r)`,
`draw_modal_backdrop(style)`, `draw_modal(r, title, style)` → content rect,
`modal_rect(w, h)`.

### Badge / pill / card / empty state / hint bar
`badge(x, y, text, style)` → its rect (flow the next one after it);
`pill(x, y, text, color, style)` — translucent tinted status chip (pass
`t.success` / `t.warn` / `t.danger` / `t.accent`); `card(r, style)` → padded
content rect (`shadow: false` to skip the drop shadow);
`empty_state(r, title, hint_text, style)` — centered "nothing here" copy;
`hint_bar(r, hints, style)` with `hint(key, label)` — the bottom-edge keyboard
hint strip.

### Splitter
```
state sp = splitter_state(0.3)
let s = splitter(sp, content, {min_a: 180, min_b: 260})
sp = s.state    // draw into s.a and s.b
```
Draggable divider; `style: {axis ("x"|"y"), min_a, min_b, band, gap, line,
active}`. Replaces the hand-rolled `left_frac`/`drag_div` pattern.

### Table
```
state tb = table_state()          // {selected, scroll, sort_col, sort_asc}
tb = table(tb, r, [{label: "Name", w: 0.6}, {label: "Size", w: 90}], rows)
```
Header + rows with column sizing (`w` ≥ 1 px, 0 < `w` < 1 weight, absent =
equal flex), list keyboard/mouse selection, zebra striping, scrollbar, and
click-to-sort headers. The table records the sort *request* — sort your rows
by `tb.sort_col`/`tb.sort_asc` before passing them in; it never reorders data.
`table_col_rects(r, cols)` exposes the column layout.

### Pixel-budget wrap
`wrap_px(s, avail_px, size)` → list of lines measured with `text_width`
(`size` a px number or a style record). Explicit `\n` splits first; a word
wider than the budget hard-breaks.

### Async load state
The `X_ready/X_err/X_loading` pattern, blessed:
```
state ls = load_state()
ls = load_poll(ls, key, query("diff", sha))   // any pending or plain value
if draw_load(content, ls) then …draw ls.value… end
```
`load_state/loading/ready/failed`, `load_begin(ls, key)`, `load_ok(ls, v)`,
`load_fail(ls, msg)`. `load_poll` restarts when `key` changes (the stale
`ls.value` stays readable while the new load is in flight) and absorbs
pending values via the core `is_ready`/`is_error`/`error_of`. `draw_load`
paints spinner/error/idle into `r` and returns true when ready.

### Context menu / drag & drop
Unchanged from level 2: `menu_state`/`menu_item`/`menu_sep`/
`menu_open_on_right_click`/`menu_blocking`/`context_menu` (draw late, guard
early — see the comment in ui.ptl), and `drag_state`/`drag_update`/
`insertion_index(_x)`.

## Text + color helpers

`truncate_tail/head`, `wrap` (char budget), `wrap_px` (pixel budget),
`preview`, `ellipsize`, `ellipsize_tail`, `fit_parts`, `fit_parts_n`,
`draw_text_right/center`, `mix`, `lerp_color`, `luma`, `contrast_text`,
`elapsed`.

### Measure the face you draw

Inside a widget there is exactly one way to put text on screen: build a style
record once and hand it to **both** `text_width` and `draw_text`.

```
let ts = {size: t.font_md, font: t.font, color: t.text}
let x = r.x + (r.w - text_width(label, ts)) / 2      // measured in ts
draw_text(label, {x: x, y: y}, ts)                   // drawn in ts
```

The bare pair — `draw_text(s, pos, size, color)` with `text_width(s, size)` —
cannot be used for anything positioned, because the two do not mean the same
thing: the draw goes to the host's default *face* while the measure falls back
to the default *metric table*. Ask for a proportional face and measure it with
the monospace ruler and a centred title lands about a fifth of its width off.
Every alignment helper, every widget, and the ellipsize/wrap family all take a
style record for this reason.

### Compositing flat tints

A hover wash, a disabled label, a hairline divider or a zebra stripe is a
translucent ink over a surface you already know. Drawing it translucent is not
idempotent — two 50% fills on one pixel read 75%, so a widget drawn twice in a
frame comes out the wrong colour.

| Call | Returns |
|---|---|
| `over(bg, fg, a)` | `fg` composited over `bg` in sRGB, **opaque**. `a` is 0–255, or a 0.0–1.0 float |
| `tint(bg, fg, pct)` | `over` with the alpha written as a percentage |
| `hairline(bg, ink)` | the 10% rule every divider wants |
| `alpha_pct(p)` | a percentage as the 0–255 alpha the primitives take |

This is for tints over a **known** surface. Genuine translucency — a modal
backdrop, a fading tooltip, anything over content the widget did not paint —
still takes a plain alpha, because there is nothing to composite against ahead
of time. Widgets that flatten a tint (`pill`, `tab_bar`, `table`) take an `on`
style key naming the surface they sit on.

### Text that would rather be rotated

**Rotated text is not available** — no host here can rotate a run; the
rasterizers place a run by its left/top and scale, and that is the whole
transform. The two honest workarounds:

| Call | Draws |
|---|---|
| `draw_text_along(points, s, style)` | one upright glyph at a time along a polyline (an arc of text, a label following a route) |
| `draw_axis_labels(r, ticks[, style])` | upright x-axis labels under their ticks, thinned when they collide, or staggered onto two rows with `{stagger: true}` |
| `draw_axis_labels_y(r, ticks[, style])` | the y-axis counterpart, right-aligned beside each tick |

`ticks` is a list of `{x, label}` (or `{y, label}`); both return how many were
drawn.

## Draw primitives — gradients, shadows, nested clips

Beyond the fills and strokes (`draw_rect`, `draw_rect_rounded`, `draw_line`,
`fill_arc`, …), each of which takes a rect record and a color record:

| Call | Draws |
|---|---|
| `draw_rect_gradient(rect, c0, c1, angle[, a0, a1])` | a linear gradient across the rect |
| `draw_rect_gradient_rounded(rect, radius, c0, c1, angle[, a0, a1])` | the same, with rounded corners |
| `draw_circle_gradient(center, radius, c0, c1[, a0, a1])` | a disc shading center → rim |
| `linear_gradient(rect, stops, angle[, radius])` | 3+ stops, subdivided into bands |
| `draw_shadow(rect, {radius, blur, spread, dx, dy, color, a})` | a CSS box-shadow |
| `draw_shadow(rect, radius, blur, color[, a])` | the positional short form |
| `clip_push(rect[, radius])` / `clip_pop()` | a clip that nests inside the enclosing one |
| `clip(rect[, radius])` / `clip_none()` | replace / clear the clip (unchanged) |
| `draw_image(source, rect[, a[, radius]])` | a bitmap, optionally round-cornered |

`angle` is radians, clockwise from +x with y growing downward — the screen
convention `fill_arc` uses. `0` runs left→right, `PI/2` top→bottom.

A gradient stop may carry an `a` field, and then that is the stop's alpha. It is
the one primitive where alpha rides *with* the color, because a two-stop fade
needs two of them; the trailing `a0, a1` overloads override it.

Prefer `clip_push`/`clip_pop` inside anything reusable: `clip` *replaces* the
active clip, so a widget that clips its own contents would throw away the clip
its caller set.

### Elevation

The widget set never calls `draw_shadow` with ad-hoc numbers; it casts from
one named scale, so a card, a menu and a modal read as three heights of the
same light rather than three unrelated greys.

| Call | Draws |
|---|---|
| `draw_elevation(rect, radius, level[, style])` | the theme's elevation 1/2/3 shadow (`style` overrides `{blur, spread, dx, dy, color, a}`) |
| `elevation(level)` | that level's opts record, for a call that needs to tweak it |
| `draw_focus_ring(rect, radius[, style])` | the accent bloomed around a focused widget instead of a doubled border |
| `draw_scroll_shadow(header_rect, frac[, style])` | the fade a sticky header casts over content scrolled under it; `frac` 0 draws nothing |

`card` is elevation 1, `context_menu` and `tooltip` are 2, `draw_modal` is 3.

### Scrims, fades and area fills

| Call | Draws |
|---|---|
| `draw_scrim(rect[, style])` | a band fading from solid at one `edge` to nothing — text over a photo |
| `draw_fade(rect[, style])` | the same in the theme background: content dissolving at the end of a scroll region |
| `draw_area_fill(points, baseline_y, color[, style])` | the fill under a line chart, fading to transparent at the baseline |

`draw_area_fill` cuts each segment into `step`-px columns and gives each its
own vertical gradient. That is exact rather than approximate: the fill's alpha
is a function of y alone, so starting each column at its own top with the
alpha the global ramp has *there* reproduces one ramp across the whole area.

`button` and `badge` take `gradient: [c0, c1]` (plus `gradient_angle`) for a
ramp-filled face; `button` also takes `radius` and `elevation`.

`draw_shadow` is one command rather than a stack of translucent rounded rects
on purpose — alpha blending is not idempotent, so nested rings double-composite
and show every seam. Hosts tessellate it as a single non-overlapping mesh
(`petal_ui::tess::shadow_mesh`).

## Compatibility

Level 3 is strictly additive: every pre-existing export keeps its signature,
and apps that shadow prelude names with their own (`fn spinner`,
`fn draw_scrollbar`, …) keep working — implicit-import bindings are weak.
Level 5 adds the gradient/shadow/nested-clip primitives above, and is additive
in the same way. Check a binary's surface with `garden --version --json`
(`prelude.exports`) or `petal_ui::PRELUDE_LEVEL`.

Level 6 is additive in signature but **not** in appearance, and deliberately
so: widget text moved from the host's default face to the theme's `font`
(`"ui"`), and the three hard-edged offset rectangles that stood in for
shadows became real `draw_shadow` commands off the elevation scale. A drawer
that wants the old typography back adds `theme_set({font: "mono"})`; there is
no way to ask for the old shadows back, because they were not shadows.

## Testing your UI

`harness::Headless` drives a script without a window: `.click(x, y)`,
`.key(...)`, `.frame()`, then assert on `.state()` / draw commands. See
`petal-ui/tests/widgets.rs` for one worked test per widget, and
`docs/dev/headless-ui-run.md` for the CLI equivalent.
