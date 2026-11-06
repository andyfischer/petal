# Drawing guide

The draw primitives `import ui` gives a script, and how to use them well:
measuring text in the face you draw, compositing flat tints, every fill and
stroke shape, gradients, shadows and the elevation scale, nested clips, and
offscreen layers.

- For one line on every export (images, retained drawing with `cached_layer`
  and `draw_rects`, text styles and fonts), see [reference.md](reference.md).
- For what a host does with the commands these calls emit, see
  [host-contract.md](host-contract.md).

## Text and colour helpers

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

The bare pair, `draw_text(s, pos, size, color)` with `text_width(s, size)`,
cannot be used for anything positioned: the draw uses the host's default face
while the measure uses the default metric table, and with a proportional face
a centred title lands noticeably off. Every alignment helper, every widget,
and the ellipsize/wrap family take a style record for this reason.

`text_width` rounds to a whole pixel. Where the fraction matters — a column
position `col * cw` in a monospace editor — measure with
`text_advance(s, ts)`, which takes the same style and returns the float.
There is no need to measure a long run and divide.

A space measures like any other glyph: `text_advance(" ", ts)` is the gap a
space adds in that style (7.8 at size 13 in the default face), and leading or
trailing spaces count toward `text_width`. Use it for a word gap or an indent
step rather than guessing a fraction of the size.

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

## Draw primitives — fills and strokes

Every fill and stroke comes in up to three shapes: **flat** (packed ints, what
the native takes), **mixed** (flat coordinates and a color record) and
**record** (a rect / center / point record and a color record). `a` is alpha,
0–255, and `width` a stroke width in pixels; both are optional everywhere they
appear, defaulting to `255` and `1` (a `radius` defaults to `0`).

| Primitive | Flat | Mixed | Record |
|---|---|---|---|
| `draw_rect` | `x, y, w, h, r, g, b[, a]` | `x, y, w, h, c[, a]` | `rect, c[, a]` |
| `draw_rect_rounded` | `x, y, w, h, radius, r, g, b[, a]` | `x, y, w, h, radius, c[, a]` | `rect, radius, c[, a]` |
| `draw_rect_outline` | `x, y, w, h, r, g, b[, a[, width]]` | `x, y, w, h, c[, a[, width]]` | `rect, c[, a[, width]]` |
| `draw_rect_rounded_outline` | `x, y, w, h, radius, r, g, b[, a[, width]]` | `x, y, w, h, radius, c[, a[, width]]` | `rect, radius, c[, a[, width]]` |
| `draw_line` | `x1, y1, x2, y2, r, g, b[, a[, width]]` | `x1, y1, x2, y2, c[, a[, width]]` | `p1, p2, c[, a[, width]]` |
| `draw_circle` | `cx, cy, radius, r, g, b[, a]` | `cx, cy, radius, c[, a]` | `center, radius, c[, a]` |
| `draw_circle_outline` | `cx, cy, radius, r, g, b[, a[, width]]` | `cx, cy, radius, c[, a[, width]]` | `center, radius, c[, a[, width]]` |
| `draw_ellipse` | `cx, cy, rx, ry, r, g, b[, a]` | `cx, cy, rx, ry, c[, a]` | `center, rx, ry, c[, a]` |
| `draw_ellipse_outline` | `cx, cy, rx, ry, r, g, b[, a[, width]]` | `cx, cy, rx, ry, c[, a[, width]]` | `center, rx, ry, c[, a[, width]]` |
| `fill_arc` | `cx, cy, r_in, r_out, a0, a1, r, g, b[, a]` | `cx, cy, r_in, r_out, a0, a1, c[, a]` | `center, r_in, r_out, a0, a1, c[, a]` |
| `fill_triangle` | `x1, y1, x2, y2, x3, y3, r, g, b[, a]` | `x1, y1, x2, y2, x3, y3, c[, a]` | `p1, p2, p3, c[, a]` |
| `fill_fan` | `cx, cy, points, r, g, b[, a]` | `cx, cy, points, c[, a]` | `center, points, c[, a]` |
| `fill_poly`, `fill_polygon` | `points, r, g, b[, a]` | | `points, c[, a]` |
| `draw_polyline`, `draw_polygon_outline` | `points, r, g, b[, a[, width]]` | | `points, c[, a[, width]]` |
| `draw_text` | `text, x, y, size, r, g, b[, a]` | `text, x, y, size, c[, a]` | `text, pos, size, c[, a]` |
| `draw_text` (styled) | | `text, x, y, style` | `text, pos, style` |
| `draw_image` | `source, x, y, w, h[, a[, radius]]` | | `source, rect[, a[, radius]]` |

