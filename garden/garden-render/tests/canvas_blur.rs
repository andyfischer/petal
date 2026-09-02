//! End-to-end checks for the offscreen-canvas primitives: drawing into a
//! canvas and compositing it back, snapshotting the frame, blurring, group
//! opacity, and painter's order across target switches — all through the
//! real GPU pipeline and read back as pixels.
//!
//! Needs a GPU adapter; skips (stays green) without one.

use garden_render::{ClipMask, Color, HeadlessRenderer, Primitive, Rect, Scene};

const W: f32 = 64.0;
const H: f32 = 64.0;
const FULL: Rect = Rect::new(0.0, 0.0, W, H);

fn renderer() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new((W, H), 1.0) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

fn px(cap: &garden_render::Capture, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * cap.width + x) * 4) as usize;
    [
        cap.rgba[i],
        cap.rgba[i + 1],
        cap.rgba[i + 2],
        cap.rgba[i + 3],
    ]
}

fn near(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tol)
}

fn quad(rect: Rect, color: Color) -> Primitive {
    Primitive::Quad { rect, color }
}

fn canvas_draw(id: u32, rect: Rect, alpha: f32) -> Primitive {
    Primitive::CanvasDraw {
        id,
        rect,
        alpha,
        clip: FULL,
        mask: ClipMask::NONE,
    }
}

#[test]
fn a_canvas_drawn_into_and_composited_lands_where_it_is_placed() {
    let Some(mut r) = renderer() else { return };
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            Primitive::Canvas {
                id: 1,
                size: (16.0, 16.0),
            },
            Primitive::Target { id: 1 },
            // Canvas-relative coordinates: fills the whole canvas.
            quad(Rect::new(0.0, 0.0, 16.0, 16.0), Color::rgb(1.0, 0.0, 0.0)),
            Primitive::Target { id: 0 },
            canvas_draw(1, Rect::new(32.0, 32.0, 16.0, 16.0), 1.0),
        ],
    };
    let cap = r.capture(&scene);
    assert!(
        near(px(&cap, 40, 40), [255, 0, 0, 255], 2),
        "{:?}",
        px(&cap, 40, 40)
    );
    // Outside the placed rect the frame is untouched — including the region
    // the canvas-relative quad would have covered had it hit the frame.
    assert!(
        near(px(&cap, 8, 8), [0, 0, 0, 255], 2),
        "{:?}",
        px(&cap, 8, 8)
    );
    assert!(near(px(&cap, 20, 40), [0, 0, 0, 255], 2));
}

#[test]
fn canvas_draw_alpha_scales_the_whole_layer() {
    let Some(mut r) = renderer() else { return };
    // Two overlapping opaque quads inside the layer, drawn at 50%: the
    // overlap must show only the top quad at 50%, not a double-blend.
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            Primitive::Canvas {
                id: 1,
                size: (64.0, 64.0),
            },
            Primitive::Target { id: 1 },
            quad(Rect::new(0.0, 0.0, 40.0, 64.0), Color::rgb(0.0, 1.0, 0.0)),
            quad(Rect::new(20.0, 0.0, 44.0, 64.0), Color::rgb(1.0, 1.0, 1.0)),
            Primitive::Target { id: 0 },
            canvas_draw(1, FULL, 0.5),
        ],
    };
    let cap = r.capture(&scene);
    // Overlap at x=30: white at 50% over black → mid grey, no green.
    let p = px(&cap, 30, 32);
    assert!(near(p, [128, 128, 128, 255], 6), "{p:?}");
    // Green-only region at x=10.
    let p = px(&cap, 10, 32);
    assert!(near(p, [0, 128, 0, 255], 6), "{p:?}");
}

