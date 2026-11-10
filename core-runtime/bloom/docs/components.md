# bloom — component reference

Every export, what it returns, and the options it takes. The runnable version
of this document is [`examples/ui/bloom-gallery`](../../../examples/ui/bloom-gallery/).

Conventions used throughout:

- **Rects** are `rect(x, y, w, h)` (the built-in `Rect`) or any `{x, y, w, h}`
  record. **Colors** are `{r, g, b}` records; hex literals like `#3c8cff` work.
- **A component returns its value**, on the frame it changes: `button` returns
  true when clicked, `switch(r, on)` returns the new bool. There are no
  callbacks and no event objects.
- **The last argument is always an optional `opts` record.** Any key it omits
  comes from the theme, so `{}` (or the shorter arity) is the themed component.
- **Animation is internal.** Every component holds its own animators in
  call-path-keyed `state`. Two callsites never share them; a component inside a
  `for` loop gets one animator per iteration.
- **Every export is type-annotated**, so `petal lint` catches a wrong argument
  at the callsite rather than at the draw call. Note that a rect parameter is
  `record`, not `Rect`, precisely because of the first convention above: both
  spellings are accepted, and `Rect` would reject the plain-record one.

## Theme

`bloom.theme()` is the live token record: the host palette (whatever
`ui_theme()` resolves to — Garden's editor colors, an SDL app's, the built-in
dark default), plus bloom's own tokens, plus your overrides.

```petal
bloom.theme_set({accent: #7c5cff, radius: 10, speed: 1.4})
bloom.theme_reset()
```

Colors come from the host: `bg`, `panel`, `surface`, `overlay`, `text`,
`muted`, `dim`, `accent`, `success`, `warn`, `danger`, `outline`, `border`,
`hover`, `selection`, the `space_*` / `radius_*` / `font_*` scales and
`elevation_1/2/3`. bloom adds:

| Token | Default | Role |
|---|---|---|
| `speed` | 1.0 | multiplies every duration — `theme_set({speed: 0})` freezes the UI for a pixel diff |
| `hover_rate` / `press_rate` | 18 / 34 | how fast the interaction fades run |
| `open_dur` | 0.16 | menu / popover entrance |
| `press_scale` | 0.97 | how far a face squashes when pressed |
| `radius` / `radius_sm` / `radius_lg` / `radius_pill` | 7 / 4 / 12 / 999 | corner scale |
| `pad_x` / `pad_y` / `gap` / `row_h` | 12 / 7 / 8 / 30 | rhythm |
| `hover_pct` / `press_pct` / `ring_pct` / `border_pct` | 8 / 14 / 55 / 14 | ink strengths, in percent |

Helpers: `tone(t, c, amt)` (lighten toward `text` / darken toward `bg`),
`ink_on(t, c)`, `variant_fill/ink/solid(t, name)`, `dur(t, secs)`,
`rate(t, r)`.

Every label bloom draws is placed by [text-layout](../../text-layout/), which
asks the host where a run's ink actually lands rather than assuming it is
`size` px tall starting at `y`. That is why a bloom label is centered on the
line it claims to be on, and why a component's own text needs no arithmetic
from the caller. Reach past bloom for it (`import text_layout`) when a panel
draws text of its own.

## Painting

| Call | Draws |
|---|---|
| `ts(t, size, color)` / `ts_bold` / `ts_a(t, size, color, a)` | the text style record you **measure and draw with** — one record for `text_width` and `draw_text`, or centered labels land off |
| `text_in(s, r, style[, align])` | one line inside a rect, its **cap height** centered vertically; `align` is `"left"`/`"center"`/`"right"`. Returns the run's width |
| `text_block_in(s, r, style[, opts])` | text that may be several lines: wrapped to `r`, elided if it still does not fit. `opts` is [text-layout](../../text-layout/)'s (`lines`, `lead`, `align`, `valign`). Returns the layout record, whose `h` sizes a panel to its text |
| `text_cut(s, style, w)` | `s` shortened to `w` px with a trailing "…" |
| `text_extent(s, style, w)` | `{w, h}` — the size `s` takes in a box `w` px wide |
| `surface(t, r, radius, level)` | a raised panel: shadow at `level` 1–3, face, hairline |
| `stroke(t, r, radius, color[, width, inner])` | a rounded outline (there is no rounded-outline primitive) |
| `wash(t, base, hot, active)` | the composited hover/press tint over a known surface |
| `focus_ring(t, r, radius, amount)` | the accent bloomed *outside* a control, so focus never resizes it |
| `divider(t, r, axis)` / `hair(t)` | a hairline, and the color one uses |
| `icon(name, r, color[, w])` | one of the 22 glyphs |

