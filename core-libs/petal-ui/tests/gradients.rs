//! The gradient / shadow / nested-clip draw vocabulary, end to end: the
//! prelude wrappers, the natives' argument handling, and the serialized shape
//! every host consumer decodes.

use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;

fn cmds(source: &str) -> Vec<DrawCommand> {
    let mut ui = Headless::new(source).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.frame().unwrap().to_vec()
}

/// Compact JSON for a command — what a host that ships the stream over the
/// wire (Garden's debug server, a GPP client) actually sees.
fn json(cmd: &DrawCommand) -> String {
    serde_json::to_string(cmd).unwrap()
}

#[test]
fn rect_gradient_takes_records_or_flat_args_and_carries_both_stops() {
    let out = cmds(
        "draw_rect_gradient({x: 1, y: 2, w: 30, h: 40}, #ff0000, #0000ff, 0.0)\n\
         draw_rect_gradient(1, 2, 30, 40, 255, 0, 0, 255, 0, 0, 255, 255, 0.0)",
    );
    let want = DrawCommand::RectGradient {
        x: 1,
        y: 2,
        w: 30,
        h: 40,
        radius: 0,
        r0: 255,
        g0: 0,
        b0: 0,
        a0: 255,
        r1: 0,
        g1: 0,
        b1: 255,
        a1: 255,
        angle: 0.0,
    };
    // The record overload and the native's own flat form must land on exactly
    // the same command — the whole point of the wrapper being a wrapper.
    assert_eq!(out, vec![want.clone(), want]);
}

#[test]
fn a_gradient_stop_carries_its_own_alpha() {
    // A stop's `a` field is the stop's alpha — the one primitive where alpha
    // rides with the color, because a fade needs two of them. The trailing
    // `a0, a1` overload overrides it.
    let out = cmds(
        "draw_rect_gradient({x: 0, y: 0, w: 10, h: 10}, {r: 1, g: 2, b: 3, a: 40}, #000000, 0.0)\n\
         draw_rect_gradient({x: 0, y: 0, w: 10, h: 10}, {r: 1, g: 2, b: 3, a: 40}, #000000, 0.0, 200, 0)",
    );
    match (&out[0], &out[1]) {
        (
            DrawCommand::RectGradient { a0, a1, .. },
            DrawCommand::RectGradient { a0: b0, a1: b1, .. },
        ) => {
            assert_eq!((*a0, *a1), (40, 255));
            assert_eq!((*b0, *b1), (200, 0));
        }
        other => panic!("expected two rect gradients, got {other:?}"),
    }
}

#[test]
fn rounded_rect_gradient_carries_the_radius_and_the_angle() {
    let out =
        cmds("draw_rect_gradient_rounded({x: 0, y: 0, w: 10, h: 20}, 6, #ffffff, #000000, 1.5)");
    match &out[0] {
        DrawCommand::RectGradient { radius, angle, .. } => {
            assert_eq!(*radius, 6);
            assert!((angle - 1.5).abs() < 1e-6);
        }
        other => panic!("expected a rect gradient, got {other:?}"),
    }
}

#[test]
fn circle_gradient_accepts_a_center_record_or_flat_coords() {
    let out = cmds(
        "draw_circle_gradient({x: 8, y: 9}, 5, #ffffff, #000000)\n\
         draw_circle_gradient(8, 9, 5, #ffffff, #000000)",
    );
    let want = DrawCommand::CircleGradient {
        cx: 8,
        cy: 9,
        radius: 5,
        r0: 255,
        g0: 255,
        b0: 255,
        a0: 255,
        r1: 0,
        g1: 0,
        b1: 0,
        a1: 255,
    };
    assert_eq!(out, vec![want.clone(), want]);
}

#[test]
fn a_multi_stop_gradient_subdivides_into_bands_that_meet_edge_to_edge() {
    // Three stops over a 100px-wide rect: two bands, each carrying one
    // consecutive pair, tiling the rect with no gap and no overlap.
    let out =
        cmds("linear_gradient({x: 0, y: 0, w: 100, h: 20}, [#ff0000, #00ff00, #0000ff], 0.0)");
    assert_eq!(out.len(), 2);
    match (&out[0], &out[1]) {
        (
            DrawCommand::RectGradient {
                x: x0,
                w: w0,
                r0: ar0,
                g1: ag1,
                ..
            },
            DrawCommand::RectGradient {
                x: x1,
                w: w1,
                g0: bg0,
                b1: bb1,
                ..
            },
        ) => {
            assert_eq!((*x0, *w0), (0, 50));
            assert_eq!((*x1, *w1), (50, 50));
            assert_eq!(x0 + *w0 as i32, *x1, "bands must meet exactly");
            // Band 0 runs red→green, band 1 green→blue: the shared stop is
            // the same color on both sides of the seam.
            assert_eq!((*ar0, *ag1), (255, 255));
            assert_eq!((*bg0, *bb1), (255, 255));
        }
        other => panic!("expected two banded gradients, got {other:?}"),
    }
}

#[test]
fn a_backwards_angle_lays_the_bands_out_from_the_far_end() {
    // Pointing at PI the gradient runs right→left, so the first pair of stops
    // belongs on the *right* of the rect.
    let out = cmds(
        "linear_gradient({x: 0, y: 0, w: 100, h: 20}, [#ff0000, #00ff00, #0000ff], 3.14159265)",
    );
    match &out[0] {
        DrawCommand::RectGradient { x, r0, g1, .. } => {
            assert_eq!(*x, 50);
            assert_eq!((*r0, *g1), (255, 255));
        }
        other => panic!("expected a banded gradient, got {other:?}"),
    }
}

