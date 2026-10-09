//! The native effect audit over the example corpus: every panel app, driven
//! by a monkey scenario, with every native call bracketed by activity
//! snapshots and held against the effect row the native declared. See
//! `petal::effect_audit` and docs/tasks/declarative-effect-refactoring.md.
//!
//! An under-declared native is a staleness bug the memo cannot see (the row
//! says less than the native does, so a memoized scope replays without
//! calling it), and fails the test. Over-declared facets are printed under
//! `--nocapture`: not bugs (a row is the union over every path), but the
//! list of rows the corpus never fully exercised.

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
fn no_native_does_more_than_it_declares_across_the_corpus() {
    let frames = 45;
    let mut under: Vec<String> = Vec::new();
    // name → (facets, apps that called it)
    let mut over: BTreeMap<String, (Vec<String>, usize)> = BTreeMap::new();
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
                FindingKind::OverDeclared => {
                    let e = over.entry(f.name.clone()).or_default();
                    for facet in &f.facets {
                        if !e.0.contains(facet) {
                            e.0.push(facet.clone());
                        }
                    }
                    e.1 += 1;
                }
            }
        }
    }
    assert!(
        called_total > 0,
        "no native was ever called across the corpus"
    );
    eprintln!(
        "{} natives declare a facet the corpus never exercised:",
        over.len()
    );
    for (name, (facets, apps)) in &over {
        eprintln!("  {name:<28} {apps:>3} apps  {}", facets.join(" "));
    }
    assert!(
        under.is_empty(),
        "under-declared natives:\n  {}",
        under.join("\n  ")
    );
}