Icon names (`bloom.ICONS`): `check` `close` `plus` `minus` `chevron_down`
`chevron_up` `chevron_right` `chevron_left` `arrow_right` `search` `menu`
`more` `dot` `circle` `play` `pause` `folder` `file` `warning` `info` `star`
`trash`. An unknown name draws a dot, so a typo is visible in place.

## Motion

The core the components are built from; use it directly for anything they do
not cover.

| Call | Returns |
|---|---|
| `ease_to(target[, rate])` | a value easing toward `target` (rate is 1/seconds; ~18 lands in 110 ms). **The first frame snaps**, so nothing fades in from nowhere on load or hot reload |
| `ease_flag(on[, rate])` | 0 → 1 as `on` flips — the hover/press primitive |
| `spring(target[, {stiffness, damping, velocity}])` | a real second-order spring: overshoots, settles *exactly* |
| `enter(on, dur)` | linear 0 → 1 while `on`, back down while not — reaches its ends in a known time, for staged entrances |
| `impulse(trigger[, dur])` | 1.0 on the frame `trigger` goes true, decaying to 0 — ripples, flashes |
| `shake(trigger, px)` | a damped horizontal shake, for rejected input |
| `age()` | seconds since this callsite first ran |
| `stagger(i, step, dur)` | item `i`'s progress through a cascading entrance |
| `pulse(period)` / `wave(period)` | clock-driven, so callsites sharing a period stay in phase |
| `ease_in/out/in_out/back/elastic(t)` | curves, `t` clamped to [0, 1] |
| `lerp_rect(a, b, t)`, `scale_rect(r, s)`, `offset_rect(r, dx, dy)` | rect interpolation |

Everything except `pulse` / `wave` integrates `dt()`, which the headless
harness pins — so a bloom UI settles identically in a trace and on screen.

## Interaction

`probe(r[, enabled])` is the unit every component is built on:

```petal
let p = bloom.probe(r)
// p.hover  p.down  p.press  p.click  p.hot (0→1 eased)  p.active  p.rect  p.enabled
```

`click` means **press and release inside**: pressing a control and sliding off
it cancels. `probe_over(r)` is the same but ignores input capture — for an
overlay's own contents.

| Call | Does |
|---|---|
| `hovering(r)` / `clicked(r)` | the one-bool shorthands |
| `drag(r)` | `{active, x, y, dx, dy, start_x, start_y}`; keeps tracking outside `r` |
| `capture(owner)` / `is_captured()` / `capture_release()` | an overlay claims the pointer for a frame; every `probe` goes inert while a claim is live |
| `focusable(id)` | announce a control in draw order (= tab order) and report whether it has focus |
| `focused(id)` / `focus_set(id)` / `focus_clear()` / `focus_id()` | `ui`'s runtime focus: `has_focus` / `focus_request` / `focus_owner` |
| `key(k)` / `chord(k, mods)` | a key press, ignored while an overlay holds input; `mods` is `"cmd"`, `"ctrl"`, `"alt"`, `"shift"`, or `"cmdctrl"` |

Tab and Shift+Tab walk the ring the previous frame built — the only complete
ring that exists when the key arrives — so focus moves on the frame after the
keypress.

## Buttons

```petal
bloom.button(r, label[, opts]) -> clicked?
```

| `opts` | |
|---|---|
| `variant` | `"default"` (outlined), `"primary"`, `"danger"`, `"success"`, `"warn"`, `"ghost"` (wash only), `"quiet"` |
| `icon` / `icon_end` | an icon name before / after the label |
| `size`, `radius`, `pill`, `align` | `align: "left"` for a rail item |
| `disabled` | inert and dimmed — and it *fades* out, rather than snapping |
| `loading` | a spinner instead of the label |
| `id` | makes it focusable: Tab reaches it, Return/Space fires it (with a flash, since there is no pointer to press) |
| `ripple` | `false` to switch the click ripple off |

| Other | |
|---|---|
| `icon_button(r, name[, opts])` | circular icon-only button |
| `segmented(r, labels, index[, opts])` → new index | the thumb **springs** between cells |
| `chip(x, y, label[, opts])` → its rect, and `chip_width(label, opts)` to place one first |
| `link(x, y, label[, opts])` → clicked? | underline grows from the left on hover |
| `spinner(cx, cy, radius, color)` | the arc, for your own controls |

