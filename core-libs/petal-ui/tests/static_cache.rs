//! The static-value cache over the panel corpus. See `petal::static_cache`
//! and docs/tasks/static-value-optimization.md (S1).
//!
//! The cache keeps top-level values that depend on no dynamic data between
//! frames and skips the code that builds them. It must change nothing a
//! frame draws or stores. This is the differential oracle for that: every
//! corpus app, driven by a monkey scenario, frame for frame the same with
//! the cache on and off, under each combination of the other switches that
//! change what a frame executes (the optimizer, memoization, the frame
//! gate).
//!
//! Each of them again with observations on, which is how Garden runs a
//! panel: a frame that skips a static segment must leave the buffer a host
//! reads (`Env::get_observations_json`, and the buffer itself term by
//! term) as the frame that executed it does. See `petal::static_cache`,
//! "Observations".

use std::path::{Path, PathBuf};

mod common;
use common::{assert_corpus_is_live, par_corpus};

use petal::policy::RunPolicy;
use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;

struct Run {
    frames: Vec<String>,
    /// Runs of static instructions jumped over, and instructions saved.
    skips: u64,
    skipped_instructions: u64,
    /// Observations recorded again for skipped segments.
    replayed: u64,
    slots: usize,
    rejected: Vec<String>,
    instructions: u64,
    allocations: u64,
}

fn drive(app: &Path, includes: &[PathBuf], policy: RunPolicy, seed: u64, frames: usize) -> Run {
    drive_observed(app, includes, policy, seed, frames, false)
}

/// What a host that observes reads after a frame: the map by qualified
/// name, and the buffer term by term (ids are the same on both sides: one
/// compile of one text).
fn observed(ui: &Headless) -> serde_json::Value {
    let by_name = ui.env.get_observations_json(ui.program_id(), ui.stack_id());
    let heap = ui.env.heap_for(ui.stack_id()).expect("the stack's heap");
    let mut by_term: Vec<(u32, serde_json::Value)> =
        ui.env.observations().iter().map(|(t, v)| (t.0, petal::value::value_to_json(&v, heap))).collect();
    by_term.sort_by_key(|(t, _)| *t);
    serde_json::json!({ "names": by_name, "terms": by_term })
}

fn drive_observed(
    app: &Path,
    includes: &[PathBuf],
    policy: RunPolicy,
    seed: u64,
    frames: usize,
    observe: bool,
) -> Run {
    let size = (800, 600);
    let mut ui = Headless::panel_from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    ui.env.set_heap_stats(true);
    ui.set_policy(policy);
    if observe {
        ui.env.observations_mut().enable();
    }
    let scenario = Scenario::monkey(seed, frames, size);
    let mut records = Vec::with_capacity(frames);
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        let outcome = ui.frame().map(|_| ());
        let record = serde_json::json!({
            "commands": ui.commands,
            "state": ui.state(),
            "prints": ui.env.take_output(),
            "error": outcome.err(),
            "observed": if observe { observed(&ui) } else { serde_json::Value::Null },
        });
        records.push(record.to_string());
    }
    let sid = ui.stack_id();
    let values = ui.env.static_values(sid).unwrap_or_default();
    Run {
        frames: records,
        skips: values.stats.skips,
        skipped_instructions: values.stats.skipped_instructions,
        replayed: values.stats.replayed,
        slots: values.slots.len(),
        rejected: values.warnings(),
        instructions: ui.env.instructions_retired(sid).unwrap_or(0),
        allocations: ui.env.alloc_stats_for(sid).map_or(0, |s| s.total()),
    }
}

