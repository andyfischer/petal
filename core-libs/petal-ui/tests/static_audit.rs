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

/// The entry file's static `let`s, as the app binds them on its first frame.
fn first_frame_statics(
    app: &Path,
    includes: &[PathBuf],
    mode: StaticAuditMode,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    use petal::static_classify::{DeclClass, DeclKind};
    let size = (800, 600);
    let mut ui = Headless::panel_from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    ui.env.set_echo(false);
    ui.env.set_seed(1);
    ui.set_policy(RunPolicy::EXPLAIN);
    ui.env.observations_mut().enable();
    ui.env.set_static_audit(mode);
    let (pid, sid) = (ui.program_id(), ui.stack_id());
    let names: Vec<String> = {
        let program = ui.env.get_program(pid).expect("the loaded program");
        ui.env
            .static_report(program)
            .entry()
            .filter(|d| d.class == DeclClass::Static)
            .filter(|d| matches!(d.kind, DeclKind::Let | DeclKind::ConfigLet))
            .map(|d| d.name.clone())
            .collect()
    };
    let _ = ui.frame();
    let observed = ui.env.get_observations_json(pid, sid);
    names
        .into_iter()
        .filter_map(|n| observed.get(&n).map(|v| (n, v.clone())))
        .collect()
}

/// The static-only build is a faithful preview of keeping static values: what
/// it builds for each static declaration is what the whole program builds.
/// A static statement that quietly read a value from a skipped (dynamic)
/// statement would build something else here, and move no counter doing it.
#[test]
fn a_static_only_build_makes_the_values_the_whole_program_makes() {
    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0usize;
    for (app, (whole, alone)) in par_corpus(|app, includes| {
        (
            first_frame_statics(app, includes, StaticAuditMode::Off),
            first_frame_statics(app, includes, StaticAuditMode::StaticOnly),
        )
    }) {
        for (name, value) in &whole {
            match alone.get(name) {
                Some(built) if built == value => compared += 1,
                Some(built) => {
                    let brief = |v: &serde_json::Value| v.to_string().chars().take(100).collect::<String>();
                    failures.push(format!(
                        "{}: {name} is {} in the whole program and {} built alone",
                        app.display(),
                        brief(value),
                        brief(built)
                    ));
                }
                None => failures.push(format!(
                    "{}: {name} was not built by the static-only run",
                    app.display()
                )),
            }
        }
    }
    eprintln!("static-only build against the whole program: {compared} static values equal, {} differ", failures.len());
    assert!(compared > 500, "only {compared} static values were compared");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
