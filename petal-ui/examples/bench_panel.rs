//! Time a panel script's per-frame cost under the headless harness.
//!
//!   cargo run --release --example bench_panel -- <file.ptl> [frames] [WxH]
//!       [--observe] [--profile] [--policy <name>] [--no-gate] [--no-memo] [--wiggle]
//!       [--scenario s.json|monkey:<seed>]
//!
//! Frames run under the `fast` run policy unless `--policy` names another
//! (see `petal::policy`); `--no-gate` / `--no-memo` switch one layer off.
//! Under the frame gate, with no input change a
//! script that reads no clock is skipped after its first frame, so a quiet
//! bench measures the gate rather than the script. `--wiggle` moves the
//! pointer one pixel each frame, the typical interactive frame. Calls are
//! memoized (see docs/dev/memo-scopes.md) unless the policy says not; the
//! memo's counters are reported either way.
//!
//! `--scenario` drives the frames with a `petal-ui-run` scenario (see
//! docs/dev/headless-ui-run.md) instead of a still or wiggling pointer, so a
//! realistic session can be timed. Frames the gate skipped cost microseconds
//! and would drown the percentiles, so the ones that ran are also reported
//! on their own, along with the session's total script time.
use std::time::Instant;

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut scenario_spec: Option<String> = None;
    let mut policy = None;
    let mut flags: Vec<String> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut it = raw.into_iter();
    while let Some(a) = it.next() {
        if a == "--scenario" {
            scenario_spec = Some(it.next().expect("--scenario wants a file or monkey:<seed>"));
        } else if a == "--policy" {
            let spec = it.next().expect("--policy wants a run policy name");
            policy = Some(petal::policy::RunPolicy::parse(&spec).unwrap_or_else(|e| panic!("{e}")));
        } else if a.starts_with("--") {
            flags.push(a);
        } else {
            positional.push(a);
        }
    }
    let path = positional
        .first()
        .expect("usage: bench_panel <file.ptl> [frames] [WxH]");
    let frames: usize = positional.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let (w, h) = positional
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1200, 800));

    // Garden panels run with observation on (that is how `panel.values` and
    // the debug server's /state read a frame's bindings), so the flag mirrors
    // the real embedding rather than the harness default.
    let observe = flags.iter().any(|a| a == "--observe");
    let profile = flags.iter().any(|a| a == "--profile");
    let no_gate = flags.iter().any(|a| a == "--no-gate");
    let no_memo = flags.iter().any(|a| a == "--no-memo");
    let wiggle = flags.iter().any(|a| a == "--wiggle");
    let scenario = scenario_spec.map(|spec| match spec.strip_prefix("monkey:") {
        Some(seed) => petal_ui::scenario::Scenario::monkey(
            seed.parse().expect("monkey seed"),
            frames,
            (w, h),
        ),
        None => petal_ui::scenario::Scenario::from_json_str(
            &std::fs::read_to_string(&spec).expect("read scenario"),
        )
        .expect("parse scenario"),
    });

    let src = std::fs::read_to_string(path).expect("read script");
    let compile_start = Instant::now();
    let mut ui = petal_ui::harness::Headless::with_size(&src, w, h).expect("compile");
    let compile_ms = compile_start.elapsed().as_secs_f64() * 1e3;

    let mut policy = policy.unwrap_or_else(|| ui.policy());
    policy.gate &= !no_gate;
    policy.memo &= !no_memo;
    ui.set_policy(policy);
    if observe {
        ui.env.observations_mut().enable();
    }
    if profile {
        ui.env.profile_mut().set_enabled(true);
    }

    // Warm up (first frame initializes `state`).
    let n_cmds = ui.frame().expect("first frame").len();

    let mut times = Vec::with_capacity(frames);
    let mut run_times = Vec::with_capacity(frames);
    for i in 0..frames {
        if let Some(s) = &scenario {
            // Frame 0 of the scenario is the first timed frame.
            s.apply(&mut ui, i);
        }
        if wiggle {
            ui.mouse_move(100 + (i % 2) as i32, 100);
        }
        let skipped_before = ui.frames_skipped;
        let t = Instant::now();
        ui.frame().expect("frame");
        let ms = t.elapsed().as_secs_f64() * 1e3;
        times.push(ms);
        if ui.frames_skipped == skipped_before {
            run_times.push(ms);
        }
    }
    let wall: f64 = times.iter().sum();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = times.iter().sum();
    println!("compile: {compile_ms:.1} ms");
    println!("draw commands: {n_cmds}");
    println!(
        "frames run: {}  skipped: {}",
        ui.frames_run.saturating_sub(1),
        ui.frames_skipped
    );
    let m = ui.memo_stats();
    println!(
        "memo: hits {}  misses {}  records {}  inlined {}  effectful {}  reexecs {}  cutoffs {}  cold {}  slots {}",
        m.hits,
        m.misses,
        m.records,
        m.inlined,
        m.effectful,
        m.reexecs,
        m.cutoffs,
        m.cold,
        ui.env.memo_slots(ui.stack_id()),
    );
    println!(
        "frame ms: min {:.2}  p50 {:.2}  p90 {:.2}  max {:.2}  mean {:.2}",
        times[0],
        times[times.len() / 2],
        times[times.len() * 9 / 10],
        times[times.len() - 1],
        total / times.len() as f64,
    );
    if !run_times.is_empty() {
        run_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "run-frame ms: min {:.2}  p50 {:.2}  p90 {:.2}  max {:.2}  ({} frames)",
            run_times[0],
            run_times[run_times.len() / 2],
            run_times[run_times.len() * 9 / 10],
            run_times[run_times.len() - 1],
            run_times.len(),
        );
    }
    println!("total script ms: {total:.1}");
    if profile {
        let elapsed = std::time::Duration::from_secs_f64(wall / 1e3);
        print!(
            "{}",
            ui.env
                .profile()
                .report(Some(elapsed), |nid| ui.env.native_fn_name(nid), 12)
        );
    }
}
