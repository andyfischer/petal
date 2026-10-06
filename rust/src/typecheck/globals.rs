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
use crate::native_fn::NativeSignature;
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

/// The parameter names of the petal-ui natives, for named arguments: each
/// native's name and one spec per call form
/// ([`NativeSignature::parse`]; the core builtins' twin is
/// [`crate::builtins::BUILTIN_PARAMS`]).
///
/// It lives here, beside [`PETAL_UI_NATIVES`], for the same reason that list
/// does: `petal check` has to answer for a native it never registers. But
/// this table is not a copy to keep in sync — petal-ui declares its natives'
/// parameters *from* it as it registers them (`petal_ui::params`), so what the
/// checker accepts is what the host binds.
///
/// The colour channels and alpha are `r`, `g`, `b`, `a`, as the `ui` prelude's
/// overloads spell them, so a call reads the same whether it lands on a
/// prelude `fn` or on the bare native. The text natives take their style as a
/// record or as a bare size with an optional face name; each shape is its own
/// form, told apart by the names used (`style:` or `size:`).
pub const PETAL_UI_NATIVE_PARAMS: &[(&str, &[&str])] = &[
    // input.rs
    ("mouse_x", &[""]),
    ("mouse_y", &[""]),
    ("hovered", &["rect"]),
    ("mouse_dx", &[""]),
    ("mouse_dy", &[""]),
    ("mouse_down", &["button"]),
    ("mouse_pressed", &["button"]),
    ("mouse_released", &["button"]),
    ("scroll_x", &[""]),
    ("scroll_y", &[""]),
    ("key_down", &["key"]),
    ("key_pressed", &["key"]),
    ("key_released", &["key"]),
    ("mod_shift", &[""]),
    ("mod_ctrl", &[""]),
    ("mod_alt", &[""]),
    ("mod_cmd", &[""]),
    ("drag_active", &[""]),
    ("drag_start_x", &[""]),
    ("drag_start_y", &[""]),
    ("click_count", &[""]),
    ("text_input", &[""]),
    ("grab_mouse", &[""]),
    ("release_mouse", &[""]),
    ("dt", &[""]),
    ("time", &[""]),
    ("frame_count", &[""]),
    ("screen_width", &[""]),
    ("screen_height", &[""]),
    ("ui_version", &[""]),
    // draw.rs
    ("draw_image", &["source, x, y, w, h, a?, radius?"]),
    ("clear", &["r, g, b"]),
    ("draw_rect", &["x, y, w, h, r, g, b, a?"]),
    ("draw_rect_rounded", &["x, y, w, h, radius, r, g, b, a?"]),
    ("draw_rect_outline", &["x, y, w, h, r, g, b, a?, width?"]),
    (
        "draw_rect_rounded_outline",
        &["x, y, w, h, radius, r, g, b, a?, width?"],
    ),
    (
        "draw_rect_gradient",
        &["x, y, w, h, r0, g0, b0, a0, r1, g1, b1, a1, angle"],
    ),
    (
        "draw_rect_gradient_rounded",
        &["x, y, w, h, radius, r0, g0, b0, a0, r1, g1, b1, a1, angle"],
    ),
    (
        "draw_circle_gradient",
        &["cx, cy, radius, r0, g0, b0, a0, r1, g1, b1, a1"],
    ),
    (
        "draw_shadow",
        &["x, y, w, h, radius, blur, spread, dx, dy, r, g, b, a?"],
    ),
    ("draw_line", &["x1, y1, x2, y2, r, g, b, a?, width?"]),
    ("draw_polyline", &["points, r, g, b, a?, width?"]),
    ("draw_circle", &["cx, cy, radius, r, g, b, a?"]),
    ("draw_circle_outline", &["cx, cy, radius, r, g, b, a?, width?"]),
    ("draw_ellipse", &["cx, cy, rx, ry, r, g, b, a?"]),
    ("draw_ellipse_outline", &["cx, cy, rx, ry, r, g, b, a?, width?"]),
    ("fill_arc", &["cx, cy, r_in, r_out, a0, a1, r, g, b, a?"]),
    ("fill_triangle", &["x1, y1, x2, y2, x3, y3, r, g, b, a?"]),
    ("fill_poly", &["points, r, g, b, a?"]),
    ("fill_polygon", &["points, r, g, b, a?"]),
    ("fill_fan", &["cx, cy, points, r, g, b, a?"]),
    (
        "draw_text",
        &["text, x, y, style", "text, x, y, size, r, g, b, a?"],
    ),
    ("clip", &["x, y, w, h, radius?"]),
    ("clip_none", &[""]),
    ("clip_push", &["x, y, w, h, radius?"]),
    ("clip_pop", &[""]),
    ("text_width", &["text, style", "text, size, font?"]),
    ("text_advance", &["text, style", "text, size, font?"]),
    ("text_metrics", &["style", "size, font?"]),
    (
        "text_wrap",
        &[
            "text, style, max_width",
            "text, size, max_width",
            "text, size, font, max_width",
        ],
    ),
    (
        "text_ellipsize",
        &[
            "text, style, max_width, where?",
            "text, size, max_width, where?",
            "text, size, font, max_width, where?",
        ],
    ),
    (
        "text_index_at",
        &["text, style, dx", "text, size, dx", "text, size, font, dx"],
    ),
    ("font", &["name"]),
    ("fonts", &[""]),
    ("create_canvas", &["w, h"]),
    ("draw_to", &["id"]),
    ("draw_to_screen", &[""]),
    ("draw_canvas", &["id, x, y, a?, w?, h?"]),
    ("snapshot_to", &["id, x, y"]),
    ("blur_canvas", &["id, radius"]),
    // host_data.rs
    ("host_data", &["kind, arg"]),
];

