//! The frame gate: a frame whose inputs are exactly what the last run read,
//! and whose last run settled, is skipped and its output retained.
//!
//! Two kinds of test. The first pins the gate's *semantics* on small scripts:
//! what makes a frame run (a read binding moving, a `state` write, the clock,
//! randomness, host data) and what does not. The second is the differential
//! oracle over the example corpus: every panel app, driven by a monkey
//! scenario, must produce the same commands and state frame-for-frame whether
//! the gate is on or off. A gate that skips a frame it should have run shows
//! up here as a stale frame.

use std::path::{Path, PathBuf};

mod common;
use common::{assert_corpus_is_live, corpus};

use petal::run_deps::RunReason;
use petal_ui::harness::Headless;
use petal_ui::host_data::fixture_provider;
use petal_ui::scenario::Scenario;

fn ui(src: &str) -> Headless {
    let mut ui = Headless::new(src).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.env.set_echo(false);
    ui
}

/// Run `n` quiet frames and return how many of them ran the script.
fn quiet_runs(ui: &mut Headless, n: usize) -> u64 {
    let before = ui.frames_run;
    for _ in 0..n {
        ui.frame().unwrap();
    }
    ui.frames_run - before
}

#[test]
fn a_script_that_reads_nothing_runs_once_and_is_then_skipped() {
    let mut ui = ui("draw_rect({x: 0, y: 0, w: 10, h: 10}, {r: 1, g: 2, b: 3, a: 255})");
    ui.frame().unwrap();
    assert!(!ui.last_frame_skipped);
    assert_eq!(ui.last_run_reason, Some(RunReason::NoRecord));
    let first = ui.commands.clone();
    assert_eq!(quiet_runs(&mut ui, 10), 0, "nothing read, nothing changes");
    assert!(ui.last_frame_skipped);
    assert_eq!(ui.commands, first, "a skipped frame serves the retained output");
    assert_eq!(ui.frames_run, 1);
    assert_eq!(ui.frames_skipped, 10);
}

#[test]
fn the_gate_can_be_turned_off() {
    let mut ui = ui("draw_rect({x: 0, y: 0, w: 10, h: 10}, {r: 1, g: 2, b: 3, a: 255})");
    ui.gate = false;
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 5), 5);
    assert_eq!(ui.frames_skipped, 0);
}

#[test]
fn a_read_binding_moving_runs_the_frame_and_an_unread_one_does_not() {
    let mut ui = ui("let x = mouse_x()\n\
                     draw_rect({x: x, y: 0, w: 10, h: 10}, {r: 1, g: 2, b: 3, a: 255})");
    ui.mouse_move(5, 5);
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 3), 0);

    // Vertical motion only: mouse_y is bound but never read.
    ui.mouse_move(5, 50);
    ui.frame().unwrap();
    assert!(ui.last_frame_skipped, "mouse_y was never read");

    ui.mouse_move(7, 50);
    ui.frame().unwrap();
    assert!(!ui.last_frame_skipped);
    let sym = ui.env.intern_symbol("mouse_x");
    assert_eq!(ui.last_run_reason, Some(RunReason::BindingChanged(sym)));
    assert!(matches!(
        ui.commands[0],
        petal_ui::draw::DrawCommand::Rect { x: 7, .. }
    ));
}

#[test]
fn hover_probes_run_only_when_the_pointer_moves() {
    let mut ui = ui("let r = {x: 0, y: 0, w: 50, h: 50}\n\
                     let c = if hovered(r) then {r: 9, g: 9, b: 9, a: 255} else {r: 1, g: 1, b: 1, a: 255} end\n\
                     draw_rect(r, c)");
    ui.mouse_move(100, 100);
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 4), 0);
    ui.mouse_move(10, 10);
    ui.frame().unwrap();
    assert!(!ui.last_frame_skipped);
    assert!(matches!(
        ui.commands[0],
        petal_ui::draw::DrawCommand::Rect { r: 9, g: 9, b: 9, .. }
    ));
    assert_eq!(quiet_runs(&mut ui, 4), 0);
}