The names in the table are the parameters' own, so any of these calls can be
written with [named arguments](../../../docs/syntax/function-overloading.md#named-arguments),
and the names say which shape is meant:

```petal
let red = #e5484d
let card = rect(20, 20, 200, 120)

draw_rect_outline(x: 0, y: 0, w: 10, h: 4, c: red, a: 255, width: 2)
draw_rect_rounded_outline(rect: card, radius: 8, c: red, width: 2)   // alpha skipped
draw_line(0, 40, 200, 40, c: red, width: 3)
draw_circle(center: {x: 60, y: 60}, radius: 12, c: red, a: 128)
draw_text("Total", x: 24, y: 28, size: 13, c: red)
```

Naming `width` without `a` leaves alpha at its default, which no positional
call can do.

That holds with the coordinates passed positionally too:
`draw_circle_outline(cx, cy, radius, c, width: 2)` and
`draw_ellipse_outline(cx, cy, rx, ry, c, width: 2)` draw at full alpha with a
2px stroke.

A positional call is unaffected by any of this: it means what it always has.
Where two shapes take the same number of arguments (`draw_circle(center,
radius, c, a)` and `draw_circle(cx, cy, radius, c)`), a positional call is
told apart by whether the first differing argument is a number.

## Draw primitives — gradients, shadows, nested clips

Beyond the fills and strokes (`draw_rect`, `draw_rect_rounded`, `draw_line`,
`fill_arc`, `draw_polyline`, `draw_polygon_outline`, …), each of which takes
either a geometry record or flat coordinates, plus a color record:

| Call | Draws |
|---|---|
| `draw_rect_gradient(rect, c0, c1, angle[, a0, a1])` | a linear gradient across the rect |
| `draw_rect_gradient_rounded(rect, radius, c0, c1, angle[, a0, a1])` | the same, with rounded corners |
| `draw_circle_gradient(center, radius, c0, c1[, a0, a1])` | a disc shading center → rim |
| `linear_gradient(rect, stops, angle[, radius])` | 3+ stops, subdivided into bands |
| `draw_shadow(rect, {radius, blur, spread, dx, dy, color, a})` | a CSS box-shadow |
| `draw_shadow(rect, radius, blur, color[, a])` | the positional short form |
| `clip_push(rect[, radius])` / `clip_pop()` | a clip that nests inside the enclosing one (flat: `clip_push(x, y, w, h[, radius])`) |
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

A rounded clip is a *mask*, not a scissor, so the circular avatar is either
`draw_image(src, r, 255, r.w / 2)` or the same image drawn under
`clip_push(r, r.w / 2)`. How far the rounding reaches is the host's business:
Garden cuts shapes and images against the mask and falls back to the bounding
rect for text; a host with no mask of its own draws the square shape rather
than nothing.

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

`draw_area_fill` cuts each segment into `step`-px columns, each with its own
vertical gradient, so the result is one continuous ramp across the area.

`button` and `badge` take `gradient: [c0, c1]` (plus `gradient_angle`) for a
ramp-filled face; `button` also takes `radius` and `elevation`.

`draw_shadow` is one command rather than a stack of translucent rounded rects
because nested translucent rings double-composite and show every seam. Hosts
tessellate it as a single non-overlapping mesh (`petal_ui::tess::shadow_mesh`).

## Layers — offscreen canvases, backdrops and blur

A *layer* is a canvas the size of a rect that a closure draws into, then
composites back at that rect. Every offscreen effect is an operation on such
a canvas: draw into it (`draw_to`), fill it from what is already on screen
(`snapshot_to`), filter it (`blur_canvas`), composite it (`draw_canvas`, with
opacity and an optional destination size).

| Call | Does |
|---|---|
| `layer(rect, body)` / `layer(rect, {a, blur}, body)` | draw `body()` into a canvas at (0, 0) = `rect`'s top-left, optionally blur it, composite at `rect` at opacity `a` |
| `snapshot(rect)` | a canvas holding what is under `rect` right now |
| `draw_backdrop_blur(rect, radius)` | blur what is under `rect`, in place |
| `draw_material(rect[, {kind, radius, tint, a, blur, hairline}])` | the iOS-style translucent material: the backdrop blurred under a tint, clipped to `radius`; `kind` is `"thin"` / `"regular"` / `"thick"` |
| `canvas(w, h)` / `canvas(rect)` | a bare canvas id, for hand-rolled effects |
| `draw_canvas(id, x, y[, a[, w, h]])` / `draw_canvas(id, at[, opts])` (`at` a point or rect, `opts` `{a}`) | composite a canvas, at opacity `a`, scaled to `w`×`h` |

The body of a `layer` draws in canvas-local coordinates, and `draw_to` returns
the target it replaced, so layers nest without a stack anyone has to keep:

```petal
// Overlapping translucent shapes as one object at 50% — drawing each at 50%
// would darken every overlap, since blending is not idempotent.
layer(card, {a: 128}, fn()
  draw_circle({x: 40, y: 40}, 40, c1)
  draw_circle({x: 80, y: 40}, 40, c2)
end)

// A glow: the shape, blurred.
layer(halo, {blur: 12}, fn() draw_circle({x: halo.w / 2, y: halo.h / 2}, halo.w / 2, accent) end)

// A tab bar the content scrolls under. Draw the content first — the
// snapshot reads what is already there.
draw_material(bar, {kind: "regular", hairline: "top"})
```

A host without render targets draws nothing for a canvas, so a helper that has
to stay legible everywhere paints its flat fallback outside the layer:
`draw_material` draws its tint after the blurred snapshot, which on such a host
is the tint alone.