/// The parameter lists a host native is known to declare, when the checker
/// can say without the host's `Env`: the petal-ui natives, from
/// [`PETAL_UI_NATIVE_PARAMS`]. `None` for any other host native (Garden's, the
/// SDL runner's, a `--native` name), whose registration this process never
/// sees.
pub fn host_native_signatures(name: &str) -> Option<Vec<NativeSignature>> {
    let (_, specs) = PETAL_UI_NATIVE_PARAMS.iter().find(|(n, _)| *n == name)?;
    specs.iter().map(|s| NativeSignature::parse(s).ok()).collect()
}

/// Check the names a call to native `name` writes against the parameter lists
/// it declares, and say what is wrong — `None` when some form takes the call.
///
/// Worded as the checker words the same slip on a Petal `fn`
/// (`Checker::check_named_args`): the VM's sentence, plus the parameters that
/// do exist or the argument that already filled the slot. `names` is parallel
/// to the call's arguments, `None` for a positional one.
pub fn named_native_call_error(
    name: &str,
    sigs: &[NativeSignature],
    names: &[Option<&str>],
) -> Option<String> {
    // Only the shape of the call matters here, so the "arguments" bound are
    // their own indices.
    let indices: Vec<usize> = (0..names.len()).collect();
    if sigs.iter().any(|s| s.bind(name, &indices, names).is_ok()) {
        return None;
    }
    // The form the call was aimed at, chosen as the run chooses it
    // (`native_fn::bind_native_args`): the only one, or the one with exactly
    // this many parameters. With several forms and none of that length there
    // is no single parameter list to hold the names against, and the run's
    // own summary of the forms is the report.
    let sig = match sigs {
        [only] => only,
        _ => match sigs.iter().find(|s| s.params().len() == names.len()) {
            Some(sig) => sig,
            None => {
                return crate::native_fn::bind_native_args(name, sigs, &indices, names).err();
            }
        },
    };
    let params = sig.params();
    let mut filled: Vec<Option<usize>> = vec![None; params.len()];
    for (i, written) in names.iter().enumerate() {
        let Some(written) = written else {
            if let Some(cell) = filled.get_mut(i) {
                *cell = Some(i);
            }
            continue;
        };
        let Some(slot) = params.iter().position(|p| p == written) else {
            let known: Vec<String> = params.iter().map(|p| format!("'{p}'")).collect();
            let known = if known.is_empty() {
                "none".to_string()
            } else {
                known.join(", ")
            };
            return Some(format!(
                "{name}() has no parameter named '{written}' (parameters: {known})"
            ));
        };
        if let Some(first) = filled[slot] {
            return Some(format!(
                "{name}() got multiple values for parameter '{written}' (argument {} already fills it)",
                first + 1
            ));
        }
        filled[slot] = Some(i);
    }
    // Neither of those: a required parameter left out, a gap below an
    // optional one, or too many arguments. The binder's own sentence says
    // which.
    sig.bind(name, &indices, names).err()
}

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

    /// The canonical `--host` value for this profile.
    pub fn name(self) -> &'static str {
        match self {
            HostProfile::Core => "core",
            HostProfile::Ui => "ui",
            HostProfile::Garden => "garden",
            HostProfile::Sdl => "sdl",
            HostProfile::GardenConfig => "garden-config",
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

/// Report every call to, or read of, a global that nothing defines, and
/// every named argument a native cannot take.
///
/// `native_signatures` answers for the running `Env`: `None` when it registers
/// no native of that name, else the parameter lists that native declares
/// ([`Env::native_signatures`](crate::env::Env::native_signatures)). `host` is
/// the set the target host adds (see [`host_names`]). A `BuiltinCall` naming
/// neither is a call that fails with "Unknown builtin"; an `Undefined
/// variable` error term naming neither is a read that fails. A read of a host
/// native (`let f = draw_rect`) compiles to that same error term under a table
/// that lacks it, which is why reads are filtered by the host set too.
///
/// A `BuiltinCall` that *does* resolve is reported when it still carries
/// argument names — the compiler strips them from every call whose names bind
/// (`Compiler::normalize_named_builtin_calls`), so what is left fails when it
/// runs:
///
/// - a native that declares no parameters refuses names outright
///   (`builtin 'print' does not accept named arguments`);
/// - one that declares them is checked against them — an unknown name, a slot
///   filled twice, a required parameter left out — in the words the checker
///   uses for a Petal `fn` ([`named_native_call_error`]).
///
/// A native this process only knows by name (the host set) is checked when it
/// is a petal-ui one, whose parameters core carries
/// ([`host_native_signatures`]). Any other — Garden's, the SDL runner's, a
/// `--native` name — is given the benefit of the doubt: its host may well
/// declare parameters this process cannot see, and reporting an error on a
/// call that runs is worse than leaving a bad name to the run.
///
/// One diagnostic per source position, in term order. Each is an error
/// ([`Severity::Error`](crate::diagnostic::Severity::Error)): the line fails
/// whenever it runs, so `check` fails on it without `--strict`.
pub fn unresolved_globals<'a>(
    program: &Program,
    native_signatures: impl Fn(&str) -> Option<&'a [NativeSignature]>,
    host: &HashSet<String>,
) -> Vec<Diagnostic> {
    let known = |name: &str| native_signatures(name).is_some() || host.contains(name);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for term in &program.terms {
        let message = match term.op {
            TermOp::BuiltinCall(cid) => {
                let Some(name) = program.get_string_constant(cid) else {
                    continue;
                };
                if !known(name) {
                    format!(
                        "unknown function `{name}`: nothing by that name is in scope, and it is not \
                         a builtin (running this line fails with \"Unknown builtin: {name}\")"
                    )
                } else {
                    if term.arg_names.iter().all(Option::is_none) {
                        continue;
                    }
                    let names: Vec<Option<&str>> = term
                        .arg_names
                        .iter()
                        .map(|n| n.and_then(|c| program.get_string_constant(c)))
                        .collect();
                    let host_sigs;
                    let sigs: &[NativeSignature] = match native_signatures(name) {
                        Some(sigs) => sigs,
                        None => match host_native_signatures(name) {
                            Some(sigs) => {
                                host_sigs = sigs;
                                &host_sigs
                            }
                            // A host native whose registration is out of
                            // sight: not ours to refuse.
                            None => continue,
                        },
                    };
                    if sigs.is_empty() {
                        // Same wording as the VM's refusal, like the
                        // checker's other named-argument errors.
                        format!(
                            "builtin '{name}' does not accept named arguments (pass them by position)"
                        )
                    } else {
                        match named_native_call_error(name, sigs, &names) {
                            Some(message) => message,
                            None => continue,
                        }
                    }
                }
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
            out.push(Diagnostic::error(*span, message));
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

/// The checkout's `petal-libs/` directory, holding the packages Garden
/// registers for every panel (`bloom`, `text_layout`). `None` when this build
/// could not see it.
pub fn garden_packages_dir() -> Option<std::path::PathBuf> {
    #[cfg(all(petal_libs_dir, not(target_arch = "wasm32")))]
    {
        Some(std::path::PathBuf::from(env!("PETAL_LIBS_DIR")))
    }
    #[cfg(not(all(petal_libs_dir, not(target_arch = "wasm32"))))]
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
        unresolved_globals(program, |n| env.native_signatures(n), &host)
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
    fn a_named_argument_to_an_undeclared_builtin_is_reported() {
        let got = unresolved(
            "if false then print(1, sep: 2) end\nprint(1, 2)",
            HostProfile::Core,
            &[],
        );
        assert_eq!(
            got,
            ["1:15 builtin 'print' does not accept named arguments (pass them by position)"]
        );
        // A Petal `fn` of the same call shape is not a native at all.
        let src = "fn f(a, b) a - b end\nprint(f(b: 1, a: 2), sum(xs: [1]))";
        assert!(unresolved(src, HostProfile::Core, &[]).is_empty());
    }

    #[test]
    fn names_a_builtin_declares_are_accepted_and_checked() {
        let ok = "print(clamp(5, lo: 0, hi: 3), clamp(hi: 3, value: 5, lo: 0), [1, 2, 3].slice(start: 1))";
        assert!(unresolved(ok, HostProfile::Core, &[]).is_empty());
        // Inside an untaken branch, so only this pass ever reports them.
        let got = unresolved(
            "if false then\n  clamp(5, low: 0, hi: 3)\n  clamp(5, value: 0, hi: 3)\n  clamp(5, hi: 3)\n  slice([1], end: 1)\nend",
            HostProfile::Core,
            &[],
        );
        assert_eq!(
            got,
            [
                "2:3 clamp() has no parameter named 'low' (parameters: 'value', 'lo', 'hi')",
                "3:3 clamp() got multiple values for parameter 'value' (argument 1 already fills it)",
                "4:3 clamp() is missing a value for parameter 'lo'",
                "5:3 slice() is missing a value for parameter 'start'",
            ]
        );
    }

    #[test]
    fn a_builtin_with_several_forms_takes_any_of_them() {
        let ok = "print(random(max: 2), random(min: 1, max: 2), distance(v1: vec2(0, 0), v2: vec2(1, 1)))";
        assert!(unresolved(ok, HostProfile::Core, &[]).is_empty());
        let got = unresolved("if false then random(lo: 1, hi: 2) end", HostProfile::Core, &[]);
        assert_eq!(
            got,
            ["1:15 random() has no parameter named 'lo' (parameters: 'min', 'max')"]
        );
    }

    #[test]
    fn host_natives_are_checked_when_core_knows_their_parameters() {
        // petal-ui's parameters are carried here, so they are checked without
        // the host's `Env`…
        assert!(unresolved("mouse_down(button: 0)", HostProfile::Ui, &[]).is_empty());
        assert!(
            unresolved("clip(x: 0, y: 0, w: 4, h: 4, radius: 2)", HostProfile::Ui, &[]).is_empty()
        );
        assert_eq!(
            unresolved("mouse_down(btn: 0)", HostProfile::Ui, &[]),
            ["1:1 mouse_down() has no parameter named 'btn' (parameters: 'button')"]
        );
        assert_eq!(
            unresolved("mouse_x(button: 0)", HostProfile::Ui, &[]),
            ["1:1 mouse_x() has no parameter named 'button' (parameters: none)"]
        );
        // …and any other host native's names are left to the run.
        assert!(unresolved("navigate(to: 1)", HostProfile::Garden, &[]).is_empty());
        assert!(unresolved("mine(a: 1)", HostProfile::Core, &["mine"]).is_empty());
    }

    #[test]
    fn every_host_parameter_table_entry_is_listed_and_parses() {
        for (name, specs) in PETAL_UI_NATIVE_PARAMS {
            assert!(PETAL_UI_NATIVES.contains(name), "{name} is not a petal-ui native");
            assert!(!specs.is_empty(), "{name}");
            for spec in *specs {
                NativeSignature::parse(spec).unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
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
