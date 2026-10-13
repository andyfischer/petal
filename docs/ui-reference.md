# `ui` reference

Everything a script gets from `import ui`: every input helper, draw call,
layout helper and widget, with its signature and one line on what it does.
It is the index. For the reasoning behind a widget, worked examples and the
theme tokens, see
[the component library guide](../core-libs/petal-ui/docs/components.md); for
what a host has to do to draw any of this, see
[the host contract](ui-host-contract.md).

A test (`core-libs/petal-ui/tests/reference.rs`) fails if a name the package
exports is missing from this page.

## Conventions

- **Immediate mode.** The whole script runs every frame. A widget is a
  function call that draws and returns what happened: `if button(r, "Save")
  then save() end`.
- **A rect** is `rect(x, y, w, h)`, or any `{x, y, w, h}` record. `rect` gives
  the methods (`r.right()`, `r.center_x()`, `r.inset(4)`).
- **A colour** is a `{r, g, b}` record, which is what a hex literal is
  (`#3c8cff`). `draw_rect`, `draw_polyline`, `draw_rects` and `draw_image`'s
  `tint` also take a packed int, `r * 65536 + g * 256 + b`.
- **Alpha** is a trailing `a`, 0–255, on every draw call. Strokes also take a
  trailing `width`.
- **`style`** is an optional last argument on every widget: a record that
  overrides what the theme says. `{}` is the themed widget, and leaving it
  off is the same.
- **Widget state** that outlives a frame (`list_state()`, `modal_state()`…) is
  a record you keep in `state` and pass in; the widget returns the next one.
- Button 0 is the left mouse button. Key names are lower case: `"space"`,
  `"escape"`, `"a"`.

## Input

Polled, never called back. An *edge* is true on one frame.

| Call | What it is |
|---|---|
| `mouse_x()`, `mouse_y()` | The pointer, in logical pixels. |
| `mouse_dx()`, `mouse_dy()` | Relative motion this frame, also while the pointer is grabbed. |
| `mouse_down(button)` | Held. |
| `mouse_pressed(button)` | Went down this frame. |
| `mouse_released(button)` | Came up this frame. |
| `click_count()` | 1, 2, 3… on the frame of a press: a double click is `mouse_pressed(0) && click_count() == 2`. |
| `drag_active()` | The left button is held and the pointer has moved more than 3 px since the press. |
| `drag_start_x()`, `drag_start_y()` | Where that press landed. |
| `scroll_x()`, `scroll_y()` | Wheel motion this frame. |
| `key_down(key)`, `key_pressed(key)`, `key_released(key)` | Level, and the two edges. |
| `mod_shift()`, `mod_ctrl()`, `mod_alt()`, `mod_cmd()` | Modifiers held. |
| `text_input()` | The text typed this frame, as the OS composed it. Use it for typing; `key_pressed` for commands. |
| `grab_mouse()`, `release_mouse()` | Lock and hide the pointer (a first-person camera), and give it back. |
| `clipboard_get()`, `clipboard_set(text)` | The system clipboard. |
| `dt()` | Seconds since the last frame. |
| `time()` | Seconds on a monotonic clock. |
| `elapsed()` | Seconds since this *call site* first ran: a timer per place it is written. |
| `frame_count()` | Frames so far. Reading it makes a script run every frame. |
| `screen_width()`, `screen_height()` | The drawable, in logical pixels. |
| `ui_version()` | The interface version (1). |
| `host_data(kind, arg)` | Ask the host for data it publishes (host specific). |

## The pointer and a rect

