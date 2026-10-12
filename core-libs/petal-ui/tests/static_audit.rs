//! The static-value audit over the example corpus: every panel app, driven by
//! a monkey scenario, with the dynamic inputs watched while each top-level
//! term the classifier calls static is evaluated. See `petal::static_audit`
//! and docs/tasks/static-value-optimization.md (S0).
//!
//! A static term that reads a binding, host data, a resource, the random
//! stream or `state`, emits, has an effect, or calls a native that is not
//! registered constant is a wrong answer from the classifier or a wrong
//! `CONST` row, and would be a stale value the day static declarations are
//! kept between runs. It fails the test, named.
//!
//! Each app is audited twice: as it runs under its host (`Watch`, every
//! frame of the scenario), and as a build of its static statements alone
//! (`StaticOnly`), which must also run to the end without raising.

use std::path::{Path, PathBuf};

mod common;
use common::par_corpus;

use petal::policy::RunPolicy;
use petal::static_audit::{Report, StaticAuditMode};
use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;

fn audit(
    app: &Path,
    includes: &[PathBuf],
    mode: StaticAuditMode,
    seed: u64,
    frames: usize,
) -> (Report, Option<String>) {
    let size = (800, 600);
    let mut ui = Headless::panel_from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    // Every frame runs and nothing replays, so every call is made and seen.
    ui.set_policy(RunPolicy::EXPLAIN);
    ui.env.set_static_audit(mode);
    let scenario = Scenario::monkey(seed, frames, size);
    let mut error = None;
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        if let Err(e) = ui.frame()
            && error.is_none()
        {
            error = Some(e.to_string());
        }
    }
    (ui.env.static_audit_report(), error)
}

#[test]
fn no_static_term_touches_a_dynamic_input_across_the_corpus() {
    let mut failures: Vec<String> = Vec::new();
    let (mut apps, mut decls, mut calls) = (0usize, 0usize, 0u64);
    for (app, (report, _)) in
        par_corpus(|app, includes| audit(app, includes, StaticAuditMode::Watch, 1, 45))
    {
        apps += 1;
        decls += report.static_declarations;
        calls += report.native_calls;
        for f in &report.findings {
            failures.push(format!("{}: {f}", app.display()));
        }
    }
    eprintln!(
        "static audit (watch): {apps} apps, {decls} static declarations, {calls} native calls under static terms, {} failures",
        failures.len()
    );
    assert!(decls > 0 && calls > 0, "the audit saw nothing: {decls} declarations, {calls} calls");
    assert!(failures.is_empty(), "static terms touched dynamic inputs:\n  {}", failures.join("\n  "));
}

#[test]
fn the_static_declarations_build_alone_across_the_corpus() {
    let mut failures: Vec<String> = Vec::new();
    let (mut apps, mut decls, mut run, mut skipped, mut calls) = (0usize, 0usize, 0usize, 0usize, 0u64);
    for (app, (report, error)) in
        par_corpus(|app, includes| audit(app, includes, StaticAuditMode::StaticOnly, 1, 3))
    {
        apps += 1;
        decls += report.static_declarations;
        run += report.statements_run;
        skipped += report.statements_skipped;
        calls += report.native_calls;
        for f in &report.findings {
            failures.push(format!("{}: {f}", app.display()));
        }
        if let Some(e) = error
            && report.findings.is_empty()
        {
            failures.push(format!("{}: the build raised: {e}", app.display()));
        }
    }
    eprintln!(
        "static audit (static only): {apps} apps, {decls} static declarations, {run} statements run, {skipped} skipped, {calls} native calls, {} failures",
        failures.len()
    );
    assert!(run > 0 && skipped > 0, "nothing was sliced: {run} run, {skipped} skipped");
    assert!(failures.is_empty(), "static-only builds failed:\n  {}", failures.join("\n  "));
}
