# Widgets guide

How the widgets in `import ui` behave and why they are shaped the way they
are: the conventions they share, the theme, RectCut layout, motion, and each
widget in depth. The widgets are written in Petal, in `prelude/widgets.ptl`
(module `ui/widgets`, which `ui` re-exports).

- For one line on every export, see [reference.md](reference.md).
- For draw primitives, text measuring and offscreen layers, see
  [drawing.md](drawing.md).

The worked example showing all of it is `garden/examples/panels/gallery.ptl`:

```
cd core-libs/petal-ui && cargo run --bin petal-ui-run -- \
    ../../garden/examples/panels/gallery.ptl --frames 60
```

Everything is **immediate mode**: the script re-runs top to bottom every
frame, widgets draw when called and return their new value. Retained state
lives in `state` (keyed per call path — two callsites of the same widget never
share internal state). Layers, hit testing, named targets, flow layout and
`ui_measure` are covered in [reference.md](reference.md#layers-and-hit-testing).

## Conventions

1. **Explicit state records.** Widgets with cross-frame state take and return
   a plain record you keep in `state`:
   `state files = list_state()` … `files = list_update(files, …)`.
   Multiple instances coexist because *you* hold the state.
2. **Update/draw split.** Where it pays, an interactive widget comes in three
   forms: `x_update(...)` (logic, no pixels), `draw_x(...)` (pixels, no
   logic), and `x(...)` (both). Use the split forms to keep the input
   handling and bring your own paint.
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
   `petal_ui::input::bind_host_palette` (Garden binds its full palette, so
   widgets paint in Garden's colors with no script-side setup);
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

`font` is deliberately *not* the host's default face, which is monospace in
every host here. A panel that wants monospace chrome (a diff viewer, a data
grid) opts back in with one line, and every widget follows:

```
theme_set({font: "mono"})
```

Text a panel draws itself with the bare `draw_text(s, pos, size, color)` form
is unaffected: that still means the host's own face.

A style record and a theme record are interchangeable — `theme_set` carries
extra keys through, so an app can hang its own names on the theme.

Widget text used to be set in the host's default face, and the widgets' shadows
used to be offset rectangles. They are now the theme's `font` and real
`draw_shadow` commands from the elevation scale. A panel that wants the old
typography back adds `theme_set({font: "mono"})`.

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
`button(r, label, style)` → clicked? — `style: {bg, hover, fg, outline, size,
font, radius, gradient, gradient_angle, elevation, id}`. `id` is the name it
registers under (default: the label), for `ui_target_rect` and
`Headless::click_target`.

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
hover-scoped; page keys gated on `active`). `scroll_keys(offset, total,
visible)` adds a reader's keys for the region with the keyboard: j/k or
down/up a line, space a page, home/end.

### Focus registry
The runtime holds the focus: `has_focus(id)`, `focus_owner()`,
`focus_request(id)` (`nil` clears), `focus_next/prev(ids)`,
`focus_update(ids)` (Tab / Shift+Tab over the ring).
`section_label(label, x, y, active, style)` marks the focused region.

### Text field (caret, selection, clipboard, undo)
`text_field(id, r, buf, style)` → `{text, caret, submitted,
changed, anchor, sel_start, sel_end, scroll, overflow, blurred, cancelled}`.
`blurred` is true on the frame after the field lost the keyboard (the cue to
apply an edit on blur) and `cancelled` when Escape was pressed in it; the
field keeps the focus on Escape, so a caller that reverts drops it. `buf` is a plain
string kept in the caller's `state`; the caret, the selection and the undo
history are widget-internal state, per callsite. `caret`, `anchor`,
`sel_start` and `sel_end` are **character** offsets (what `char_slice` takes),
so non-ASCII text needs no care; `sel_start == sel_end` means no selection.

| Input | Does |
|---|---|
| click / shift+click / drag | focus and place the caret / extend the selection / select |
| a press anywhere else | gives the focus up, unless a higher layer took the press |
| double click / triple click | select the word / everything |
| left, right, home, end | move the caret; with shift, extend the selection |
| alt or ctrl + left/right | by word; cmd + left/right goes to either end |
| backspace, delete | the selection, else one character (alt/ctrl+backspace: back to the word start; cmd+backspace: to the start) |
| cmd+A | select all |
| cmd+C / cmd+X / cmd+V | copy, cut, paste through the host clipboard |
| cmd+Z, cmd+shift+Z or cmd+Y | undo, redo |
| return | sets `submitted` |

Cmd and Ctrl are interchangeable for the shortcuts. Typing replaces the
selection; a run of typing is one undo step, a word at a time. A paste loses
its line breaks. When `buf` is not what the field last returned (the app
cleared or replaced it), the history is dropped. Text wider than the box
scrolls to keep the caret in view and is clipped.

Under Garden a panel does not see Cmd/Ctrl chords unless it claims them, so a
panel with a text field states, near the top of the frame, the ones it wants:
`for k in ["a", "c", "x", "v", "z", "y"] do claim_key(k, "cmd") end`.

Split forms, for an app that draws its own box:
`text_field_update(id, r, buf, style)` is the logic alone and returns the
same record. It reads `{size, font, inset, text_style, pointer, scroll}` from
`style`: the type keys must match what is drawn (`text_style` is a whole style
record, for an app with its own type ramp); `pointer: false` keeps the mouse
out for a frame (the field is under a modal); `scroll: true` turns on the
horizontal scroll, which the caller then honours by drawing the text
`res.scroll` px to the left. `draw_text_field(r, text, has, caret, view,
style)` is the stock look; `view` is the update's result (it reads
`sel_start`, `sel_end`, `scroll`, `overflow`), and the shorter forms without
it draw a caret only. Style adds `selection` (the highlight colour) and
`placeholder` (dim text while empty).

