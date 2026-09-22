//! Unknown globals: names a program calls or reads that nothing defines.
//!
//! The compiler does not reject an unresolved name. A call to one becomes a
//! static `BuiltinCall` that is resolved against the native table at run time
//! ("Unknown builtin: x"), and a read becomes a deferred `Error` term
//! ("Undefined variable: x"). Both are deliberate: a host registers its natives
//! on its own `Env`, and a line that never runs should not stop the rest of a
//! script. But it means `petal check` used to pass a program that is certain
//! to fail the moment the line runs.
//!
//! [`unresolved_globals`] closes that hole after the compile, by scanning the
//! program's IR for those two term shapes and reporting each name the host
//! does not provide. The host's set is the `Env`'s native table plus a
//! [`HostProfile`]: the core `petal` CLI registers only the core builtins, so
//! it cannot see a host's natives and has to be told which host the script is
//! written for. The default is [`HostProfile::Ui`], the petal-ui set every
//! sample app and panel script draws with.
//!
//! The name lists below are checked against the real registrations by tests in
//! the embedding crates (`petal-ui/tests/host_natives.rs`), which is what keeps
//! them from drifting when a host grows a native.

use std::collections::HashSet;

use crate::diagnostic::Diagnostic;
use crate::program::{Program, TermOp};

/// The natives `petal_ui::register_all` adds on top of the core builtins:
/// input, timing, drawing (canvas ops included) and `host_data`.
pub const PETAL_UI_NATIVES: &[&str] = &[
    // input.rs
    "mouse_x",
    "mouse_y",
    "hovered",
    "mouse_dx",
    "mouse_dy",
    "mouse_down",
    "mouse_pressed",
    "mouse_released",
    "scroll_x",
    "scroll_y",
    "key_down",
    "key_pressed",
    "key_released",
    "mod_shift",
    "mod_ctrl",
    "mod_alt",
    "mod_cmd",
    "drag_active",
    "drag_start_x",
    "drag_start_y",
    "click_count",
    "text_input",
    "grab_mouse",
    "release_mouse",
    "dt",
    "time",
    "frame_count",
    "screen_width",
    "screen_height",
    "ui_version",
    // draw.rs
    "draw_image",
    "clear",
    "draw_rect",
    "draw_rect_rounded",
    "draw_rect_outline",
    "draw_rect_rounded_outline",
    "draw_rect_gradient",
    "draw_rect_gradient_rounded",
    "draw_circle_gradient",
    "draw_shadow",
    "draw_line",
    "draw_polyline",
    "draw_circle",
    "draw_circle_outline",
    "draw_ellipse",
    "draw_ellipse_outline",
    "fill_arc",
    "fill_triangle",
    "fill_poly",
    "fill_polygon",
    "fill_fan",
    "draw_text",
    "clip",
    "clip_none",
    "clip_push",
    "clip_pop",
    "text_width",
    "text_advance",
    "text_metrics",
    "text_wrap",
    "text_ellipsize",
    "text_index_at",
    "font",
    "fonts",
    "create_canvas",
    "draw_to",
    "draw_to_screen",
    "draw_canvas",
    "snapshot_to",
    "blur_canvas",
    // host_data.rs
    "host_data",
];

/// The natives Garden adds for its scripts: the panel host's channels (theme,
/// `emit`/`mutate`/`navigate`, the text-view regions, the panel store, the
/// `query` cache) and the config host's layout builders (`init.ptl`). The two
/// hosts are separate `Env`s, but a checker that accepts the union misses only
/// a config-only native called from a panel, which the run still reports.
///
/// `petal_ui::panel_stubs` registers the panel half as inert stand-ins, so
/// the petal-ui test that checks [`PETAL_UI_NATIVES`] checks this too.
pub const GARDEN_NATIVES: &[&str] = &[
    // garden-script/src/panel.rs (and petal-ui's panel_stubs.rs)
    "panel_theme",
    "palette",
    "emit",
    "mutate",
    "mutate_result",
    "claim_key",
    "request_frame",
    "animating",
    "navigate",
    "nav_arg",
    "navigate_replace",
    "navigate_back",
    "navigate_forward",
    "text_view",
    "edit_view",
    "edit_view_text",
    "edit_view_projection",
    "edit_view_edits",
    "text_view_line_styles",
    "text_view_scroll_to",
    "text_view_wrap",
    // garden-script/src/panel_store.rs
    "panel_store_get",
    "panel_store_set",
    // garden-script/src/query.rs
    "query",
    "invalidate",
    // garden-script/src/native_fns.rs (the config host)
    "editor",
    "process",
    "panel",
    "row",
    "column",
    "layout",
    "color_theme",
    "color_scheme",
];

