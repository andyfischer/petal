//! Static values inside function bodies, over the panel corpus. See
//! `petal::static_cache` (the `in_fn` module) and
//! docs/tasks/static-value-optimization.md ("Later: constants inside
//! function bodies").
//!
//! One test: every corpus app, driven by a monkey scenario, frame for
//! frame the same with the cache on, with it off, and with it on and
//! checking every value it keeps for a function body against what the
//! code builds on each call.
//!
//! And three measurements, which print tables and test nothing:
//!
//! ```sh
//! cargo test --release --test static_in_functions -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

mod common;
use common::par_corpus;

use petal::policy::RunPolicy;
use petal::static_survey::{SiteCounter, survey};
use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;

struct Row {
    /// Sites found in the program, and how many of them ran.
    sites: usize,
    ran: usize,
    mutated: u32,
    weightless: u32,
    /// Over the whole run.
    instructions: u64,
    allocations: u64,
    site_instructions: u64,
    site_allocations: u64,
    /// What a slot per site would leave out: every entry after a site's
    /// first costs one load in place of its instructions, and allocates
    /// nothing.
    saved_instructions: f64,
    saved_allocations: f64,
    /// The heaviest sites: (instructions under it, entries, allocations, where).
    top: Vec<(u64, u64, u64, String)>,
}

fn drive(app: &Path, includes: &[PathBuf], policy: RunPolicy, seed: u64, frames: usize) -> Row {
    let size = (800, 600);
    let mut ui = Headless::panel_from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    ui.env.set_heap_stats(true);
    ui.set_policy(policy);
    let (found, places) = {
        let program = ui.env.get_program(ui.program_id()).expect("the program");
        let analysis = ui.env.static_analysis(program);
        let found = survey(program, &analysis);
        let places: Vec<String> = found
            .sites
            .iter()
            .map(|s| {
                let f = &program.functions[s.function as usize];
                let at = program.source_map.get(s.head).map_or(String::new(), |span| {
                    let file = program.source_map.file(span.file).map_or("?", |f| f.name.as_str());
                    format!("{file}:{}", span.start.line)
                });
                format!("{at} in {}", f.name.as_deref().unwrap_or("<lambda>"))
            })
            .collect();
        (found, places)
    };
    ui.env.profile_mut().set_enabled(true);
    ui.env.profile_mut().sites = Some(Box::new(SiteCounter::new(&found)));
    let scenario = Scenario::monkey(seed, frames, size);
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        let _ = ui.frame();
        ui.env.take_output();
        let allocs = ui.env.alloc_stats_for(ui.stack_id()).map_or(0, |s| s.total());
        if let Some(sites) = ui.env.profile_mut().sites.as_deref_mut() {
            sites.end_run(allocs);
        }
    }
    let sid = ui.stack_id();
    let counter = ui.env.profile_mut().sites.take().expect("the counter");
    let mut top: Vec<(u64, u64, u64, String)> = (0..found.sites.len())
        .filter(|&i| counter.entered[i] > 0)
        .map(|i| (counter.instructions[i], counter.entered[i], counter.allocations[i], places[i].clone()))
        .collect();
    top.sort_by(|a, b| b.0.cmp(&a.0));
    Row {
        sites: found.sites.len(),
        ran: top.len(),
        mutated: found.mutated,
        weightless: found.weightless,
        instructions: ui.env.instructions_retired(sid).unwrap_or(0),
        allocations: ui.env.alloc_stats_for(sid).map_or(0, |s| s.total()),
        site_instructions: counter.instructions.iter().sum(),
        site_allocations: counter.allocations.iter().sum(),
        saved_instructions: (0..found.sites.len())
            .filter(|&i| counter.entered[i] > 1)
            .map(|i| {
                let (n, e) = (counter.instructions[i] as f64, counter.entered[i] as f64);
                ((e - 1.0) * (n / e - 1.0)).max(0.0)
            })
            .sum(),
        saved_allocations: (0..found.sites.len())
            .filter(|&i| counter.entered[i] > 1)
            .map(|i| counter.allocations[i] as f64 * (counter.entered[i] as f64 - 1.0) / counter.entered[i] as f64)
            .sum(),
        top,
    }
}

