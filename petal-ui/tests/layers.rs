//! The layer vocabulary end to end: the canvas natives (with their new
//! optional arguments), the prelude's `layer` / `snapshot` / `draw_material`
//! wrappers, and the serialized shape every host decodes.

use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;

fn cmds(source: &str) -> Vec<DrawCommand> {
    let mut ui = Headless::new(source).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.frame().unwrap().to_vec()
}

fn json(cmd: &DrawCommand) -> String {
    serde_json::to_string(cmd).unwrap()
}

fn tags(cmds: &[DrawCommand]) -> Vec<String> {
    cmds.iter()
        .map(|c| {
            let v: serde_json::Value = serde_json::from_str(&json(c)).unwrap();
            v["op"].as_str().unwrap().to_string()
        })
        .collect()
}

#[test]
fn draw_canvas_takes_opacity_and_a_destination_size() {
    let out = cmds(
        "let c = create_canvas(10, 10)\n\
         draw_canvas(c, 1, 2)\n\
         draw_canvas(c, 1, 2, 128)\n\
         draw_canvas(c, 1, 2, 128, 20, 30)\n\
         draw_canvas(c, {x: 1, y: 2, w: 20, h: 30}, {a: 128})\n\
         draw_canvas(c, rect(1, 2, 20, 30))",
    );
    assert_eq!(
        out[1],
        DrawCommand::DrawCanvas {
            id: 1,
            x: 1,
            y: 2,
            a: 255,
            w: 0,
            h: 0
        }
    );
    assert_eq!(
        out[2],
        DrawCommand::DrawCanvas {
            id: 1,
            x: 1,
            y: 2,
            a: 128,
            w: 0,
            h: 0
        }
    );
    let scaled = DrawCommand::DrawCanvas {
        id: 1,
        x: 1,
        y: 2,
        a: 128,
        w: 20,
        h: 30,
    };
    assert_eq!(out[3], scaled);
    assert_eq!(out[4], scaled);
    assert_eq!(
        out[5],
        DrawCommand::DrawCanvas {
            id: 1,
            x: 1,
            y: 2,
            a: 255,
            w: 20,
            h: 30
        }
    );
    // The optional fields stay off the wire at their defaults, so a host
    // decoding the old three-field shape keeps working.
    assert_eq!(json(&out[1]), r#"{"op":"draw_canvas","id":1,"x":1,"y":2}"#);
    assert_eq!(
        json(&scaled),
        r#"{"op":"draw_canvas","id":1,"x":1,"y":2,"a":128,"w":20,"h":30}"#
    );
}

#[test]
fn draw_to_returns_the_previous_target_so_layers_nest() {
    let out = cmds(
        "layer(rect(0, 0, 40, 40), fn()\n\
             draw_rect(0, 0, 1, 1, 1, 1, 1)\n\
             layer(rect(5, 5, 10, 10), {a: 100}, fn()\n\
                 draw_rect(0, 0, 1, 1, 2, 2, 2)\n\
             end)\n\
             draw_rect(0, 0, 1, 1, 3, 3, 3)\n\
         end)",
    );
    assert_eq!(
        tags(&out),
        [
            "create_canvas",
            "set_target",
            "rect",
            "create_canvas",
            "set_target",
            "rect",
            "set_target",
            "draw_canvas",
            "rect",
            "set_target",
            "draw_canvas"
        ]
    );
    // The inner layer hands the target back to the *outer canvas*, not the
    // screen — the whole reason `draw_to` returns the previous target.
    assert_eq!(out[6], DrawCommand::SetTarget { id: 1 });
    assert_eq!(
        out[7],
        DrawCommand::DrawCanvas {
            id: 2,
            x: 5,
            y: 5,
            a: 100,
            w: 0,
            h: 0
        }
    );
    assert_eq!(out[9], DrawCommand::SetTarget { id: 0 });
}

#[test]
fn snapshot_and_blur_natives_emit_their_commands() {
    let out = cmds(
        "let c = create_canvas(30, 20)\n\
         snapshot_to(c, 4, 6)\n\
         blur_canvas(c, 12)\n\
         blur_canvas(c, 2.6)",
    );
    assert_eq!(out[1], DrawCommand::Snapshot { id: 1, x: 4, y: 6 });
    assert_eq!(out[2], DrawCommand::BlurCanvas { id: 1, radius: 12 });
    assert_eq!(out[3], DrawCommand::BlurCanvas { id: 1, radius: 3 });
    assert_eq!(json(&out[1]), r#"{"op":"snapshot","id":1,"x":4,"y":6}"#);
    assert_eq!(json(&out[2]), r#"{"op":"blur_canvas","id":1,"radius":12}"#);
}

#[test]
fn a_material_is_a_clipped_blurred_snapshot_under_a_tint() {
    let out = cmds("draw_material(rect(0, 0, 100, 44), {kind: \"thick\", radius: 8})");
    assert_eq!(
        tags(&out),
        [
            "clip_push",
            "create_canvas",
            "snapshot",
            "blur_canvas",
            "draw_canvas",
            "rect",
            "clip_pop"
        ]
    );
    assert_eq!(
        out[0],
        DrawCommand::ClipPush {
            x: 0,
            y: 0,
            w: 100,
            h: 44,
            radius: 8
        }
    );
    assert_eq!(
        out[1],
        DrawCommand::CreateCanvas {
            id: 1,
            w: 100,
            h: 44
        }
    );
    assert_eq!(out[3], DrawCommand::BlurCanvas { id: 1, radius: 32 });
    match &out[5] {
        DrawCommand::Rect { a, radius, .. } => {
            assert_eq!(*a, 215, "thick material tint opacity");
            assert_eq!(*radius, 0, "the clip rounds the corners, not the fill");
        }
        other => panic!("expected the tint fill, got {other:?}"),
    }
}

#[test]
fn a_material_hairline_and_explicit_tint_are_honoured() {
    let out = cmds(
        "draw_material(rect(0, 10, 100, 44), {tint: #ff0000, a: 40, blur: 5, hairline: \"top\"})",
    );
    assert_eq!(
        tags(&out),
        [
            "clip_push",
            "create_canvas",
            "snapshot",
            "blur_canvas",
            "draw_canvas",
            "rect",
            "rect",
            "clip_pop"
        ]
    );
    assert_eq!(out[3], DrawCommand::BlurCanvas { id: 1, radius: 5 });
    match &out[5] {
        DrawCommand::Rect { r, g, b, a, .. } => assert_eq!((*r, *g, *b, *a), (255, 0, 0, 40)),
        other => panic!("expected the tint fill, got {other:?}"),
    }
    match &out[6] {
        DrawCommand::Rect { y, h, .. } => assert_eq!((*y, *h), (10, 1), "top hairline"),
        other => panic!("expected the hairline, got {other:?}"),
    }
}

#[test]
fn a_backdrop_blur_snapshots_the_rect_and_draws_it_back_in_place() {
    let out = cmds("draw_backdrop_blur(rect(3, 4, 50, 20), 10)");
    assert_eq!(
        out,
        vec![
            DrawCommand::CreateCanvas {
                id: 1,
                w: 50,
                h: 20
            },
            DrawCommand::Snapshot { id: 1, x: 3, y: 4 },
            DrawCommand::BlurCanvas { id: 1, radius: 10 },
            DrawCommand::DrawCanvas {
                id: 1,
                x: 3,
                y: 4,
                a: 255,
                w: 0,
                h: 0
            },
        ]
    );
}

#[test]
fn canvas_ids_and_the_target_reset_every_frame() {
    let mut ui = Headless::new(
        "let c = create_canvas(4, 4)\n\
         draw_to(c)\n\
         draw_rect(0, 0, 1, 1, 0, 0, 0)",
    )
    .unwrap();
    let first = ui.frame().unwrap().to_vec();
    let second = ui.frame().unwrap().to_vec();
    assert_eq!(
        first, second,
        "ids restart at 1 and the target at the screen"
    );
    assert_eq!(first[0], DrawCommand::CreateCanvas { id: 1, w: 4, h: 4 });
}
