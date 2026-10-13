//! What the static part of a frame costs, with the static-value cache off
//! and on, for a program whose host is not here to run the rest.
//!
//! ```text
//! cargo run --release --example static_frame -- <file.ptl> [frames]
//! ```
//!
//! A script written for a host cannot run under the plain CLI: its dynamic
//! code calls natives nothing here registers. Its static declarations can,
//! because static code calls only constant natives. This runs the static-only
//! build of the file (`petal run --static-only`: every top-level statement
//! that is not static is jumped over) for a number of frames on one stack,
//! once with the cache off and once with it on, and reports the instructions
//! and allocations of the first frame and of a later one.
//!
//! The difference between the two later frames is what the cache takes out
//! of every frame of the real program: the dynamic statements cost the same
//! on both sides. It is exact for instructions and allocations and says
//! nothing about what the dynamic code then does with a kept table (a memo
//! hit on an argument that is the same object, for one).

use std::path::{Path, PathBuf};

use petal::env::Env;
use petal::policy::RunPolicy;
use petal::static_audit::StaticAuditMode;
use petal::stats::AllocKind;

struct Frame {
    instructions: u64,
    allocations: [u64; AllocKind::ALL.len()],
    micros: f64,
}

fn measure(file: &Path, source: &str, cache: bool, frames: usize) -> (Vec<Frame>, String) {
    let mut env = Env::new();
    env.set_echo(false);
    env.set_check_only(true);
    env.set_heap_stats(true);
    env.set_policy(RunPolicy::FAST.with_static_cache(cache));
    env.set_static_audit(StaticAuditMode::StaticOnly);
    env.set_static_only_keeps(true);
    let pid = env.load_program_diag(source, Some(file)).unwrap_or_else(|e| panic!("{e}"));
    let sid = env.create_stack(pid).unwrap();
    let mut out = Vec::new();
    let counts = |env: &Env| {
        let stats = env.alloc_stats_for(sid).expect("stack");
        (env.instructions_retired(sid).unwrap_or(0), AllocKind::ALL.map(|k| stats.get(k)))
    };
    for _ in 0..frames {
        env.reset_stack(sid).unwrap();
        let before = counts(&env);
        let t = std::time::Instant::now();
        if let Err(e) = env.run(sid) {
            panic!("the static-only build raised: {e}");
        }
        let micros = t.elapsed().as_secs_f64() * 1e6;
        let after = counts(&env);
        env.take_output();
        let mut allocations = [0; AllocKind::ALL.len()];
        for (i, a) in allocations.iter_mut().enumerate() {
            *a = after.1[i] - before.1[i];
        }
        out.push(Frame { instructions: after.0 - before.0, allocations, micros });
    }
    let report = env.static_audit_report();
    let values = env.static_values(sid).unwrap_or_default();
    let filled = values
        .slots
        .iter()
        .filter(|(_, s)| matches!(s, petal::env::StaticSlotState::Filled(_)))
        .count();
    let summary = format!(
        "{} static declarations, {} statements run, {} skipped; {} slots ({} filled), {} checks a frame over {} segments, {} rejected; {} audit findings",
        report.static_declarations,
        report.statements_run,
        report.statements_skipped,
        values.slots.len(),
        filled,
        values.runs,
        values.segments,
        values.warnings().len(),
        report.findings.len(),
    );
    (out, summary)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(file) = args.next().map(PathBuf::from) else {
        eprintln!("usage: static_frame <file.ptl> [frames]");
        std::process::exit(2);
    };
    let frames: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(30).max(3);
    let source = std::fs::read_to_string(&file).expect("script file");
    println!("{}: the static-only build, {frames} frames on one stack", file.display());
    let mut later = Vec::new();
    for cache in [false, true] {
        let (run, summary) = measure(&file, &source, cache, frames);
        println!("cache {}: {summary}", if cache { "on" } else { "off" });
        let mut times: Vec<f64> = run[1..].iter().map(|f| f.micros).collect();
        times.sort_by(|a, b| a.total_cmp(b));
        for (label, f) in [("first frame", &run[0]), ("later frame", &run[frames - 1])] {
            let kinds: Vec<String> = AllocKind::ALL
                .iter()
                .zip(f.allocations)
                .filter(|(_, n)| *n > 0)
                .map(|(k, n)| format!("{} {n}", k.label()))
                .collect();
            println!(
                "  {label}: {} instructions, {} allocations ({})",
                f.instructions,
                f.allocations.iter().sum::<u64>(),
                kinds.join(", ")
            );
        }
        println!("  later frames: min {:.1} µs, median {:.1} µs", times[0], times[times.len() / 2]);
        later.push((run[frames - 1].instructions, run[frames - 1].allocations));
    }
    let (off, on) = (&later[0], &later[1]);
    let map = AllocKind::ALL.iter().position(|k| *k == AllocKind::Map).unwrap();
    println!(
        "per frame the cache takes out {} instructions, {} allocations, {} of them records",
        off.0 as i64 - on.0 as i64,
        off.1.iter().sum::<u64>() as i64 - on.1.iter().sum::<u64>() as i64,
        off.1[map] as i64 - on.1[map] as i64,
    );
}
