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
`ui`) for the deprecation period; compile warning `[ui-widget-moved]` names
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

### Done (2026-11-10)

`ui/interaction` split out; the compile warning `[ui-widget-moved]`
(core/src/compiler/moved_widgets.rs, once per name per file, silenced by
`// petal-lint-ignore ui-widget-moved`); bloom's `fields`, `display` and
`panes` modules, `button`'s `fill` / `ink`, focus on the `ui` runtime;
every caller in this repo and in `~/petal-creative` ported; petal-web-canvas
built with `bloom`; worlds-fair re-vendored (it draws its own field and
scrollbar on `ui/interaction` and does not use bloom). An open bloom dialog
or popover keeps the controls drawn into it live (`capture(owner, area)`,
`pointer_free(r)`), and an open bloom menu raises `ui`'s modal floor, so
controls built on `ui.interact` underneath go inert as they did under
`ui`'s menus.

### Left

- [x] **Cheesecake** (2026-11-11): its widget calls are bloom's (cheesecake
  `7246a93`) and it is re-vendored at `0349c2a3`.
- [x] **The website** (2026-11-11, petal-lang.org re-vendored at
  `0349c2a3`). Its DM sample `settings-card.ptl` draws its card, buttons and
  meter with in-file `draw_*` helpers over `draw_rect_rounded` and
  `ui_theme()`, because garden-web is built without bloom (`garden-script`
  turns on `petal/libraries` only for its tests).
- [ ] **Remove the deprecated widgets** from `ui/widgets` once the
  deprecation period ends, with `moved_widgets.rs`. The vitest fixture
  `test/vitest/fixtures/implicit-imports/late_app.ptl` needs another
  colliding `ui` name then.
- [ ] **bloom ignores `ui`'s hit testing.** Its `probe` reads the mouse
  directly, so bloom controls are not `interact` targets: `click_target` and
  `--targets` cannot find them by name, and they ignore `with_layer`,
  `ui_block` and `ui_modal` (examples/ui/layers wraps its buttons in
  `interact` for that reason), and `bloom.dialog` does not set
  `pointer_captured()`. Building `probe` on `interact` would make bloom a
  proper client of `ui`'s interaction layer; it needs the dialog's content
  drawn on the dialog's layer.
- [ ] **Bare names from `import bloom` win over `ui`'s.** Names both export
  (`clicked`, `ease_out`, `ease_in_out`, `theme`, `theme_set`,
  `theme_reset`, `popover_open`, `popover_close`, `select`) silently switch
  to bloom's when a file adds `import bloom`, with no warning; the ports pin
  them as `ui.x`. Either rename bloom's, or have the compiler warn when a
  second plain import changes what a bare name resolves to.
- [ ] **Style options bloom lacks**, which ports had to drop: `menu` /
  `context_menu` colours, `row_h`, `radius` and font; `slider` track and
  knob colours; `progress` track and radius; `spinner` width.
- [ ] **Regenerated reports.** `docs/dev/direct-manipulation-v2-study.{md,json}`
  still describe the old `l10b-ui-panel` goals (re-run `full_study` with
  `DM_STUDY_WRITE`).

## Needs a display

- [ ] **SDL sub-pixel in a live window.** `petal-sdl --subpixel` (with the
  antialiased fills for fractional polygons, ellipses and arcs) is covered by
  the renderer's unit tests on a software surface, but has not been looked
  at in a live or HiDPI window.
