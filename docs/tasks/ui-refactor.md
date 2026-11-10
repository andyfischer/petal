# Reorganizing petal-ui: what is left

Status: 2026-11-10. The reorganization this tracked is done: `ui.ptl` is
split into modules behind an unchanged facade (R1), the headless tooling is
`core-libs/petal-ui-headless` and Garden's panel stubs are
`garden_script::headless` (R2), and the core-feature tests are
`test/ui-corpus/` (R3). See `docs/skills/project-organization.md` for where
things live now and `docs/dev/performance.md` ("Text wrap: native vs Petal")
for the text-wrap measurement.

Decided 2026-11-09: module-qualified names stay as they are
(`ui/widgets::button`), and `hit::roll`, `focus` and `popover` stay in Rust.

## R4: bloom owns the widgets

Decided 2026-11-09. `ui` ends at primitives plus interaction; every drawn
widget comes from bloom. Callers are **ported onto bloom's API** (bloom's
semantics: click on release, per-callsite state, overlays capture input), not
moved to a verbatim copy of the old widgets.

### What stays in `ui`

The parts of `ui/widgets` that draw nothing move to a new module
`ui/interaction`, re-exported by the facade and not deprecated:

| Group | Names |
|---|---|
| Focus (the Rust focus runtime) | `has_focus`, `focus_owner`, `focus_request`, `shortcut`, `focus_next`, `focus_prev`, `focus_update` |
| Lists and scrolling | `list_state`, `list_row_rect`, `list_update`, `ensure_visible`, `ensure_visible_px`, `scroll_update`, `scroll_keys` |
| Text editing (no pixels) | `text_field_update`, `text_area_update`, `text_area_height` |
| Popover runtime | `popover_open`, `popover_close`, `popover_shown`, `popover_owner` |
| Drag and drop | `drag_state`, `drag_update`, `drag_cancel`, `dragging`, `insertion_index`, `insertion_index_x` |
| Load state (data, no pixels) | `load_state`, `load_loading`, `load_ready`, `load_failed`, `load_begin`, `load_ok`, `load_fail`, `load_poll` |
| Colour text | `color_to_hex`, `color_from_hex`, `same_color` |

### What moves to bloom

Everything else in `ui/widgets` is deprecated in `ui` and has a bloom
replacement. The old definitions stay in `ui/widgets` (still re-exported by
`ui`) for the deprecation period; `petal lint` rule `ui-widget-moved` names
the replacement at every call.

| `ui` (deprecated) | bloom |
|---|---|
| `button(r, label[, style])` | `button(r, label[, opts])`; `opts.fill` / `opts.ink` for a caller's own colours |
| `checkbox`, `radio_group`, `slider`, `segmented`, `tooltip`, `spinner` | same names |
| `toggle(r, on)` | `switch(r, on)` |
| `progress_bar(r, frac)` | `progress(r, frac)` |
| `spinner_glyph()` | `spinner_glyph()` |
| `dropdown(r, options, current)` | `select(r, options, index)` |
| `stepper(r, options, current)` (cycles options) | `cycle(r, options, index)` (new); bloom's own `stepper` is numeric |
| `text_field(id, r, buf)`, `draw_text_field` | `text_field(r, buf, {id})`, `draw_text_field`, rebuilt on `ui.text_field_update` (selection, clipboard, undo) |
| `text_area(id, r, buf)`, `draw_text_area` | `text_area(r, buf, {id})`, `draw_text_area` (new, on `ui.text_area_update`) |
| `commit_field`, `number_field`, `color_field`, `swatches` | same names (new), `id` in `opts` |
| `context_menu(m, items)` + `menu_state`, `menu_show`, `menu_close`, `menu_open_on_right_click`, `menu_item`, `menu_sep`, `menu_rect`, `menu_metrics` | `context_menu(area, items)` / `menu(id, anchor, items)` + `menu_open_at` / `menu_close`; items are strings or `{label, disabled, sep, …}` records |
| `popover(id, anchor, items)` | `menu(id, anchor, items)` + `menu_open(id)` |
| `tab_bar(r, labels, active)` | `tabs(r, labels, index)` (new) |
| `modal`, `modal_state`, `modal_open`, `modal_close`, `modal_blocking`, `modal_rect`, `modal_update`, `draw_modal`, `draw_modal_backdrop` | `dialog(id, w, h, title)` + `dialog_open` / `dialog_close` / `dialog_is_open` |
| `badge`, `pill`, `card`, `empty_state`, `hint`, `hint_bar`, `section_label` | same names (new) |
| `splitter_state`, `splitter` | `splitter(r, frac[, opts])` → `{frac, a, b}` (new) |
| `table_state`, `table_col_rects`, `table` | `table(r, cols, rows[, opts])` (new, per-callsite state) |
| `draw_scrollbar` | `scrollbar(r, count, rows, scroll)` (new) |
| `draw_load` | `load_view(r, ls)` (new) |

### One focus system

bloom's focus cell is rebuilt on `ui`'s runtime focus: `focusable`,
`focused`, `focus_set`, `focus_clear` and `focus_id` read and write
`has_focus` / `focus_request` / `focus_owner`, and bloom keeps its draw-order
Tab ring. A panel mixing `ui.text_field_update` and bloom controls has one
keyboard owner.

### Hosts

bloom is an opt-in cargo feature. Every host whose scripts use widgets has to
carry it: petal-web-canvas gains the `bloom` feature (the playground's
examples use widgets); petal-web-html and garden-web stay without it.

### Steps

- [ ] P1 `ui`: split `ui/widgets` into `ui/interaction` (kept) and the
  deprecated widgets; lint rule `ui-widget-moved`.
- [ ] P2 bloom: focus on the `ui` runtime; `text_field` on
  `text_field_update`; the new components; `opts.fill` / `opts.ink`;
  docs/components.md.
- [ ] P3 Callers in this repo: `examples/`, `garden/`, `integrations/`,
  `test/`, `core-libs/`, plus the golden and scenario tests they feed.
- [ ] P4 petal-web-canvas with bloom; the docs (`ui` and bloom READMEs,
  project-organization skill).
- [ ] P5 `~/petal-creative`. Cheesecake is not migrated: it gets the lint
  warnings and moves during the deprecation period.
- [ ] Later: remove the deprecated widgets from `ui/widgets`.

## Needs a display

- [ ] **SDL sub-pixel in a live window.** `petal-sdl --subpixel` (with the
  antialiased fills for fractional polygons, ellipses and arcs) is covered by
  the renderer's unit tests on a software surface, but has not been looked
  at in a live or HiDPI window.