```petal
let res = text_field_update("name", r, buf, {text_style: MY_STYLE, inset: 0})
buf = res.text
for q in text_range_rects(buf, MY_STYLE, res.sel_start, res.sel_end) do
  draw_rect(r.x + q.x, r.y + q.y, q.w, q.h, MY_HIGHLIGHT)
end
draw_text(buf, {x: r.x, y: r.y}, MY_STYLE)
```

### Text area (multi-line)
`text_area(id, r, buf, style)` is `text_field` for text with line breaks,
with the same selection, clipboard, undo and focus rules (the editing keys are
one shared implementation). It returns `{text, caret, changed, anchor,
sel_start, sel_end, scroll, overflow, rows, line_h, content_h}` — no
`submitted`, since Return is a line break.

| Input | Does |
|---|---|
| return | inserts a line break |
| up / down | a row up or down, keeping the column across short rows |
| cmd + up / down | to the start / end of the text |
| home / end, cmd + left / right | to the start / end of the row |
| wheel over the box | scrolls text taller than the box (by rows) |

The text wraps to the box with `text_wrap_rows`; `res.rows` is that wrap and
`res.scroll` how many pixels it is shifted up. A caret that moves, or a text
that changes, is scrolled into view; on a quiet frame the wheel is free to
scroll away from it. A paste keeps its line breaks. Style keys are the
field's plus `line_gap` (pixels added to the face's line height, default 2)
and `select_all` (select the whole text on this frame, for text the next
keystroke should replace).
Split forms: `text_area_update(id, r, buf, style)` and
`draw_text_area(r, res, has, style)`. `text_area_height(text, w, min_lines,
max_lines, style)` sizes a box that grows with its text (3 to 12 rows by
default).

### Clipboard
`clipboard_get()` → the clipboard's text (`""` when it has none);
`clipboard_set(text)` replaces it. A write lands when the frame ends, so it
reads back from the next frame. Line breaks arrive as `\n`. With a host
clipboard attached, the text is refreshed on frames where `v` or `insert`
went down, which is when a paste happens; a host with none keeps the text in
memory, so copies still carry between fields. `text_field` calls both; an app
needs them only for its own copy and paste commands. Host side:
`petal_ui::clipboard` (`set_clipboard_provider`, and `flush_clipboard` after
the run for a host that does not use `FrameCore`).

