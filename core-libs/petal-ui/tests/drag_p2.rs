//! `ui/drag`'s `between` and `vary` (P2 of
//! `docs/proposals/declarative-dragging.md`), end to end under the headless
//! harness: blending across two and three targets, clamping to the hull,
//! `vary` tracking the pointer by one solver step a frame (forward
//! differences and dual numbers, bounds, constraints, `on_drop`, `vary`
//! under `closest`), a reload in the middle of a `vary` drag, and the three
//! P2 examples (`examples/ui/d2-switch`, `d2-dial`, `d2-scheduler`) against
//! their checked-in drag scenarios.

use petal_ui::harness::Headless;
use serde_json::{Value, json};

fn ui(body: &str) -> Headless {
    let src = format!("import ui\nimport ui/drag\n{body}");
    let mut h = Headless::new(&src).unwrap_or_else(|e| panic!("compile failed: {e}"));
    h.env.set_echo(false);
    h
}

fn frame(h: &mut Headless) {
    if let Err(e) = h.frame() {
        panic!("frame failed: {e}");
    }
}

fn state(h: &Headless, name: &str) -> Value {
    h.state().get(name).cloned().unwrap_or(Value::Null)
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

/// Press at (x, y), then move through `path` one frame per point.
fn press_and_move(h: &mut Headless, x: i32, y: i32, path: &[(i32, i32)]) {
    h.mouse_move(x, y);
    frame(h);
    h.mouse_down(0);
    frame(h);
    for &(px, py) in path {
        h.mouse_move(px, py);
        frame(h);
    }
}

fn release(h: &mut Headless) {
    h.mouse_up(0);
    frame(h);
}

/// A knob at (x(v), y(v)) for a model `{v}`, dragged by `spec`; `shown` is
/// where `place` drew it, `drops` what was committed.
fn knob_app(pos: &str, spec: &str) -> Headless {
    ui(&format!(
        r#"
import autodiff
state m = {{v: 0.0, w: 0.0}}
state shown = nil
state drops = []
fn knob(m)
  {pos}
end
var cur = nil
fn view(m)
  let k = place("k", knob(m))
  draggable("k", k, fn() {spec} end)
  set cur = k
  nil
end
let d = drag_session(m, view)
view(d.preview)
shown = {{x: (get cur).x, y: (get cur).y}}
if d.dropped then
  drops = drops ++ [d.drop]
  m = d.drop
end
"#
    ))
}

#[test]
fn vary_tracks_the_pointer_along_a_line() {
    // The knob slides along x with v: x = 100 + 2·v. Grabbed at its centre
    // (110, 110), every frame's one step lands it under the pointer (the
    // view is linear, so one Gauss-Newton step is exact).
    let mut h = knob_app("{x: 100 + 2 * m.v, y: 100, w: 20, h: 20}", r#"vary(m, "v")"#);
    press_and_move(&mut h, 110, 110, &[(120, 110), (150, 140), (190, 90)]);
    let s = state(&h, "shown");
    assert!((num(&s["x"]) - 180.0).abs() < 0.6, "{s}");
    assert_eq!(num(&s["y"]), 100.0);
    release(&mut h);
    let v = num(&state(&h, "m")["v"]);
    assert!((v - 40.0).abs() < 0.3, "{v}");
}

#[test]
fn vary_respects_bounds_and_constraints() {
    // Bounds: v in [0, 25] → x in [100, 150].
    let mut h = knob_app("{x: 100 + 2 * m.v, y: 100, w: 20, h: 20}", r#"vary(m, ["v"], {bounds: [0, 25]})"#);
    press_and_move(&mut h, 110, 110, &[(130, 110), (300, 110), (310, 110)]);
    assert_eq!(num(&state(&h, "shown")["x"]), 150.0);
    press_and_move(&mut h, 160, 110, &[(150, 110), (0, 110), (-10, 110)]);
    assert_eq!(num(&state(&h, "shown")["x"]), 100.0);
    release(&mut h);

    // A constraint refuses v > 10; the step is shortened until it holds.
    let mut h = knob_app(
        "{x: 100 + 2 * m.v, y: 100, w: 20, h: 20}",
        r#"vary(m, "v", {constraint: fn(s) s.v <= 10 end})"#,
    );
    press_and_move(&mut h, 110, 110, &[(115, 110), (200, 110), (200, 110), (200, 110), (200, 110)]);
    let x = num(&state(&h, "shown")["x"]);
    assert!(x <= 120.0 && x > 117.0, "stopped short of the constraint: {x}");
}

#[test]
fn vary_solves_two_levers_for_a_point() {
    // A free point: x from v, y from w; both are solved together.
    let mut h = knob_app("{x: 100 + m.v, y: 50 + 3 * m.w, w: 10, h: 10}", r#"vary(m, [["v"], ["w"]])"#);
    press_and_move(&mut h, 105, 55, &[(110, 60), (140, 95)]);
    let s = state(&h, "shown");
    assert!((num(&s["x"]) - 135.0).abs() < 0.6 && (num(&s["y"]) - 90.0).abs() < 0.6, "{s}");
}

#[test]
fn dual_and_differences_give_the_same_drag() {
    // A dial: the knob on a circle. Both sensitivities follow the pointer
    // round a quarter turn and drop at the same angle.
    let pos = "{x: 200 + cos(m.v) * 100 - 8, y: 200 + sin(m.v) * 100 - 8, w: 16, h: 16}";
    let path = [(300, 210), (296, 240), (280, 270), (250, 290), (220, 297), (200, 300), (200, 300)];
    let mut angles = Vec::new();
    for spec in [r#"vary(m, "v")"#, r#"vary(m, "v", {sensitivity: "dual"})"#] {
        let mut h = knob_app(pos, spec);
        press_and_move(&mut h, 300, 200, &path);
        let s = state(&h, "shown");
        assert!((num(&s["x"]) - 192.0).abs() < 1.5 && (num(&s["y"]) - 292.0).abs() < 1.5, "{spec}: {s}");
        release(&mut h);
        angles.push(num(&state(&h, "m")["v"]));
    }
    assert!((angles[0] - std::f64::consts::FRAC_PI_2).abs() < 0.02, "{angles:?}");
    assert!((angles[0] - angles[1]).abs() < 0.01, "{angles:?}");
}

#[test]
fn a_dual_that_loses_its_derivative_falls_back_to_differences() {
    // `value_of` (as a native that needs a float would) drops the dual's
    // derivative, while the knob does move: the check at the first frame
    // switches to differences, and the drag still follows the pointer.
    let mut h = knob_app("{x: 100 + 2 * value_of(m.v), y: 100, w: 20, h: 20}", r#"vary(m, "v", {sensitivity: "dual"})"#);
    press_and_move(&mut h, 110, 110, &[(115, 110), (140, 110)]);
    let x = num(&state(&h, "shown")["x"]);
    assert!((x - 130.0).abs() <= 0.6, "{x}");
}

#[test]
fn a_view_that_snaps_is_differenced_at_a_larger_step() {
    // The knob moves in whole steps of 0.5 px per 0.25 of v: nothing moves
    // at the first difference step, so a larger one is tried; then it
    // tracks the pointer to within a step.
    let mut h = knob_app("{x: 100 + floor(m.v * 4) / 2, y: 100, w: 20, h: 20}", r#"vary(m, "v")"#);
    press_and_move(&mut h, 110, 110, &[(115, 110), (140, 110)]);
    for _ in 0..4 {
        frame(&mut h);
    }
    let x = num(&state(&h, "shown")["x"]);
    assert!((x - 130.0).abs() <= 1.0, "{x}");
}

#[test]
fn on_drop_applies_to_a_vary_drop() {
    let mut h = knob_app(
        "{x: 100 + 2 * m.v, y: 100, w: 20, h: 20}",
        r#"vary(m, "v") |> on_drop(fn(s) {...s, v: round(s.v / 5) * 5} end)"#,
    );
    press_and_move(&mut h, 110, 110, &[(120, 110), (153, 110), (153, 110)]);
    release(&mut h);
    assert_eq!(num(&state(&h, "m")["v"]), 20.0);
}

#[test]
fn vary_under_closest_picks_the_nearest_track() {
    // Two tracks (w = 0 at y = 100, w = 1 at y = 200); on each, v slides
    // the knob along x. closest picks the track whose solved knob is
    // nearest: the timeline of the paper's §8.
    let mut h = knob_app(
        "{x: 100 + m.v, y: 100 + 100 * m.w, w: 20, h: 20}",
        r#"closest([vary({...m, w: 0}, "v"), vary({...m, w: 1}, "v")])"#,
    );
    press_and_move(&mut h, 110, 110, &[(130, 120), (160, 190), (180, 205), (180, 205)]);
    let s = state(&h, "shown");
    assert!((num(&s["x"]) - 170.0).abs() < 0.6 && num(&s["y"]) == 200.0, "{s}");
    release(&mut h);
    let m = state(&h, "m");
    assert_eq!(num(&m["w"]), 1.0);
    assert!((num(&m["v"]) - 70.0).abs() < 0.6, "{m}");
}

#[test]
fn between_two_targets_is_the_segment_and_blends_numbers_and_colours() {
    // A switch: off at x = 100, on at x = 200; its fill is a colour and its
    // label alpha a number. Halfway, both are halfway.
    let mut h = ui(r#"
state on = false
state seen = nil
var cur = nil
fn view(on)
  let k = place("k", rect(if on then 200 else 100 end, 100, 20, 20))
  draggable("k", k, fn() between([false, true]) end)
  let c = place("c", if on then #00ff00 else #808080 end)
  let a = place("a", if on then 255 else 55 end)
  set cur = {x: k.x, y: k.y, c: c, a: a}
  nil
end
let d = drag_session(on, view)
view(d.preview)
seen = get cur
if d.dropped then on = d.drop end
"#);
    // Grab the knob's centre and pull it off its segment: it stays on it.
    press_and_move(&mut h, 110, 110, &[(130, 140), (160, 160)]);
    let s = state(&h, "seen");
    assert_eq!(s["x"], json!(150.0), "{s}");
    assert_eq!(s["y"], json!(100.0), "{s}");
    assert_eq!(s["c"], json!({"r": 64, "g": 192, "b": 64}), "{s}");
    assert_eq!(s["a"], json!(155.0), "{s}");
    // Past the end it stops at the end.
    h.mouse_move(400, 110);
    frame(&mut h);
    assert_eq!(num(&state(&h, "seen")["x"]), 200.0);
    release(&mut h);
    assert_eq!(state(&h, "on"), json!(true));
}

#[test]
fn a_reload_mid_vary_drag_keeps_dragging_with_the_new_view() {
    let src = |scale: &str| {
        format!(
            r#"import ui
import ui/drag
state m = {{v: 0.0}}
state shown = nil
state why = nil
var cur = nil
fn view(m)
  let k = place("k", {{x: 100 + {scale} * m.v, y: 100, w: 20, h: 20}})
  draggable("k", k, fn() vary(m, "v") |> on_drop(fn(s) {{...s, v: round(s.v)}} end) end)
  set cur = k.x
  nil
end
let d = drag_session(m, view)
view(d.preview)
shown = get cur
if d.dropped then m = d.drop end
if d.cancelled then why = d.reason end
"#
        )
    };
    let mut h = Headless::new(&src("2")).unwrap();
    h.env.set_echo(false);
    press_and_move(&mut h, 110, 110, &[(120, 110), (150, 110), (150, 110)]);
    assert!((num(&state(&h, "shown")) - 140.0).abs() < 0.6);
    // The view now moves the knob 4 px per unit: the drag goes on, and the
    // solver finds the new v that keeps the knob under the pointer.
    let sid = h.stack_id();
    h.env.reload_program(sid, &src("4"), None).unwrap();
    for _ in 0..4 {
        frame(&mut h);
    }
    assert!((num(&state(&h, "shown")) - 140.0).abs() < 0.6, "{} (cancelled: {})", state(&h, "shown"), state(&h, "why"));
    release(&mut h);
    // on_drop (a function, re-read from the view's draggable) still applies.
    assert_eq!(num(&state(&h, "m")["v"]), 10.0);
}

// ── The examples ──────────────────────────────────────────────────────────

/// Run `examples/ui/<name>/app.ptl` against its `scenarios/drag.json`,
/// returning (the state, the draw commands) after every frame.
fn run_example(name: &str) -> Vec<(Value, Vec<Value>)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ui").join(name);
    let scenario: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("scenarios/drag.json")).unwrap()).unwrap();
    let size = scenario["size"].as_array().unwrap();
    let mut h = Headless::from_file_with_size(
        &root.join("app.ptl"),
        size[0].as_i64().unwrap() as i32,
        size[1].as_i64().unwrap() as i32,
    )
    .unwrap();
    h.env.set_echo(false);
    let events = scenario["events"].as_array().unwrap();
    let mut out = Vec::new();
    for f in 0..scenario["frames"].as_u64().unwrap() {
        for e in events.iter().filter(|e| e["at"].as_u64() == Some(f)) {
            if let Some(p) = e.get("mouse_move") {
                h.mouse_move(p[0].as_i64().unwrap() as i32, p[1].as_i64().unwrap() as i32);
            }
            if e.get("mouse_down").is_some() {
                h.mouse_down(0);
            }
            if e.get("mouse_up").is_some() {
                h.mouse_up(0);
            }
        }
        let cmds = serde_json::to_value(h.frame().unwrap_or_else(|e| panic!("frame failed: {e}"))).unwrap();
        out.push((Value::Object(h.state()), cmds.as_array().cloned().unwrap_or_default()));
    }
    out
}

/// The draw commands of kind `op` whose `radius` (or `h`) is `size`.
fn shapes<'a>(cmds: &'a [Value], op: &str, field: &str, size: f64) -> Vec<&'a Value> {
    cmds.iter().filter(|c| c["op"] == op && c[field].as_f64() == Some(size)).collect()
}

#[test]
fn example_three_way_switch() {
    let s = run_example("d2-switch");
    let knob = |f: usize| {
        let k = shapes(&s[f].1, "circle", "radius", 26.0);
        (num(&k[0]["cx"]), num(&k[0]["cy"]), k[0]["g"].as_i64().unwrap())
    };
    // Inside the triangle the knob is under the pointer, its colour mixed.
    assert_eq!((knob(11).0, knob(11).1), (190.0, 160.0));
    assert!(knob(11).2 > 143 && knob(11).2 < 180, "{:?}", knob(11));
    // Outside it, the knob stops on the nearest edge (off → low → high).
    let (x, y, _) = knob(14);
    assert!((x - 242.0).abs() < 1.0 && (y - 184.0).abs() < 1.0, "{x}, {y}");
    let last = &s.last().unwrap().0;
    assert_eq!(last["pos"], json!("high"));
    assert_eq!(last["changes"], json!(1));
    // And settles onto the high position after the drop.
    assert_eq!((knob(49).0, knob(49).1), (180.0, 80.0));
}

#[test]
fn example_rotary_dial() {
    let s = run_example("d2-dial");
    // While dragging, the knob stays on the dial and near the pointer.
    for f in [6, 8, 10, 14] {
        let k = shapes(&s[f].1, "circle", "radius", 16.0);
        let (x, y) = (num(&k[0]["cx"]), num(&k[0]["cy"]));
        // (Draw commands are whole pixels.)
        assert!(((x - 180.0).hypot(y - 170.0) - 100.0).abs() < 1.5, "frame {f}: off the dial at ({x}, {y})");
    }
    let k = shapes(&s[14].1, "circle", "radius", 16.0);
    assert!((num(&k[0]["cx"]) - 180.0).abs() < 1.5 && (num(&k[0]["cy"]) - 270.0).abs() < 1.5);
    let last = &s.last().unwrap().0;
    assert!((num(&last["dial"]["angle"]) - std::f64::consts::FRAC_PI_2).abs() < 0.02, "{last}");
    assert_eq!(last["turns"], json!(1));
}

#[test]
fn example_scheduler() {
    let s = run_example("d2-scheduler");
    let bar = |f: usize, x: f64| -> Value {
        s[f].1.iter().find(|c| c["h"].as_f64() == Some(36.0) && (num(&c["x"]) - x).abs() < 0.5).cloned().unwrap()
    };
    // Dragging review's bar up moves it between rooms, never in time.
    for f in [5, 8, 11] {
        assert_eq!(num(&bar(f, 130.0)["x"]), 130.0);
    }
    assert!(num(&bar(11, 130.0)["y"]) < 90.0);
    let after_first = &s[20].0;
    assert_eq!(after_first["sched"]["meetings"][1], json!({"id": "review", "start": 60, "end": 180, "room": 0}));
    // Dragging retro's end handle past the end of the day: the constraint
    // holds it at the day's end, and on_drop rounds it to five minutes.
    assert!(num(&bar(36, 490.0)["w"]) > 120.0);
    let last = &s.last().unwrap().0;
    assert_eq!(last["sched"]["meetings"][4], json!({"id": "retro", "start": 420, "end": 600, "room": 1}));
    assert_eq!(last["edits"], json!(2));
}
