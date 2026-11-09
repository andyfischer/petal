//! `ui/drag` tooling (P4 of `docs/proposals/declarative-dragging.md`):
//! `drag_explain` / `drag_debug` and the host's debug key, the `drag_drops`
//! announcement, the scenario `drag` sugar, and `petal-ui-run --drag-trace`.

use std::process::Command;

use petal_ui_headless::harness::Headless;
use petal_ui_headless::scenario::Scenario;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_petal-ui-run");

/// A five-row reorderable list (the d2-list shape) that prints what
/// `drag_explain()` says every frame a drag is in flight.
const LIST: &str = r#"
state items = ["A", "B", "C", "D", "E"]
fn view(items)
  for i in range(0, len(items)) key items[i] do
    let k = items[i]
    let r = place(k, rect(40, 60 + i * 52, 240, 44))
    draggable(k, r, fn() closest(moves(items, i)) |> floating() end)
    place(k, r, fn(p) draw_rect(p, #446688) end)
  end
  nil
end
let d = drag_session(items, view)
view(d.preview)
let ex = drag_explain()
if ex != nil then
  print("{ex.choice}|{ex.branch}|{ex.speculations}|{len(ex.targets)}|{ex.targets[0].gap}")
end
if d.dropped then items = d.drop end
"#;

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

/// Drag row A from its middle down `dys` one frame each, without releasing;
/// returns every line printed along the way.
fn drag_down(h: &mut Headless, dys: &[i32]) -> Vec<String> {
    let mut out = Vec::new();
    h.mouse_move(160, 82);
    frame(h);
    h.mouse_down(0);
    frame(h);
    for dy in dys {
        h.mouse_move(160, 82 + dy);
        frame(h);
        out.extend(h.env.take_output());
    }
    out
}

#[test]
fn drag_explain_reads_the_table_without_speculating() {
    let mut h = ui(LIST);
    let lines = drag_down(&mut h, &[10, 40, 104, 110]);
    let rows: Vec<Vec<&str>> = lines.iter().map(|l| l.split('|').collect()).collect();
    // The drag starts once past the slop; that frame renders the 5 states.
    assert_eq!(rows[0][2], "5", "{lines:?}");
    // Every frame after reads the table only.
    for r in &rows[1..] {
        assert_eq!(r[2], "0", "{lines:?}");
        assert_eq!(r[3], "5");
    }
    // 104 px down is two rows: target 3, through floating > closest.
    assert_eq!(rows[2][0], "3", "{lines:?}");
    assert_eq!(rows[2][1], r#"["floating", "closest", "fixed #3"]"#);
    // Target 1 (A's own slot) is 104 px from the element.
    assert_eq!(rows[2][4].parse::<f64>().unwrap().round(), 104.0);
}

#[test]
fn drag_debug_draws_only_when_the_host_key_is_on_and_speculates_nothing() {
    let mut h = ui(LIST);
    frame(&mut h);
    let plain = h.commands.len();
    h.input.set_debug("drag", true);
    frame(&mut h);
    // Idle: one label says there is no drag.
    assert!(h.commands.len() > plain);
    let texts = |h: &Headless| -> Vec<String> {
        serde_json::to_value(&h.commands).unwrap().as_array().unwrap().iter()
            .filter_map(|c| c["text"].as_str().map(str::to_string)).collect()
    };
    assert!(texts(&h).iter().any(|t| t.contains("no drag")), "{:?}", texts(&h));

    let lines = drag_down(&mut h, &[10, 40, 104]);
    // The overlay's own map and table cost no speculative run.
    assert!(lines[1..].iter().all(|l| l.split('|').nth(2) == Some("0")), "{lines:?}");
    let t = texts(&h);
    assert!(t.iter().any(|s| s.starts_with("drag `A`: 5 targets")), "{t:?}");
    assert!(t.iter().any(|s| s.contains("branch floating > closest > fixed #3")), "{t:?}");
    assert!(t.iter().any(|s| s == "#3 fixed at (160, 186)"), "{t:?}");
    assert!(t.iter().any(|s| s.starts_with("#3 ")), "a distance label per target: {t:?}");
    // The drop-zone map: translucent bands, one colour per target.
    let cmds = serde_json::to_value(&h.commands).unwrap();
    let bands = cmds.as_array().unwrap().iter().filter(|c| c["op"] == "rect" && c["a"] == 40).count();
    assert!(bands >= 5, "{bands} zone bands");

    h.input.set_debug("drag", false);
    h.mouse_up(0);
    frame(&mut h);
    frame(&mut h);
    assert!(!texts(&h).iter().any(|s| s.starts_with("drag `")));
}

#[test]
fn a_drop_is_announced_on_the_drops_channel() {
    let mut h = ui(LIST);
    drag_down(&mut h, &[10, 60, 104]);
    assert!(petal_ui_headless::drag_trace::take_drops(&mut h.env).is_empty(), "no drop mid-drag");
    h.mouse_up(0);
    frame(&mut h);
    let drops = petal_ui_headless::drag_trace::take_drops(&mut h.env);
    assert_eq!(
        drops,
        vec![json!({"key": "A", "model": ["A", "B", "C", "D", "E"], "drop": ["B", "C", "A", "D", "E"]})]
    );
    assert!(petal_ui_headless::drag_trace::take_drops(&mut h.env).is_empty(), "drained");
}

#[test]
fn the_trace_reads_the_session_out_of_the_state() {
    let mut h = ui(LIST);
    drag_down(&mut h, &[10, 60, 104]);
    let pointer = (h.input.mouse_x, h.input.mouse_y);
    let all = petal_ui_headless::drag_trace::explain_all(&h.state(), pointer, true);
    assert_eq!(all.len(), 1);
    let e = &all[0];
    assert_eq!(e["key"], "A");
    assert_eq!(e["choice"], 3);
    assert_eq!(e["branch"], json!(["floating", "closest", "fixed #3"]));
    assert_eq!(e["targets"].as_array().unwrap().len(), 5);
    assert_eq!(e["targets"][2]["drop"], json!(["B", "C", "A", "D", "E"]));
    assert_eq!(e["gap"], json!(0.0));
}

/// `drag: {target, to, frames}`: the target is a place key (found as
/// `drag:<key>`), `to` another element's key, and the gesture is held.
#[test]
fn scenario_drag_sugar_drives_a_held_drag() {
    let mut h = ui(LIST);
    let s = Scenario::from_json_str(
        r#"{"events": [{"at": 2, "drag": {"target": "A", "to": "C", "frames": 5}}]}"#,
    )
    .unwrap();
    for f in 0..14 {
        let missing = s.apply(&mut h, f);
        assert!(missing.is_empty(), "frame {f}: {missing:?}");
        frame(&mut h);
    }
    assert_eq!(h.state()["items"], json!(["B", "C", "A", "D", "E"]));

    // A key `ui/drag` placed but no draggable registered resolves through
    // where it was drawn (the scenario reads the engine's state).
    let mut h = ui(&LIST.replace(
        "draggable(k, r, fn() closest(moves(items, i)) |> floating() end)",
        "if k != \"E\" then draggable(k, r, fn() closest(moves(items, i)) |> floating() end) end",
    ));
    let s = Scenario::from_json_str(
        r#"{"events": [{"at": 2, "drag": {"target": "A", "to": "E", "frames": 6}}]}"#,
    )
    .unwrap();
    for f in 0..14 {
        assert!(s.apply(&mut h, f).is_empty());
        frame(&mut h);
    }
    assert_eq!(h.state()["items"], json!(["B", "C", "D", "E", "A"]));

    // An unknown name is reported, and nothing is pressed.
    let mut h = ui(LIST);
    let s = Scenario::from_json_str(r#"{"events": [{"at": 1, "drag": {"target": "Z", "to": [0, 0]}}]}"#).unwrap();
    frame(&mut h);
    assert_eq!(s.apply(&mut h, 1), vec!["Z".to_string()]);
}

#[test]
fn petal_ui_run_drag_trace() {
    let dir = std::env::temp_dir().join(format!("drag-trace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let app = dir.join("app.ptl");
    std::fs::write(&app, format!("import ui\nimport ui/drag\n{LIST}")).unwrap();
    let scenario = dir.join("drag.json");
    std::fs::write(
        &scenario,
        r#"{"size": [320, 340], "frames": 14,
            "events": [{"at": 2, "drag": {"target": "A", "to": "D", "frames": 6}}]}"#,
    )
    .unwrap();
    let out = Command::new(BIN)
        .arg(&app)
        .args(["--scenario", scenario.to_str().unwrap(), "--drag-trace", "--ui-debug", "drag"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("drag \"A\": #4 of 5 (floating > closest > fixed #4)"),
        "{stderr}"
    );
    assert!(stderr.contains("drop \"A\" -> [\"B\",\"C\",\"D\",\"A\",\"E\"]"), "{stderr}");
    let records: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records[0]["drag"], Value::Null, "idle frames carry no drag");
    let mid = records.iter().find(|r| r["drag"]["sessions"][0]["choice"] == 4).expect("a frame on target 4");
    let s = &mid["drag"]["sessions"][0];
    assert_eq!(s["targets"].as_array().unwrap().len(), 5);
    assert!(s["targets"][0].get("drop").is_none(), "the per-frame trace leaves states out");
    let dropped = records.iter().find(|r| !r["drag"]["drops"].is_null() && r["drag"]["drops"][0]["key"] == "A");
    assert!(dropped.is_some());
    assert_eq!(records.last().unwrap()["state"]["items"], json!(["B", "C", "D", "A", "E"]));
    // `--ui-debug drag` drew the debugger into the trace.
    assert!(mid["commands"].to_string().contains("5 targets"));
    let _ = std::fs::remove_dir_all(&dir);
}