/// Where two frame records first differ, for a readable failure.
fn first_difference(a: &str, b: &str) -> String {
    let (a, b): (serde_json::Value, serde_json::Value) =
        (serde_json::from_str(a).unwrap(), serde_json::from_str(b).unwrap());
    // What was observed, name by name.
    if let (Some(x), Some(y)) = (a["observed"]["names"].as_object(), b["observed"]["names"].as_object()) {
        for (k, v) in x {
            if y.get(k) != Some(v) {
                return format!("observed {k}: {v} vs {}", y.get(k).map_or("<absent>".to_string(), |v| v.to_string()));
            }
        }
        if let Some(k) = y.keys().find(|k| !x.contains_key(*k)) {
            return format!("observed {k}: <absent> vs {}", y[k]);
        }
    }
    for field in ["error", "prints", "commands", "state", "observed"] {
        let (x, y) = (&a[field], &b[field]);
        if x == y {
            continue;
        }
        return match (x, y) {
            (serde_json::Value::Array(xs), serde_json::Value::Array(ys)) => {
                let i = xs.iter().zip(ys).position(|(p, q)| p != q).unwrap_or(xs.len().min(ys.len()));
                format!(
                    "{field}[{i}] of {}/{}: {} vs {}",
                    xs.len(),
                    ys.len(),
                    xs.get(i).map_or("<none>".to_string(), |v| v.to_string()),
                    ys.get(i).map_or("<none>".to_string(), |v| v.to_string()),
                )
            }
            _ => format!("{field}: {x} vs {y}"),
        };
    }
    "no field differs".to_string()
}

/// The cache against no cache, with everything else held equal, for each
/// setting of everything else.
#[test]
fn frames_are_the_same_with_the_cache_on_and_off_across_the_corpus() {
    let frames = 45;
    let seed = 1u64;
    // The optimizer on with and without the memo; the optimizer off; and the
    // default policy, where the frame gate also decides which frames run.
    let policies = ["replay", "replay-memo", "baseline", "fast"];
    let results = par_corpus(|app, includes| {
        let mut totals = (0u64, 0usize, Vec::new());
        for name in policies {
            let base = RunPolicy::parse(name).unwrap();
            let off = drive(app, includes, base.with_static_cache(false), seed, frames);
            let on = drive(app, includes, base.with_static_cache(true), seed, frames);
            assert_corpus_is_live(app, &off.frames);
            assert_eq!(off.slots, 0, "{}: the cache is off under {name}-static", app.display());
            for (i, (a, b)) in off.frames.iter().zip(&on.frames).enumerate() {
                assert!(
                    a == b,
                    "{} seed {seed}: frame {i} differs with the static cache on under {name}: {}",
                    app.display(),
                    first_difference(a, b),
                );
            }
            if name == "replay" {
                totals = (on.skips, on.slots, on.rejected);
            }
        }
        totals
    });
    let skips: u64 = results.iter().map(|(_, r)| r.0).sum();
    let slots: usize = results.iter().map(|(_, r)| r.1).sum();
    let keeping = results.iter().filter(|(_, r)| r.1 > 0).count();
    assert!(skips > 0, "no static run was ever skipped across the corpus");
    assert!(
        keeping * 2 > results.len(),
        "only {keeping} of {} apps keep any static value ({slots} slots)",
        results.len()
    );
    // Nothing in the corpus builds a static value that cannot be kept.
    let rejected: Vec<String> = results
        .iter()
        .flat_map(|(app, r)| r.2.iter().map(move |w| format!("{}: {w}", app.display())))
        .collect();
    assert!(rejected.is_empty(), "{}", rejected.join("\n"));
}