The raw tests look at the pointer and the rect and nothing else. The layered
ones are what widgets use: they know about what is drawn on top, about modal
dialogs and about clipping. See [Layers and hit testing](#layers-and-hit-testing).

| Call | What it is |
|---|---|
| `point_in(px, py, r)` | Whether a point is inside a rect. |
| `hovered(r)` | Raw: the pointer is inside `r`. |
| `clicked(r)` | Raw: `hovered(r) && mouse_pressed(0)`. |
| `interact(r, id?)` | Layered: `{hover, down, pressed, released, click}`. Registers `r`. |
| `ui_hot(r, id?)` | Layered: just the hover. Registers `r`. |
| `ui_block(r, id?)` | Registers `r` and asks nothing: a panel's background. |
| `pointer_captured()` | The UI has the pointer: gate world clicks with it. |
| `pointer_captured(layer)` | Something on a layer above `layer` has it. |

`interact` fields:

| Field | True when |
|---|---|
| `hover` | The pointer is over the rect and nothing above has the pointer. |
| `down` | The left button is held and went down inside the rect. Stays true if the pointer slides off. |
| `pressed` | The left button went down this frame, over the rect. |
| `released` | The left button came up this frame, over the rect. |
| `click` | Released over the rect, having been pressed inside it. |

## Layers and hit testing

Immediate mode has no z order: a button drawn early cannot know a panel drawn
later covers it. A layer is how a script says so.

| Call | What it does |
|---|---|
| `ui_layer()` | The current layer. A frame starts on 0. |
| `ui_layer(n)` | Everything from here on is on layer `n`. Returns the layer it replaced. |
| `with_layer(n, body)` | Runs `body()` on layer `n`, goes back, returns what `body` returned. |
| `ui_overlay(n, body)` | The same with every clip lifted: a popup opened from inside a scrolled list. |
| `ui_modal()` | Everything below the current layer is inert, wherever the pointer is. Call it every frame the dialog is open. |
| `UI_LAYER_BASE`, `UI_LAYER_POPUP`, `UI_LAYER_MODAL`, `UI_LAYER_TOOLTIP` | The layers the built-in widgets use: 0, 1000, 2000, 3000. An app's panels go in between. |

A layer orders two things:

- **Drawing.** The host draws lower layers first, and within a layer in the
  order drawn. A tooltip drawn in the middle of a frame on a high layer still
  comes out on top.
- **The pointer.** A rect is hot only if nothing registered on a higher layer
  is under the pointer and no higher layer is modal. Rects on the *same* layer
  do not block each other.

```petal
// A panel over the world. Its background takes the pointer; the world only
// gets a click the UI did not want.
with_layer(10, fn()
  ui_block(panel)
  if button(close_r, "Close") then open = false end
end)
if mouse_pressed(0) && !pointer_captured() then
  select_unit_at(mouse_x(), mouse_y())
end
```

**How it is resolved, and the one gap.** "What is above me under the pointer"
is answered from the rects registered *last frame* (complete, one frame old)
and from the rects already found hot *this frame* (exact, but only what has
run so far). So:

- A covered widget is inert from the frame after its cover appears, and from
  the same frame if the cover's code runs first.
- `pointer_captured()` called after the UI has drawn is exact on the very
  frame a panel opens under the pointer. No click leaks to the world.
- The gap: a widget on a lower layer that runs *before* a higher one that
  first appears this frame sees one frame of `hover`, and a `pressed` if the
  press lands on exactly that frame. `click` has no such gap, because by the
  release the cover is a frame old. Use `click` for anything that must not
  fire through a panel.

The clip in force counts: a row scrolled half out of its list is not hot over
the part that is clipped away.

## Named targets

Every registered rect can carry an id, which a script, a test or an agent can
look up instead of re-deriving a layout.

| Call | What it does |
|---|---|
| `interact(r, id)`, `ui_hot(r, id)`, `ui_block(r, id)` | Register `r` under `id` (a string or an int). |
| `ui_target(id, r)` | Name a rect without making it block the pointer: a drop zone, a region a tutorial points at. |
| `ui_target_rect(id)` | The rect registered under `id` on the last frame, or `nil`. |

The built-in widgets register themselves: `button`, `checkbox` and each tab of
a `tab_bar` under their label, each row of a `radio_group` under its label, a
`text_field` under its focus id, a `table` header under its column label, a
`modal` under its title. `style.id` replaces the default where a widget takes
a style (`button(r, "OK", {id: "confirm-ok"})`).

```petal
// A tutorial highlights another module's button without knowing its layout.
let r = ui_target_rect("End turn")
if r != nil then draw_focus_ring(r, 4) end
```

A host reads the same registry; see [the host contract](ui-host-contract.md#the-target-registry).

## Drawing

Each shape comes flat (`draw_line(x1, y1, x2, y2, r, g, b)`), with records
(`draw_line(p1, p2, c)`), and mixed (`draw_line(x1, y1, x2, y2, c)`). All take
a trailing `a`; strokes a trailing `width`. Arguments can be named.

| Call | What it draws |
|---|---|
| `clear(r, g, b)` | Fill the drawable. |
| `draw_rect(rect, c, a?)`, `draw_rect(x, y, w, h, c, a?)` | A filled rect. |
| `draw_rect_rounded(rect, radius, c, a?)` | With rounded corners. |
| `draw_rect_outline(rect, c, a?, width?)` | A stroked rect. |
| `draw_rect_rounded_outline(rect, radius, c, a?, width?)` | Stroked and rounded. |
| `draw_rects(rects, palette?, dx?, dy?)` | Many filled rects in one call. See [Retained drawing](#retained-drawing). |
| `draw_line(p1, p2, c, a?, width?)` | A line. |
| `draw_polyline(points, c, a?, width?)` | One open stroked path. Translucent joins do not darken. |
| `draw_polygon_outline(points, c, a?, width?)` | A closed stroked path. |
| `draw_circle(center, radius, c, a?)`, `draw_circle_outline(center, radius, c, a?, width?)` | A disc, a ring. |
| `draw_ellipse(center, rx, ry, c, a?)`, `draw_ellipse_outline(center, rx, ry, c, a?, width?)` | An ellipse. |
| `fill_arc(center, r_in, r_out, a0, a1, c, a?)` | A ring segment. Angles in radians, clockwise from +x. |
| `fill_triangle(p1, p2, p3, c, a?)` | A triangle. |
| `fill_poly(points, c, a?)` | A convex polygon. |
| `fill_polygon(points, c, a?)` | Any simple polygon, concave included. |
| `fill_fan(center, points, c, a?)` | A triangle fan from a centre. |
| `draw_rect_gradient(rect, c0, c1, angle)`, `draw_rect_gradient_rounded(rect, radius, c0, c1, angle)` | A two-stop linear gradient. A stop may carry its own `a`. |
| `draw_circle_gradient(center, radius, c0, c1)` | A radial gradient, centre to rim. |
| `linear_gradient(rect, stops, angle, radius?)` | Any number of evenly spaced stops. |
| `GRAD_RIGHT`, `GRAD_DOWN`, `GRAD_LEFT`, `GRAD_UP` | The four axis-aligned angles. |
| `draw_shadow(rect, opts)` | A box shadow: `{radius, blur, spread, dx, dy, color, a}`. |
| `elevation(level)`, `draw_elevation(r, radius, level, style?)` | The theme's shadow scale, 1–3. Draw before the surface. |
| `draw_focus_ring(r, radius, style?)` | An accent glow around a rect. |
| `draw_scroll_shadow(header_rect, frac, style?)` | The shadow a sticky header casts on scrolled content. |
| `draw_scrim(r, style?)` | A band fading to nothing, for text over an image. |
| `draw_fade(r, style?)` | Dissolves content at the edge of a scroll region. |
| `draw_area_fill(points, baseline_y, color, style?)` | The faded fill under a line chart. |

### Images

| Call | What it does |
|---|---|
| `draw_image(source, rect, a?, radius?)`, `draw_image(source, x, y, w, h, a?, radius?)` | A host-resolved bitmap scaled into the destination. |
| `draw_image(source, rect, opts)` | With `opts`: `{a, radius, tint, src, slice}`. |
| `image_load(source)` | Asks the host to have the image ready. True once it is. |
| `image_size(source)` | `{w, h}` in pixels, or `nil` while the host does not have it. |

| `opts` field | Meaning |
|---|---|
| `tint` | A colour the image's pixels are multiplied by: one white sprite, recoloured per use. |
| `src` | `{x, y, w, h}` in the image's own pixels: one cell of an atlas. |
| `slice` | Nine-slice insets in the image's own pixels, one number or `{l, t, r, b}`. Corners are drawn unscaled, edges stretch one way, the middle both. |

```petal
draw_image("ui/atlas.png", r, {src: rect(32, 0, 16, 16), tint: #ffcc00})
draw_image("ui/frame.png", panel, {slice: 6})
```

A host that lacks one of these draws the image without it.

### Clipping

| Call | What it does |
|---|---|
| `clip_push(rect, radius?)`, `clip_pop()` | Clip inside the clip already in force, and put it back. Use these in anything reusable. |
| `clip(rect, radius?)`, `clip_none()` | Replace the clip, and clear it. |
| `clip_rect()` | The visible area the clip in force leaves, or `nil`. |

### Offscreen layers

| Call | What it does |
|---|---|
| `layer(r, opts?, body)` | Draws `body()` into a canvas the size of `r` and composites it. `opts`: `{a, blur}`. Group opacity, glows. |
| `canvas(w, h)`, `create_canvas(w, h)` | A transparent offscreen canvas, good for this frame. |
| `draw_to(id)`, `draw_to_screen()` | Redirect drawing. Returns the target replaced. |
| `draw_canvas(id, at, opts?)` | Composite a canvas at a point or scaled into a rect. |
| `snapshot(r)`, `snapshot_to(id, x, y)` | Copy what is already painted under `r`. |
| `blur_canvas(id, radius)` | Gaussian blur, in place. |
| `draw_backdrop_blur(r, radius)` | Blur what is under `r`. |
| `draw_material(r, opts?)` | The translucent blurred bar content scrolls under. |

## Retained drawing

A frame's commands live one frame. For something that changes rarely and
costs a lot to say, say it once.

| Call | What it does |
|---|---|
| `cached_layer(key, rect, body)` | Runs `body()` only when `key` or the rect's size changes; on every other frame the host redraws what it drew last time, at the rect's current position. |
| `draw_rects(rects, palette?, dx?, dy?)` | One command for many rects. |

```petal
// A minimap of a few hundred cells, emitted once per map version.
cached_layer(map_version, minimap_rect, fn()
  draw_rects(cells, PALETTE)          // (0, 0) is the layer's top-left
end)
```

`cached_layer`:

- `key` is any value, compared by content: a version counter, a record of the
  inputs.
- The body draws in the layer's own coordinates, and the result is clipped to
  the rect's size. Moving the rect does not run the body.
- The body only draws. It does not run on most frames, so an `interact` or a
  `state` inside it would come and go.
- Layers nest. The body must not open an offscreen `layer` or use `clip` (the
  replace form); `clip_push` is fine.
- Each call site is one layer, and in a loop one per iteration, like `state`.
- A layer that is not drawn for a frame is let go and recorded again when it
  comes back.

`draw_rects`: `rects` is five numbers per rect, `x, y, w, h, c`, as a flat
list, an `f64_array`, or a list of `[x, y, w, h, c]` lists. With a `palette`
(a list of colours, which may carry an `a`) `c` is an index into it. Without
one `c` is the colour: a packed int, or in the list forms a colour record.
`dx`, `dy` move the whole batch, so a list can be built once in its own
coordinates and drawn anywhere.

## Text

| Call | What it does |
|---|---|
| `draw_text(text, pos, style)`, `draw_text(text, x, y, style)` | Draw a run. `y` is the top of the line. |
| `draw_text(text, x, y, size, c, a?)` | The same in the host's default face. |
| `draw_text_center(text, cx, y, style)`, `draw_text_right(text, right_x, y, style)` | Centred on, or ending at, an x. |
| `draw_text_along(points, s, style)` | A run laid along a polyline. |
| `text_width(text, style)` | Width in pixels. Measure the record you draw with. |
| `text_advance(text, style)` | The same, unrounded. |
| `text_metrics(style)` | `{baseline, descent, line_height, cap_height, x_height}` for a face and size. |
| `text_wrap(text, style, max_width)` | A list of lines that fit. |
| `text_wrap_rows(s, style, max_width)` | The lines with their character ranges, for carets and selections. |
| `text_row_of(rows, at)` | Which wrapped row a character index is on. |
| `text_range_rects(s, style, a, b, line_h?)` | The rects that cover characters `a..b`: a selection highlight. |
| `text_ellipsize(text, style, max_width, where?)` | Trimmed to fit with an ellipsis. `where` is `"tail"` or `"head"`. |
| `text_index_at(text, style, dx)` | The character index at a pixel offset: click to caret. |
| `ellipsize(s, avail_px, size)`, `ellipsize_tail(s, avail_px, size)` | Trim to a pixel width keeping the start, or the end (a file path). |
| `truncate_head(s, max_chars)`, `truncate_tail(s, max_chars)` | Trim to a character count. |
| `wrap(s, max_chars)`, `wrap_px(s, avail_px, size)` | Word wrap by characters, or by pixels. |
| `preview(text, max_chars, max_lines)` | `{lines, truncated}`: the first few wrapped lines of a long string. |
| `fit_parts(parts, avail_px, size)`, `fit_parts_n(parts, avail_px, size)` | Join as many leading segments as fit; `_n` also says how many. |
| `draw_axis_labels(r, ticks, style?)`, `draw_axis_labels_y(r, ticks, style?)` | Chart tick labels that drop or stagger instead of overlapping. |

A **text style** is a record. Every field is optional.

| Field | Meaning |
|---|---|
| `size` | Pixel size. |
| `color` | A colour record; its `a` is the run's alpha. |
| `font` | A face: a family name, a host role (`"ui"`, `"mono"`), or a fallback list. |
| `weight` | 400 regular, 600 semibold, 700 bold. |
| `italic` | `true` for italic. |
| `spacing` or `tracking` | Extra pixels between letters. Measured as well as drawn. |
| `shadow` | `true`, or `{dx, dy, color, a}`: one copy under the text, offset (1, 1) and black at alpha 160 by default. |
| `outline` | `true`, a colour, or `{color, width, a}`: a ring around the glyphs, 1 px and black by default. |

```petal
let title = {size: 22, color: #ffffff, tracking: 2, outline: #000000}
draw_text("ROUND 3", 40, 20, title)
```

`shadow` and `outline` fade with the text's own alpha.

**Font objects** build a style from a face:

| Call | What it does |
|---|---|
| `font(name)`, `font(name, size)` | A style record naming a face the host can draw. |
| `font_size(f, size)`, `font_weight(f, w)`, `font_bold(f)`, `font_italic(f)`, `font_spacing(f, px)`, `font_color(f, c)` | A copy with one thing changed. |
| `fonts()` | The families the host can draw. |

## Colour

| Call | What it does |
|---|---|
| `mix(a, b, t)`, `lerp_color(a, b, t)` | Blend two colours. |
| `luma(c)` | Perceived brightness, 0–255. |
| `contrast_text(bg)` | Black or white, whichever reads on `bg`. |
| `over(bg, fg, a)` | `fg` at alpha `a` over `bg`, as an opaque colour. An int `a` is 0–255, a float 0.0–1.0. |
| `tint(bg, fg, pct)` | The same with a percentage. |
| `hairline(bg, ink)` | A separator colour for a surface. |
| `alpha_pct(p)` | A percentage as a 0–255 alpha. |

## Theme

| Call | What it does |
|---|---|
| `theme` | The built-in dark theme, as a record: colours, spacing, radii, type sizes, elevations. |
| `ui_theme()` | The theme in force: an explicit `theme_set`, else the host's palette, else `theme`. |
| `theme_set(t)`, `theme_reset()` | Set and clear the explicit theme. |
| `theme_from_palette(p)` | Project a host palette onto the theme's keys. |

## Layout

Rects in, rects out. Nothing here draws.

| Call | What it returns |
|---|---|
| `rect(x, y, w, h)` | A rect. |
| `cut_left(r, px)`, `cut_right(r, px)`, `cut_top(r, px)`, `cut_bottom(r, px)` | `{cut, rest}`: a slice off one edge, and what is left. |
| `split_h(r, frac, gap?)`, `split_v(r, frac, gap?)` | `{a, b}`: two rects side by side, or stacked. |
| `pad(r, n)`, `pad(r, h, v)`, `pad(r, l, t, rt, b)` | `r` inset. |
| `hstack(r, count, gap?)`, `vstack(r, count, gap?)` | A list of `count` equal rects, 8 px apart unless said. |
| `row(r, widths, gap)`, `col(r, heights, gap)` | A list of rects of given sizes: an int is fixed pixels, a float is a weight over what is left. |

### Flow layout

A cursor that hands out rects one after another, so a panel is written without
`y = y + 17` at every line.

| Call | What it does |
|---|---|
| `flow_begin(r, opts?)` | Start a flow in `r`. `opts`: `{pad, gap, axis}`; `axis` is `"y"` (default) or `"x"`. |
| `flow_next(size)` | The next rect: `size` long, and as wide as the region. |
| `flow_next(w, h)` | The next rect, exactly that size. |
| `flow_space(n)` | Leave `n` px, on top of the gap. |
| `flow_rest()` | What is left of the region: a list under a toolbar. |
| `flow_used()` | `{w, h}` taken so far, padding included. |
| `flow_end()` | Close the innermost flow and return `flow_used()` for it. |

```petal
flow_begin(panel, {pad: 8, gap: 4})
draw_text("Settings", flow_next(20), title)
vol = slider(flow_next(18), vol, 0, 1)
flow_begin(flow_next(24), {axis: "x", gap: 6})   // a row of buttons
if button(flow_next(80), "Apply") then apply() end
if button(flow_next(80), "Cancel") then close() end
flow_end()
let used = flow_end()
```

A flow is not stopped at the end of its region. The rects go on past it, and
`flow_end()` says how far they went, which for a region inside a scroll view
is the content height.

### Measuring

| Call | What it does |
|---|---|
| `ui_measure(body)` | Runs `body()` without drawing, registering or hovering anything, and returns what it returns. |
| `ui_measuring()` | True inside one. |

One function is both the measuring pass and the drawing pass, so the two
cannot disagree:

```petal
fn rows(r)
  flow_begin(r, {gap: 4})
  for it in items do row(flow_next(22), it) end
  flow_end()
end
let content = ui_measure(fn() rows(rect(0, 0, list.w, 0)) end)
// scroll by content.h, then call rows(...) again to draw
```

The body does run: `state` it writes is written.

## Motion

| Call | What it does |
|---|---|
| `approach(cur, target, rate)` | Move toward a target exponentially, `rate` in 1/seconds. Lands exactly, so a settled value stops redrawing. |
| `ease_out(t)`, `ease_in_out(t)` | Easing curves over 0–1. |

## Widgets

Every widget takes an optional trailing `style`.

### Buttons and controls

| Call | Returns |
|---|---|
| `button(r, label, style?)` | True on the frame it is pressed. `style`: `{bg, hover, fg, outline, size, font, radius, gradient, elevation, id}`. |
| `checkbox(r, label, value, style?)` | The new value. |
| `toggle(r, value, style?)` | The new value: an animated switch. |
| `radio_group(r, labels, selected, style?)` | The new selected index. One row per label. |
| `slider(r, value, lo, hi, style?)` | The new value, a float. `style.key` keys the drag by identity in a reordering list. |
| `tab_bar(r, labels, active, style?)` | The new active index. |
| `splitter_state(frac?)`, `splitter(sp, r, style?)` | A draggable divider. Returns `{state, a, b, divider}`: the next state, the two panes, and the divider's rect. |

### Feedback

| Call | What it does |
|---|---|
| `progress_bar(r, frac, style?)` | A bar filled to `frac` (0–1). |
| `spinner(cx, cy, radius, style?)` | A rotating arc. |
| `spinner_glyph()` | One character of a text spinner. |
| `tooltip(anchor, text, style?)` | Shows `text` after the pointer rests on `anchor`. Drawn on the tooltip layer, so it can be called anywhere in the frame. |
| `badge(x, y, text, style?)`, `pill(x, y, text, color, style?)` | A small label chip, plain or tinted. Returns the rect it drew. |
| `card(r, style?)` | A raised surface. Returns the padded content rect. |
| `empty_state(r, title, hint_text, style?)` | Centred "nothing here" copy. |
| `hint(key, label)`, `hint_bar(r, hints, style?)` | The keyboard-hint strip along the bottom of a pane. |
| `section_label(label, x, y, active, style?)` | A heading that shows which region has the keyboard. |

### Lists, scrolling and tables

| Call | What it does |
|---|---|
| `list_state()` | `{selected, scroll}`, to keep in `state`. |
| `list_update(lst, item_count, visible_rows, r, active?)` | Arrow keys, page keys, click to select, wheel to scroll. Returns the next state. |
| `list_row_rect(r, visible_rows, i, scroll)` | Where row `i` is. |
| `ensure_visible(scroll, selected, visible_rows)`, `ensure_visible_px(scroll, row_off, row_h, view_h)` | The scroll that keeps a row in view, by rows or by pixels. |
| `scroll_update(offset, total_lines, visible_lines, r, active?)` | Wheel and page keys for a region with no selection. Returns the next offset. |
| `draw_scrollbar(r, count, rows, scroll, style?)` | A proportional thumb on the right edge. |
| `table_state()`, `table(tb, r, cols, rows_data, style?)` | A sortable table with a header. Returns the next state. |
| `table_col_rects(r, cols)` | The column rects a table would use. |

### Text entry and focus

| Call | What it does |
|---|---|
| `focus_state()` | The record that says which widget has the keyboard. |
| `focused(fc, id)`, `focus_set(fc, id)`, `focus_clear(fc)` | Ask and change. |
| `focus_next(fc, ids)`, `focus_prev(fc, ids)`, `focus_update(fc, ids)` | Move through a ring of ids; `focus_update` does Tab and Shift+Tab. |
| `text_field(fc, id, r, buf, style?)` | A single-line editor with a caret, selection, clipboard and undo. Returns `{focus, text, caret, submitted, changed, …}`. |
| `text_field_update(fc, id, r, buf, style?)`, `draw_text_field(r, text, has, …)` | The two halves, for a field with its own chrome. |

### Menus, dialogs and drag and drop

| Call | What it does |
|---|---|
| `menu_state()`, `menu_item(label, enabled?)`, `menu_sep()` | The pieces of a context menu. |
| `menu_show(m, x, y, tag)`, `menu_open_on_right_click(m, r, tag)`, `menu_close(m)` | Open and close it. |
| `context_menu(m, items, style?)` | Draws an open menu and resolves a choice: `{menu, index, label, tag, rect}`. Modal while open. |
| `menu_blocking(m)`, `menu_rect(m, items, style?)`, `menu_metrics(style?)` | Whether it is open, where it will be, and the numbers it is laid out with. |
| `modal_state()`, `modal_open(m)`, `modal_close(m)` | A dialog's state. |
| `modal(m, w, h, title, style?)` | Backdrop, chrome, Escape and click-outside. Returns `{modal, open, rect, content}`; draw the body into `content` when `open`. Everything drawn before it is inert while it is open. |
| `modal_blocking(m)`, `modal_rect(w, h)`, `modal_update(m, r)`, `draw_modal_backdrop(style?)`, `draw_modal(r, title, style?)` | The pieces, for a dialog with its own layout. |
| `drag_state()`, `drag_update(ds, item_id, r)`, `dragging(ds, item_id)`, `drag_cancel(ds)` | Drag an item; `drag_update` reports the drop. |
| `insertion_index(rects, y)`, `insertion_index_x(rects, x)` | Where a drop belongs among rows, or columns. |

### Async

| Call | What it does |
|---|---|
| `load_state()`, `load_begin(ls, key)`, `load_ok(ls, value)`, `load_fail(ls, msg)` | A load's state, set by hand. |
| `load_poll(ls, key, pv)` | One step: restart on a new key, absorb a pending value. |
| `load_loading(ls)`, `load_ready(ls)`, `load_failed(ls)` | Ask. |
| `draw_load(r, ls, style?)` | Draws the spinner or the error; true when the content is ready to draw. |

## Testing a UI

`petal_ui::harness::Headless` runs a script with no window. Feed it input,
run frames, and read state, draw commands and the target registry:

```rust
let mut ui = Headless::with_ui("state n = 0\nif button(rect(8, 8, 80, 24), \"Go\") then n = n + 1 end")?;
ui.frame()?;
ui.click_target("Go")?;                 // by name, not by coordinates
assert_eq!(ui.state_int("n"), Some(1));
```

`ui.targets()` lists every registered rect. `petal-ui-run` does the same from
the command line; see [headless-ui-run.md](dev/headless-ui-run.md).
