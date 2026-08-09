//! The scene pass is multisampled, so tessellated geometry gets smooth edges.
//!
//! Garden draws every shape a panel asks for — circles, lines, rounded corners —
//! as plain triangles with no coverage information of their own. Before the
//! scene pass was multisampled, that meant binary coverage: a diagonal edge was
//! either background or foreground, never in between, and panels full of meters
//! and rings visibly stepped. This renders one diagonal and looks for the
//! in-between pixels that only antialiasing can produce.
//!
//! Like `cjk_raster`, this needs a GPU adapter and skips without one, so
//! `cargo test -p garden-render` stays green in headless CI.

use garden_render::{Color, HeadlessRenderer, Primitive, Rect, Scene, Vertex};

/// Pixels that are neither the background nor the fill — the partial coverage
/// along an antialiased edge. `bg` and `fg` are the expected extremes, and a
/// pixel counts as intermediate when it is meaningfully far from both.
fn edge_pixels(cap: &garden_render::Capture, bg: [u8; 3], fg: [u8; 3]) -> u32 {
    let dist = |px: [u8; 3], to: [u8; 3]| {
        (px[0] as i32 - to[0] as i32).abs()
            + (px[1] as i32 - to[1] as i32).abs()
            + (px[2] as i32 - to[2] as i32).abs()
    };
    let mut edge = 0;
    for i in (0..cap.rgba.len()).step_by(4) {
        let px = [cap.rgba[i], cap.rgba[i + 1], cap.rgba[i + 2]];
        if dist(px, bg) > 24 && dist(px, fg) > 24 {
            edge += 1;
        }
    }
    edge
}

#[test]
fn a_diagonal_edge_is_antialiased() {
    let (w, h) = (64.0_f32, 64.0_f32);
    let mut renderer = match HeadlessRenderer::new((w, h), 1.0) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            return;
        }
    };

    // Black on white, so partial coverage lands unmistakably between the two.
    let bg = Color::rgb(1.0, 1.0, 1.0);
    let fg = Color::rgb(0.0, 0.0, 0.0);

    // A right triangle filling the lower-left half: one long diagonal, and two
    // axis-aligned edges that antialiasing must leave alone.
    let scene = Scene {
        bg,
        primitives: vec![Primitive::Mesh {
            vertices: vec![
                Vertex::new((0.0, 0.0), fg),
                Vertex::new((0.0, h), fg),
                Vertex::new((w, h), fg),
            ],
            clip: Rect::new(0.0, 0.0, w, h),
        }],
    };

    let cap = renderer.capture(&scene);
    let edge = edge_pixels(&cap, [255, 255, 255], [0, 0, 0]);

    // The diagonal crosses ~64 rows, and every one of them should contribute a
    // partially covered pixel. A single-sampled pass produces none at all, so
    // the useful assertion is simply "many", not an exact count that would
    // change with the sample level.
    assert!(
        edge >= 32,
        "expected a soft diagonal, got {edge} intermediate pixels in {}x{} — \
         the scene pass looks single-sampled",
        cap.width,
        cap.height
    );
}
