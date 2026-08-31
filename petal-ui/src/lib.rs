//! petal-ui — the standard interactivity layer shipped with Petal.
//!
//! Embedders that render interactive graphics from Petal scripts share one
//! contract instead of hand-copying it (see `docs/building-apps.md`):
//!
//! - [`input`] (Layer 0): a normalized [`input::InputEvent`] stream, an
//!   [`input::InputState`] that derives level/edge semantics, canonical key
//!   names, [`input::bind_input`] for the per-frame uniforms, and
//!   [`input::register_input`] for the standard input natives.
//! - [`draw`] (Layer 2): the tagged `draw_commands` output-buffer protocol —
//!   [`draw::register_draw`] natives on the script side,
//!   [`draw::take_draw_commands`] decoding into [`draw::DrawCommand`] on the
//!   host side. Hosts implement only rasterization.
//! - [`text`]: the font-metrics registry behind that vocabulary — what a host
//!   publishes about the faces it can render ([`text::bind_font_metrics`] and
//!   friends) so `text_width` measures what will actually be drawn, plus the
//!   optional [`text::FontSource`] a host attaches so a script can name *any*
//!   face the machine has (`font("Helvetica")`) and have it measured on
//!   demand. Its public items are re-exported from [`draw`].
//! - the `ui` Petal module (Layer 1): interaction primitives written in Petal
//!   (`hovered`, `clicked`, `button`, `list_update`, …), a panel-global focus
//!   registry (`focus_state`/`focused`/`focus_set`, `focus_next`/`focus_prev`,
//!   `focus_update`) and the focus-aware `text_field` widget + gated
//!   `list_update` built on it, registered with [`register_prelude`] and
//!   delivered through the module system as an implicit import.
//! - [`tess`]: shared CPU tessellation for the commands a host can't fill in
//!   one go — currently the soft shadow, whose non-overlapping mesh every
//!   host needs and none should re-derive.
//! - [`harness`]: a headless test driver so widget logic is unit-testable
//!   with no renderer attached.
//! - [`scenario`]: declarative, replayable input scripts for that driver
//!   (hand-written or generated from a seed), used by the `petal-ui-run` CLI.
//!
//! The standard owns *semantics* (what `key_pressed` means, what a drag is);
//! the host keeps *policy* (which keys it reserves, when a script ticks,
//! focus routing). The frame contract every host implements:
//!
//! ```text
//! input.event(...)                 // as host events arrive
//! input.begin_frame(dt)            // promote edges for this frame
//! bind_frame_info / bind_input     // uniforms into the Env
//! clear_draw_commands              // defensive
//! env.reset_stack + env.run        // the script draws the whole frame
//! take_draw_commands               // rasterize
//! ```

pub mod draw;
pub mod harness;
pub mod host_data;
pub mod input;
pub mod panel_stubs;
pub mod pending;
pub mod scenario;
pub mod tess;
pub mod text;

/// Version of the petal-ui contract, exposed to scripts as `ui_version()`.
/// Bump when native signatures, binding names, or prelude semantics change
/// incompatibly.
pub const UI_VERSION: i64 = 1;

/// Monotonic counter of *additive* prelude growth, the companion to
/// [`UI_VERSION`]: that one counts incompatible changes (which is why it is
/// still 1 after years of additions), this one increments every time the
/// prelude gains a symbol or an overload, so a host or a script can ask "is
/// this prelude new enough to have X?" with `>= N` instead of calling X and
/// reading the error.
///
/// A host that reports capability by *name* (Garden's `garden --version`
/// derives the export list from [`prelude_source`]) does not need this; it is
/// for the coarse "how new is this?" check.
///
/// Levels:
/// - 1 — the prelude as of the first release (`UI_VERSION` 1).
/// - 2 — `luma(c)`, `contrast_text(bg)`, `text_field_update(fc, id, r, buf)`
///   and the 4-argument `draw_text_field(r, text, has, style)` (2026-08-12).
/// - 3 — the component-library expansion (2026-08-25): host-palette theme
///   resolution ([`input::bind_host_palette`]; `ui_theme()` defaults from the
///   host, semantic tokens + spacing/radius/type scales on `theme`), RectCut
///   layout (`cut_left/right/top/bottom`, `split_h/split_v`, `pad`,
///   `hstack/vstack`, `row/col`), motion (`approach`, `ease_out`,
///   `ease_in_out`), and the widget set: `spinner`/`spinner_glyph`,
///   `progress_bar`, `checkbox`, `toggle`, `radio_group`, `slider`, `tab_bar`,
///   `tooltip`, the modal family (`modal_state` … `modal`), `badge`/`pill`,
///   `card`, `empty_state`, `hint`/`hint_bar`, `splitter_state`/`splitter`,
///   the table family (`table_state`/`table_col_rects`/`table`), `wrap_px`,
///   the load-state family (`load_state` … `draw_load`), and caret editing in
///   `text_field` (`{focus, text, caret, submitted}`, 5-argument
///   `draw_text_field(r, text, has, caret, style)`).
/// - 4 — font objects (2026-08-30): `font(name)` / `font(name, size)` and the
///   decorations `font_size`, `font_weight`, `font_bold`, `font_italic`,
///   `font_spacing`, `font_color`, plus the `fonts()` native listing the
///   families the host can draw. Backed by the optional
///   [`text::FontSource`] a host attaches, which is what lets a script name a
///   face installed on the machine rather than only the host's own.
/// - 5 — gradients, shadows and nested clips (2026-08-30):
///   `draw_rect_gradient` / `draw_rect_gradient_rounded` /
///   `draw_circle_gradient` and the multi-stop `linear_gradient(rect, stops,
///   angle, [radius])`; `draw_shadow(rect, opts)` and its positional form,
///   backed by [`tess::shadow_mesh`]; `clip_push`/`clip_pop` (clips that nest
///   instead of replacing), a trailing `radius` on `clip` and `draw_image`,
///   and the record overloads of `clip`.
pub const PRELUDE_LEVEL: u32 = 5;

/// Name of the Petal-source prelude module: `import ui`.
pub const MODULE_NAME: &str = "ui";

/// The Petal source of the `ui` module.
pub fn prelude_source() -> &'static str {
    include_str!("../prelude/ui.ptl")
}

/// Register the `ui` Petal module and make it an implicit import, so scripts
/// call `button(...)`, `clicked(...)`, `list_update(...)` with zero ceremony
/// (implicit bindings are weak — a script's own declarations shadow them, and
/// an explicit `import ui` is a no-op).
///
/// Hosts with their own implicit imports should instead call
/// `env.register_module(petal_ui::MODULE_NAME, petal_ui::prelude_source())`
/// and compose `set_implicit_imports` themselves — it replaces the whole
/// list.
pub fn register_prelude(env: &mut petal::env::Env) {
    env.register_module(MODULE_NAME, prelude_source());
    env.set_implicit_imports(&[MODULE_NAME]);
}

/// Register everything a typical host wants in one call: the input natives,
/// the draw natives (without the optional canvas ops), and the `ui` module
/// as an implicit import.
pub fn register_all(env: &mut petal::env::Env) {
    input::register_input(env);
    draw::register_draw(env);
    host_data::register_host_data(env);
    register_prelude(env);
}