#[test]
fn a_snapshot_of_the_frame_captures_what_was_drawn_so_far() {
    let Some(mut r) = renderer() else { return };
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 1.0),
        primitives: vec![
            quad(Rect::new(0.0, 0.0, 32.0, 64.0), Color::rgb(1.0, 0.0, 0.0)),
            Primitive::Canvas {
                id: 7,
                size: (32.0, 32.0),
            },
            // Straddles the red/blue boundary.
            Primitive::Snapshot {
                id: 7,
                from: (16.0, 16.0),
                clip: FULL,
            },
            // Cover the frame, then draw the snapshot elsewhere.
            quad(FULL, Color::rgb(0.0, 0.0, 0.0)),
            canvas_draw(7, Rect::new(0.0, 0.0, 32.0, 32.0), 1.0),
        ],
    };
    let cap = r.capture(&scene);
    // Snapshot region was frame (16..48, 16..48): left half red, right blue.
    assert!(
        near(px(&cap, 4, 4), [255, 0, 0, 255], 2),
        "{:?}",
        px(&cap, 4, 4)
    );
    assert!(
        near(px(&cap, 28, 4), [0, 0, 255, 255], 2),
        "{:?}",
        px(&cap, 28, 4)
    );
    // Elsewhere the covering black quad stands.
    assert!(near(px(&cap, 50, 50), [0, 0, 0, 255], 2));
}

#[test]
fn blur_spreads_a_hard_edge_and_preserves_flat_regions() {
    let Some(mut r) = renderer() else { return };
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            Primitive::Canvas {
                id: 1,
                size: (64.0, 64.0),
            },
            Primitive::Target { id: 1 },
            quad(Rect::new(0.0, 0.0, 64.0, 64.0), Color::rgb(0.0, 0.0, 0.0)),
            quad(Rect::new(32.0, 0.0, 32.0, 64.0), Color::rgb(1.0, 1.0, 1.0)),
            Primitive::Target { id: 0 },
            Primitive::Blur { id: 1, radius: 3.0 },
            canvas_draw(1, FULL, 1.0),
        ],
    };
    let cap = r.capture(&scene);
    // The edge at x=32 has become a ramp.
    let at = |x: u32| px(&cap, x, 32)[0] as i32;
    assert!(at(2) <= 4, "far left stays black: {}", at(2));
    assert!(at(61) >= 251, "far right stays white: {}", at(61));
    assert!(at(30) > 20 && at(30) < 235, "ramp at 30: {}", at(30));
    assert!(at(34) > 20 && at(34) < 235, "ramp at 34: {}", at(34));
    assert!(at(28) < at(32) && at(32) < at(36), "monotone ramp");
}

#[test]
fn a_large_blur_radius_takes_the_downsampled_path_and_still_blurs() {
    let Some(mut r) = renderer() else { return };
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            Primitive::Canvas {
                id: 1,
                size: (64.0, 64.0),
            },
            Primitive::Target { id: 1 },
            quad(Rect::new(0.0, 0.0, 64.0, 64.0), Color::rgb(0.0, 0.0, 0.0)),
            quad(Rect::new(32.0, 0.0, 32.0, 64.0), Color::rgb(1.0, 1.0, 1.0)),
            Primitive::Target { id: 0 },
            Primitive::Blur {
                id: 1,
                radius: 20.0,
            },
            canvas_draw(1, FULL, 1.0),
        ],
    };
    let cap = r.capture(&scene);
    let at = |x: u32| px(&cap, x, 32)[0] as i32;
    // With sigma 20 the ramp spans essentially the whole width.
    assert!(at(8) > 10, "left is lifted: {}", at(8));
    assert!(at(56) < 245, "right is pulled down: {}", at(56));
    assert!(at(16) < at(32) && at(32) < at(48), "monotone ramp");
    assert!((at(32) - 128).abs() < 24, "centre near mid: {}", at(32));
}

