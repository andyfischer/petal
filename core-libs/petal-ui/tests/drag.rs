//! `ui/drag` — declarative dragging (P0 of
//! `docs/proposals/declarative-dragging.md`), end to end under the headless
//! harness: the candidate-set helpers, the spec constructors as data, and
//! `drag_session` driving real presses, moves, releases, Escape and a hot
//! reload. The three examples (`examples/ui/d2-*`) are run against their
//! checked-in drag scenarios and must reach the states they are written to
//! reach.

use petal_ui_headless::harness::Headless;
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

fn prints(h: &mut Headless) -> Vec<String> {
    h.env.take_output()
}

/// Press at (x, y), then move through `path` one frame per point, without
/// releasing.
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

#[test]
fn candidate_sets() {
    let mut h = ui(r#"
assert_eq(moves(["A", "B", "C"], 1), [["B", "A", "C"], ["A", "B", "C"], ["A", "C", "B"]])
assert_eq(moves_nested([["a", "b"], ["c"]], "b"),
  [[["b", "a"], ["c"]], [["a", "b"], ["c"]], [["a"], ["b", "c"]], [["a"], ["c", "b"]]])
// Items are found by `id` (or `key`), and columns may be records.
assert_eq(moves_nested([{n: 1, cards: [{id: 7}]}, {n: 2, cards: []}], 7, "cards"),
  [[{n: 1, cards: [{id: 7}]}, {n: 2, cards: []}], [{n: 1, cards: []}, {n: 2, cards: [{id: 7}]}]])
assert_eq(moves_nested([["a"]], "zz"), [])
assert_eq(choices({a: {b: 1}, c: 0}, ["a", "b"], [5, 6]), [{a: {b: 5}, c: 0}, {a: {b: 6}, c: 0}])
assert_eq(choices({room: 0}, "room", [1, 2]), [{room: 1}, {room: 2}])

let t = [{n: "a", children: [{n: "a1", children: []}]}, {n: "b", children: []}]
let a_moves = tree_moves(t, [0])
// Moving `a` never puts it inside itself or its child a1.
for s in a_moves do
  assert(len(s) >= 1)
  for top in s do
    if top.n == "b" then
      for k in top.children do assert(k.n == "a") end
    end
  end
end
assert_eq(a_moves, [
  [{n: "a", children: [{n: "a1", children: []}]}, {n: "b", children: []}],
  [{n: "b", children: []}, {n: "a", children: [{n: "a1", children: []}]}],
  [{n: "b", children: [{n: "a", children: [{n: "a1", children: []}]}]}]])
// A child can move out to the root, above or below its old parent.
let a1_moves = tree_moves(t, [0, 0])
assert(contains(a1_moves, [{n: "a1", children: []}, {n: "a", children: []}, {n: "b", children: []}]))
assert(contains(a1_moves, [{n: "a", children: []}, {n: "b", children: [{n: "a1", children: []}]}]))
// Every state appears once.
assert_eq(len(a1_moves), len(Map(for s in a1_moves do [s, true] end)))
// `into: false` keeps it among siblings.
// (b above or below a, or above or below a1).
assert_eq(len(tree_moves(t, [1], {into: false})), 4)
state ok = true
"#,
    );
    frame(&mut h);
    assert_eq!(state(&h, "ok"), json!(true));
}