### Your own colours

`button`'s `opts` also take `fill` (the face colour, in place of the variant's;
a solid face) and `ink` (the label and icon colour; defaults to a colour that
reads on `fill`).

## Controls

| Call | Returns |
|---|---|
| `switch(r, on[, opts])` | the new bool; the knob springs and stretches toward its travel |
| `checkbox(r, label, on[, opts])` | the new bool; the tick is *drawn in*, stroke by stroke |
| `radio_group(r, labels, index[, opts])` | the new index (`opts.row_h`) |
| `slider(r, value, lo, hi[, opts])` | the new float. Press-to-jump, drag keeps tracking outside; `opts.step` quantizes, and the value floats over the knob while dragging |
| `stepper(r, value, lo, hi, step)` | the new value; the number rolls as it changes |
| `progress(r, frac[, opts])` | — ; `frac` in [0, 1] eases toward its target, negative runs the indeterminate sweep |

The text field is in [Fields](#fields).

## Fields

The editing is `ui`'s: `ui.text_field_update` and `ui.text_area_update` hold
the caret, the selection, the clipboard and undo, and own the keyboard through
the runtime focus (`has_focus` / `focus_request`). bloom paints them and puts
each one in its Tab ring, so a bloom field and any `ui` code beside it share
one keyboard owner.

Every field takes its focus id as **`opts.id`**. Without one it falls back to
its placeholder, then to `"field"` — fine for a panel's one search box, wrong
for two fields. An int id works.

| Call | Returns |
|---|---|
| `text_field(r, buf[, opts])` | `{text, caret, submitted, changed, anchor, sel_start, sel_end, scroll, overflow, blurred, cancelled, focused}` |
| `text_area(r, buf[, opts])` | `{text, caret, changed, anchor, sel_start, sel_end, scroll, overflow, rows, line_h, content_h, focused}` — no `submitted`; Return is a line break |
| `draw_text_field(r, res, focused[, opts])` | `res` — the paint half, for a caller that runs `ui.text_field_update` itself |
| `draw_text_area(r, res, focused[, opts])` | `res` — the paint half of `text_area` over `ui.text_area_update` |

```petal
state name = ""
let res = bloom.text_field(r, name, {id: "name", placeholder: "Name"})
name = res.text
if res.submitted then … end
```

Mouse: a click focuses and places the caret, shift+click extends the
selection, a drag selects, a double click selects a word and a triple click
everything; a press elsewhere gives the focus up. Keys: the arrows (alt/ctrl
by word, cmd to either end, shift to select), backspace / delete (alt/ctrl by
word, cmd to the start), cmd+A/C/X/V, cmd+Z and cmd+shift+Z / cmd+Y, Return
(`submitted`) and Escape (`cancelled`; the field keeps the focus). `blurred`
is true on the frame after the field lost the keyboard. A text area adds
up/down by row, cmd+up/down to either end, and wheel scrolling.

| `opts` | |
|---|---|
| `id` | the focus id |
| `placeholder` | drawn dim while the text is empty |
| `size`, `color` / `text_style` | the run's style — `text_style` whole, or one built from `size` and `color` |
| `inset` | text inset (`pad_x` for a field, 8 for an area) |
| `radius`, `bg`, `focus`, `caret`, `selection` | the box's look |
| `scroll` | `text_field` only, default true: the text scrolls to keep the caret in view |
| `pointer` | false keeps the mouse out for this frame (it is out anyway while an overlay holds input) |
| `line_gap` | `text_area` only, extra px between rows (2) |

When you drive `ui.text_field_update` yourself, give it the same
`text_style` and `inset` you pass `draw_text_field`, or the caret lands off
the glyphs.

## Inspector fields

The controls of a property inspector. They apply only when the edit is done:
the result holds the new value on the frame it is committed (Return, or the
field losing the keyboard) and the old one, or nil, before. Escape abandons
the edit and gives the focus up. Each keeps its edit in per-callsite `state`,
so in a loop key it by the row (`for p in props key p.name do … end`).

| Call | Returns |
|---|---|
| `commit_field(r, current[, opts])` | `{text, editing}` — `text` is nil except on the frame the edit applies. `opts` is `text_field`'s plus `start` (take the keyboard as soon as it shows: a rename opened by a double-click) |
| `number_field(r, value[, opts])` | `{value, changed}`. Drag sideways to scrub (shift for fine), double-click or Tab to type. `opts`: `id`, `step` (1.0 per px), `places` (2), `min`, `max`, `fine` (0.1), `tip` (the hover hint; `""` for none), `size`, `color`, `bg`, `radius` |
| `color_field(r, color[, opts])` | `{color}` — nil unless a valid new colour was typed (`#rrggbb` / `#rrggbbaa`). `opts` is `commit_field`'s plus `chip_radius`, `outline` |
| `swatches(r, colors, current[, opts])` | the colour clicked (on release), or nil. Chips are `r.h` square; `current` is ringed. `opts`: `gap` (4), `radius`, `ring`, `hover` |

`ui` keeps `color_to_hex`, `color_from_hex` and `same_color` for colour text
of your own.

## Menus

One menu is open at a time, program-wide, and the library holds that fact.

| Call | Returns |
|---|---|
| `menu(id, anchor, items[, opts])` | the index chosen this frame, else -1. Safe to call every frame; does nothing while closed |
| `dropdown(r, label, items[, opts])` | a button that opens a menu under itself → chosen index or -1 |
| `select(r, options, index[, opts])` | the new index |
| `menu_bar(r, menus[, opts])` | `{menu, item}`, both -1 for nothing. Hovering along the bar switches menus once one is open |
| `context_menu(area, items[, opts])` | right-click anywhere in `area` → chosen index or -1 |
| `menu_open(id)` / `menu_open_at(id, x, y)` / `menu_close()` / `menu_is_open(id)` / `menu_any_open()` / `menu_size(items, opts)` | the plumbing |

An item is a string or a record: `{label, hint, icon, disabled, sep, danger}`.
`sep: true` draws a divider and cannot be chosen. Menus flip up and pull left
rather than draw off-screen, handle up/down/Return/Escape, close on an outside
click, and **capture input** while open — so the panel underneath needs no
guard of its own.

## Display

Components that show rather than take input. They were `ui`'s; the calls and
return values are unchanged, and each `opts` takes the keys `ui`'s style record
did.

| Call | Returns / draws |
|---|---|
| `badge(x, y, text[, opts])` | its rect, so a row of them flows. `opts`: `size`, `bg`, `fg`, `gradient: [c0, c1]`, `gradient_angle` |
| `pill(x, y, text, color[, opts])` | its rect. The status chip: `color` tints the fill and the text. `opts`: `size`, `on` (the surface the tint is flattened against) |
| `card(r[, opts])` | the padded content rect. Shadow, face, hairline. `opts`: `radius`, `pad`, `bg`, `outline`, `shadow: false`, `elevation` (1) |
| `empty_state(r, title, hint[, opts])` | — ; centred dim copy for an empty region; `hint` may be `""`. `opts`: `size`, `fg` |
| `hint(key, label)` | `{key, label}` |
| `hint_bar(r, hints[, opts])` | — ; key caps and labels along a bottom edge. `opts`: `bg`, `rule`, `size`, `fg`, `cap`, `cap_fg` |
| `section_label(label, x, y, active[, opts])` | the run's width. Accent with an underline that grows in when `active` (pass `has_focus(id)`). `opts`: `active`, `inactive`, `size` |
| `scrollbar(r, count, rows, scroll[, opts])` | — ; a thumb on `r`'s right edge, only when `count > rows`; it brightens while the pointer is over `r`. `opts`: `track`, `thumb`, `width` (4) |
| `load_view(r, ls[, opts])` | true when `ls` (a `ui` `load_state` record) is ready — draw the value then. Otherwise a spinner while loading, the error while failed, a hint while idle. `opts`: `radius`, `error_title`, `idle_title` |
| `spinner_glyph()` | `"|"`, `"/"`, `"-"` or `"\\"` on the frame clock, for a line of text |

`spinner(cx, cy, radius[, color | opts])` (from bloom/button) takes a colour or
an `{color}` record, and the theme accent without either.

## Panes

| Call | Returns |
|---|---|
| `tabs(r, labels, index[, opts])` | the new index. Click (release inside) switches; the underline springs to the new tab. `opts`: `size`, `gap`, `fg`, `fg_active`, `underline`, `hover`, `rule`, `on` |
| `cycle(r, options, index[, opts])` | the new index. ‹ value ›: the arrows step and wrap, a click on the value steps forward. `opts`: `labels` (shown instead of `str(option)`), `size`, `bg`, `hover`, `fg`, `radius`, `arrow_w` (22). (`stepper` is the numeric one.) |
| `splitter(r, frac[, opts])` | `{frac, a, b, divider, dragging}`. Pass your fraction in, store `frac`; draw into `a` (left / top) and `b`. The drag is per callsite and keeps tracking off the band. `opts`: `axis` (`"x"` side by side, the default, or `"y"`; `"h"` / `"v"` alias them), `min_a` / `min_b` (px, 80), `min` / `max` (fraction bounds, 0.02 / 0.98), `band` (9), `gap` (4), `line`, `active` |
| `table(r, cols, rows[, opts])` | `{selected, scroll, sort_col, sort_asc, changed, clicked}`. `cols` are `{label, w}` (`w` ≥ 1 px, 0 < w < 1 a weight, missing an equal share); `rows` a list of row lists. Selection (keys, a press on a row, the wheel) and the header sort request live per callsite; the caller sorts its own rows. `opts`: `row_h` (26), `head_h` (28), `size`, `fg`, `head_bg`, `selection`, `zebra`, `rule`, `on`, `active` (keyboard on), and `selected` / `sort_col` / `sort_asc` to set those from the caller every frame |
| `table_columns(r, cols)` | the column rects `table` uses, for drawing cells of your own |

The table stands still while an open bloom menu holds the pointer.

## Overlay layering: `overlays()`

A menu's popup belongs above everything, but the call that opens it sits where
its button is — usually early in the frame. bloom keeps a **late layer** for
that: menus and tooltips hand their painting to a closure, and

```petal
bloom.overlays()      // the last line of your frame
```

runs them. An app that never calls it still gets its overlays — they are
painted at the first bloom overlay call of the next frame, which is where an
immediate-mode library would have drawn them anyway — so this is an
improvement to opt into, never a way to lose a menu. `defer_paint(fn() … end)`
puts your own painting on the same layer.

Dialogs, popovers and toasts paint immediately, because the app draws *into*
them right after the call; put those calls late in the frame yourself, as the
gallery does.

## Overlays

| Call | Does |
|---|---|
| `tooltip(anchor, text[, opts])` | fades in after the pointer rests for `opts.delay` (0.4 s). Call it late in the frame |
| `toast(text[, kind])` | raise a toast from anywhere: `"info"`, `"success"`, `"warn"`, `"danger"` |
| `toasts(r[, opts])` | draw and expire the stack (`life`, `width`, `height`, `corner`) |
| `toast_count()` / `toasts_clear()` | the queue |
| `dialog(id, w, h, title[, opts])` | `{open, content, closed}` — draw into `content` while open. Backdrop fade, pop-in, Escape / backdrop / close-button dismissal, input captured |
| `dialog_open(id)` / `dialog_close()` / `dialog_is_open(id)` | |
| `popover(id, anchor, w, h)` | `{open, content}` — a floating card you fill yourself |
| `popover_open(id)` / `popover_close()` / `popover_is_open(id)` | |
| `banner(r, text, kind, show)` | an inline status strip that slides in; returns whether it is visible |
| `skeleton(r[, {radius, phase}])` | the shimmering placeholder for data in flight |

## Composing with the `ui` prelude

bloom owns the widgets; `ui` keeps the primitives and the interaction logic
under them. Layout (`cut_left`, `row`, `col`, `pad`, `split_h`), list and
scroll state (`list_update`, `scroll_update`), the editing logic behind a
field (`text_field_update`, `text_area_update`), the keyboard focus
(`has_focus`, `focus_request`, `shortcut`), drag and drop, the load state,
gradients, layers and the draw primitives all still come from there, and
bloom paints through them. Mixing the two in one script is expected:

```petal
import bloom

let all = rect(0, 0, screen_width(), screen_height())
let bar = cut_top(all, 34)                  // ui layout
if bloom.button(bar.cut, "Run") then … end  // bloom component
```

bloom's focus *is* `ui`'s: `bloom.focused(id)` is `has_focus(id)`, and a
field built on `text_field_update` and a bloom control share one keyboard
owner. bloom adds the draw-order Tab ring on top.

`ui`'s own drawn widgets (`ui.button`, `ui.text_field`, `ui.context_menu`,
…) are deprecated: each call warns `[ui-widget-moved]` with its bloom
replacement, and they go once the deprecation period ends. The table of
replacements is in [docs/tasks/ui-refactor.md](../../../docs/tasks/ui-refactor.md).
They click on press where bloom clicks on release, so two components of
different families stacked on the same pixel would both see the same
gesture.
