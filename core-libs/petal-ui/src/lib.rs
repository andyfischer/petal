//! petal-ui — the standard interactivity layer shipped with Petal.
//!
//! Embedders that render interactive graphics from Petal scripts share one
//! contract instead of hand-copying it (see `docs/usage/building-apps.md`):
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
//!   `list_update` built on it, registered with [`register_prelude`] as the
//!   package `ui`, which a script reaches with `import ui`.
//! - [`hit`]: layered hit testing and the target registry — what
//!   `interact`, `ui_layer`, `ui_modal` and `pointer_captured` in the prelude
//!   are built on, and what a host reads to click a widget by name.
//!   [`input::bind_input`] rolls it once a frame; a host does nothing else.
//! - [`compose`]: what the drain does to a frame before a host sees it —
//!   draw layers sorted, retained layers (`cached_layer`) and batches
//!   (`draw_rects`) lowered to the classic vocabulary unless the host asked
//!   for them with [`compose::set_host_caps`].
//! - [`tess`]: shared CPU tessellation for the commands a host can't fill in
//!   one go — currently the soft shadow, whose non-overlapping mesh every
//!   host needs and none should re-derive.
//! - [`frame_core`]: the shared per-frame core (env + stack, input, clock,
//!   seed, frame gate, memo counters) every per-frame embedding runs on.
//! - [`harness`]: a headless test driver, a thin wrapper over that core, so
//!   widget logic is unit-testable with no renderer attached.
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
//! if !env.run_needed(stack)        // the frame gate: nothing the last run
//!     { keep last frame's output } //   read has changed → skip the run
//! clear_draw_commands              // defensive
//! env.reset_stack + env.run        // the script draws the whole frame
//! take_draw_commands               // rasterize
//! ```
//!
//! The gate ([`petal::env::Env::run_needed`]) is exact rather than heuristic:
//! the runtime records which bindings a run read and whether it settled, so a
//! host that never reads `time()` stops running while the pointer is still,
//! and one mid-animation keeps ticking. A host that feeds a native from data
//! outside the binding table reports changes with
//! [`petal::env::Env::note_host_data_changed`] (or forces a frame with
//! [`petal::env::Env::invalidate_run`]). [`harness::Headless`] shows the
//! sequence.

pub mod clipboard;
pub mod compose;
pub mod draw;
pub mod frame_core;
pub mod harness;
pub mod hit;
pub mod host_data;
pub mod input;
pub mod panel_stubs;
mod params;
pub use params::{interface, raw_declarations, script_for};
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
/// - 6 — proportional type, real elevation, sRGB tints (2026-08-31): the theme
///   gained a `font` face (defaulting to `"ui"`, so widget text is no longer
///   monospace) and an `elevation_1/2/3` scale; every widget now measures and
///   draws through one style record; `over` / `tint` / `hairline` /
///   `alpha_pct` composite flat tints to opaque colours; `draw_elevation`,
///   `draw_focus_ring`, `draw_scroll_shadow`, `draw_scrim`, `draw_fade`,
///   `draw_area_fill`, `draw_text_along` and the axis-label helpers.
/// - 7 — layers, backdrops and blur (2026-09-01): the offscreen-canvas ops
///   joined the standard draw set and grew `snapshot_to` (copy what is
///   already painted into a canvas), `blur_canvas` (an in-place Gaussian) and
///   opacity/scaling on `draw_canvas`; `draw_to` returns the target it
///   replaced. The prelude builds `layer(rect, opts, body)` (a closure drawn
///   into a canvas and composited back — group opacity, a blurred glow),
///   `snapshot(rect)`, `draw_backdrop_blur` and `draw_material` (the
///   translucent background-sampling surface behind an iOS bar) on those.
/// - 8 — text editing (2026-10-09): `text_field` gained a selection, the
///   clipboard and undo (its result adds `changed`, `anchor`, `sel_start`,
///   `sel_end`, `scroll`, `overflow`; `draw_text_field` a 6-argument form
///   taking that result); `clipboard_get` / `clipboard_set` over the
///   [`clipboard`] channel; `text_wrap_rows` (rows with their character
///   ranges), `text_row_of` and `text_range_rects`. `text_wrap` no longer
///   collapses a run of spaces inside a row.
/// - 9 — layers, hit testing, retained drawing (2026-10-13): `ui_layer` /
///   `with_layer` / `ui_overlay` (a layer orders drawing and the pointer),
///   `ui_modal`, `interact` / `ui_hot` / `ui_block`, `pointer_captured`, and
///   the target registry (`ui_target`, `ui_target_rect`, [`hit::targets`]) —
///   every prelude widget now goes through them and registers itself;
///   `cached_layer` and `draw_rects` ([`compose`]); `draw_rect` as one native
///   in every shape, and a packed-int colour there and in `draw_polyline`;
///   `draw_image` with `tint` / `src` / `slice`, `image_load`, `image_size`;
///   `shadow` / `outline` / `tracking` on a text style; `flow_begin` …
///   `flow_end`, `ui_measure`, `clip_rect`. `clip_none` is a prelude function
///   over the native now (the prelude tracks the clip in force).
pub const PRELUDE_LEVEL: u32 = 9;