#[test]
fn a_state_write_that_changes_the_slot_keeps_the_frame_running() {
    let mut ui = ui("state n = 0\n\
                     n = n + 1\n\
                     draw_text(\"{n}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 5), 5, "a counter never settles");
    assert_eq!(ui.last_run_reason, Some(RunReason::StateUnsettled));
    assert_eq!(ui.state_int("n"), Some(6));
}

#[test]
fn a_state_write_of_an_equal_value_settles() {
    // Rewritten every frame, but with the value it already holds.
    let mut ui = ui("state n = 0\n\
                     n = mouse_x() * 0\n\
                     draw_text(\"{n}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    // Frame 1 created the slot, so frame 2 runs once more; then it is quiet.
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 5), 0);
}

#[test]
fn a_record_rebuilt_with_equal_contents_settles() {
    let mut ui = ui("state r = {a: 1, b: \"x\"}\n\
                     r = {a: 1, b: \"x\"}\n\
                     draw_text(\"{r.a}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 5), 0, "records compare by content for the gate");
}

#[test]
fn a_state_var_set_is_seen_through_the_cell() {
    let mut ui = ui("state var n = 0\n\
                     if mouse_pressed(0) then set n = get n + 1 end\n\
                     draw_text(\"{get n}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 3), 0);

    ui.click(5, 5).unwrap();
    assert_eq!(ui.state_int("n"), Some(1));
    // The release edge runs one more frame (mouse_buttons_pressed changed),
    // then the frame settles at the new value.
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 3), 0);
    assert!(matches!(&ui.commands[0], petal_ui::draw::DrawCommand::Text { text, .. } if text == "1"));
}

#[test]
fn a_state_var_written_every_frame_never_settles() {
    let mut ui = ui("state var n = 0\n\
                     set n = get n + 1\n\
                     draw_text(\"{get n}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 5), 5);
    assert_eq!(ui.state_int("n"), Some(6));
}

#[test]
fn an_in_place_accumulator_is_a_change() {
    // The top-level `state xs = []` + `xs = append(xs, …)` idiom is rewritten
    // to mutate the slot's list in place, so old and new share a heap id; the
    // gate must still see the write as a change.
    let mut ui = ui("state xs = []\n\
                     if key_pressed(\"a\") then xs = append(xs, len(xs)) end\n\
                     draw_text(\"{len(xs)}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 3), 0);
    ui.key("a").unwrap();
    assert!(!ui.last_frame_skipped);
    assert!(matches!(&ui.commands[0], petal_ui::draw::DrawCommand::Text { text, .. } if text == "1"));
    // Release edge, then quiet.
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 3), 0);

    let mut acc = self::ui("state xs = []\n\
                     xs = append(xs, 1)\n\
                     draw_text(\"{len(xs)}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    acc.frame().unwrap();
    assert_eq!(quiet_runs(&mut acc, 4), 4, "appending every frame never settles");
    assert!(matches!(&acc.commands[0], petal_ui::draw::DrawCommand::Text { text, .. } if text == "5"));
}

#[test]
fn the_clock_and_randomness_keep_a_frame_running() {
    let mut ui = ui("draw_text(\"{time()}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 3), 3);
    let sym = ui.env.intern_symbol("time");
    assert_eq!(ui.last_run_reason, Some(RunReason::BindingChanged(sym)));

    let mut rng = self::ui("draw_text(\"{random(0, 1)}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    rng.env.set_seed(1);
    rng.frame().unwrap();
    assert_eq!(quiet_runs(&mut rng, 3), 3);
    assert_eq!(rng.last_run_reason, Some(RunReason::RngConsumed));
}

#[test]
fn host_data_reads_re_run_when_the_host_reports_a_change() {
    let mut ui = ui("let d = host_data(\"n\", \"\")\n\
                     draw_text(\"{d}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    let fixtures = |n: i64| {
        fixture_provider(&serde_json::json!([{ "kind": "n", "arg": "", "value": n }])).unwrap()
    };
    ui.set_data_provider(fixtures(1));
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 3), 0, "a fixture does not change on its own");
    assert!(ui.env.run_deps(ui.stack_id()).is_some_and(|d| d.host_read()));

    ui.set_data_provider(fixtures(2));
    ui.frame().unwrap();
    assert_eq!(ui.last_run_reason, Some(RunReason::HostDataChanged));
    assert!(matches!(&ui.commands[0], petal_ui::draw::DrawCommand::Text { text, .. } if text == "2"));
    assert_eq!(quiet_runs(&mut ui, 3), 0);
}

#[test]
fn invalidate_forces_exactly_one_run() {
    let mut ui = ui("draw_rect({x: 0, y: 0, w: 10, h: 10}, {r: 1, g: 2, b: 3, a: 255})");
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 2), 0);
    ui.invalidate();
    ui.frame().unwrap();
    assert_eq!(ui.last_run_reason, Some(RunReason::Forced));
    assert_eq!(quiet_runs(&mut ui, 2), 0);
}