/// The natives Garden's config host (`init.ptl`, a layout script) registers:
/// the layout builders and the theme setters. That host is its own `Env`, with
/// the core builtins and these alone: no petal-ui natives and no `ui` prelude.
/// Checking such a script as `garden` would resolve `row(children)` against
/// the prelude's 3-argument `row` and warn about a call that runs fine.
pub const GARDEN_CONFIG_NATIVES: &[&str] = &[
    "editor",
    "process",
    "panel",
    "row",
    "column",
    "layout",
    "color_theme",
    "color_scheme",
];

/// The natives petal-desktop-sdl (`integrations/petal-desktop-sdl`) adds on
/// top of the petal-ui set: the example launcher and plain-text file I/O.
pub const SDL_NATIVES: &[&str] = &[
    "example_count",
    "example_name",
    "example_path",
    "launch_script",
    "load_text_file",
    "save_text_file",
    "file_exists",
];

/// Which host a script is written for, and so which natives beyond the `Env`'s
/// own table it may call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HostProfile {
    /// The core builtins only.
    Core,
    /// Core plus the petal-ui set ([`PETAL_UI_NATIVES`]).
    #[default]
    Ui,
    /// Core, petal-ui, and Garden's extras ([`GARDEN_NATIVES`]).
    Garden,
    /// Core, petal-ui, and the desktop SDL runner's extras ([`SDL_NATIVES`]).
    Sdl,
    /// Garden's config host (`init.ptl`, layout scripts): core plus
    /// [`GARDEN_CONFIG_NATIVES`], without petal-ui or its prelude.
    GardenConfig,
}

impl HostProfile {
    /// Parse a `--host` value.
    pub fn from_name(name: &str) -> Option<HostProfile> {
        match name {
            "core" | "none" => Some(HostProfile::Core),
            "ui" | "petal-ui" => Some(HostProfile::Ui),
            "garden" => Some(HostProfile::Garden),
            "sdl" | "desktop" => Some(HostProfile::Sdl),
            "garden-config" => Some(HostProfile::GardenConfig),
            _ => None,
        }
    }

    /// Whether this host imports the petal-ui `ui` prelude implicitly.
    pub fn uses_ui_prelude(self) -> bool {
        !matches!(self, HostProfile::Core | HostProfile::GardenConfig)
    }

    /// The natives this host adds to the core table.
    pub fn natives(self) -> impl Iterator<Item = &'static str> {
        let (ui, garden): (&[&str], &[&str]) = match self {
            HostProfile::Core => (&[], &[]),
            HostProfile::Ui => (PETAL_UI_NATIVES, &[]),
            HostProfile::Garden => (PETAL_UI_NATIVES, GARDEN_NATIVES),
            HostProfile::Sdl => (PETAL_UI_NATIVES, SDL_NATIVES),
            HostProfile::GardenConfig => (&[], GARDEN_CONFIG_NATIVES),
        };
        ui.iter().chain(garden.iter()).copied()
    }
}

/// The names a host provides beyond what the program's own table resolved:
/// the profile's natives plus any the caller names (`--native`).
pub fn host_names(profile: HostProfile, extra: &[String]) -> HashSet<String> {
    profile
        .natives()
        .map(str::to_string)
        .chain(extra.iter().cloned())
        .collect()
}

/// Report every call to, or read of, a global that nothing defines.
///
/// `is_native` answers whether the running `Env` registers a native of that
/// name; `host` is the set the target host adds (see [`host_names`]). A
/// `BuiltinCall` naming neither is a call that fails with "Unknown builtin";
/// an `Undefined variable` error term naming neither is a read that fails. A
/// read of a host native (`let f = draw_rect`) compiles to that same error
/// term under a table that lacks it, which is why reads are filtered by the
/// host set too.
///
/// One diagnostic per source position, in term order.
pub fn unresolved_globals(
    program: &Program,
    is_native: impl Fn(&str) -> bool,
    host: &HashSet<String>,
) -> Vec<Diagnostic> {
    let known = |name: &str| is_native(name) || host.contains(name);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for term in &program.terms {
        let message = match term.op {
            TermOp::BuiltinCall(cid) => {
                let Some(name) = program.get_string_constant(cid) else {
                    continue;
                };
                if known(name) {
                    continue;
                }
                format!(
                    "unknown function `{name}`: nothing by that name is in scope, and it is not \
                     a builtin (running this line fails with \"Unknown builtin: {name}\")"
                )
            }
            TermOp::Error(cid) => {
                let Some(msg) = program.get_string_constant(cid) else {
                    continue;
                };
                let Some(rest) = msg.strip_prefix("Undefined variable: ") else {
                    continue;
                };
                // The compiler appends a hint after an em dash for common
                // slips from other languages (`null`, `elif`, …).
                let name = rest.split(' ').next().unwrap_or(rest);
                if known(name) {
                    continue;
                }
                let hint = rest[name.len()..].trim_start();
                if hint.is_empty() {
                    format!("undefined variable `{name}`")
                } else {
                    format!("undefined variable `{name}` {hint}")
                }
            }
            _ => continue,
        };
        let Some(span) = program.source_map.get(term.id) else {
            continue;
        };
        if seen.insert((
            span.file,
            span.start.line,
            span.start.column,
            message.clone(),
        )) {
            out.push(Diagnostic {
                span: *span,
                message,
            });
        }
    }
    out
}

