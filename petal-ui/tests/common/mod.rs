//! The app corpus both differential oracles (`gating.rs`, `memo.rs`) run over.
//!
//! It was the `examples/` tree plus two Garden panels. That excluded the
//! largest real Petal codebase in the repo — Garden's own panels and GPP apps —
//! which is how the frame gate and memoized scopes both shipped without ever
//! having been differentially checked against the code they were built for.
//! This module is that corpus, in one place, so the two oracles cannot drift
//! apart on what they cover.
//!
//! Four Garden scripts are deliberately out (see [`EXCLUDED`]). Everything else
//! that exists is in, and [`assert_corpus_is_live`] is what keeps an app that
//! silently stopped producing frames — a missing `-I`, a renamed native — from
//! reading as a pass, which is exactly how three apps hid for one round of this
//! work.

use std::path::{Path, PathBuf};

/// Garden scripts that cannot be driven headlessly, and why. Each would error
/// on frame 0 in *every* variant, which compares equal and proves nothing.
pub const EXCLUDED: &[(&str, &str)] = &[
    ("gpp-test-app/src/runtime_error.ptl", "a deliberate error fixture"),
    ("gpp-test-app/src/runtime_error_long.ptl", "a deliberate error fixture"),
    ("garden-app/src/petal_ide/ir_view.ptl", "needs host `stages` data no stub provides"),
    ("init.ptl", "Garden's config script, not a panel: needs the `editor` natives"),
];

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn excluded(path: &Path) -> bool {
    let s = path.to_string_lossy();
    EXCLUDED.iter().any(|(suffix, _)| s.ends_with(suffix))
}

/// Every driveable app, with the module search paths it needs.
///
/// `petal-libs` is on the path for all of them: several Garden GPP apps import
/// `bloom` from there, and an app that does not import it is unaffected.
pub fn corpus() -> Vec<(PathBuf, Vec<PathBuf>)> {
    let root = repo_root();
    let libs = vec![root.join("petal-libs")];
    let mut apps = Vec::new();

    for group in ["productivity", "dashboards", "games", "ui"] {
        let dir = root.join("examples").join(group);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let app = e.path().join("app.ptl");
            if app.exists() {
                apps.push((app, libs.clone()));
            }
        }
    }

    // Garden: every example panel, and every GPP app's drawer. These run under
    // `panel_stubs`, which stands in for the host natives Garden registers.
    let garden = root.join("garden");
    if let Ok(entries) = std::fs::read_dir(garden.join("examples/panels")) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "ptl") && !excluded(&p) {
                apps.push((p, libs.clone()));
            }
        }
    }
    for group in ["gpp-apps", "gpp-python"] {
        let Ok(dirs) = std::fs::read_dir(garden.join(group)) else { continue };
        for d in dirs.flatten() {
            // `gpp-apps/<app>/src/*.ptl`, `gpp-python/<app>/*.ptl`.
            for sub in [d.path().join("src"), d.path()] {
                let Ok(entries) = std::fs::read_dir(&sub) else { continue };
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "ptl") && !excluded(&p) {
                        apps.push((p, libs.clone()));
                    }
                }
            }
        }
    }

    apps.sort();
    apps.dedup();
    assert!(apps.len() >= 25, "corpus looks wrong: {} apps", apps.len());
    assert!(
        apps.iter().any(|(p, _)| p.starts_with(&garden)),
        "the Garden half of the corpus is missing"
    );
    apps
}

/// Every app must actually draw something on some frame. A corpus entry that
/// errors on frame 0 — a module it cannot resolve, a native with no stub —
/// produces an empty trace, and two empty traces compare equal: the oracle
/// reports a pass while testing nothing at all.
#[allow(dead_code)] // not every test binary drives the corpus by frames
pub fn assert_corpus_is_live(app: &Path, frames: &[String]) {
    let drew = frames.iter().any(|f| {
        serde_json::from_str::<serde_json::Value>(f)
            .ok()
            .and_then(|v| v["commands"].as_array().map(|c| !c.is_empty()))
            .unwrap_or(false)
    });
    assert!(
        drew,
        "{} drew nothing on any frame — it is in the corpus but not being tested. \
         Either give it what it needs to run, or add it to common::EXCLUDED with a reason.",
        app.display()
    );
}
