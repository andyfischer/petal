//! What this `garden` binary *is*: version, build stamp, and the named
//! features a client can probe.
//!
//! The problem this solves: an installed `garden` used to carry no identity at
//! all, so the only way to find out whether it had `--panel-wake`, or
//! `/state?values=`, or a prelude with `contrast_text` in it, was to call the
//! thing and read the error — and the error ("unknown option", "no endpoint
//! GET /state?values=none", "Unknown builtin: contrast_text") does not
//! distinguish "never existed" from "your binary is old". Now there are two
//! ways to ask up front:
//!
//! ```text
//! garden --version           # human line
//! garden --version --json    # the same report as JSON
//! curl 127.0.0.1:$PORT/version
//! ```
//!
//! and a client degrades deliberately by testing a name in `features`.
//!
//! **Adding a feature flag**: append one line to [`HOST_FEATURES`] naming the
//! endpoint or flag you just added, in the same commit that adds it, and give
//! it a `# landed in` note in the doc that describes it (`docs/debug-server.md`,
//! `docs/petal-graphical-panels.md`). Names are dotted and stable —
//! `<area>.<feature>` — and are never renamed or removed once published, since
//! old clients test them by string. `cli.*` names are checked against the real
//! argument parser by a unit test in `lib.rs`, so an advertised flag that no
//! longer parses fails the build instead of shipping a lying `--version`.
//!
//! Prelude capability is *derived*, not listed: [`prelude_exports`] scans the
//! `ui.ptl` source compiled into this binary, so it cannot drift the way a
//! hand-written list (or a doc) can.

use serde_json::{json, Value};

/// The crate version (`garden-app`'s `Cargo.toml`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// UTC day this binary was compiled (`build.rs`).
pub const BUILD_DATE: &str = env!("GARDEN_BUILD_DATE");
/// Short git hash the build came from, or `"unknown"`.
pub const GIT_COMMIT: &str = env!("GARDEN_GIT_COMMIT");
/// Committer date of that commit (`yyyy-mm-dd`), or `"unknown"`.
pub const GIT_DATE: &str = env!("GARDEN_GIT_DATE");
/// `"1"` when the worktree had uncommitted changes at build time.
pub const GIT_DIRTY: &str = env!("GARDEN_GIT_DIRTY");

/// Named capabilities of *this* build. See the module docs for the rules.
pub const HOST_FEATURES: &[&str] = &[
    // Argument-parser flags. `cli.<name>` must correspond to `--<name>`; a
    // unit test in `lib.rs` proves the parser still accepts each one.
    "cli.headless",
    "cli.no-menu",
    "cli.panel-wake", // landed in 216ec76, 2026-08-12
    "cli.subprocess",
    "cli.term",
    "cli.version", // landed with this module
    // Debug-server endpoints.
    "debug.panel-reset", // landed in 216ec76, 2026-08-12
    "debug.tick",        // landed in 216ec76, 2026-08-12
    "debug.version",     // landed with this module
    "debug.windows",
    // `GET /state?values=…` / `?values_prefix=…` narrowing.
    "state.values-filter", // landed in 57b2c8e, 2026-08-12
    // `/state`'s `identity` block carries a `build` object.
    "state.identity-build", // landed with this module
    // Every `/scene` primitive carries a `visible` flag: whether anything of it
    // survives its clip. Without it a headless test cannot tell a drawn row from
    // a clipped-away one.
    "debug.scene-visible", // landed in 6937a22, 2026-08-15
    // Host-handled panel `mutate` names (open_path, open_file_dialog, …).
    "panel.host-mutate",
    // A panel's active `clip(...)` is applied to text (and meshes and images),
    // in every frontend — so a drawer no longer has to cull its own half-rows.
    "panel.text-clip", // landed in 6937a22, 2026-08-15
    // `navigate(screen, arg)` and `nav_arg()`: a navigation carries the subject
    // its target screen is for, stored per history entry so back/forward keep it.
    "panel.nav-arg",
    // Back/forward re-issue the restored entry's `navigate` mutation, so a
    // subprocess app's own handler re-primes the screen's data on a revisit
    // instead of the entry coming back drawn from whatever the provider holds.
    "panel.nav-replay",
    // `mutate(name, arg)` returns a handle and `mutate_result(handle)` reads the
    // outcome back, so a mutation's success or failure is observable.
    "panel.mutate-handle",
    // The petal-ui prelude is reported by name under `prelude.exports`.
    "prelude.exports",
    // The Petal this binary embeds accepts `a?.b` / `a?.[i]` — absence-tolerant
    // reads without a `??` fallback.
    "lang.optional-access",
];

/// Is `name` a feature of this build? The in-process form of the check a
/// client makes against `features` — kept beside the list so an internal
/// caller (a future degrade-in-place path) does not re-implement it.
#[allow(dead_code)]
pub fn has_feature(name: &str) -> bool {
    HOST_FEATURES.contains(&name)
}