#[test]
fn specs_are_data() {
    let mut h = ui(r#"
assert_eq(fixed_state(3), {d2: "fixed", state: 3, drop: 3})
// A plain state where a spec goes means fixed_state(state).
assert_eq(closest([1, 2]), {d2: "closest", options: [fixed_state(1), fixed_state(2)], stickiness: 0})
assert_eq(closest([1], {stickiness: 8}).stickiness, 8)
assert_eq(floating(closest([1])), {d2: "floating", inner: closest([1])})
assert_eq(closest([1]) |> floating(), floating(closest([1])))
assert_eq(snap_radius(1, 20, {chain: true}), {d2: "snap_radius", inner: fixed_state(1), px: 20, chain: true})
assert_eq(when_far(1, 2).px, 60)
assert_eq(drop_target("trash", []), {d2: "drop_target", key: "trash", state: [], drop: []})
// on_drop is applied when the spec is built, so the spec holds no function.
let s = closest([1, 2]) |> floating() |> on_drop(fn(x) x * 10 end)
assert_eq(s, floating(closest([{d2: "fixed", state: 1, drop: 10}, {d2: "fixed", state: 2, drop: 20}])))
assert_eq(switch_to([1], "k"), {d2: "switch_to", state: [1], drop: [1], key: "k", follow: nil})
assert_eq(switch_to([1], "k", fn() closest([[1]]) end).follow, closest([[1]]))
// P2 shapes exist as data now.
assert_eq(between([1, 2]).d2, "between")
assert_eq(vary({a: 1}, [["a"]], {bounds: [[0, 9]]}).bounds, [[0, 9]])
// The builtin `fixed` still formats numbers with ui/drag imported.
assert_eq(fixed(3.14159, 2), "3.14")
state ok = true
"#,
    );
    frame(&mut h);
    assert_eq!(state(&h, "ok"), json!(true));
}

/// A five-row list: `spec` is the body of the spec closure, with `items`
/// and `i` in scope. Rows are 40 high every 50 px from y = 40, x 40..260.
fn list_app(spec: &str, extra: &str) -> Headless {
    ui(&format!(
        r#"
state items = ["A", "B", "C", "D", "E"]
state events = []
state ys = []
// The displayed y of each row, as `place` last returned it.
state var shown = Map()
fn slot(i)
  rect(40, 40 + i * 50, 220, 40)
end
fn view(items)
  for i in range(0, len(items)) key items[i] do
    let k = items[i]
    let r = place(k, slot(i))
    draggable(k, r, fn() {spec} end)
    place(k, r, fn(p)
      set shown[k] = p.y
      draw_rect(p, :808080)
    end)
  end
  {extra}
  nil
end
let d = drag_session(items, view)
view(d.preview)
ys = for k in ["A", "B", "C", "D", "E"] do if (get shown[k]) == nil then nil else int(round(get shown[k])) end end
if d.dropped then
  events = events ++ [{{dropped: d.drop}}]
  items = d.drop
end
if d.cancelled then events = events ++ [{{cancelled: d.reason}}] end
if d.active then events = events ++ [{{target: d.target}}] end

"#
    ))
}

#[test]
fn reorder_with_closest_and_floating() {
    let mut h = list_app("closest(moves(items, i)) |> floating()", "");
    // Press A, drag it down past B and C.
    press_and_move(&mut h, 100, 60, &[(104, 90), (108, 150), (110, 160)]);
    let ev = state(&h, "events");
    assert!(ev.as_array().unwrap().iter().any(|e| e.get("target").is_some()), "{ev}");
    // B slides up toward slot 0 over several frames rather than jumping.
    let b_y = state(&h, "ys")[1].as_i64().unwrap();
    assert!(b_y > 40 && b_y < 90, "B is mid-slide: {b_y}");
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["B", "C", "A", "D", "E"]));
    // A settles from the pointer into slot 2 (y 140).
    h.frames(40).unwrap();
    assert_eq!(state(&h, "ys"), json!([140, 40, 90, 190, 240]));
}

#[test]
fn a_press_without_movement_is_a_click_not_a_drag() {
    let mut h = list_app("closest(moves(items, i)) |> floating()", "");
    press_and_move(&mut h, 100, 60, &[(101, 61)]);
    release(&mut h);
    h.frames(2).unwrap();
    assert_eq!(state(&h, "events"), json!([]));
    assert_eq!(state(&h, "items"), json!(["A", "B", "C", "D", "E"]));
}

#[test]
fn escape_cancels_and_the_element_settles_home() {
    let mut h = list_app("closest(moves(items, i)) |> floating()", "");
    press_and_move(&mut h, 100, 60, &[(104, 90), (110, 200)]);
    h.key("escape").unwrap();
    release(&mut h);
    h.frames(40).unwrap();
    let ev = state(&h, "events");
    assert!(ev.as_array().unwrap().contains(&json!({"cancelled": "escape"})), "{ev}");
    assert_eq!(state(&h, "items"), json!(["A", "B", "C", "D", "E"]));
    assert_eq!(state(&h, "ys"), json!([40, 90, 140, 190, 240]));
}