#[test]
fn setting_state_from_the_host_forces_a_run() {
    let mut ui = ui("state n = 0\n\
                     draw_text(\"{n}\", vec2(0, 0), 12, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 2), 0);
    let (pid, sid) = (ui.program_id(), ui.stack_id());
    ui.env
        .set_state_from_json(pid, sid, "n", &serde_json::json!(7))
        .unwrap();
    ui.frame().unwrap();
    assert_eq!(ui.last_run_reason, Some(RunReason::Forced));
    assert!(matches!(&ui.commands[0], petal_ui::draw::DrawCommand::Text { text, .. } if text == "7"));
}

#[test]
fn a_failing_frame_is_gated_like_any_other() {
    let mut ui = ui("let x = mouse_x()\n\
                     if x > 10 then error(\"too far\") end\n\
                     draw_rect({x: x, y: 0, w: 10, h: 10}, {r: 1, g: 2, b: 3, a: 255})");
    ui.mouse_move(20, 0);
    assert!(ui.frame().is_err());
    // Same inputs, same failure: no point re-running.
    assert!(ui.frame().is_ok(), "a skipped frame reports no error");
    assert!(ui.last_frame_skipped);
    ui.mouse_move(5, 0);
    assert!(ui.frame().is_ok());
    assert!(!ui.last_frame_skipped);
}

#[test]
fn prelude_widgets_settle_on_a_quiet_frame() {
    // Widgets that animate toward a target (a toggle's knob) must stop
    // reading the clock once they arrive, or every panel with one would tick
    // forever.
    let mut ui = ui("state on = false\n\
                     state pos = 0.0\n\
                     pos = approach(pos, if on then 1.0 else 0.0 end, 16.0)\n\
                     if clicked({x: 0, y: 0, w: 40, h: 20}) then on = !on end\n\
                     draw_rect({x: int(pos * 20), y: 0, w: 20, h: 20}, {r: 1, g: 1, b: 1, a: 255})");
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert_eq!(quiet_runs(&mut ui, 5), 0, "settled: nothing to animate");
    ui.click(5, 5).unwrap();
    // Animating toward 1.0: runs until it snaps.
    let animating = quiet_runs(&mut ui, 60);
    assert!(animating > 3 && animating < 60, "ran {animating} of 60 frames");
    assert_eq!(quiet_runs(&mut ui, 5), 0, "settled again");
    assert!(matches!(ui.commands[0], petal_ui::draw::DrawCommand::Rect { x: 20, .. }));
}

// ── Differential oracle over the example corpus ──────────────────────────


fn drive(app: &Path, includes: &[PathBuf], gate: bool, seed: u64, frames: usize) -> (Vec<String>, u64) {
    let size = (800, 600);
    let mut ui = Headless::from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    petal_ui::panel_stubs::register_panel_stubs(&mut ui.env);
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    ui.gate = gate;
    let scenario = Scenario::monkey(seed, frames, size);
    let mut records = Vec::with_capacity(frames);
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        let outcome = ui.frame().map(|_| ());
        let record = serde_json::json!({
            "commands": ui.commands,
            "state": ui.state(),
            "error": outcome.err(),
        });
        records.push(record.to_string());
    }
    (records, ui.frames_skipped)
}

