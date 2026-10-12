//! The static-value classifier held against what the panel corpus actually
//! computes. See `petal::static_classify` and
//! docs/tasks/static-value-optimization.md (S0).
//!
//! A declaration the classifier calls static is one whose initializer "can
//! only ever produce one value for a given source text". This drives every
//! corpus app for a run of frames under a monkey scenario (input, the clock,
//! the random seed and `state` all moving) and requires each such
//! declaration to hold the same value on every frame it is bound. A value
//! that moves is a wrong answer from the classifier, or a native registered
//! `CONST` that is not one — and either would become a stale value the day
//! a static declaration is kept between runs.
//!
//! It is the classifier's own oracle, and a weaker one than the planned
//! `--static-audit`: it sees a wrong answer only when the corpus happens to
//! move the value.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod common;
use common::par_corpus;

use petal::policy::RunPolicy;
use petal::static_classify::{DeclClass, DeclKind, StaticSummary};
use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;

struct Outcome {
    summary: StaticSummary,
    /// Static declarations whose value was observed on two or more frames.
    compared: usize,
    /// `name: first value -> later value` for each one that moved.
    moved: Vec<String>,
}

/// A value, cut short: a moved table can be thousands of characters.
fn brief(value: &serde_json::Value) -> String {
    let text = value.to_string();
    match text.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

fn drive(app: &Path, includes: &[PathBuf], seed: u64, frames: usize) -> Outcome {
    let size = (800, 600);
    let mut ui = Headless::panel_from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    // Every frame runs and nothing replays: each frame binds every top-level
    // name afresh, which is the thing being compared.
    ui.set_policy(RunPolicy::EXPLAIN);
    ui.env.observations_mut().enable();

    let (pid, sid) = (ui.program_id(), ui.stack_id());
    let report = {
        let program = ui.env.get_program(pid).expect("the loaded program");
        ui.env.static_report(program)
    };
    // The entry file's static data. A name the file binds once is observed
    // under its bare name; a module's is qualified and left out.
    let names: Vec<String> = report
        .entry()
        .filter(|d| d.class == DeclClass::Static)
        .filter(|d| matches!(d.kind, DeclKind::Let | DeclKind::ConfigLet | DeclKind::Const))
        .map(|d| d.name.clone())
        .collect();

    let scenario = Scenario::monkey(seed, frames, size);
    let mut first: BTreeMap<String, (serde_json::Value, usize)> = BTreeMap::new();
    let mut moved: Vec<String> = Vec::new();
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        let _ = ui.frame();
        let observed = ui.env.get_observations_json(pid, sid);
        for name in &names {
            let Some(value) = observed.get(name) else { continue };
            match first.get_mut(name) {
                None => {
                    first.insert(name.clone(), (value.clone(), 1));
                }
                Some((seen, count)) => {
                    *count += 1;
                    if seen != value && !moved.iter().any(|m| m.starts_with(&format!("{name}:"))) {
                        moved.push(format!(
                            "{name}: {} -> {} (frame {frame})",
                            brief(seen),
                            brief(value)
                        ));
                    }
                }
            }
        }
    }
    Outcome {
        summary: report.summary(false),
        compared: first.values().filter(|(_, count)| *count > 1).count(),
        moved,
    }
}

#[test]
fn a_static_declaration_holds_one_value_on_every_frame_across_the_corpus() {
    let frames = 45;
    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0;
    let mut total = StaticSummary::default();
    let results = par_corpus(|app, includes| drive(app, includes, 1, frames));
    let apps = results.len();
    for (app, outcome) in results {
        compared += outcome.compared;
        total.declarations += outcome.summary.declarations;
        total.static_data += outcome.summary.static_data;
        total.static_read_by_dynamic += outcome.summary.static_read_by_dynamic;
        total.functions += outcome.summary.functions;
        total.functions_static_pure += outcome.summary.functions_static_pure;
        total.dynamic += outcome.summary.dynamic;
        for m in outcome.moved {
            failures.push(format!("{}: {m}", app.display()));
        }
    }
    eprintln!(
        "{apps} apps, {} declarations: {} static ({} read by dynamic code), \
         {} functions ({} static-pure), {} dynamic; {compared} static values compared over {frames} frames",
        total.declarations,
        total.static_data,
        total.static_read_by_dynamic,
        total.functions,
        total.functions_static_pure,
        total.dynamic,
    );
    assert!(
        compared > 500,
        "only {compared} static declarations were observed on more than one frame: \
         the oracle is not looking at the corpus"
    );
    assert!(
        failures.is_empty(),
        "a declaration classified static changed value between frames:\n  {}",
        failures.join("\n  ")
    );
}