/// The same with observations on, which is how Garden runs every panel: the
/// cache used to be off there. What a host reads from the buffer after each
/// frame is part of the frame.
#[test]
fn observations_are_the_same_with_the_cache_on_and_off_across_the_corpus() {
    let frames = 45;
    let seed = 1u64;
    let policies = ["replay", "replay-memo", "baseline", "fast"];
    let results = par_corpus(|app, includes| {
        let mut totals = (0u64, 0usize, 0u64);
        for name in policies {
            let base = RunPolicy::parse(name).unwrap();
            let off = drive_observed(app, includes, base.with_static_cache(false), seed, frames, true);
            let on = drive_observed(app, includes, base.with_static_cache(true), seed, frames, true);
            assert_corpus_is_live(app, &off.frames);
            assert_eq!(off.slots, 0, "{}: the cache is off under {name}-static", app.display());
            for (i, (a, b)) in off.frames.iter().zip(&on.frames).enumerate() {
                assert!(
                    a == b,
                    "{} seed {seed}: frame {i} differs with the static cache on under {name}, observed: {}",
                    app.display(),
                    first_difference(a, b),
                );
            }
            if name == "replay" {
                totals = (on.skips, on.slots, on.replayed);
            }
        }
        totals
    });
    let skips: u64 = results.iter().map(|(_, r)| r.0).sum();
    let slots: usize = results.iter().map(|(_, r)| r.1).sum();
    let replayed: u64 = results.iter().map(|(_, r)| r.2).sum();
    let keeping = results.iter().filter(|(_, r)| r.1 > 0).count();
    eprintln!("{} apps observed: {keeping} keep a value, {slots} slots, {skips} skips, {replayed} observations replayed", results.len());
    assert!(skips > 0 && replayed > 0, "nothing was skipped ({skips}) or replayed ({replayed}) under observation");
    assert!(
        keeping * 2 > results.len(),
        "only {keeping} of {} apps keep any static value under observation ({slots} slots)",
        results.len()
    );
}

/// What the cache saves on each corpus app: instructions and allocations
/// per frame with it on and off, every frame run (`replay`). Not a test of
/// anything: run it for the numbers.
///
/// ```sh
/// cargo test --release --test static_cache -- --ignored --nocapture measure
/// ```
#[test]
#[ignore = "prints a table; run by hand"]
fn measure_what_the_cache_saves_on_the_corpus() {
    measure(false);
}

/// The same table with observations on: what a Garden panel pays and
/// saves, since Garden observes every panel. Cache off is what every
/// panel ran before the cache stayed on under observation.
///
/// ```sh
/// cargo test --release --test static_cache -- --ignored --nocapture measure_observed
/// ```
#[test]
#[ignore = "prints a table; run by hand"]
fn measure_observed_what_the_cache_saves_on_the_corpus() {
    measure(true);
}

fn measure(observe: bool) {
    let frames = 60;
    let seed = 1u64;
    let rows = par_corpus(|app, includes| {
        let off = drive_observed(app, includes, RunPolicy::REPLAY.with_static_cache(false), seed, frames, observe);
        let on = drive_observed(app, includes, RunPolicy::REPLAY, seed, frames, observe);
        (off.instructions, on.instructions, off.allocations, on.allocations, on.slots, on.skipped_instructions)
    });
    let root = common::repo_root();
    println!("| app | slots | instructions/frame off | on | change | allocations/frame off | on | change |");
    println!("|---|---|---|---|---|---|---|---|");
    let pct = |a: u64, b: u64| if a == 0 { 0.0 } else { (b as f64 - a as f64) * 100.0 / a as f64 };
    let (mut ti, mut tj, mut ta, mut tb) = (0u64, 0u64, 0u64, 0u64);
    for (app, (i_off, i_on, a_off, a_on, slots, _)) in &rows {
        let f = frames as u64;
        println!(
            "| {} | {slots} | {} | {} | {:+.1}% | {} | {} | {:+.1}% |",
            app.strip_prefix(&root).unwrap_or(app).display(),
            i_off / f,
            i_on / f,
            pct(*i_off, *i_on),
            a_off / f,
            a_on / f,
            pct(*a_off, *a_on),
        );
        ti += i_off;
        tj += i_on;
        ta += a_off;
        tb += a_on;
    }
    println!(
        "| all {} apps | {} | {} | {} | {:+.2}% | {} | {} | {:+.2}% |",
        rows.len(),
        rows.iter().map(|(_, r)| r.4).sum::<usize>(),
        ti / frames as u64,
        tj / frames as u64,
        pct(ti, tj),
        ta / frames as u64,
        tb / frames as u64,
        pct(ta, tb),
    );
}
