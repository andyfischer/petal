//! The headless wall clock: `time()` advances by the harness's fixed `dt` on
//! every frame, so animation written against the clock (the `ui` prelude's
//! `elapsed`, `spinner`) actually runs in a headless trace.
//!
//! The clock is a pure function of the frame count — never the system clock —
//! which is what keeps two runs of the same script byte-identical.

use petal_ui::harness::{FRAME_DT, Headless};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_petal-ui-run");

fn close(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() < 1e-9, "{what}: {a} vs {b}");
}

#[test]
fn time_advances_by_dt_each_frame_and_equals_the_accumulated_dt() {
    // The script sums `dt()` itself; the harness clock must track that sum.
    let src = "state acc = 0.0\n\
               state t = 0.0\n\
               t = time()\n\
               acc = acc + dt()";
    let mut ui = Headless::new(src).unwrap();

    assert_eq!(ui.time, 0.0, "the harness starts at a deterministic t0 = 0");
    for frame in 0..30 {
        ui.frame().unwrap();
        let t = ui.state_float("t").unwrap();
        let acc = ui.state_float("acc").unwrap();
        // Frame N is stamped at N × dt; by the end of it, dt seconds more have
        // been published, which is exactly what the script accumulated.
        close(t, frame as f64 * FRAME_DT, "time() at frame N");
        close(acc, t + FRAME_DT, "accumulated dt");
        close(ui.time, acc, "the clock is the accumulated dt");
    }
}

#[test]
fn elapsed_measures_real_progress_without_the_driver_touching_the_clock() {
    // Before this, `elapsed()` returned 0.0 for every frame of every headless
    // run, so nothing written against the clock could be tested at all.
    let src = "state e = 0.0\ne = elapsed()";
    let mut ui = Headless::new(src).unwrap();
    ui.frames(60).unwrap();
    close(
        ui.state_float("e").unwrap(),
        59.0 * FRAME_DT,
        "elapsed() since its first call on frame 0",
    );
}

#[test]
fn assigning_the_clock_still_wins_for_the_next_frame() {
    // Tests that jump the clock (tooltip delays, long fades) keep working: the
    // assignment is what the next frame publishes, and the automatic advance
    // resumes from there.
    let src = "state t = 0.0\nt = time()";
    let mut ui = Headless::new(src).unwrap();
    ui.time = 10.0;
    ui.frame().unwrap();
    close(ui.state_float("t").unwrap(), 10.0, "assigned clock");
    ui.frame().unwrap();
    close(
        ui.state_float("t").unwrap(),
        10.0 + FRAME_DT,
        "then it advances again",
    );
}

#[test]
fn two_runs_of_a_clock_driven_app_produce_identical_traces() {
    let dir = std::env::temp_dir().join(format!("petal-ui-clock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let app = dir.join("app.ptl");
    std::fs::write(
        &app,
        "state t = 0.0\nstate e = 0.0\nt = time()\ne = elapsed()\n\
         draw_rect(10 + floor(time() * 60.0), 10, 20, 20, 255, 0, 0)\n",
    )
    .unwrap();

    let go = |name: &str| -> Vec<u8> {
        let out = dir.join(name);
        let status = Command::new(BIN)
            .args([
                app.to_str().unwrap(),
                "--frames",
                "20",
                "--out",
                out.to_str().unwrap(),
            ])
            .status()
            .expect("spawn");
        assert!(status.success(), "petal-ui-run failed");
        std::fs::read(&out).unwrap()
    };
    let a = go("a.jsonl");
    let b = go("b.jsonl");
    assert_eq!(a, b, "the same script twice is byte-identical");

    // And the clock actually moved inside that trace.
    let times: Vec<f64> = String::from_utf8(a)
        .unwrap()
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["state"]["t"].as_f64().unwrap()
        })
        .collect();
    assert_eq!(times.len(), 20);
    close(times[0], 0.0, "frame 0");
    close(times[19], 19.0 * FRAME_DT, "frame 19");
    assert!(
        times.windows(2).all(|w| w[1] > w[0]),
        "time() is strictly increasing across the trace: {times:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
