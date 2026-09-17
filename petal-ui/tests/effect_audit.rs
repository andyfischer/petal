//! The native effect audit over the example corpus: every panel app, driven
//! by a monkey scenario, with every native call bracketed by activity
//! snapshots and held against the effect row the native declared. See
//! `petal::effect_audit` and docs/tasks/declarative-effect-refactoring.md.
//!
//! An under-declared native is a staleness bug the memo cannot see (the row
//! says less than the native does, so a memoized scope replays without
//! calling it), and fails the test. Undeclared natives are the ones still to
//! migrate; they are printed, with what they were seen doing, so the list is
//! one `--nocapture` away.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod common;
use common::corpus;

use petal::effect_audit::{FindingKind, Report};
use petal::policy::RunPolicy;
use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;

fn audit(app: &Path, includes: &[PathBuf], seed: u64, frames: usize) -> Report {
    let size = (800, 600);
    let mut ui = Headless::from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    petal_ui::panel_stubs::register_panel_stubs(&mut ui.env);
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    // Every frame runs, and scopes replay: the same shape as the memo
    // oracle, so what the audit sees is what the memo classified.
    ui.set_policy(RunPolicy::REPLAY);
    ui.env.set_effect_audit(true);
    let scenario = Scenario::monkey(seed, frames, size);
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        let _ = ui.frame();
    }
    ui.env.effect_audit_report()
}

#[test]
fn no_declared_native_does_more_than_it_declares_across_the_corpus() {
    let frames = 45;
    let mut under: Vec<String> = Vec::new();
    // name → (facets, apps that called it)
    let mut undeclared: BTreeMap<String, (Vec<String>, usize)> = BTreeMap::new();
    let mut called_total = 0;
    for (app, includes) in corpus() {
        let report = audit(&app, &includes, 1, frames);
        called_total += report.called;
        for f in &report.findings {
            match f.kind {
                FindingKind::UnderDeclared => under.push(format!(
                    "{}: {} was seen doing {} its row does not declare",
                    app.display(),
                    f.name,
                    f.facets.join(", "),
                )),
                FindingKind::Undeclared => {
                    let e = undeclared.entry(f.name.clone()).or_default();
                    for facet in &f.facets {
                        // `binding a,b` merges with another app's `binding b,c`.
                        let names = facet.strip_prefix("binding ").map_or_else(
                            || vec![facet.clone()],
                            |list| list.split(',').map(|n| format!("binding {n}")).collect(),
                        );
                        for name in names {
                            if !e.0.contains(&name) {
                                e.0.push(name);
                            }
                        }
                    }
                    e.1 += 1;
                }
                FindingKind::OverDeclared => {}
            }
        }
    }
    assert!(
        called_total > 0,
        "no native was ever called across the corpus"
    );
    eprintln!(
        "{} natives still undeclared across the corpus:",
        undeclared.len()
    );
    for (name, (facets, apps)) in &undeclared {
        let facets = if facets.is_empty() {
            "(silent)".to_string()
        } else {
            facets.join(" ")
        };
        eprintln!("  {name:<28} {apps:>3} apps  {facets}");
    }
    assert!(
        under.is_empty(),
        "under-declared natives:\n  {}",
        under.join("\n  ")
    );
}
