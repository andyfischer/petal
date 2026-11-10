//! The `ui` widgets that moved to bloom (docs/tasks/ui-refactor.md, R4).
//!
//! bloom owns the drawn widgets now. The old definitions stay in the `ui`
//! package's `ui/widgets` module for a deprecation period, and a call that
//! reaches one of them from outside the `ui` package gets a warning naming
//! its bloom replacement, once per name per file. `// petal-lint-ignore
//! ui-widget-moved` silences it at a site, as for any compiler warning that
//! carries a rule id. The table goes when `ui/widgets` does.

/// The rule id the warning carries.
pub(crate) const RULE: &str = "ui-widget-moved";

/// The module the deprecated widgets live in.
pub(crate) const MODULE: &str = "ui/widgets";

/// What replaces `name` from [`MODULE`]: the bloom call, written as a call
/// so the message reads as an instruction.
pub(crate) fn replacement(name: &str) -> &'static str {
    match name {
        "button" => "`bloom.button(r, label[, opts])`",
        "checkbox" => "`bloom.checkbox(r, label, on[, opts])`",
        "radio_group" => "`bloom.radio_group(r, labels, index[, opts])`",
        "slider" => "`bloom.slider(r, value, lo, hi[, opts])`",
        "segmented" => "`bloom.segmented(r, labels, index[, opts])`",
        "tooltip" => "`bloom.tooltip(anchor, text[, opts])`",
        "spinner" => "`bloom.spinner(cx, cy, radius[, opts])`",
        "spinner_glyph" => "`bloom.spinner_glyph()`",
        "toggle" => "`bloom.switch(r, on[, opts])`",
        "progress_bar" => "`bloom.progress(r, frac[, opts])`",
        "dropdown" => "`bloom.select(r, options, index[, opts])`",
        "stepper" => "`bloom.cycle(r, options, index[, opts])`",
        "text_field" => "`bloom.text_field(r, buf, {id})`",
        "draw_text_field" => "`bloom.draw_text_field(r, res, focused[, opts])`",
        "text_area" => "`bloom.text_area(r, buf, {id})`",
        "draw_text_area" => "`bloom.draw_text_area(r, res, focused[, opts])`",
        "commit_field" => "`bloom.commit_field(r, current, {id})`",
        "number_field" => "`bloom.number_field(r, value, {id})`",
        "color_field" => "`bloom.color_field(r, color, {id})`",
        "swatches" => "`bloom.swatches(r, colors, current[, opts])`",
        "context_menu" | "menu_state" | "menu_show" | "menu_close"
        | "menu_open_on_right_click" | "menu_rect" | "menu_metrics" => {
            "`bloom.context_menu(area, items)`, or `bloom.menu(id, anchor, items)` with \
             `bloom.menu_open_at` / `bloom.menu_close`"
        }
        "menu_item" | "menu_sep" => {
            "a plain item: a string, or a `{label, disabled, sep}` record, in a bloom menu"
        }
        "popover" => "`bloom.menu(id, anchor, items)` with `bloom.menu_open(id)`",
        "tab_bar" => "`bloom.tabs(r, labels, index[, opts])`",
        "modal" | "modal_state" | "modal_open" | "modal_close" | "modal_blocking"
        | "modal_rect" | "modal_update" | "draw_modal" | "draw_modal_backdrop" => {
            "`bloom.dialog(id, w, h, title)` with `bloom.dialog_open` / `bloom.dialog_close`"
        }
        "badge" => "`bloom.badge(x, y, text[, opts])`",
        "pill" => "`bloom.pill(x, y, text, color[, opts])`",
        "card" => "`bloom.card(r[, opts])`",
        "empty_state" => "`bloom.empty_state(r, title, hint[, opts])`",
        "hint" | "hint_bar" => "`bloom.hint_bar(r, hints[, opts])` with `{key, label}` hints",
        "section_label" => "`bloom.section_label(label, x, y, active[, opts])`",
        "splitter" | "splitter_state" => "`bloom.splitter(r, frac[, opts])`",
        "table" | "table_state" | "table_col_rects" => "`bloom.table(r, cols, rows[, opts])`",
        "draw_scrollbar" => "`bloom.scrollbar(r, count, rows, scroll[, opts])`",
        "draw_load" => "`bloom.load_view(r, ls[, opts])`",
        _ => "its bloom counterpart",
    }
}

/// The warning for a call of `name` from [`MODULE`].
pub(crate) fn message(name: &str) -> String {
    format!(
        "`ui`'s `{name}` is deprecated: bloom owns the widgets now. Use {} (`import bloom`) \
         [{RULE}]",
        replacement(name)
    )
}