/// The frames of one run, what the stack keeps for function bodies at the
/// end (slots, slots holding a value), and what the check found.
fn frames_of(
    app: &Path,
    includes: &[PathBuf],
    policy: RunPolicy,
    check: bool,
    frames: usize,
) -> (Vec<String>, usize, usize, Vec<String>) {
    let size = (800, 600);
    let mut ui = Headless::panel_from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    ui.env.set_echo(false);
    ui.env.set_seed(1);
    ui.env.set_static_check(check);
    ui.set_policy(policy);
    let scenario = Scenario::monkey(1, frames, size);
    let mut records = Vec::with_capacity(frames);
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        let outcome = ui.frame().map(|_| ());
        records.push(
            serde_json::json!({
                "commands": ui.commands,
                "state": ui.state(),
                "prints": ui.env.take_output(),
                "error": outcome.err(),
            })
            .to_string(),
        );
    }
    let values = ui.env.static_values(ui.stack_id()).unwrap_or_default();
    let filled = values
        .in_functions
        .iter()
        .filter(|(_, s)| matches!(s, petal::env::StaticSlotState::Filled(_)))
        .count();
    (records, values.in_functions.len(), filled, values.mismatches)
}

/// The oracle for function-body constants over real programs: the cache
/// must change nothing a frame draws or stores, and no value it keeps may
/// differ from what the code that builds it builds on a later call.
#[test]
fn function_constants_change_nothing_across_the_corpus() {
    let frames = 30;
    let policies = ["replay", "replay-memo", "baseline", "fast"];
    let results = par_corpus(|app, includes| {
        let mut totals = (0usize, 0usize);
        for name in policies {
            let base = RunPolicy::parse(name).unwrap();
            let (off, none, _, _) = frames_of(app, includes, base.with_static_cache(false), false, frames);
            let (on, slots, filled, _) = frames_of(app, includes, base.with_static_cache(true), false, frames);
            let (checked, _, _, mismatches) = frames_of(app, includes, base.with_static_cache(true), true, frames);
            common::assert_corpus_is_live(app, &off);
            assert_eq!(none, 0, "{}: the cache is off under {name}-static", app.display());
            for (what, side) in [("on", &on), ("checking itself", &checked)] {
                if let Some(i) = side.iter().zip(&off).position(|(a, b)| a != b) {
                    panic!("{}: frame {i} differs with the cache {what} under {name}", app.display());
                }
            }
            assert!(
                mismatches.is_empty(),
                "{} under {name}: {}",
                app.display(),
                mismatches.join("; ")
            );
            if name == "replay" {
                totals = (slots, filled);
            }
        }
        totals
    });
    let slots: usize = results.iter().map(|(_, r)| r.0).sum();
    let filled: usize = results.iter().map(|(_, r)| r.1).sum();
    let keeping = results.iter().filter(|(_, r)| r.1 > 0).count();
    eprintln!("{} apps: {slots} slots for function-body constants, {filled} filled, {keeping} apps keep one", results.len());
    assert!(
        keeping * 2 > results.len() && filled > 200,
        "only {keeping} of {} apps keep a function-body constant ({filled} of {slots} slots filled)",
        results.len()
    );
}

/// What one run of an app cost, and what its table held.
struct Cost {
    instructions: u64,
    allocations: u64,
    slots: usize,
    skips: u64,
    kinds: Vec<(&'static str, u64)>,
    collections: u64,
}

fn cost(app: &Path, includes: &[PathBuf], policy: RunPolicy, seed: u64, frames: usize) -> Cost {
    let size = (800, 600);
    let mut ui = Headless::panel_from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    ui.env.set_heap_stats(true);
    ui.set_policy(policy);
    let scenario = Scenario::monkey(seed, frames, size);
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        let _ = ui.frame();
        ui.env.take_output();
    }
    let sid = ui.stack_id();
    let values = ui.env.static_values(sid).unwrap_or_default();
    Cost {
        instructions: ui.env.instructions_retired(sid).unwrap_or(0),
        allocations: ui.env.alloc_stats_for(sid).map_or(0, |s| s.total()),
        slots: values.in_functions.len(),
        skips: values.stats.skips,
        kinds: ui.env.alloc_stats_for(sid).map_or(Vec::new(), |s| s.iter().map(|(k, n)| (k.label(), n)).collect()),
        collections: ui.env.heap_for(sid).map_or(0, |h| h.collections()),
    }
}

