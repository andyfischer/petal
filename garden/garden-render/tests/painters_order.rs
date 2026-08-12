//! Painter's order holds across primitive *kinds*, not just within one.
//!
//! The renderer used to draw the scene in three fixed passes — quads, then
//! meshes, then text — so a text run always composited on top of every shape no
//! matter where it sat in `Scene::primitives`. That made overlays impossible to
//! build: a Petal panel's context menu could paint its own background over the
//! list underneath, but the list's *text* punched straight back through it, and
//! panel authors had to hand-suppress whatever text the menu was going to cover.
//!
//! These render a shape over text and text over a shape and read the pixels.
//! Like the other GPU tests here, they skip without an adapter.

use garden_render::{Color, HeadlessRenderer, Primitive, Rect, Scene, TextStyle, Vertex};

const W: f32 = 96.0;
const H: f32 = 48.0;

fn renderer() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new((W, H), 1.0) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

/// A big text run in the top-left, guaranteed to put ink where the shapes below
/// are drawn.
fn text() -> Primitive {
    Primitive::Text {
        pos: (0.0, 0.0),
        text: "MMMM".to_string(),
        color: Color::rgb(1.0, 1.0, 1.0),
        clip: Rect::new(0.0, 0.0, W, H),
        size: 32.0,
        style: TextStyle::default(),
    }
}

fn cover_quad(color: Color) -> Primitive {
    Primitive::Quad {
        rect: Rect::new(0.0, 0.0, W, H),
        color,
    }
}

fn cover_mesh(color: Color) -> Primitive {
    Primitive::Mesh {
        vertices: vec![
            Vertex::new((0.0, 0.0), color),
            Vertex::new((0.0, H), color),
            Vertex::new((W, H), color),
            Vertex::new((0.0, 0.0), color),
            Vertex::new((W, H), color),
            Vertex::new((W, 0.0), color),
        ],
        clip: Rect::new(0.0, 0.0, W, H),
    }
}

/// How many pixels differ meaningfully from `expect`.
fn pixels_unlike(cap: &garden_render::Capture, expect: [u8; 3]) -> u32 {
    let mut n = 0;
    for i in (0..cap.rgba.len()).step_by(4) {
        let d = (cap.rgba[i] as i32 - expect[0] as i32).abs()
            + (cap.rgba[i + 1] as i32 - expect[1] as i32).abs()
            + (cap.rgba[i + 2] as i32 - expect[2] as i32).abs();
        if d > 24 {
            n += 1;
        }
    }
    n
}

/// Sanity anchor: with the text drawn *last* it must be visible, so the
/// "covered" cases below are really about order and not about the text failing
/// to render at all.
#[test]
fn text_over_a_shape_is_visible() {
    let Some(mut r) = renderer() else { return };
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![cover_quad(Color::rgb(0.0, 0.0, 0.0)), text()],
    };
    let cap = r.capture(&scene);
    assert!(
        pixels_unlike(&cap, [0, 0, 0]) > 100,
        "text drawn last should be visible over the quad"
    );
}

#[test]
fn a_quad_drawn_after_text_covers_it() {
    let Some(mut r) = renderer() else { return };
    let red = Color::rgb(1.0, 0.0, 0.0);
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        // Text first, then an opaque quad over the whole target.
        primitives: vec![text(), cover_quad(red)],
    };
    let cap = r.capture(&scene);
    let leaked = pixels_unlike(&cap, [255, 0, 0]);
    assert_eq!(
        leaked, 0,
        "an opaque quad drawn after a text run must hide it completely; \
         {leaked} pixels of text showed through"
    );
}

#[test]
fn a_mesh_drawn_after_text_covers_it() {
    let Some(mut r) = renderer() else { return };
    let red = Color::rgb(1.0, 0.0, 0.0);
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![text(), cover_mesh(red)],
    };
    let cap = r.capture(&scene);
    // The two triangles meet on a diagonal, which antialiases against itself;
    // allow that seam rather than demanding a perfectly flat field.
    let leaked = pixels_unlike(&cap, [255, 0, 0]);
    assert!(
        leaked <= (W * 1.5) as u32,
        "an opaque mesh drawn after a text run must hide it; \
         {leaked} pixels showed through"
    );
}

/// The overlay shape actually in use: a menu panel painted over a list, with
/// the menu's own label on top. Text below the panel is hidden, text above it
/// survives — both halves in one scene, which is what a context menu needs.
#[test]
fn an_overlay_hides_the_text_beneath_it_and_keeps_its_own() {
    let Some(mut r) = renderer() else { return };
    let panel = Color::rgb(0.0, 0.0, 1.0);
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            text(),            // the list underneath
            cover_quad(panel), // the menu background
            Primitive::Text {
                // the menu's own label, drawn after the panel
                pos: (0.0, 0.0),
                text: "MMMM".to_string(),
                color: Color::rgb(1.0, 1.0, 0.0),
                clip: Rect::new(0.0, 0.0, W, H),
                size: 32.0,
                style: TextStyle::default(),
            },
        ],
    };
    let cap = r.capture(&scene);

    // No white (the covered run) survives...
    let mut white = 0;
    let mut yellow = 0;
    for i in (0..cap.rgba.len()).step_by(4) {
        let (rr, gg, bb) = (cap.rgba[i], cap.rgba[i + 1], cap.rgba[i + 2]);
        if rr > 200 && gg > 200 && bb > 200 {
            white += 1;
        }
        if rr > 200 && gg > 200 && bb < 80 {
            yellow += 1;
        }
    }
    assert_eq!(white, 0, "the covered text run must not show through");
    assert!(
        yellow > 100,
        "the overlay's own label must draw on top of it (got {yellow} px)"
    );
}
