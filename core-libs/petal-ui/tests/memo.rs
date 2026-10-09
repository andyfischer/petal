//! Memoized scopes: a user-function call whose arguments, captures and
//! recorded reads are what they were last frame is replayed from its record
//! instead of run. See docs/dev/memo-scopes.md and `petal::memo`.
//!
//! Two kinds of test, as for the frame gate. The first pins the semantics on
//! small scripts: what is replayed, what keeps a scope running, what a replay
//! must preserve (state inside a skipped widget, observations, output order).
//! The second is the differential oracle over the example corpus: every panel
//! app, driven by a monkey scenario with the frame gate off so every frame
//! runs, must produce the same commands, state and observations frame for
//! frame with memoization on and off.

use std::path::{Path, PathBuf};

mod common;
use common::{assert_corpus_is_live, corpus};

use petal::policy::RunPolicy;
use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;

const WHITE: &str = "{r: 255, g: 255, b: 255, a: 255}";

fn ui(src: &str) -> Headless {
    let mut ui = Headless::new(src).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.env.set_echo(false);
    // Every frame runs: the memo is what is under test, not the gate.
    ui.set_policy(RunPolicy::REPLAY);
    ui
}

fn rows(n: usize) -> String {
    format!(
        "fn row(i)\n\
           let r = {{x: 0, y: i * 20, w: 100, h: 20}}\n\
           let c = if hovered(r) then {{r: 9, g: 9, b: 9, a: 255}} else {{r: 1, g: 1, b: 1, a: 255}} end\n\
           draw_rect(r, c)\n\
           draw_text(\"row {{i}}\", vec2(4, i * 20), 12, {WHITE})\n\
         end\n\
         for i in range(0, {n}) do row(i) end\n"
    )
}

#[test]
fn a_widget_whose_inputs_did_not_change_is_replayed() {
    let mut ui = ui(&rows(5));
    ui.frame().unwrap();
    let first = ui.commands.clone();
    assert_eq!(ui.memo_stats().records, 5, "each row recorded on the first frame");
    ui.frame().unwrap();
    let stats = ui.memo_stats();
    assert_eq!(stats.hits, 5, "every row replayed on the second frame");
    assert_eq!(ui.commands, first, "a replayed frame draws what a run would");
    assert_eq!(ui.commands.len(), 10);
}

#[test]
fn memoization_can_be_turned_off() {
    let mut ui = ui(&rows(3));
    ui.set_policy(RunPolicy::REPLAY.with_memo(false));
    ui.frame().unwrap();
    ui.frame().unwrap();
    let stats = ui.memo_stats();
    assert_eq!((stats.hits, stats.records), (0, 0));
}

#[test]
fn a_probe_that_keeps_its_answer_is_a_cutoff() {
    let mut ui = ui(&rows(5));
    ui.mouse_move(50, 30); // inside row 1
    ui.frame().unwrap();
    assert!(matches!(ui.commands[2], DrawCommand::Rect { r: 9, .. }), "row 1 is hovered");

    // The pointer moves, but stays in row 1: no row's `hovered` answer
    // changes, so every row is replayed.
    ui.mouse_move(60, 35);
    ui.frame().unwrap();
    let s = ui.memo_stats();
    assert_eq!((s.hits, s.misses), (5, 0));

    // Into row 3: exactly the two rows whose answer flipped re-run.
    ui.mouse_move(60, 70);
    ui.frame().unwrap();
    let s2 = ui.memo_stats();
    assert_eq!((s2.hits - s.hits, s2.misses - s.misses), (3, 2));
    assert!(matches!(ui.commands[2], DrawCommand::Rect { r: 1, .. }));
    assert!(matches!(ui.commands[6], DrawCommand::Rect { r: 9, .. }));
}