/// What keeping the values built inside function bodies saves on each
/// corpus app: the cache with them against the cache without, every frame
/// run (`replay`).
///
/// ```sh
/// cargo test --release --test static_in_functions -- --ignored --nocapture measure_what
/// ```
#[test]
#[ignore = "prints a table; run by hand"]
fn measure_what_keeping_them_saves_on_the_corpus() {
    let frames: usize = std::env::var("PETAL_SURVEY_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let without = RunPolicy::REPLAY.with_opts(RunPolicy::REPLAY.opts.with_static_in_functions(false));
    let rows = par_corpus(|app, includes| {
        (cost(app, includes, without, 1, frames), cost(app, includes, RunPolicy::REPLAY, 1, frames))
    });
    let root = common::repo_root();
    let f = frames as u64;
    let pct = |a: u64, b: u64| if a == 0 { 0.0 } else { (b as f64 - a as f64) * 100.0 / a as f64 };
    println!("| app | slots | instructions/frame without | with | change | allocations/frame without | with | change |");
    println!("|---|---|---|---|---|---|---|---|");
    let mut all = [0u64; 4];
    let mut slots = 0;
    for (app, (off, on)) in &rows {
        assert_eq!(off.slots, 0);
        println!(
            "| {} | {} | {} | {} | {:+.2}% | {} | {} | {:+.2}% |",
            app.strip_prefix(&root).unwrap_or(app).display(),
            on.slots,
            off.instructions / f,
            on.instructions / f,
            pct(off.instructions, on.instructions),
            off.allocations / f,
            on.allocations / f,
            pct(off.allocations, on.allocations),
        );
        for (k, v) in [off.instructions, on.instructions, off.allocations, on.allocations].into_iter().enumerate() {
            all[k] += v;
        }
        slots += on.slots;
        let _ = (off.skips, on.skips);
        if on.allocations > off.allocations || on.instructions > off.instructions {
            let kinds: Vec<String> = off
                .kinds
                .iter()
                .zip(&on.kinds)
                .filter(|(a, b)| a.1 != b.1)
                .map(|(a, b)| format!("{} {} -> {}", a.0, a.1, b.1))
                .collect();
            println!(
                "  ^ more with than without: {} (collections {} -> {})",
                kinds.join(", "),
                off.collections,
                on.collections
            );
        }
    }
    println!(
        "| all {} apps | {slots} | {} | {} | {:+.2}% | {} | {} | {:+.2}% |",
        rows.len(),
        all[0] / f,
        all[1] / f,
        pct(all[0], all[1]),
        all[2] / f,
        all[3] / f,
        pct(all[2], all[3]),
    );
}

#[test]
#[ignore = "prints a table; run by hand"]
fn measure_constants_inside_functions_on_the_corpus() {
    let frames: usize = std::env::var("PETAL_SURVEY_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let policy = std::env::var("PETAL_SURVEY_POLICY").unwrap_or_else(|_| "replay".to_string());
    let policy = RunPolicy::parse(&policy).expect("a policy name");
    let rows = par_corpus(|app, includes| drive(app, includes, policy, 1, frames));
    let root = common::repo_root();
    let f = frames as f64;
    let pct = |part: f64, whole: u64| if whole == 0 { 0.0 } else { part * 100.0 / whole as f64 };
    println!("| app | sites | ran | instructions/frame | under a site | a slot per site saves | allocations/frame | under a site | a slot per site saves |");
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut all = [0f64; 6];
    let (mut sites, mut ran, mut mutated, mut weightless) = (0, 0, 0, 0);
    let line = |name: String, sites: usize, ran: usize, v: [f64; 6]| {
        println!(
            "| {name} | {sites} | {ran} | {:.0} | {:.1} | {:.1} ({:.2}%) | {:.0} | {:.1} | {:.1} ({:.2}%) |",
            v[0] / f,
            v[1] / f,
            v[2] / f,
            pct(v[2], v[0] as u64),
            v[3] / f,
            v[4] / f,
            v[5] / f,
            pct(v[5], v[3] as u64),
        );
    };
    for (app, r) in &rows {
        let v = [
            r.instructions as f64,
            r.site_instructions as f64,
            r.saved_instructions,
            r.allocations as f64,
            r.site_allocations as f64,
            r.saved_allocations,
        ];
        line(app.strip_prefix(&root).unwrap_or(app).display().to_string(), r.sites, r.ran, v);
        for (k, x) in v.into_iter().enumerate() {
            all[k] += x;
        }
        sites += r.sites;
        ran += r.ran;
        mutated += r.mutated;
        weightless += r.weightless;
    }
    line(format!("all {} apps", rows.len()), sites, ran, all);
    println!("left out: {mutated} whose value reaches a write, {weightless} that allocate and call nothing");
    println!("\nthe heaviest sites of each app (instructions, entries, allocations per frame):");
    for (app, r) in &rows {
        for (i, e, a, at) in r.top.iter().take(3) {
            println!(
                "  {}: {:.1} / {:.1} / {:.1}  {at}",
                app.strip_prefix(&root).unwrap_or(app).display(),
                *i as f64 / f,
                *e as f64 / f,
                *a as f64 / f
            );
        }
    }
}