#[test]
fn blurring_a_translucent_layer_does_not_darken_its_edge() {
    let Some(mut r) = renderer() else { return };
    // A white square on a transparent canvas, blurred, drawn over white:
    // premultiplied blur keeps every pixel white (alpha varies, color does
    // not). A straight-alpha blur would bleed transparent black in.
    let scene = Scene {
        bg: Color::rgb(1.0, 1.0, 1.0),
        primitives: vec![
            Primitive::Canvas {
                id: 1,
                size: (64.0, 64.0),
            },
            Primitive::Target { id: 1 },
            quad(Rect::new(16.0, 16.0, 32.0, 32.0), Color::rgb(1.0, 1.0, 1.0)),
            Primitive::Target { id: 0 },
            Primitive::Blur { id: 1, radius: 4.0 },
            canvas_draw(1, FULL, 1.0),
        ],
    };
    let cap = r.capture(&scene);
    for x in [12u32, 16, 20, 32, 44, 48, 52] {
        let p = px(&cap, x, 32);
        assert!(near(p, [255, 255, 255, 255], 3), "x={x}: {p:?}");
    }
}

#[test]
fn painters_order_holds_across_target_switches() {
    let Some(mut r) = renderer() else { return };
    // Frame: red; canvas: green; frame after: blue quad over the left half
    // of where the canvas will land; canvas composited last → green wins on
    // the right, blue was covered on the left too since the draw is after.
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            quad(FULL, Color::rgb(1.0, 0.0, 0.0)),
            Primitive::Canvas {
                id: 1,
                size: (32.0, 32.0),
            },
            Primitive::Target { id: 1 },
            quad(Rect::new(0.0, 0.0, 32.0, 32.0), Color::rgb(0.0, 1.0, 0.0)),
            Primitive::Target { id: 0 },
            quad(Rect::new(0.0, 0.0, 64.0, 64.0), Color::rgb(0.0, 0.0, 1.0)),
            canvas_draw(1, Rect::new(16.0, 16.0, 32.0, 32.0), 1.0),
            quad(Rect::new(16.0, 16.0, 8.0, 8.0), Color::rgb(1.0, 1.0, 0.0)),
        ],
    };
    let cap = r.capture(&scene);
    assert!(near(px(&cap, 2, 2), [0, 0, 255, 255], 2), "blue frame");
    assert!(
        near(px(&cap, 40, 40), [0, 255, 0, 255], 2),
        "canvas over blue"
    );
    assert!(
        near(px(&cap, 18, 18), [255, 255, 0, 255], 2),
        "quad over canvas"
    );
}

#[test]
fn drawing_into_a_missing_target_is_dropped_not_misplaced() {
    let Some(mut r) = renderer() else { return };
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            Primitive::Target { id: 99 },
            quad(FULL, Color::rgb(1.0, 0.0, 0.0)),
            Primitive::Target { id: 0 },
            quad(Rect::new(0.0, 0.0, 8.0, 8.0), Color::rgb(0.0, 1.0, 0.0)),
            canvas_draw(99, FULL, 1.0),
        ],
    };
    let cap = r.capture(&scene);
    assert!(near(px(&cap, 32, 32), [0, 0, 0, 255], 2));
    assert!(near(px(&cap, 4, 4), [0, 255, 0, 255], 2));
}

#[test]
fn canvases_are_reused_across_frames_without_bleeding() {
    let Some(mut r) = renderer() else { return };
    let frame = |color: Color| Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            Primitive::Canvas {
                id: 1,
                size: (64.0, 64.0),
            },
            Primitive::Target { id: 1 },
            quad(Rect::new(0.0, 0.0, 32.0, 64.0), color),
            Primitive::Target { id: 0 },
            canvas_draw(1, FULL, 1.0),
        ],
    };
    let _ = r.capture(&frame(Color::rgb(1.0, 0.0, 0.0)));
    let cap = r.capture(&frame(Color::rgb(0.0, 1.0, 0.0)));
    assert!(near(px(&cap, 16, 32), [0, 255, 0, 255], 2));
    // The right half was cleared with the canvas — no red from last frame.
    assert!(near(px(&cap, 48, 32), [0, 0, 0, 255], 2));
}