#[test]
fn state_inside_a_replayed_widget_survives_and_keeps_counting() {
    let src = format!(
        "fn counter(i)\n\
           state n = 0\n\
           if clicked({{x: i * 30, y: 0, w: 25, h: 20}}) then n = n + 1 end\n\
           draw_text(\"{{n}}\", vec2(i * 30, 0), 12, {WHITE})\n\
         end\n\
         for i in range(0, 3) do counter(i) end\n"
    );
    let mut ui = ui(&src);
    ui.frame().unwrap();
    ui.click(35, 5).unwrap();
    assert!(matches!(&ui.commands[1], DrawCommand::Text { text, .. } if text == "1"));
    // Quiet frames: the counters are replayed, and the sweep keeps their
    // state because the replay retains what the scope touched.
    let before = ui.memo_stats().hits;
    ui.frames(5).unwrap();
    assert!(ui.memo_stats().hits > before, "quiet frames replay the counters");
    assert_eq!(ui.state().get("[1]/counter/n").and_then(|v| v.as_i64()), Some(1));
    // The second click continues from the kept state, not from zero.
    ui.click(35, 5).unwrap();
    assert!(matches!(&ui.commands[1], DrawCommand::Text { text, .. } if text == "2"));
    assert_eq!(ui.state().get("[1]/counter/n").and_then(|v| v.as_i64()), Some(2));
}

#[test]
fn a_scope_that_prints_is_never_replayed() {
    let src = format!(
        "fn noisy(i)\n\
           print(\"row {{i}}\")\n\
           draw_text(\"{{i}}\", vec2(0, i * 20), 12, {WHITE})\n\
         end\n\
         for i in range(0, 3) do noisy(i) end\n"
    );
    let mut ui = ui(&src);
    for _ in 0..3 {
        ui.frame().unwrap();
        assert_eq!(ui.env.take_output().len(), 3, "every frame prints every row");
    }
    assert!(ui.memo_stats().effectful >= 3);
}

#[test]
fn a_callback_recreated_each_frame_still_matches() {
    let src = format!(
        "state var n = 0\n\
         fn btn(r, on_click)\n\
           if clicked(r) then on_click() end\n\
           draw_rect(r, {WHITE})\n\
           draw_text(\"{{get n}}\", vec2(r.x, r.y), 12, {WHITE})\n\
         end\n\
         btn({{x: 0, y: 0, w: 40, h: 20}}, fn()\n  set n = get n + 1\nend)\n"
    );
    let mut ui = ui(&src);
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert_eq!(ui.memo_stats().hits, 1, "the closure is a new value every frame, but the same function over the same cell");
    ui.click(5, 5).unwrap();
    assert!(matches!(&ui.commands[1], DrawCommand::Text { text, .. } if text == "1"));
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert!(matches!(&ui.commands[1], DrawCommand::Text { text, .. } if text == "1"));
    assert_eq!(ui.state_int("n"), Some(1));
}

#[test]
fn a_scope_whose_own_cell_escapes_is_not_replayed() {
    // `make` hands out a closure over its own `var`; replaying `make` would
    // hand out the *same* cell, still holding last frame's increments.
    let src = format!(
        "fn make()\n\
           var k = 0\n\
           let bump = fn()\n\
             set k = get k + 1\n\
             get k\n\
           end\n\
           bump\n\
         end\n\
         let c = make()\n\
         let total = c() + c()\n\
         draw_text(\"{{total}}\", vec2(0, 0), 12, {WHITE})\n"
    );
    let mut ui = ui(&src);
    for _ in 0..4 {
        ui.frame().unwrap();
        assert!(matches!(&ui.commands[0], DrawCommand::Text { text, .. } if text == "3"));
    }
    assert!(ui.memo_stats().effectful > 0);
}

#[test]
fn a_call_result_is_never_mutated_in_place_by_its_caller() {
    // With memoization on, `mk`'s result may be the record's own value, so
    // the append must copy rather than grow it in place; otherwise the cached
    // list would grow by one every frame.
    let src = format!(
        "fn mk() [1, 2] end\n\
         let xs = mk()\n\
         xs = append(xs, 3)\n\
         draw_text(\"{{len(xs)}}\", vec2(0, 0), 12, {WHITE})\n"
    );
    let mut ui = ui(&src);
    for _ in 0..4 {
        ui.frame().unwrap();
        assert!(matches!(&ui.commands[0], DrawCommand::Text { text, .. } if text == "3"));
    }
}