#[test]
fn the_floating_element_draws_on_the_drag_layer() {
    let mut h = list_app("closest(moves(items, i)) |> floating()", "");
    press_and_move(&mut h, 100, 60, &[(104, 90)]);
    h.mouse_move(108, 120);
    let cmds = h.frame().unwrap().to_vec();
    // A comes first in the list but is drawn last: it is on UI_LAYER_DRAG.
    let ys: Vec<i64> = cmds
        .iter()
        .filter_map(|c| {
            let v = serde_json::to_value(c).unwrap();
            (v["op"] == "rect").then(|| v["y"].as_f64().unwrap().round() as i64)
        })
        .collect();
    assert_eq!(*ys.last().unwrap(), 120 - 20, "{ys:?}");
}

#[test]
fn releasing_over_nothing_cancels() {
    // Only drop targets: away from the trash there is no target.
    let mut h = list_app(
        r#"floating(closest([drop_target("trash", remove_at(items, i))]))"#,
        r#"place("trash", rect(300, 40, 60, 60))"#,
    );
    press_and_move(&mut h, 100, 60, &[(120, 70), (200, 80)]);
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["A", "B", "C", "D", "E"]));
    assert!(state(&h, "events").as_array().unwrap().contains(&json!({"cancelled": "no target"})));
}

#[test]
fn drop_target_removes_the_item() {
    let mut h = list_app(
        r#"closest(moves(items, i) ++ [drop_target("trash", remove_at(items, i))]) |> floating()"#,
        r#"place("trash", rect(300, 40, 60, 60))"#,
    );
    // B (index 1) onto the trash.
    press_and_move(&mut h, 100, 110, &[(150, 100), (320, 60)]);
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["A", "C", "D", "E"]));
}

#[test]
fn snap_radius_stops_floating_near_a_target() {
    let mut h = list_app("closest(moves(items, i)) |> floating() |> snap_radius(10)", "");
    // 30 px off the nearest slot: floating, drawn under the pointer.
    press_and_move(&mut h, 100, 60, &[(104, 80), (100, 90)]);
    assert_eq!(state(&h, "ys")[0], json!(70));
    // 3 px off slot 1 (y 90): snapped into it, and eased there.
    h.mouse_move(100, 113);
    frame(&mut h);
    h.frames(30).unwrap();
    assert_eq!(state(&h, "ys")[0], json!(90));
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["B", "A", "C", "D", "E"]));
}

#[test]
fn when_far_switches_behaviour() {
    // Near a slot: reorder. Far (past 30 px): the trash-only behaviour,
    // where nothing is a target unless over the trash.
    let mut h = list_app(
        r#"when_far(closest(moves(items, i)), closest([drop_target("trash", remove_at(items, i))]), {px: 30}) |> floating()"#,
        r#"place("trash", rect(300, 40, 60, 60))"#,
    );
    press_and_move(&mut h, 100, 60, &[(104, 70), (220, 62)]);
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["A", "B", "C", "D", "E"]));
    press_and_move(&mut h, 100, 60, &[(104, 70), (105, 110)]);
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["B", "A", "C", "D", "E"]));
}

#[test]
fn stickiness_keeps_the_last_choice() {
    let mut h = list_app("closest(moves(items, i), {stickiness: 20}) |> floating()", "");
    // The element's centre is the pointer's y here. At 70 slot 0 (target 1)
    // is nearest; at 92 slot 1 is 14 px nearer, which stickiness 20 absorbs.
    press_and_move(&mut h, 100, 60, &[(104, 70), (100, 92)]);
    let last = state(&h, "events").as_array().unwrap().last().cloned().unwrap();
    assert_eq!(last, json!({"target": 1}));
    h.mouse_move(100, 115);
    frame(&mut h);
    let last = state(&h, "events").as_array().unwrap().last().cloned().unwrap();
    assert_eq!(last, json!({"target": 2}));
}

