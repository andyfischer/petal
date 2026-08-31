//! End-to-end check that a run can be drawn in a font that is *not* compiled
//! into the binary.
//!
//! Garden ships two faces. The claim this file tests is that a panel script
//! naming a third one — any family installed on the machine — actually gets
//! different glyphs on screen, and that the advance table the script measures
//! with describes those glyphs rather than the embedded monospace's.
//!
//! Needs a GPU adapter and at least one installed proportional family; both
//! are optional, so the test skips rather than fails on a headless or
//! font-less machine.

use garden_render::fonts::{self, FontId};
use garden_render::{Capture, Color, HeadlessRenderer, Primitive, Rect, Scene, TextStyle};

const W: f32 = 400.0;
const H: f32 = 60.0;
const TEXT: &str = "Hamburgefonstiv";

fn renderer() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new((W, H), 1.0) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

/// A family that is installed here, is not one of the embedded two, and whose
/// name is stable enough to hardcode. `None` skips the test.
fn a_system_family() -> Option<String> {
    [
        "Georgia",
        "Times New Roman",
        "Helvetica",
        "Arial",
        "Verdana",
    ]
    .into_iter()
    .find(|name| !fonts::resolve(name).is_embedded())
    .map(str::to_string)
}

fn scene_in(font: FontId) -> Scene {
    Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![Primitive::Text {
            pos: (4.0, 4.0),
            text: TEXT.to_string(),
            color: Color::rgb(1.0, 1.0, 1.0),
            clip: Rect::new(0.0, 0.0, W, H),
            size: 24.0,
            style: TextStyle {
                font,
                ..TextStyle::default()
            },
        }],
    }
}

/// The lit pixels of a capture, as a bitmap — two identical renders compare
/// equal, two different faces do not.
fn lit(cap: &Capture) -> Vec<bool> {
    cap.rgba.chunks_exact(4).map(|p| p[0] > 40).collect()
}

/// Horizontal extent of the drawn ink, in physical pixels.
fn ink_right(cap: &Capture) -> u32 {
    let mut right = 0;
    for x in 0..cap.width {
        if (0..cap.height).any(|y| cap.rgba[((y * cap.width + x) as usize) * 4] > 40) {
            right = x;
        }
    }
    right
}

#[test]
fn a_system_family_draws_different_glyphs_than_the_embedded_one() {
    let Some(mut r) = renderer() else { return };
    let Some(name) = a_system_family() else {
        eprintln!("skipping: no non-embedded family installed");
        return;
    };
    let id = fonts::resolve(&name);
    let system = r.capture(&scene_in(id));
    let mono = r.capture(&scene_in(FontId::MONO));
    assert_ne!(
        lit(&system),
        lit(&mono),
        "{name} rendered pixel-identically to the embedded monospace face — \
         the family name never reached the shaper"
    );
    // Not just different: proportional, so the same string is narrower than in
    // a fixed-pitch face. This is what catches a "resolved to *some* other
    // font" pass that isn't actually the one asked for.
    assert!(
        ink_right(&system) < ink_right(&mono),
        "{name} should set {TEXT:?} narrower than monospace \
         (system ends {}, mono ends {})",
        ink_right(&system),
        ink_right(&mono)
    );
}

/// The measurement half: what a script's `text_width` sums has to describe the
/// face that was actually drawn, or every centered and right-aligned run in it
/// lands wrong.
#[test]
fn the_published_advances_describe_the_face_that_gets_drawn() {
    let Some(mut r) = renderer() else { return };
    let Some(name) = a_system_family() else {
        eprintln!("skipping: no non-embedded family installed");
        return;
    };
    let id = fonts::resolve(&name);
    let ratios = fonts::advance_ratios(id, 400, false);
    let size = 24.0f64;
    let predicted: f64 = TEXT.chars().map(|c| ratios[c as usize] * size).sum();

    let cap = r.capture(&scene_in(id));
    // The run starts at x=4 and the last glyph's ink stops short of its own
    // advance, so the drawn extent is a little under the summed width. A couple
    // of pixels of slack covers that; a table measured from the wrong face is
    // off by far more.
    let drawn = ink_right(&cap) as f64 - 4.0;
    assert!(
        drawn <= predicted && predicted - drawn < 6.0,
        "{name}: measured {predicted:.1}px but drew ink out to {drawn:.1}px"
    );
}
