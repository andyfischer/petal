//! Translucent color composites in **sRGB space**, the way CSS does.
//!
//! The renderer used to linearize every color on the CPU and let an sRGB
//! surface re-encode on store, so `ALPHA_BLENDING` mixed light physically:
//! 50% black over white came out `#bbbbbb` instead of the `#808080` a designer
//! reads off every other tool. Every translucent overlay, hairline and shadow
//! in the ecosystem was therefore tuned against a number that disagreed with
//! the design it came from.
//!
//! These are exact acceptance values, not vibes: `255 - a` for black over
//! white, at three alphas, for a quad, a mesh and a text run — the three
//! pipelines that must agree with each other or a translucent label drifts
//! away from the translucent rect it sits on.

use garden_render::{Color, HeadlessRenderer, Primitive, Rect, Scene, TextStyle, Vertex};

const W: f32 = 64.0;
const H: f32 = 64.0;
/// Readback tolerance: MSAA resolve and the u8 round-trip each cost under a
/// count, but the sum can land either side of the exact value.
const TOL: i32 = 1;

fn renderer() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new((W, H), 1.0) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

fn white() -> Color {
    Color::rgb(1.0, 1.0, 1.0)
}

/// Black at `alpha`/255.
fn black(alpha: u8) -> Color {
    Color::rgba(0.0, 0.0, 0.0, alpha as f32 / 255.0)
}

/// The red channel at `(x, y)` of a capture of `prims` over a white ground.
fn pixel(renderer: &mut HeadlessRenderer, prims: Vec<Primitive>, x: u32, y: u32) -> u8 {
    let shot = renderer.capture(&Scene {
        bg: white(),
        primitives: prims,
    });
    shot.rgba[((y * shot.width + x) * 4) as usize]
}

fn assert_near(got: u8, want: u8, what: &str) {
    assert!(
        (got as i32 - want as i32).abs() <= TOL,
        "{what}: expected {want} +/-{TOL}, got {got}"
    );
}

/// The headline numbers: black over white at a = 26 / 64 / 128 must read
/// 230 / 191 / 128 (i.e. `255 - a`), not the 243 / 224 / 187 that linear-light
/// blending produced.
#[test]
fn a_translucent_quad_blends_the_way_css_blends() {
    let Some(mut r) = renderer() else { return };
    for (alpha, want) in [(26u8, 230u8), (64, 191), (128, 128)] {
        let got = pixel(
            &mut r,
            vec![Primitive::Quad {
                rect: Rect::new(0.0, 0.0, 40.0, 40.0),
                color: black(alpha),
            }],
            8,
            8,
        );
        assert_near(got, want, &format!("quad a={alpha}"));
    }
}

/// The mesh pipeline is a second shader with its own vertex color path, so it
/// gets its own reading rather than inheriting the quad's.
#[test]
fn a_translucent_mesh_matches_the_quad() {
    let Some(mut r) = renderer() else { return };
    for (alpha, want) in [(26u8, 230u8), (64, 191), (128, 128)] {
        let c = black(alpha);
        let quad = Rect::new(0.0, 0.0, 40.0, 40.0);
        let (x1, y1) = (quad.x + quad.w, quad.y + quad.h);
        let got = pixel(
            &mut r,
            vec![Primitive::Mesh {
                vertices: vec![
                    Vertex::new((quad.x, quad.y), c),
                    Vertex::new((x1, quad.y), c),
                    Vertex::new((quad.x, y1), c),
                    Vertex::new((x1, quad.y), c),
                    Vertex::new((x1, y1), c),
                    Vertex::new((quad.x, y1), c),
                ],
                clip: Rect::new(0.0, 0.0, W, H),
            }],
            8,
            8,
        );
        assert_near(got, want, &format!("mesh a={alpha}"));
    }
}

/// Text goes through glyphon, which has its own pipeline, its own atlas and
/// its own idea of color management. If it is left in glyphon's `Accurate`
/// mode while the shapes composite in sRGB, a translucent glyph lands on a
/// different gray than a translucent rect of the same color — a mismatch that
/// is nearly invisible per-pixel and completely wrong in aggregate.
///
/// A glyph's own coverage antialiasing means we cannot name an exact value, so
/// this compares the *darkest* pixel of a half-alpha black run against the
/// same half-alpha black as a quad: full coverage inside a stem must reach the
/// same gray.
#[test]
fn translucent_text_lands_on_the_same_gray_as_a_translucent_quad() {
    let Some(mut r) = renderer() else { return };
    let alpha = 128u8;

    let quad = pixel(
        &mut r,
        vec![Primitive::Quad {
            rect: Rect::new(0.0, 0.0, W, H),
            color: black(alpha),
        }],
        8,
        8,
    );

    // A big, dense glyph so some pixel is fully covered.
    let shot = r.capture(&Scene {
        bg: white(),
        primitives: vec![Primitive::Text {
            pos: (0.0, 0.0),
            text: "MMM".to_string(),
            color: black(alpha),
            clip: Rect::new(0.0, 0.0, W, H),
            size: 48.0,
            style: TextStyle::default(),
        }],
    });
    let darkest = shot
        .rgba
        .chunks_exact(4)
        .map(|px| px[0])
        .min()
        .expect("capture is not empty");

    assert_near(darkest, quad, "fully-covered glyph vs. quad");
}