#[test]
fn without_stickiness_the_nearest_wins() {
    let mut h = list_app("closest(moves(items, i)) |> floating()", "");
    press_and_move(&mut h, 100, 60, &[(104, 70), (100, 92)]);
    let last = state(&h, "events").as_array().unwrap().last().cloned().unwrap();
    assert_eq!(last, json!({"target": 2}));
}

#[test]
fn ambiguous_targets_warn_once_and_closest_takes_the_first() {
    // Two different states that both put A in slot 0.
    let mut h = list_app(
        r#"closest([items, ["A", "C", "B", "D", "E"]]) |> floating()"#,
        "",
    );
    press_and_move(&mut h, 100, 60, &[(104, 70), (104, 72)]);
    let out = prints(&mut h);
    let warnings: Vec<_> = out.iter().filter(|p| p.contains("both place `A`")).collect();
    assert_eq!(warnings.len(), 1, "{out:?}");
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["A", "B", "C", "D", "E"]));
    // A second drag does not warn again.
    press_and_move(&mut h, 100, 60, &[(104, 70)]);
    assert!(prints(&mut h).iter().all(|p| !p.contains("both place")));
}

#[test]
fn between_blends_every_key_and_drops_on_the_heaviest() {
    // A's five states lie on a line (slot centres y = 60, 110, …). With A's
    // centre at y = 130, it is 40% of the way from slot 1 to slot 2: A is
    // drawn at 0.6·90 + 0.4·140 = 110 and C, which swaps with it, at
    // 0.6·140 + 0.4·90 = 120, both at once; the drop is slot 1's state.
    // (More in tests/drag_p2.rs.)
    let mut h = list_app("between(moves(items, i))", "");
    press_and_move(&mut h, 100, 60, &[(100, 90), (100, 130)]);
    assert_eq!(state(&h, "ys"), json!([110, 40, 120, 190, 240]));
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["B", "A", "C", "D", "E"]));
}

#[test]
fn switch_to_follows_a_new_element() {
    // A palette swatch at the top: dragging it switches at once to a state
    // with a new row "N" at the end and keeps dragging N, which then
    // reorders like any row. The switch happens on the drag's first frame,
    // so the session reports N from the start.
    let mut h = ui(r#"
state items = ["A", "B"]
fn view(items)
  let sw = place("swatch", rect(300, 40, 40, 40))
  draggable("swatch", sw, fn() closest([switch_to(items ++ ["N"], "N")]) end)
  for i in range(0, len(items)) key items[i] do
    let k = items[i]
    let r = place(k, rect(40, 40 + i * 50, 220, 40))
    draggable(k, r, fn() closest(moves(items, i)) |> floating() end)
  end
  nil
end
let d = drag_session(items, view)
view(d.preview)
state keys_seen = []
if d.active && !contains(keys_seen, d.key) then keys_seen = keys_seen ++ [d.key] end
if d.dropped then items = d.drop end
"#);
    press_and_move(&mut h, 320, 60, &[(310, 70), (200, 60)]);
    assert_eq!(state(&h, "keys_seen"), json!(["N"]));
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["N", "A", "B"]));
}

#[test]
fn snap_radius_chain_continues_from_the_snapped_state() {
    // With chain, snapping into slot 1 makes that the drag's base: the next
    // targets are moves from ["B", "A", …], read from the view's draggable.
    let mut h = list_app("closest(moves(items, i)) |> snap_radius(8, {chain: true}) |> floating()", "");
    press_and_move(&mut h, 100, 60, &[(104, 70), (100, 111), (100, 161)]);
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["B", "C", "A", "D", "E"]));
}

