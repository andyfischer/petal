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

## Painting

| Call | Draws |
|---|---|
| `ts(t, size, color)` / `ts_bold` / `ts_a(t, size, color, a)` | the text style record you **measure and draw with** — one record for `text_width` and `draw_text`, or centered labels land off |
| `text_in(s, r, style[, align])` | text inside a rect, vertically centered; `align` is `"left"`/`"center"`/`"right"` |
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
| `focused(id)` / `focus_set(id)` / `focus_clear()` / `focus_id()` | the focus cell |
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

## Controls

| Call | Returns |
|---|---|
| `switch(r, on[, opts])` | the new bool; the knob springs and stretches toward its travel |
| `checkbox(r, label, on[, opts])` | the new bool; the tick is *drawn in*, stroke by stroke |
| `radio_group(r, labels, index[, opts])` | the new index (`opts.row_h`) |
| `slider(r, value, lo, hi[, opts])` | the new float. Press-to-jump, drag keeps tracking outside; `opts.step` quantizes, and the value floats over the knob while dragging |
| `stepper(r, value, lo, hi, step)` | the new value; the number rolls as it changes |
| `progress(r, frac[, opts])` | — ; `frac` in [0, 1] eases toward its target, negative runs the indeterminate sweep |
| `text_field(r, buf[, opts])` | `{text, focused, submitted, changed, caret}` |

`text_field` handles typing, arrows, home/end, backspace (alt/cmd to word
start), delete, Return, and click-to-place-caret. `opts`: `id` (focus id),
`placeholder`, `size`, `inset`, `radius`. There is no selection model.

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

bloom does not replace `ui`. Layout (`cut_left`, `row`, `col`, `pad`,
`split_h`), lists and tables, the text and color helpers, gradients, layers and
the draw primitives all still come from there, and bloom paints through them.
Mixing the two in one script is expected:

```petal
import bloom

let all = rect(0, 0, screen_width(), screen_height())
let bar = cut_top(all, 34)                  // ui layout
if bloom.button(bar.cut, "Run") then … end  // bloom component
```

The one thing to keep straight is **click semantics**: `ui.button` fires on
press, `bloom.button` on release. Two components of different families
stacked on the same pixel would both see the same gesture.