/// The petal-ui prelude's source (`import ui`), embedded when this crate is
/// built inside the Petal monorepo (see `build.rs`). A petal-ui host makes it
/// an implicit import, so `petal check` must too: without it every widget
/// call (`button(...)`) and every prelude binding (`theme`) would be an
/// unknown global. `None` in a build that cannot see `petal-ui/`.
pub fn ui_prelude_source() -> Option<&'static str> {
    #[cfg(all(petal_ui_prelude, not(target_arch = "wasm32")))]
    {
        Some(include_str!(env!("PETAL_UI_PRELUDE_PATH")))
    }
    #[cfg(not(all(petal_ui_prelude, not(target_arch = "wasm32"))))]
    {
        None
    }
}

/// Name of the petal-ui prelude module (`petal_ui::MODULE_NAME`).
pub const UI_MODULE: &str = "ui";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;

    fn unresolved(src: &str, profile: HostProfile, extra: &[&str]) -> Vec<String> {
        let mut env = Env::new();
        let pid = env.load_program(src).expect("compiles");
        let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        let host = host_names(profile, &extra);
        let program = env.get_program(pid).unwrap();
        unresolved_globals(program, |n| env.has_native(n), &host)
            .into_iter()
            .map(|d| {
                format!(
                    "{}:{} {}",
                    d.span.start.line, d.span.start.column, d.message
                )
            })
            .collect()
    }

    #[test]
    fn unknown_call_and_read_are_reported() {
        let got = unresolved(
            "totally_bogus_fn(1)\nlet y = nope + 1",
            HostProfile::Ui,
            &[],
        );
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            got[0].starts_with("1:1 unknown function `totally_bogus_fn`"),
            "{got:?}"
        );
        assert_eq!(got[1], "2:9 undefined variable `nope`");
    }

    #[test]
    fn builtins_bindings_and_declared_fns_are_known() {
        let src = "fn f(x) sqrt(x) end\nlet g = f\nprint(g(4), len([1]), f(9))";
        assert!(unresolved(src, HostProfile::Core, &[]).is_empty());
    }

    #[test]
    fn host_profile_decides_host_natives() {
        let src = "draw_rect(0, 0, 1, 1, 0, 0, 0)\nlet p = palette";
        assert_eq!(unresolved(src, HostProfile::Core, &[]).len(), 2);
        assert_eq!(unresolved(src, HostProfile::Ui, &[]).len(), 1);
        assert!(unresolved(src, HostProfile::Garden, &[]).is_empty());
        assert!(unresolved(src, HostProfile::Ui, &["palette"]).is_empty());
    }

    #[test]
    fn a_hint_after_the_name_is_kept() {
        let got = unresolved("print(null)", HostProfile::Ui, &[]);
        assert_eq!(
            got,
            ["1:7 undefined variable `null` — use 'nil' for null/empty values in Petal"]
        );
    }

    #[test]
    fn repeated_calls_on_one_line_each_report() {
        let got = unresolved("zz(1)\nzz(2)", HostProfile::Ui, &[]);
        assert_eq!(got.len(), 2, "{got:?}");
    }

    #[test]
    fn host_lists_do_not_shadow_core_builtins() {
        // A host list naming a core builtin would be harmless but would hide
        // a real drift in the petal-ui sync test, which subtracts the core set.
        let env = Env::new();
        for name in PETAL_UI_NATIVES
            .iter()
            .chain(GARDEN_NATIVES)
            .chain(SDL_NATIVES)
        {
            assert!(!env.has_native(name), "{name} is a core builtin");
        }
    }

    #[test]
    fn profile_names_parse() {
        assert_eq!(HostProfile::from_name("ui"), Some(HostProfile::Ui));
        assert_eq!(HostProfile::from_name("garden"), Some(HostProfile::Garden));
        assert_eq!(HostProfile::from_name("sdl"), Some(HostProfile::Sdl));
        assert_eq!(HostProfile::from_name("core"), Some(HostProfile::Core));
        assert_eq!(
            HostProfile::from_name("garden-config"),
            Some(HostProfile::GardenConfig)
        );
        assert_eq!(HostProfile::from_name("x"), None);
    }
}