#[test]
fn a_reload_mid_drag_respeculates_against_the_new_view() {
    let v1 = r#"import ui
import ui/drag
state items = ["A", "B", "C", "D", "E"]
state target = 0
let GAP = 50
fn view(items)
  for i in range(0, len(items)) key items[i] do
    let r = place(items[i], rect(40, 40 + i * GAP, 220, 40))
    draggable(items[i], r, fn() closest(moves(items, i)) |> floating() end)
  end
  nil
end
let d = drag_session(items, view)
view(d.preview)
if d.active then target = d.target end
if d.dropped then items = d.drop end
"#;
    let mut h = Headless::new(v1).unwrap();
    h.env.set_echo(false);
    press_and_move(&mut h, 100, 60, &[(104, 70), (100, 160)]);
    // Element top at 140: slot 2 with 50 px rows.
    assert_eq!(state(&h, "target"), json!(3));
    // Same pointer, rows now 100 px apart: slot 1 (y 140) is nearest.
    let v2 = v1.replace("let GAP = 50", "let GAP = 100");
    let sid = h.stack_id();
    h.env.reload_program(sid, &v2, None).unwrap();
    h.mouse_move(100, 161);
    frame(&mut h);
    assert_eq!(state(&h, "target"), json!(2));
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["B", "A", "C", "D", "E"]));
}

/// The examples, under their checked-in scenarios.
fn run_example(name: &str) -> Vec<Value> {
    run_example_at(&format!("ui/{name}"))
}

/// Run `examples/<rel>/app.ptl` against its `scenarios/drag.json`, returning
/// the state after every frame.
fn run_example_at(rel: &str) -> Vec<Value> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(rel);
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
    let frames = scenario["frames"].as_u64().unwrap();
    let events = scenario["events"].as_array().unwrap();
    let mut states = Vec::new();
    for f in 0..frames {
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
        frame(&mut h);
        states.push(Value::Object(h.state()));
    }
    states
}

#[test]
fn example_reorderable_list() {
    let s = run_example("d2-list");
    assert_eq!(s.last().unwrap()["items"], json!(["B", "C", "D", "A", "E"]));
    assert_eq!(s.last().unwrap()["drops"], json!(1));
}

#[test]
fn example_tower_of_hanoi() {
    let s = run_example("d2-hanoi");
    let last = s.last().unwrap();
    // Disk 1 to peg 3; then disk 2 dropped toward peg 3, which holds a
    // smaller disk, so it can only land on peg 2.
    assert_eq!(last["pegs"], json!([[4, 3], [2], [1]]));
    assert_eq!(last["moves_made"], json!(2));
}

#[test]
fn example_ring_of_beads() {
    let s = run_example("d2-beads");
    let last = s.last().unwrap();
    // A reinserted two places round, then D to the top; on_drop turns each
    // drop so A is first.
    assert_eq!(s[20]["beads"], json!(["A", "D", "E", "F", "B", "C"]));
    assert_eq!(last["beads"], json!(["A", "E", "F", "B", "C", "D"]));
    assert_eq!(last["turns"], json!(2));
}

#[test]
fn example_kanban() {
    let s = run_example_at("productivity/kanban");
    let last = s.last().unwrap();
    let ids = |c: usize| -> Vec<String> {
        last["cols"][c].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect()
    };
    // c1 picked up from BACKLOG and dropped in REVIEW, between c8 and c9.
    assert_eq!(ids(0), ["c2", "c3", "c4", "c5"]);
    assert_eq!(ids(2), ["c8", "c1", "c9"]);
    assert_eq!(last["moves"], json!(1));
    assert_eq!(last["sel_id"], json!("c1"));
    // The displayed y of a card on frame `f`, from `place`'s animation table.
    let y = |f: usize, key: &str| -> f64 {
        s[f]["ui/drag::_anim"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e[0] == key)
            .map(|e| e[1]["y"].as_f64().unwrap())
            .unwrap()
    };
    // Mid-drag, the cards around the gap glide: c2 closes BACKLOG's hole
    // (286 -> 156) and c9 makes room in REVIEW (286 -> 416), over frames.
    assert!(y(9, "c2") < 286.0 && y(9, "c2") > 156.0, "{}", y(9, "c2"));
    assert!(y(12, "c9") > 286.0 && y(12, "c9") < 416.0, "{}", y(12, "c9"));
    // After the release the dropped card eases into its slot.
    assert!((y(49, "c1") - 286.0).abs() < 0.5, "{}", y(49, "c1"));
    assert!((y(49, "c9") - 416.0).abs() < 0.5, "{}", y(49, "c9"));
}