/// Name of the Petal-source prelude module: `import ui`.
pub const MODULE_NAME: &str = "ui";

/// The Petal source of the `ui` module.
pub fn prelude_source() -> &'static str {
    include_str!("../prelude/ui.ptl")
}

/// Name of the input half of the interface: `import ui/input`.
pub const INPUT_MODULE_NAME: &str = "ui/input";

/// The Petal source of the `ui/input` module: the declarations of the input,
/// timing and frame natives, which `ui` re-exports.
pub fn input_source() -> &'static str {
    include_str!("../prelude/input.ptl")
}

/// Every module of the `ui` package, as `(package-relative name, source)` —
/// what [`petal::env::Env::register_package`] takes.
pub fn package_modules() -> [(&'static str, &'static str); 2] {
    [("input", input_source()), (MODULE_NAME, prelude_source())]
}

/// Register the `ui` interface package in memory: the `native fn`
/// declarations of every petal-ui native plus the widget prelude written in
/// Petal over them. A script reaches all of it with `import ui` — there is
/// no implicit import — and a host that registers only the input natives has
/// its scripts write `import ui/input`. Registering also gives the petal-ui natives already
/// registered in `env` their declared parameter names, so call this after
/// the `register_*` functions (as [`register_all`] does).
///
/// The `petal` binary bundles the same source, so `petal check` resolves
/// `import ui` with no host; this registration takes precedence over it.
pub fn register_prelude(env: &mut petal::env::Env) {
    env.register_package(
        MODULE_NAME,
        package_modules()
            .into_iter()
            .chain([(petal::package::MANIFEST_FILE, MANIFEST)]),
    )
    .expect("the ui package registers");
}

/// The `ui` package's manifest (`prelude/petal.toml`): its version is the
/// version of the interface a petal-ui host provides.
pub const MANIFEST: &str = include_str!("../prelude/petal.toml");

/// Announce that `env` provides the `ui` interface, at the version of the
/// `ui` package ([`MANIFEST`]): a package whose `petal.toml` says
/// `[requires] ui = ">=…"` (bloom does) then loads here. Call it after
/// [`register_prelude`], in a host that registers the whole interface —
/// input, draw and `host_data` — as [`register_all`] does. A host that
/// registers only part of it (the input natives, for `import ui/input`)
/// does not provide `ui`.
pub fn provide_interface(env: &mut petal::env::Env) {
    env.provides_package(MODULE_NAME)
        .expect("the ui package is registered with its manifest");
}

/// The interface packages a host built on [`register_all`] answers for —
/// what it passes to [`petal::env::Env::verify_interface`], together with
/// the packages declaring its own natives.
pub const INTERFACE_PACKAGES: &[&str] = &[MODULE_NAME];

/// Register everything a typical host wants in one call: the input natives,
/// the draw natives (canvas ops included), and the `ui` package (`import ui`).
pub fn register_all(env: &mut petal::env::Env) {
    input::register_input(env);
    draw::register_draw(env);
    host_data::register_host_data(env);
    register_prelude(env);
    provide_interface(env);
}