#[test]
fn a_two_stop_linear_gradient_is_a_single_command() {
    // No banding to do, so it degrades to exactly `draw_rect_gradient` — a
    // multi-stop helper must not cost anything in the common case.
    let out = cmds("linear_gradient({x: 0, y: 0, w: 10, h: 10}, [#ff0000, #0000ff], 0.0)");
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], DrawCommand::RectGradient { .. }));
}

#[test]
fn a_rounded_multi_stop_gradient_clips_its_bands_to_the_rounded_shape() {
    // The corners come from a rounded clip, not from the bands: only the end
    // bands touch a corner, so rounding each one would notch the middle.
    let out =
        cmds("linear_gradient({x: 0, y: 0, w: 90, h: 20}, [#ff0000, #00ff00, #0000ff], 0.0, 6)");
    assert!(matches!(
        out.first(),
        Some(DrawCommand::ClipPush { radius: 6, .. })
    ));
    assert!(matches!(out.last(), Some(DrawCommand::ClipPop)));
}

#[test]
fn shadow_takes_a_partial_options_record() {
    let out =
        cmds("draw_shadow({x: 10, y: 20, w: 100, h: 50}, {radius: 8, blur: 16, dy: 4, a: 60})");
    assert_eq!(
        out,
        vec![DrawCommand::Shadow {
            x: 10,
            y: 20,
            w: 100,
            h: 50,
            radius: 8,
            blur: 16,
            spread: 0,
            dx: 0,
            dy: 4,
            r: 0,
            g: 0,
            b: 0,
            a: 60,
        }]
    );
}

#[test]
fn the_positional_shadow_form_matches_the_record_form() {
    let out = cmds(
        "draw_shadow({x: 0, y: 0, w: 10, h: 10}, 4, 12, #102030, 90)\n\
         draw_shadow({x: 0, y: 0, w: 10, h: 10}, {radius: 4, blur: 12, color: #102030, a: 90})",
    );
    assert_eq!(out[0], out[1]);
}

#[test]
fn clip_push_and_pop_reach_the_command_stream() {
    let out = cmds(
        "clip_push({x: 1, y: 2, w: 3, h: 4})\n\
         clip_push({x: 1, y: 2, w: 3, h: 4}, 6)\n\
         clip_pop()\n\
         clip({x: 1, y: 2, w: 3, h: 4}, 6)\n\
         clip_none()",
    );
    assert_eq!(
        out,
        vec![
            DrawCommand::ClipPush {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
                radius: 0
            },
            DrawCommand::ClipPush {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
                radius: 6
            },
            DrawCommand::ClipPop,
            DrawCommand::Clip {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
                radius: 6
            },
            DrawCommand::ClipNone,
        ]
    );
}

#[test]
fn an_image_can_round_its_corners() {
    let out = cmds(
        "draw_image(\"a.png\", {x: 0, y: 0, w: 32, h: 32}, 255, 16)\n\
         draw_image(\"a.png\", {x: 0, y: 0, w: 32, h: 32})",
    );
    match (&out[0], &out[1]) {
        (
            DrawCommand::Image { radius, .. },
            DrawCommand::Image {
                radius: plain_r, ..
            },
        ) => assert_eq!((*radius, *plain_r), (16, 0)),
        other => panic!("expected two images, got {other:?}"),
    }
}

#[test]
fn the_new_commands_serialize_without_their_defaults() {
    // The serialization keeps the pre-existing shape for anything a script did
    // not ask for, so a consumer sees new keys only when a feature is used —
    // the same contract `radius`/`width`/`a` already follow.
    let out = cmds(
        "draw_rect_gradient({x: 0, y: 0, w: 10, h: 10}, #ff0000, #0000ff, 0.0)\n\
         draw_shadow({x: 0, y: 0, w: 10, h: 10}, {blur: 8})\n\
         clip_push({x: 0, y: 0, w: 4, h: 4})\n\
         clip_pop()\n\
         draw_image(\"a.png\", {x: 0, y: 0, w: 4, h: 4})",
    );
    assert_eq!(
        json(&out[0]),
        r#"{"op":"rect_gradient","x":0,"y":0,"w":10,"h":10,"r0":255,"g0":0,"b0":0,"r1":0,"g1":0,"b1":255,"angle":0.0}"#
    );
    assert_eq!(
        json(&out[1]),
        r#"{"op":"shadow","x":0,"y":0,"w":10,"h":10,"blur":8,"r":0,"g":0,"b":0,"a":64}"#
    );
    assert_eq!(
        json(&out[2]),
        r#"{"op":"clip_push","x":0,"y":0,"w":4,"h":4}"#
    );
    assert_eq!(json(&out[3]), r#"{"op":"clip_pop"}"#);
    assert_eq!(
        json(&out[4]),
        r#"{"op":"image","source":"a.png","x":0,"y":0,"w":4,"h":4}"#
    );
}

#[test]
fn a_native_still_rejects_a_bad_argument() {
    // Wrapping in the prelude must not turn a type error into a silent
    // no-draw: the native's own check still fires, at the script's call site.
    let mut ui = Headless::new("draw_shadow({x: 0, y: 0, w: 1, h: 1}, \"nope\")").unwrap();
    assert!(
        ui.frame().is_err(),
        "a string where the options record goes must be an error"
    );
}
