//! Time a panel script's per-frame cost under the headless harness.
//!
//!   cargo run --release --example bench_panel -- <file.ptl> [frames] [WxH]
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args
        .first()
        .expect("usage: bench_panel <file.ptl> [frames] [WxH]");
    let frames: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let (w, h) = args
        .get(2)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1200, 800));

    // Garden panels run with observation on (that is how `panel.values` and
    // the debug server's /state read a frame's bindings), so the flag mirrors
    // the real embedding rather than the harness default.
    let observe = args.iter().any(|a| a == "--observe");
    let profile = args.iter().any(|a| a == "--profile");

    let src = std::fs::read_to_string(path).expect("read script");
    let compile_start = Instant::now();
    let mut ui = petal_ui::harness::Headless::with_size(&src, w, h).expect("compile");
    let compile_ms = compile_start.elapsed().as_secs_f64() * 1e3;

    if observe {
        ui.env.observations_mut().enable();
    }
    if profile {
        ui.env.profile_mut().set_enabled(true);
    }

    // Warm up (first frame initializes `state`).
    let n_cmds = ui.frame().expect("first frame").len();

    let mut times = Vec::with_capacity(frames);
    for _ in 0..frames {
        let t = Instant::now();
        ui.frame().expect("frame");
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let wall: f64 = times.iter().sum();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = times.iter().sum();
    println!("compile: {compile_ms:.1} ms");
    println!("draw commands: {n_cmds}");
    println!(
        "frame ms: min {:.2}  p50 {:.2}  p90 {:.2}  max {:.2}  mean {:.2}",
        times[0],
        times[times.len() / 2],
        times[times.len() * 9 / 10],
        times[times.len() - 1],
        total / times.len() as f64,
    );
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