/// Speculation is the core `speculate` call: a view that writes `state`,
/// sets a cell and draws as it goes is run on every candidate at drag start,
/// and none of it leaks into the live frame.
#[test]
fn speculating_an_imperative_view_leaves_nothing_behind() {
    let mut h = ui(r#"
state items = ["A", "B", "C"]
state var renders = 0
var drawn = 0
fn view(items)
  for i in range(0, len(items)) key items[i] do
    let k = items[i]
    state seen = 0
    seen = seen + 1
    let r = place(k, rect(40, 40 + i * 50, 220, 40))
    draggable(k, r, fn() closest(moves(items, i)) |> floating() end)
    set drawn = (get drawn) + 1
    draw_rect(r, :808080)
  end
  set renders = (get renders) + 1
  nil
end
let d = drag_session(items, view)
view(d.preview)
state rects = 0
rects = get drawn
if d.dropped then items = d.drop end
"#);
    press_and_move(&mut h, 100, 60, &[(104, 90), (104, 130)]);
    // Frame count so far: 1 (move) + 1 (down) + 2 (moves) = 4 runs, one
    // live render each. The drag start speculated 3 candidates plus the base:
    // none of their `renders` writes, cell sets or draws survived.
    assert_eq!(state(&h, "renders"), json!(4));
    assert_eq!(state(&h, "rects"), json!(3));
    let cmds = h.frame().unwrap().to_vec();
    let rects = cmds.iter().filter(|c| serde_json::to_value(c).unwrap()["op"] == "rect").count();
    assert_eq!(rects, 3);
    release(&mut h);
    assert_eq!(state(&h, "items"), json!(["B", "A", "C"]));
}

#[test]
fn moves_nested_finds_class_instances_by_id() {
    let mut h = ui(r#"
class Card
  id: string,
end
let a = Card(id: "a")
let b = Card(id: "b")
assert_eq(len(moves_nested([[a, b], []], "b")), 3)
assert_eq(moves_nested([[a, b], []], "b")[2], [[a], [b]])
"#);
    frame(&mut h);
}

/// `place_origin`: positions are eased relative to the origin, so moving the
/// origin (a scroll) moves the element at once, while a change relative to it
/// still animates.
#[test]
fn place_origin_makes_scrolling_instant() {
    let mut h = ui(r#"
state n = 0
n = n + 1
let scroll = if n >= 2 then 40 else 0 end
let y = if n >= 3 then 200 else 100 end
state shown = 0
shown = place_origin({x: 0, y: -scroll}, fn() place("k", rect(0, y - scroll, 10, 10)).y end)
"#);
    frame(&mut h);
    assert_eq!(state(&h, "shown"), json!(100));
    frame(&mut h);
    assert_eq!(state(&h, "shown"), json!(60), "the scroll is not animated");
    frame(&mut h);
    let v = state(&h, "shown").as_f64().unwrap();
    assert!(v > 60.0 && v < 160.0, "a move inside the origin eases toward 160: {v}");
}

#[test]
fn drag_key_names_the_dragged_element() {
    let mut h = ui(r#"
state items = ["A", "B"]
state dk = "none"
fn view(items)
  for i in range(0, len(items)) key items[i] do
    let r = place(items[i], rect(40, 40 + i * 50, 220, 40))
    draggable(items[i], r, fn() closest(moves(items, i)) |> floating() end)
  end
  nil
end
let d = drag_session(items, view)
view(d.preview)
dk = drag_key() ?? "none"
"#);
    frame(&mut h);
    assert_eq!(state(&h, "dk"), json!("none"));
    press_and_move(&mut h, 100, 60, &[(104, 90)]);
    assert_eq!(state(&h, "dk"), json!("A"));
    release(&mut h);
    assert_eq!(state(&h, "dk"), json!("none"));
}
