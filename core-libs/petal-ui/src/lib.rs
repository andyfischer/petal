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
//!   (`hovered`, `clicked`, `button`, `list_update`, …), focus and shortcuts
//!   (`shortcut`, `has_focus`, `focus_owner`, `focus_request`,
//!   `focus_next`/`focus_prev`, `focus_update`) and the focus-aware
//!   `text_field` widget + gated
//!   `list_update` built on it, registered with [`register_prelude`] as the
//!   package `ui`, which a script reaches with `import ui`.
//! - [`focus`]: who has the keyboard, decided once a frame by
//!   [`hit::roll`], and which keys the focused widget claims from the app's
//!   shortcuts.
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
//! `harness`, `scenario`, `drag_trace`, `panel_stubs` and the `petal-ui-run`
//! / `petal-ui-propose-edit` binaries are the headless tooling, behind the
//! default-on `tooling` feature. A host that needs only the host contract
//! above can build without it (`default-features = false`).
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
pub mod draw_wire;
#[cfg(feature = "tooling")]
pub mod drag_trace;
pub mod frame_core;
#[cfg(feature = "tooling")]
pub mod harness;
pub mod focus;
pub mod hit;
pub mod host_data;
pub mod input;
#[cfg(feature = "tooling")]
pub mod panel_stubs;
mod params;
pub use params::{interface, raw_declarations, script_for};
pub mod pending;
pub mod popover;
#[cfg(feature = "tooling")]
pub mod scenario;
pub mod subpixel;
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
/// What each level added is in `docs/changelog.md` in this crate; add the
/// new level there when you bump this.
pub const PRELUDE_LEVEL: u32 = 13;

/// Name of the Petal-source prelude module: `import ui`.
pub const MODULE_NAME: &str = "ui";

/// A file of the `ui` package as the petal binary bundles it
/// (`petal::bundled`), which is this crate's `prelude/` directory. Reading the
/// bundled copy rather than an `include_str!` of our own keeps one copy in the
/// binary, and gives a wasm build the comment-stripped text core embeds.
fn bundled_ui_file(path: &str) -> &'static str {
    petal::bundled::package(MODULE_NAME)
        .and_then(|p| p.files.iter().find(|(p, _)| *p == path))
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("the petal binary bundles ui/{path}"))
}

/// The modules the `ui` module is made of, as package-relative names
/// (`prelude/<name>.ptl`), in load order: `ui.ptl` itself is a facade of
/// `pub import`s over the public ones (`draw`, `hit`, `text`, `layout`,
/// `widgets`), and the `_`-prefixed ones are internal to the package. The
/// input half (`ui/input`, [`input_source`]) and `ui/drag` ([`drag_source`])
/// are not among them.
pub const UI_MODULES: [&str; 10] = [
    "_native", "_base", "_style", "_frame", "draw", "hit", "layout", "text", "widgets", MODULE_NAME,
];

/// Each module of [`UI_MODULES`] with its Petal source.
pub fn ui_module_sources() -> [(&'static str, &'static str); 10] {
    UI_MODULES.map(|name| (name, bundled_ui_file(&format!("{name}.ptl"))))
}

/// The Petal source of the `ui` module: every file of [`UI_MODULES`],
/// concatenated in load order — the text that was the single `ui.ptl` before
/// it was split into modules behind a facade, so a scan for its declarations
/// (`pub fn …`, `native fn …`) still sees all of them. It is not one loadable
/// module (each file imports its siblings); [`register_prelude`] registers the
/// files themselves ([`package_modules`]).
///
/// The internal modules mark their shared helpers `pub` so the package's
/// other modules can import them; the facade does not re-export those, and
/// they are all `_`-prefixed. A scan for what `import ui` binds skips names
/// that start with `_`.
pub fn prelude_source() -> &'static str {
    static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SOURCE.get_or_init(|| {
        let mut out = String::new();
        for (_, source) in ui_module_sources() {
            out.push_str(source);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    })
}

/// Name of the input half of the interface: `import ui/input`.
pub const INPUT_MODULE_NAME: &str = "ui/input";

/// The Petal source of the `ui/input` module: the declarations of the input,
/// timing and frame natives, which `ui` re-exports.
pub fn input_source() -> &'static str {
    bundled_ui_file("input.ptl")
}

/// Name of the declarative-dragging module: `import ui/drag`.
pub const DRAG_MODULE_NAME: &str = "ui/drag";

/// The Petal source of the `ui/drag` module: `place`, `draggable`,
/// `drag_session`, the drag-spec constructors and the candidate-set helpers
/// (`docs/drag.md` in this crate). It is written over `ui` and is not
/// re-exported by it: a script that drags writes `import ui/drag`.
pub fn drag_source() -> &'static str {
    bundled_ui_file("drag.ptl")
}

/// Every module of the `ui` package, as `(package-relative name, source)` —
/// what [`petal::env::Env::register_package`] takes.
pub fn package_modules() -> [(&'static str, &'static str); 12] {
    let ui = ui_module_sources();
    std::array::from_fn(|i| match i {
        0 => ("input", input_source()),
        1 => ("drag", drag_source()),
        i => ui[i - 2],
    })
}

/// Register the `ui` interface package in memory: the `native fn`
/// declarations of every petal-ui native plus the widget prelude written in
/// Petal over them. A script reaches all of it with `import ui` — there is
/// no implicit import — and a host that registers only the input natives has
/// its scripts write `import ui/input`. Registering also gives the petal-ui natives already
/// registered in `env` their declared parameter names, so call this after
/// the `register_*` functions (as [`register_all`] does).
///
/// The `petal` binary bundles the same source, so `petal lint` resolves
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
    register_all_with(env, draw::DrawOptions::default());
}

/// [`register_all`] with the draw natives registered by
/// [`draw::register_draw_with`] — `DrawOptions { subpixel: true, .. }` for a
/// host that draws [`draw::SubCommand`]s.
pub fn register_all_with(env: &mut petal::env::Env, options: draw::DrawOptions) {
    input::register_input(env);
    draw::register_draw_with(env, options);
    host_data::register_host_data(env);
    register_prelude(env);
    provide_interface(env);
}
