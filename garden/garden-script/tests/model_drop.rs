//! Structural direct manipulation on a panel (declarative dragging, D2 P4):
//! the sketch drags a top-level `let` literal with `ui/drag`, the drop is
//! announced, and the proposed edit, applied, makes the literal read as the
//! drop with the file's comments kept.

use garden_script::{
    apply_rewrites_to, buttons, propose_model_drop, InputEvent, ModelDropOutcome, PanelHost,
};

const SKETCH: &str = "import ui
import ui/drag
import garden

// The pipeline, in the order it runs.
let STEPS = [
  \"fetch\",
  \"parse\",   // tolerant
  \"check\",
]

fn view(steps)
  for i in range(0, len(steps)) key steps[i] do
    let k = steps[i]
    let r = place(k, rect(20, 20 + i * 50, 200, 40))
    draggable(k, r, fn() closest(moves(steps, i)) |> floating() end)
    place(k, r, fn(p) draw_rect(p, #446688) end)
  end
  nil
end

clear(0, 0, 0)
let d = drag_session(STEPS, view)
view(d.preview)
";

fn frame(host: &mut PanelHost, n: i64) {
    host.frame(1.0 / 60.0, n).unwrap();
}

#[test]
fn a_drop_on_a_literal_model_proposes_its_edit() {
    let mut host = PanelHost::from_source("sketch.ptl", SKETCH).unwrap();
    host.set_dimensions(400, 300);
    // As Petal IDE runs a paired canvas.
    host.set_trace_origins(true);
    host.set_dm_checkpoint(true);
    let mut n = 0;
    let mut step = |host: &mut PanelHost, ev: Option<InputEvent>| {
        if let Some(ev) = ev {
            host.input_event(ev);
        }
        frame(host, n);
        n += 1;
    };
    step(&mut host, None);
    step(&mut host, None);
    // "fetch" is drawn at y 20..60; the press is a draggable's.
    assert!(host.model_drag_at(100.0, 40.0));
    assert!(!host.model_drag_at(300.0, 280.0));
    step(&mut host, Some(InputEvent::MouseMove { x: 100, y: 40 }));
    step(&mut host, Some(InputEvent::MouseDown { button: buttons::LEFT }));
    for y in [60, 90, 120, 140] {
        step(&mut host, Some(InputEvent::MouseMove { x: 100, y }));
    }
    step(&mut host, Some(InputEvent::MouseUp { button: buttons::LEFT }));
    step(&mut host, None);

    let drops = host.take_model_drops();
    assert_eq!(drops.len(), 1, "{drops:?}");
    assert_eq!(drops[0].key, "fetch");
    assert_eq!(drops[0].drop, serde_json::json!(["parse", "check", "fetch"]));
    assert!(host.take_model_drops().is_empty(), "drained");

    let rewrites = match propose_model_drop(SKETCH, &drops[0]) {
        ModelDropOutcome::Edits(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(rewrites.len(), 1);
    assert_eq!(rewrites[0].variable.as_deref(), Some("STEPS"));
    assert!(rewrites[0].description.contains("remove STEPS[0]"), "{}", rewrites[0].description);
    let edited = apply_rewrites_to(SKETCH, &rewrites).unwrap();
    assert!(edited.contains("// The pipeline, in the order it runs."));
    assert!(edited.contains("\"parse\",   // tolerant"));
    assert!(
        edited.contains("let STEPS = [\n  \"parse\",   // tolerant\n  \"check\",\n  \"fetch\",\n]"),
        "{edited}"
    );
    // The edited sketch runs and shows the new order with no drag in flight.
    host.reload_source(&edited).unwrap();
    frame(&mut host, 100);
    assert!(host.take_model_drops().is_empty());
}

#[test]
fn a_drop_on_state_is_refused() {
    let src = SKETCH.replace("let STEPS = [", "state STEPS = [");
    let drop = garden_script::ModelDrop {
        key: "fetch".into(),
        model: serde_json::json!(["fetch", "parse", "check"]),
        drop: serde_json::json!(["parse", "fetch", "check"]),
    };
    match propose_model_drop(&src, &drop) {
        ModelDropOutcome::Refused(why) => assert!(why.contains("literal"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_debug_flag_reaches_the_sketch() {
    let src = "import ui\nimport garden\nclear(0, 0, 0)\nif ui_debug(\"drag\") then draw_rect(0, 0, 5, 5, #ffffff) end\n";
    let mut host = PanelHost::from_source("dbg.ptl", src).unwrap();
    host.set_dimensions(100, 100);
    let off = host.frame(0.0, 0).unwrap().len();
    host.set_ui_debug("drag", true);
    assert!(host.ui_debug("drag"));
    let on = host.frame(0.0, 1).unwrap().len();
    assert_eq!(on, off + 1);
}
