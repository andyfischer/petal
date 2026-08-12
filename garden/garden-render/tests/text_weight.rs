//! `TextStyle::weight` has to change pixels.
//!
//! The protocol carries a weight, `text_width` measures it, and until now the
//! renderer dropped it on the floor: the embedded family ships **Regular
//! only**, and cosmic-text resolves a bold request against the faces it has, so
//! a bold run came back identical to a regular one. No Bold face is vendored in
//! this repo (`assets/` has `JetBrainsMono-Regular.ttf` and nothing else), so
//! the renderer emboldens synthetically instead — the run is drawn twice with a
//! sub-pixel horizontal offset, thickening stems without moving any glyph.
//!
//! The layout half of that promise is the part scripts depend on: bold text
//! must occupy exactly the columns the caller computed from `text_width`.

use garden_render::{
    AtlasStats, Capture, Color, HeadlessRenderer, Primitive, Rect, Scene, TextStyle,
};

const W: f32 = 260.0;
const H: f32 = 60.0;

fn renderer() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new((W, H), 1.0) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

fn scene_with_weight(weight: u16) -> Scene {
    Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![Primitive::Text {
            pos: (4.0, 4.0),
            text: "Hamburgefons".to_string(),
            color: Color::rgb(1.0, 1.0, 1.0),
            clip: Rect::new(0.0, 0.0, W, H),
            size: 24.0,
            style: TextStyle {
                weight,
                ..TextStyle::default()
            },
        }],
    }
}

/// Total ink, as the sum of the red channel — a thicker stem raises this even
/// where it does not add a fully-lit pixel.
fn ink(cap: &Capture) -> u64 {
    cap.rgba.chunks_exact(4).map(|p| p[0] as u64).sum()
}

/// Horizontal extent of the drawn ink, in physical pixels.
fn ink_span(cap: &Capture) -> (u32, u32) {
    let (mut left, mut right) = (None, 0u32);
    for x in 0..cap.width {
        let lit = (0..cap.height).any(|y| cap.rgba[((y * cap.width + x) as usize) * 4] > 40);
        if lit {
            left.get_or_insert(x);
            right = x;
        }
    }
    (left.unwrap_or(0), right)
}

#[test]
fn bold_draws_heavier_than_regular() {
    let Some(mut r) = renderer() else { return };
    let regular = ink(&r.capture(&scene_with_weight(400)));
    let bold = ink(&r.capture(&scene_with_weight(700)));
    assert!(
        bold > regular + regular / 10,
        "weight 700 must draw visibly heavier than 400 (regular ink {regular}, bold {bold})"
    );
}

/// Weight must not shift layout: the run has to start at the same pixel and end
/// within a pixel or two of the regular run, because callers positioned the
/// following text from `text_width`, which does not know about the smear.
#[test]
fn bold_does_not_move_the_run() {
    let Some(mut r) = renderer() else { return };
    let (rl, rr) = ink_span(&r.capture(&scene_with_weight(400)));
    let (bl, br) = ink_span(&r.capture(&scene_with_weight(700)));
    assert_eq!(bl, rl, "bold must start on the same pixel as regular");
    assert!(
        br >= rr && br - rr <= 2,
        "bold must not extend past regular by more than the smear \
         (regular ends {rr}, bold ends {br})"
    );
}

/// Weights below semibold are untouched — no accidental thickening of the
/// editor's own chrome, which draws at 400.
#[test]
fn sub_semibold_weights_are_unchanged() {
    let Some(mut r) = renderer() else { return };
    let regular = ink(&r.capture(&scene_with_weight(400)));
    for w in [100u16, 300, 400, 500] {
        assert_eq!(
            ink(&r.capture(&scene_with_weight(w))),
            regular,
            "weight {w} must render exactly like regular"
        );
    }
}

/// Heavier weights are heavier still, so the axis is continuous rather than a
/// single on/off step.
#[test]
fn heavier_weights_draw_heavier() {
    let Some(mut r) = renderer() else { return };
    let semi = ink(&r.capture(&scene_with_weight(600)));
    let bold = ink(&r.capture(&scene_with_weight(700)));
    let black = ink(&r.capture(&scene_with_weight(900)));
    assert!(
        semi <= bold && bold < black,
        "ink must grow with weight (600 {semi}, 700 {bold}, 900 {black})"
    );
}

/// Atlas pressure is reported, and a normal frame reports no overflow. This is
/// the reading `/state` surfaces so a full atlas is a number an agent can read
/// rather than a screenshot it has to interpret.
#[test]
fn atlas_pressure_is_reported() {
    let Some(mut r) = renderer() else { return };

    assert_eq!(
        r.text_atlas_stats(),
        AtlasStats::default(),
        "a renderer that has drawn nothing reports no pressure"
    );

    let sizes = [10.0f32, 12.0, 14.0, 14.0, 20.0, 32.0];
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: sizes
            .iter()
            .enumerate()
            .map(|(i, &s)| Primitive::Text {
                pos: (2.0, 2.0 + i as f32 * 8.0),
                text: "size".to_string(),
                color: Color::rgb(1.0, 1.0, 1.0),
                clip: Rect::new(0.0, 0.0, W, H),
                size: s,
                style: TextStyle::default(),
            })
            .collect(),
    };
    let _ = r.capture(&scene);

    let stats = r.text_atlas_stats();
    assert_eq!(stats.runs, 6, "six text runs were staged");
    assert_eq!(stats.distinct_sizes, 5, "14.0 appears twice");
    assert_eq!(stats.dropped_batches, 0);
    assert_eq!(
        stats.overflows, 0,
        "a six-run frame must not exhaust the atlas"
    );
}
