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

/// The git checkout this binary was built from (its top level), or `""`.
pub const SOURCE_ROOT: &str = env!("GARDEN_SOURCE_ROOT");

/// The source paths whose changes make a built `garden` out of date: the
/// app, the petal-ui prelude and runtime it links, and the Petal language.
/// Markdown is excluded, so a docs-only commit does not flag a binary stale.
const SOURCE_PATHSPECS: &[&str] = &[
    ":(top)garden",
    ":(top)petal-ui",
    ":(top)rust",
    ":(top,exclude,glob)**/*.md",
    ":(top,exclude)garden/tools",
];

/// Whether this binary is behind the checkout it was built from — the
/// "testing against an old build" trap: a `garden/target/debug/garden` left
/// over from days ago still launches, and every fix since looks unfixed.
///
/// `None` when it can't be told (no git, a source tarball, a binary run on a
/// machine without its checkout). Otherwise `head` is the checkout's current
/// short HEAD and `changed` the number of source files (see
/// [`SOURCE_PATHSPECS`]) that differ between the build commit and HEAD; the
/// binary is `stale` when that is nonzero. Uncommitted edits are not counted
/// (the build stamp records only whether the tree was dirty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Freshness {
    pub head: String,
    /// `None` when the build commit isn't in the checkout at all.
    pub changed: Option<usize>,
}

impl Freshness {
    pub fn stale(&self) -> bool {
        self.changed != Some(0)
    }

    /// One loud line for stderr and `/state`, or `None` when current.
    pub fn warning(&self) -> Option<String> {
        self.stale().then(|| {
            let changed = match self.changed {
                Some(n) => format!("{n} changed source file(s) since"),
                None => "the build commit missing from its history".to_string(),
            };
            format!(
                "this garden binary was built from {GIT_COMMIT} but the checkout is at {} \
                 with {changed}; rebuild (cargo build in garden/) or you are testing old code",
                self.head
            )
        })
    }
}

/// Compare `built` (a commit) against `root`'s HEAD. Split out of
/// [`freshness`] so a test can point it at a commit of its choosing.
pub fn freshness_of(root: &str, built: &str) -> Option<Freshness> {
    if root.is_empty() || built.is_empty() || built == "unknown" {
        return None;
    }
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let head = git(&["rev-parse", "--short", "HEAD"])?;
    let mut args = vec!["diff", "--name-only", built, "HEAD", "--"];
    args.extend_from_slice(SOURCE_PATHSPECS);
    // `None`: the build commit isn't in this checkout (rewritten history,
    // another clone) — nothing vouches for the binary, so it counts as stale.
    let changed = git(&args).map(|out| out.lines().filter(|l| !l.is_empty()).count());
    Some(Freshness { head, changed })
}

/// [`freshness_of`] this binary, cached for a few seconds: `/state` asks on
/// every read, and two `git` calls per request would dominate a tight
/// polling loop.
pub fn freshness() -> Option<Freshness> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Option<Freshness>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, value)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(5) {
            return value.clone();
        }
    }
    let value = freshness_of(SOURCE_ROOT, GIT_COMMIT);
    *cache = Some((Instant::now(), value.clone()));
    value
}

/// Print the stale-binary warning to stderr, if there is one. Called once at
/// startup so a `launch.sh` log shows it beside the debug port.
pub fn warn_if_stale() {
    if let Some(warning) = freshness().and_then(|f| f.warning()) {
        eprintln!("garden: WARNING: {warning}");
    }
}