#[test]
fn gated_frames_reproduce_ungated_frames_across_the_corpus() {
    // One monkey seed over 45 frames: ~15 apps × 2 drives is already the
    // longest test in the crate under a debug build.
    let frames = 45;
    let mut skipped_total = 0;
    for (app, includes) in corpus() {
        for seed in [1u64] {
            let (full, _) = drive(&app, &includes, false, seed, frames);
            let (gated, skipped) = drive(&app, &includes, true, seed, frames);
            assert_corpus_is_live(&app, &full);
            skipped_total += skipped;
            for (i, (a, b)) in full.iter().zip(&gated).enumerate() {
                assert!(
                    a == b,
                    "{} seed {seed}: frame {i} differs under the gate\nfull:  {}\ngated: {}",
                    app.display(),
                    &a[..a.len().min(400)],
                    &b[..b.len().min(400)],
                );
            }
        }
    }
    assert!(skipped_total > 0, "the gate never skipped a frame across the corpus");
}

#[test]
fn a_quiet_corpus_mostly_idles() {
    // With no input at all, an app that is not driven by the clock should
    // settle within a few dozen frames and then skip.
    //
    // The ratio is taken over the apps that *can* idle. An app that reads
    // `time()` or `frame_count()` cannot: those bindings move on their own
    // every frame, so the gate is obliged to re-run and correctly reports
    // `BindingChanged`. Counting them as failures-to-idle would measure how
    // much of the corpus animates, not whether the gate settles.
    //
    // That exclusion is not a technicality — it is the finding. Four Garden
    // GPP apps and every worlds-fair screen read `frame_count()`, usually as
    // a once-per-frame cache key, and it costs them the frame gate entirely.
    // The in-tree `examples/` tree contains none of that idiom, which is why
    // it took running this corpus over Garden to see it.
    let mut settled = 0;
    let mut total = 0;
    let mut clock_driven = Vec::new();
    for (app, includes) in corpus() {
        let mut ui = Headless::from_file_with_paths(&app, 800, 600, &includes)
            .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
        petal_ui::panel_stubs::register_panel_stubs(&mut ui.env);
        ui.env.set_echo(false);
        ui.env.set_seed(1);
        for _ in 0..90 {
            let _ = ui.frame();
        }
        let before = ui.frames_run;
        for _ in 0..30 {
            let _ = ui.frame();
        }
        if ui.frames_run == before {
            settled += 1;
            total += 1;
            continue;
        }
        // Name the binding: `BindingChanged(SymbolId(4))` says nothing, and
        // which binding it is — the clock, or real input — is the whole
        // difference between an app that animates and a bug.
        let sym = match ui.last_run_reason {
            Some(RunReason::BindingChanged(sym)) => ui.env.symbol_name(sym).unwrap_or("?"),
            _ => "",
        };
        if matches!(sym, "time" | "frame_count" | "dt") {
            clock_driven.push(format!("{} ({sym})", app.display()));
            continue;
        }
        total += 1;
        eprintln!("{} keeps running: {:?}", app.display(), ui.last_run_reason);
    }
    eprintln!("clock-driven, cannot idle by construction:");
    for app in &clock_driven {
        eprintln!("  {app}");
    }
    assert!(
        settled * 2 >= total,
        "only {settled} of {total} non-clock-driven apps idle after 90 quiet frames"
    );
}
