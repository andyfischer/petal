//! Per-vertex color and the rounded-clip mask, read off the GPU.
//!
//! Both are properties of the mesh pipeline that no CPU test can check: a
//! gradient is *exact* only because the rasterizer interpolates vertex color
//! affinely, and the mask's antialiased edge exists only in the fragment
//! shader. Like the other GPU tests here, these skip without an adapter.

use garden_render::{ClipMask, Color, HeadlessRenderer, Primitive, Rect, Scene, Vertex};

const W: f32 = 64.0;
const H: f32 = 64.0;

fn renderer() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new((W, H), 1.0) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

fn full_clip() -> Rect {
    Rect::new(0.0, 0.0, W, H)
}

/// Two triangles covering the whole target, `left` on the left edge shading to
/// `right` on the right, optionally cut against `mask`.
fn ramp(left: Color, right: Color, mask: ClipMask) -> Primitive {
    let v = |x: f32, y: f32, c: Color| Vertex::masked((x, y), c, mask);
    Primitive::Mesh {
        vertices: vec![
            v(0.0, 0.0, left),
            v(W, 0.0, right),
            v(0.0, H, left),
            v(W, 0.0, right),
            v(W, H, right),
            v(0.0, H, left),
        ],
        clip: full_clip(),
    }
}

fn shoot(r: &mut HeadlessRenderer, bg: Color, prims: Vec<Primitive>) -> Vec<u8> {
    r.capture(&Scene {
        bg,
        primitives: prims,
    })
    .rgba
}

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let o = ((y * W as u32 + x) * 4) as usize;
    [rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]
}

/// A two-stop gradient is a *linear* function of x, so a pixel a quarter of
/// the way across must read a quarter of the way between the stops — not the
/// nearest of N bands. This is the property that lets one rounded rect's worth
/// of triangles replace the 32-band stack the fallback used.
#[test]
fn a_two_stop_gradient_interpolates_exactly() {
    let Some(mut r) = renderer() else { return };
    let black = Color::rgb(0.0, 0.0, 0.0);
    let white = Color::rgb(1.0, 1.0, 1.0);
    let shot = shoot(&mut r, black, vec![ramp(black, white, ClipMask::NONE)]);

    for (x, want) in [(8u32, 34u8), (32, 130), (56, 226)] {
        let got = px(&shot, x, 32)[0];
        // Sample centers sit at x + 0.5, so the exact value is
        // 255 * (x + 0.5) / 64; allow a count either side of it.
        assert!(
            (got as i32 - want as i32).abs() <= 2,
            "x={x}: expected ~{want}, got {got}"
        );
    }
}

/// A circular mask over a filled mesh: opaque at the center, gone at the
/// corners. The scissor rect cannot express this — the corners are inside it.
#[test]
fn a_circular_mask_cuts_the_corners_away() {
    let Some(mut r) = renderer() else { return };
    let bg = Color::rgb(0.0, 0.0, 0.0);
    let white = Color::rgb(1.0, 1.0, 1.0);
    // The largest circle that fits: radius 32 on a 64x64 rect.
    let mask = ClipMask::rounded(full_clip(), 32.0);
    let shot = shoot(&mut r, bg, vec![ramp(white, white, mask)]);

    assert_eq!(px(&shot, 32, 32)[0], 255, "center is inside the circle");
    assert_eq!(px(&shot, 1, 1)[0], 0, "top-left corner is outside it");
    assert_eq!(px(&shot, 62, 62)[0], 0, "bottom-right corner is outside it");
    // On the axis, at the rim: still covered.
    assert_eq!(px(&shot, 32, 1)[0], 255, "top edge midpoint is inside it");
}

/// The mask's edge is antialiased, not a staircase: walking the diagonal from
/// the corner toward the center has to cross at least one partially-covered
/// pixel. A hard-edged discard would step straight from 0 to 255.
#[test]
fn the_mask_edge_is_feathered() {
    let Some(mut r) = renderer() else { return };
    let bg = Color::rgb(0.0, 0.0, 0.0);
    let white = Color::rgb(1.0, 1.0, 1.0);
    let mask = ClipMask::rounded(full_clip(), 32.0);
    let shot = shoot(&mut r, bg, vec![ramp(white, white, mask)]);

    let partial = (0..32).any(|i| {
        let v = px(&shot, i, i)[0];
        v > 8 && v < 247
    });
    assert!(
        partial,
        "no partially covered pixel along the corner diagonal"
    );
}

/// A zero radius is "no mask", the state every vertex Garden itself emits is
/// in — the corners must survive.
#[test]
fn a_zero_radius_mask_clips_nothing() {
    let Some(mut r) = renderer() else { return };
    let bg = Color::rgb(0.0, 0.0, 0.0);
    let white = Color::rgb(1.0, 1.0, 1.0);
    let shot = shoot(&mut r, bg, vec![ramp(white, white, ClipMask::NONE)]);
    assert_eq!(px(&shot, 1, 1)[0], 255);
    assert_eq!(px(&shot, 62, 62)[0], 255);
}
