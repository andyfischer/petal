//! What `ui/drag`'s `between` and `vary` cost per frame, on the P2 examples.
//!
//!   cargo run --release --example bench_vary -- [--runs N]
//!
//! Drives each example's `scenarios/drag.json` (`examples/ui/d2-switch`,
//! `d2-dial`, `d2-scheduler`) from a fresh harness per run, with the frame
//! gate off so every frame is timed, and reports the minimum over `--runs`
//! (default 15) of each frame, as the median over a span of frames:
//!
//! - **idle**: frames before the press (the view drawn once; frame 0, which
//!   also builds the program's first state, is left out);
//! - **drag**: frames while the element is held, after the drag started —
//!   the median and the worst (a frame where the pointer moved and `vary`
//!   took a step; on a frame where it is already under the pointer `vary`
//!   renders nothing extra).
//!
//! `between` reads its table (no speculation per frame); `vary` renders the
//! view hypothetically once per lever for the sensitivities plus once or
//! twice for the step, every frame. The dial is also run with forward
//! differences instead of dual numbers.
use std::time::Instant;

struct Case {
    name: &'static str,
    dir: &'static str,
    /// (from, to) replacements in the app's source.
    edit: Option<(&'static str, &'static str)>,
    idle: (usize, usize),
    drag: (usize, usize),
}

fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut runs = 15usize;
    while let Some(a) = args.next() {
        if a == "--runs" {
            runs = args.next().and_then(|s| s.parse().ok()).expect("--runs N");
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ui");
    let cases = [
        Case { name: "switch: between, 3 targets", dir: "d2-switch", edit: None, idle: (1, 3), drag: (6, 20) },
        Case { name: "dial: vary, 1 lever, dual", dir: "d2-dial", edit: None, idle: (1, 3), drag: (6, 20) },
        Case {
            name: "dial: vary, 1 lever, differences",
            dir: "d2-dial",
            edit: Some((r#"{sensitivity: "dual"}"#, "{}")),
            idle: (1, 3),
            drag: (6, 20),
        },
        Case {
            name: "scheduler: bar, between 3 rooms",
            dir: "d2-scheduler",
            edit: None,
            idle: (1, 3),
            drag: (6, 14),
        },
        Case {
            name: "scheduler: end, vary + constraint",
            dir: "d2-scheduler",
            edit: None,
            idle: (1, 3),
            drag: (34, 48),
        },
    ];
    println!("{runs} runs, min per frame, median over the span (ms):");
    println!("  {:<36} {:>7} {:>7} {:>7}", "", "idle", "drag", "worst");
    for c in &cases {
        let dir = root.join(c.dir);
        let mut src = std::fs::read_to_string(dir.join("app.ptl")).expect("read app");
        if let Some((a, b)) = c.edit {
            assert!(src.contains(a), "{}: no `{a}`", c.dir);
            src = src.replace(a, b);
        }
        let scenario = petal_ui_headless::scenario::Scenario::from_json_str(
            &std::fs::read_to_string(dir.join("scenarios/drag.json")).expect("read scenario"),
        )
        .expect("parse scenario");
        let sj: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("scenarios/drag.json")).unwrap()).unwrap();
        let size = sj["size"].as_array().unwrap();
        let (w, h) = (size[0].as_i64().unwrap() as i32, size[1].as_i64().unwrap() as i32);
        let frames = c.drag.1.max(c.idle.1);
        let mut best = vec![f64::INFINITY; frames];
        for _ in 0..runs {
            let mut ui = petal_ui_headless::harness::Headless::with_size(&src, w, h).expect("compile");
            ui.env.set_echo(false);
            let mut policy = ui.policy();
            policy.gate = false;
            ui.set_policy(policy);
            for (i, b) in best.iter_mut().enumerate() {
                scenario.apply(&mut ui, i);
                let t = Instant::now();
                ui.frame().expect("frame");
                *b = b.min(t.elapsed().as_secs_f64() * 1e3);
            }
        }
        let idle = median(&best[c.idle.0..c.idle.1]);
        let drag = median(&best[c.drag.0..c.drag.1]);
        let worst = best[c.drag.0..c.drag.1].iter().cloned().fold(0.0, f64::max);
        println!("  {:<36} {:>7.3} {:>7.3} {:>7.3}", c.name, idle, drag, worst);
    }
}