/// `/state`'s `identity.freshness`: `{head, changed, stale, warning}`, or
/// null when it can't be told. `changed` is null when the build commit is not
/// in the checkout; `warning` is `""` for a current binary.
pub fn freshness_json() -> Value {
    match freshness() {
        None => Value::Null,
        Some(f) => json!({
            "head": f.head,
            "changed": f.changed,
            "stale": f.stale(),
            // Always a string (empty when current) so the reply's shape does
            // not change with the checkout's state.
            "warning": f.warning().unwrap_or_default(),
        }),
    }
}

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
    // The Garden Pane Protocol this build speaks: v2 — panel-only, JSON-RPC
    // responses correlated by id, JSON query args, version handshake. See
    // docs/gpp.md.
    "gpp.protocol-2",
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
    // Every `/scene` primitive carries `id`, its index in the draw-command
    // stream, so two scenes can be diffed.
    "debug.scene-id",
    // Every `/scene` text run carries the face it is really drawn in, its
    // weight/italic/spacing unconditionally, and its measured advance width.
    "debug.scene-text-metrics",
    // `GET /screenshot?pane=<n>` and `GET /scene?pane=<n>`: one pane's pixels,
    // and a scene rebased onto that pane's origin.
    "debug.pane-capture",
    // `POST /tick` drives a virtual clock: `time()` advances by exactly the
    // `dt` given, so a `time()`-driven animation is steppable and reproducible.
    "debug.tick-clock",
    // `POST /seed` reseeds every panel's `random()` stream.
    "debug.seed",
    // `GET /state?output=all` / `?output=<cursor>`: a non-draining read of the
    // script output, so an observer can run beside a driver.
    "state.output-cursor",
    // `/state` reports glyph-atlas pressure under `text_atlas` and unresolvable
    // font specs under `unresolved_fonts`.
    "state.text-atlas",
    // A panel script's `request_frame()` / `animating()` opt-out of the idle
    // sleep window.
    "panel.request-frame",
    // `POST /key` checks its key name where the request is parsed: an unknown
    // name is a 400 listing petal-ui's canonical `KEY_NAMES`, and the names a
    // panel reads are derived from the same table `/key` parses.
    "debug.key-validate",
    // `GET /scene?find=text:<s>` (exact, trimmed) / `?find=text~:<s>`
    // (substring): only the matching text runs, each with `rect` and `center`
    // — a text locator, so a test clicks a label rather than a coordinate.
    "debug.scene-find",
    // `GET /capture?format=png|json|text`: one capture of the settled frame,
    // with `/scene` (= json) and `/screenshot` (= the native raster) as its
    // aliases. A frontend declines a format it cannot make with a 400.
    "debug.capture",
    // `?select=panes.0.cursor,focus` on any JSON endpoint: a field projection
    // (`*` wildcards, `name*` prefixes, tail-matched qualified keys) that keeps
    // each field where it was. `?window=` may also sit anywhere in the query.
    "state.select",
    // `?select=` on a state-changing command (`/key`, `/text`, `/command`,
    // `/menu`, `/mouse`, `/theme`, `/tick`, `/seed`, `/panel/reset`) replies
    // with that projection of the settled post-command `/state` snapshot, the
    // command's own receipt fields laid on top. `/state` gains top-level
    // `cursor` / `selection` (the focused pane's), so the default input
    // acknowledgment is the projection `focus,cursor,selection`.
    "debug.command-select",
    // `POST /batch`: an array of command bodies (each with its `path`) run in
    // one event-loop visit, replying `{ok, results}`; a step's own `select=`
    // projects the settled snapshot right after it.
    "debug.batch",
    // `/state`'s `panes[].panel.frame_stats`: the frame gate's run/skip counts
    // and last run reason, and the memo table's counters, read from the
    // panel-frame core Garden shares with petal-ui's harness.
    "state.panel-frame-stats",
    // `POST /mouse` `"hover_first": true` on `click`/`down`/`drag`: a hover
    // frame at the press point runs before the press, as with a real pointer.
    "debug.mouse-hover-first",
    // `/state`'s `identity.freshness`: whether this binary is behind the
    // checkout it was built from (and a startup stderr warning when it is).
    "state.identity-freshness",
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

    /// A binary built from HEAD is current; one built from an older commit
    /// that source files changed after is stale; an unknown commit is stale
    /// and says so; no checkout means "can't tell", not a false alarm.
    #[test]
    fn freshness_compares_the_build_commit_against_head() {
        let root = SOURCE_ROOT;
        if root.is_empty() {
            return; // built without git: nothing to compare
        }
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git").arg("-C").arg(root).args(args).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let head = git(&["rev-parse", "--short", "HEAD"]);
        let now = freshness_of(root, &head).expect("a checkout");
        assert_eq!(now.changed, Some(0));
        assert!(!now.stale() && now.warning().is_none());

        // The last commit that touched a source file, and its parent: a build
        // from the parent is behind by at least that file.
        let touched = git(&["log", "-1", "--format=%h", "--", "garden/garden-app/src"]);
        let before = git(&["rev-parse", "--short", &format!("{touched}^")]);
        let old = freshness_of(root, &before).expect("a checkout");
        assert!(old.changed.unwrap() >= 1, "{old:?}");
        assert!(old.stale());
        assert!(old.warning().unwrap().contains("rebuild"));

        let bogus = freshness_of(root, "0000000").expect("a checkout");
        assert_eq!(bogus.changed, None);
        assert!(bogus.stale());

        assert_eq!(freshness_of("", &head), None);
        assert_eq!(freshness_of(root, "unknown"), None);
        assert_eq!(freshness_of("/nonexistent/dir", &head), None);
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