#[test]
fn a_replayed_scope_reports_its_observations() {
    let src = format!(
        "fn row(i)\n\
           let y = i * 20\n\
           draw_text(\"{{i}}\", vec2(0, y), 12, {WHITE})\n\
         end\n\
         for i in range(0, 3) do row(i) end\n"
    );
    let mut ui = ui(&src);
    ui.env.observations_mut().enable();
    ui.frame().unwrap();
    let first = ui.env.get_observations_json(ui.program_id(), ui.stack_id());
    assert_eq!(
        first.get("row.y").and_then(|v| v.as_i64()),
        Some(40),
        "last binding wins (keys: {:?})",
        first.keys().collect::<Vec<_>>()
    );
    ui.frame().unwrap();
    assert!(ui.memo_stats().hits >= 3);
    let second = ui.env.get_observations_json(ui.program_id(), ui.stack_id());
    assert_eq!(first, second, "a replayed scope's bindings read as a run's would");
}

#[test]
fn the_baseline_policy_disables_memoization_with_the_other_optimizations() {
    let mut ui = ui(&rows(3));
    ui.set_policy(RunPolicy::parse("baseline").unwrap());
    assert!(!ui.frame_stats().memo);
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert_eq!(ui.memo_stats().hits, 0);
}

// ── Differential oracle over the example corpus ──────────────────────────


fn drive(app: &Path, includes: &[PathBuf], policy: RunPolicy, seed: u64, frames: usize) -> (Vec<String>, u64) {
    let size = (800, 600);
    let mut ui = Headless::from_file_with_paths(app, size.0, size.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    petal_ui::panel_stubs::register_panel_stubs(&mut ui.env);
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    // Garden panels observe every frame; a replayed scope must report the
    // same bindings a run would.
    ui.env.observations_mut().enable();
    ui.set_policy(policy);
    let scenario = Scenario::monkey(seed, frames, size);
    let mut records = Vec::with_capacity(frames);
    for frame in 0..frames {
        scenario.apply(&mut ui, frame);
        let outcome = ui.frame().map(|_| ());
        let record = serde_json::json!({
            "commands": ui.commands,
            "state": ui.state(),
            "observed": ui.env.get_observations_json(ui.program_id(), ui.stack_id()),
            "error": outcome.err(),
        });
        records.push(record.to_string());
    }
    (records, ui.memo_stats().hits)
}

/// Where two frame records first differ, for a readable failure.
fn first_difference(a: &str, b: &str) -> String {
    let (a, b): (serde_json::Value, serde_json::Value) =
        (serde_json::from_str(a).unwrap(), serde_json::from_str(b).unwrap());
    for field in ["error", "commands", "state", "observed"] {
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
            (serde_json::Value::Object(xs), serde_json::Value::Object(ys)) => {
                let mut keys: Vec<&String> = xs.keys().chain(ys.keys()).collect();
                keys.sort();
                keys.dedup();
                let k = keys.iter().find(|k| xs.get(**k) != ys.get(**k)).unwrap();
                format!(
                    "{field}.{k}: {} vs {}",
                    xs.get(*k).map_or("<none>".to_string(), |v| v.to_string()),
                    ys.get(*k).map_or("<none>".to_string(), |v| v.to_string()),
                )
            }
            _ => format!("{field}: {x} vs {y}"),
        };
    }
    "no field differs".to_string()
}

/// `replay-memo` against `replay`: the same run with no memo and with
/// memoized scopes replaying. The memoized run must reproduce the unmemoized
/// frames exactly. (The rows the memo classifies natives by are checked
/// separately, facet by facet, in `tests/effect_audit.rs`.)
#[test]
fn memoized_frames_reproduce_unmemoized_frames_across_the_corpus() {
    let frames = 45;
    let mut hits_total = 0;
    for (app, includes) in corpus() {
        for seed in [1u64] {
            let (full, _) = drive(&app, &includes, RunPolicy::REPLAY.with_memo(false), seed, frames);
            let (memoized, hits) = drive(&app, &includes, RunPolicy::REPLAY, seed, frames);
            assert_corpus_is_live(&app, &full);
            hits_total += hits;
            for (i, (a, b)) in full.iter().zip(&memoized).enumerate() {
                assert!(
                    a == b,
                    "{} seed {seed}: frame {i} differs under replay: {}",
                    app.display(),
                    first_difference(a, b),
                );
            }
        }
    }
    assert!(hits_total > 0, "no scope was ever replayed across the corpus");
}