/// Every symbol the linked petal-ui prelude exports, as `name/arity` for
/// functions (one entry per overload — `text_field/4` and `text_field/5` are
/// different capabilities, which is exactly the distinction a stale binary got
/// wrong) and a bare `name` for values. Sorted and deduped.
///
/// Derived by scanning `petal_ui::prelude_source()`, so it describes the
/// prelude compiled into *this* binary and cannot go stale. The scan is
/// deliberately strict — a line must start with `export fn ` / `export let ` —
/// on the principle that a missing entry is better than a wrong one.
pub fn prelude_exports() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in petal_ui::prelude_source().lines() {
        if let Some(rest) = line.strip_prefix("export fn ") {
            let Some(open) = rest.find('(') else { continue };
            let name = rest[..open].trim();
            if name.is_empty() {
                continue;
            }
            let Some(close) = rest[open..].find(')') else {
                continue;
            };
            let params = rest[open + 1..open + close].trim();
            let arity = if params.is_empty() {
                0
            } else {
                params.matches(',').count() + 1
            };
            out.push(format!("{name}/{arity}"));
        } else if let Some(rest) = line.strip_prefix("export let ") {
            let name = rest
                .split(|c: char| c == '=' || c == ':' || c.is_whitespace())
                .find(|s| !s.is_empty())
                .unwrap_or("");
            if !name.is_empty() {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The small build stamp embedded in `/state`'s `identity` block — enough to
/// tell one build from another without growing every state response.
pub fn build_json() -> Value {
    json!({
        "version": VERSION,
        "commit": GIT_COMMIT,
        "commit_date": GIT_DATE,
        "build_date": BUILD_DATE,
        "dirty": GIT_DIRTY == "1",
        "prelude_level": petal_ui::PRELUDE_LEVEL,
    })
}

/// The full report behind `garden --version --json` and `GET /version`.
pub fn report_json() -> Value {
    json!({
        "ok": true,
        "version": VERSION,
        "build": build_json(),
        "features": HOST_FEATURES,
        "prelude": {
            "level": petal_ui::PRELUDE_LEVEL,
            "ui_version": petal_ui::UI_VERSION,
            "exports": prelude_exports(),
        },
    })
}

/// One human line plus the feature list, on **stdout** (unlike `print_usage`,
/// which is a diagnostic and goes to stderr) so it can be piped and grepped.
pub fn print_human() {
    let dirty = if GIT_DIRTY == "1" { " (dirty)" } else { "" };
    println!("garden {VERSION} ({GIT_COMMIT} {GIT_DATE}{dirty}, built {BUILD_DATE})");
    println!(
        "prelude level {} (ui_version {}, {} exports)",
        petal_ui::PRELUDE_LEVEL,
        petal_ui::UI_VERSION,
        prelude_exports().len()
    );
    println!("features: {}", HOST_FEATURES.join(" "));
}

/// The machine-readable form (`garden --version --json`).
pub fn print_json() {
    println!("{}", serde_json::to_string_pretty(&report_json()).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report has the shape clients read, and nothing in it is empty.
    #[test]
    fn report_has_the_documented_shape() {
        let r = report_json();
        assert_eq!(r["ok"], json!(true));
        assert!(!r["version"].as_str().unwrap().is_empty());
        assert!(!r["build"]["build_date"].as_str().unwrap().is_empty());
        assert!(!r["build"]["commit"].as_str().unwrap().is_empty());
        assert!(r["build"]["dirty"].is_boolean());
        let features = r["features"].as_array().unwrap();
        assert!(!features.is_empty());
        for f in features {
            let name = f.as_str().unwrap();
            assert!(name.contains('.'), "feature {name} is not <area>.<name>");
        }
        assert!(r["prelude"]["level"].as_u64().unwrap() >= 1);
        assert!(!r["prelude"]["exports"].as_array().unwrap().is_empty());
    }

    /// Feature names are unique and sorted-by-area readable; a duplicate means
    /// two commits claimed the same name for different things.
    #[test]
    fn feature_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for f in HOST_FEATURES {
            assert!(seen.insert(*f), "duplicate feature name {f}");
        }
        assert!(has_feature("cli.panel-wake"));
        assert!(!has_feature("cli.no-such-flag"));
    }

    /// The four features that a stale binary silently lacked are reported —
    /// the two prelude ones by name and arity. This is the regression test for
    /// "you cannot tell what a garden binary contains".
    #[test]
    fn prelude_exports_name_the_features_a_stale_build_lacked() {
        let exports = prelude_exports();
        for wanted in [
            "luma/1",
            "contrast_text/1",
            "text_field_update/4",
            "draw_text_field/3",
            "draw_text_field/4",
            "text_field/4",
            "text_field/5",
        ] {
            assert!(
                exports.iter().any(|e| e == wanted),
                "prelude export {wanted} missing; got {exports:?}"
            );
        }
        // And the host-side pair.
        assert!(has_feature("state.values-filter"));
        assert!(has_feature("cli.panel-wake"));
    }

    /// Every reported export really is `name/arity` (the scan never emits a
    /// half-parsed line).
    #[test]
    fn prelude_exports_are_well_formed() {
        for e in prelude_exports() {
            if let Some((name, arity)) = e.split_once('/') {
                assert!(!name.is_empty(), "empty name in {e}");
                assert!(arity.parse::<usize>().is_ok(), "bad arity in {e}");
            }
        }
    }
}