### Text ranges
For a wrapped editable area, a search highlight or a selection. Offsets are
character offsets throughout.

- `text_wrap_rows(s, style, max_width)` → `[{text, start, end}]`: `text_wrap`
  with each row's range in `s` (`char_slice(s, start, end) == text`). The
  space a soft break lands on and the newline of a hard one fall between one
  row's `end` and the next row's `start`.
- `text_row_of(rows, at)` → the index of the row a caret at offset `at` is on.
- `text_range_rects(s, style, a, b)` → `[{x, y, w, h}]`, one per row that
  characters `[a, b)` touch, relative to the text's top-left. `s` is a string
  (one row per hard newline) or the rows from `text_wrap_rows`. Rows are the
  face's `line_height` apart; a fifth argument sets the row step for text
  drawn at another one.

```petal
let rows = text_wrap_rows(body, style, 320.0)
for q in text_range_rects(rows, style, hit_start, hit_end, 21) do
  draw_rect(x + q.x, y + q.y, q.w, q.h, MARK)
end
for i in range(0, len(rows)) do
  draw_text(rows[i].text, {x: x, y: y + i * 21}, style)
end
```

### Spinner + progress
`spinner(cx, cy, radius, style)` — rotating arc off `time()`;
`spinner_glyph()` — the classic `| / - \` character, cycling on the frame
clock; `progress_bar(r, frac, style)` — `frac` in [0,1], negative for the
indeterminate sweep.

### Checkbox / toggle / radio / slider
`checkbox(r, label, v, style)` → new bool; `toggle(r, v, style)` → new bool
(animated knob); `radio_group(r, labels, selected, style)` → new index (one
row per label, `style.row_h`); `slider(r, value, lo, hi, style)` → new float
(press-to-jump, drag keeps tracking outside `r`). In a loop over a list that
can reorder, key the loop (`for item in items key item.id do`) so each
slider's drag stays with its item rather than with its row position.

### Inspector fields
The controls of a property panel, which document editors used to copy. The
keyboard ones take and return the focus registry and apply an edit only when
it is done (Return, or the field losing the keyboard); Escape drops it.

- `commit_field(id, r, current, style)` → `{text, editing}`: a
  line of text, `text` nil except on the frame it applies.
- `number_field(id, r, value, style)` → `{value, changed}`: drag
  sideways to scrub by `style.step` per pixel (× `style.fine`, 0.1, with
  shift), double-click to type; `places` (2) decimals, optional `min`/`max`,
  `tip` the hover hint (`""` for none). The drag is measured from
  `mouse_x()`, so it scrubs in every host.
- `color_field(id, r, color, style)` → `{color}`: a chip and its
  hex; `color` is nil unless a valid new one was typed. `color_to_hex(c)`,
  `color_from_hex(text)` and `same_color(a, b)` are the helpers it uses.
- `swatches(r, colors, current, style)` → the colour clicked or nil; chips
  are `r.h` square with `style.gap` (4) between, `current` ringed in accent.
- `stepper(r, options, current, style)` → option: ‹ value ›, wrapping;
  `style.labels` names the options.
- `segmented(r, labels, index, style)` → new index.
- `dropdown(r, options, current, style)` → option: a button that opens a
  `context_menu` of `options` under itself, on the popup layer (so it draws
  over later widgets and holds the pointer while open).

In a loop, key the rows (`for p in props key p.name do`) so each field's
edit state stays with its row.

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
active}`.

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
One record for the loading / ready / failed lifecycle of a value:
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
`menu_state`/`menu_item`/`menu_sep`/`menu_open_on_right_click`/
`context_menu` (draw the menu late in the frame; an open menu is modal, so
only code that reads the mouse itself stands down while `menu.open`; see the
comment in widgets.ptl), and
`drag_state`/`drag_update`/`insertion_index(_x)`.
