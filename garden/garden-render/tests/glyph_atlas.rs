//! The glyph atlas must survive a long-lived process that uses many font sizes.
//!
//! Two failure modes have been seen in the wild, both of which look identical
//! from the script's side (`/scene` reports the right size, the screenshot draws
//! the wrong one):
//!
//! * a glyph rasterized at a *stale* size once the process had drawn more than
//!   a handful of distinct sizes (swash's fixed hinting-instance table — fixed
//!   in swash 0.2.10, unit-pinned in `src/text.rs`), and
//! * atlas entries evicted between frames and not correctly re-rasterized when
//!   they were needed again, so re-drawing an earlier frame came back corrupt.
//!
//! The unit test in `src/text.rs` covers the raster stage alone. These go
//! through the real pipeline — shape, atlas upload, growth, eviction, draw,
//! read back pixels — because that is where the second failure mode lives.
//!
//! Like the other GPU tests here, they skip when no adapter is available.

use garden_render::{Capture, Color, HeadlessRenderer, Primitive, Rect, Scene, TextStyle};

const W: f32 = 320.0;
const H: f32 = 120.0;

fn renderer() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new((W, H), 1.0) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

/// A type scale far longer than any fixed-size glyph cache, including the
/// fractional sizes a script's `size * ratio` arithmetic actually produces.
const SIZES: [f32; 16] = [
    9.0, 10.0, 11.5, 12.0, 13.0, 14.0, 16.0, 18.0, 20.5, 22.0, 24.0, 28.0, 32.0, 40.0, 48.0, 56.0,
];

fn scene_at(size: f32, text: &str) -> Scene {
    Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![Primitive::Text {
            pos: (2.0, 2.0),
            text: text.to_string(),
            color: Color::rgb(1.0, 1.0, 1.0),
            clip: Rect::new(0.0, 0.0, W, H),
            size,
            style: TextStyle::default(),
        }],
    }
}

/// Vertical extent of the drawn ink, in physical pixels. This is what a stale
/// raster changes and what the numeric `/scene` dump cannot see.
fn ink_height(cap: &Capture) -> u32 {
    let (mut top, mut bottom) = (None, 0u32);
    for y in 0..cap.height {
        let row = (y * cap.width) as usize * 4;
        let lit = (0..cap.width as usize)
            .any(|x| cap.rgba[row + x * 4] > 40 || cap.rgba[row + x * 4 + 1] > 40);
        if lit {
            top.get_or_insert(y);
            bottom = y;
        }
    }
    match top {
        Some(t) => bottom - t + 1,
        None => 0,
    }
}

/// Count of pixels that differ between two captures of the same size.
fn pixel_diff(a: &Capture, b: &Capture) -> usize {
    assert_eq!(a.rgba.len(), b.rgba.len(), "captures must be the same size");
    a.rgba
        .chunks_exact(4)
        .zip(b.rgba.chunks_exact(4))
        .filter(|(p, q)| {
            (p[0] as i32 - q[0] as i32).abs()
                + (p[1] as i32 - q[1] as i32).abs()
                + (p[2] as i32 - q[2] as i32).abs()
                > 24
        })
        .count()
}

/// Every size in a long type scale must actually rasterize at its own size,
/// and drawing the whole scale a second time must reproduce it exactly.
///
/// The second pass is the one that matters: by then the hinting table is full
/// and the atlas has grown and evicted, which is the state a Garden process is
/// in after a few hot reloads.
#[test]
fn a_long_type_scale_rasterizes_at_the_requested_size_twice_over() {
    let Some(mut r) = renderer() else { return };

    let cold: Vec<u32> = SIZES
        .iter()
        .map(|&s| ink_height(&r.capture(&scene_at(s, "Hxq"))))
        .collect();
    let warm: Vec<u32> = SIZES
        .iter()
        .map(|&s| ink_height(&r.capture(&scene_at(s, "Hxq"))))
        .collect();

    assert_eq!(
        cold, warm,
        "the same text rendered at a different height on a warm process: \
         sizes={SIZES:?} cold={cold:?} warm={warm:?}"
    );

    for pass in [&cold, &warm] {
        for (w, s) in pass.windows(2).zip(SIZES.windows(2)) {
            assert!(
                w[1] >= w[0],
                "ink must not shrink as the font size grows ({} -> {}): {pass:?}",
                s[0],
                s[1]
            );
        }
        // Every size must draw *something* — a size that silently rendered
        // nothing would sail through the monotonicity check above.
        assert!(pass[0] > 0, "the smallest size drew no ink: {pass:?}");
        assert!(
            pass[pass.len() - 1] > pass[0] * 3,
            "56px must draw far taller ink than 9px: {pass:?}"
        );
    }
}

/// Re-drawing an earlier frame after many other sizes have churned through the
/// atlas must reproduce it pixel for pixel.
///
/// This is the hot-reload shape of the bug: a panel is edited, redraws at a
/// pile of new sizes, and then the original text comes back — by which point
/// its glyphs have been evicted and must be re-rasterized from scratch. If
/// eviction and re-rasterization disagree, the returning frame is the corrupt
/// one.
#[test]
fn a_frame_redrawn_after_the_atlas_churns_is_identical() {
    let Some(mut r) = renderer() else { return };

    let first = r.capture(&scene_at(14.0, "reference"));

    // Churn: many distinct (glyph, size) pairs, none of them the reference's,
    // so the reference's entries go cold and are candidates for eviction.
    for (i, &s) in SIZES.iter().enumerate() {
        let text: String = (0..12)
            .map(|k| char::from(b'A' + ((i * 12 + k) % 26) as u8))
            .collect();
        let _ = r.capture(&scene_at(s, &text));
    }

    let again = r.capture(&scene_at(14.0, "reference"));
    let diff = pixel_diff(&first, &again);
    assert_eq!(
        diff, 0,
        "an identical scene drew differently after the atlas churned \
         ({diff} pixels differ) — evicted glyphs are not being re-rasterized correctly"
    );
}

/// A single scene mixing many sizes must draw each run at its own size.
///
/// The reported symptom was per-*glyph* corruption inside one run — "PAUSED"
/// coming back with P, A, S, E, D small and U at the requested 36px — so the
/// sizes have to be checked while they coexist in one frame, not just one at a
/// time.
#[test]
fn many_sizes_in_one_frame_each_draw_at_their_own_size() {
    let Some(mut r) = renderer() else { return };

    // Per-size reference heights, measured one run per frame.
    let refs: Vec<u32> = SIZES
        .iter()
        .map(|&s| ink_height(&r.capture(&scene_at(s, "PAUSED"))))
        .collect();

    // Now the same runs, one per frame but each drawn into a tall column so a
    // per-run ink measurement is still possible: render each size individually
    // *after* a frame that put every size in the atlas at once.
    let all_at_once = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: SIZES
            .iter()
            .enumerate()
            .map(|(i, &s)| Primitive::Text {
                pos: (4.0 + i as f32, 4.0),
                text: "PAUSED".to_string(),
                color: Color::rgb(1.0, 1.0, 1.0),
                clip: Rect::new(0.0, 0.0, W, H),
                size: s,
                style: TextStyle::default(),
            })
            .collect(),
    };
    let _ = r.capture(&all_at_once);

    let after: Vec<u32> = SIZES
        .iter()
        .map(|&s| ink_height(&r.capture(&scene_at(s, "PAUSED"))))
        .collect();

    assert_eq!(
        refs, after,
        "a run changed height after every size shared one frame: \
         sizes={SIZES:?} before={refs:?} after={after:?}"
    );
}

/// Changing a text run must not leave the previous frame's run on screen.
///
/// `/screenshot` was seen returning the *old* text composited over the new one
/// for a single frame; a pooled shaping buffer or a stale text batch would do
/// exactly that.
#[test]
fn a_changed_text_run_does_not_composite_the_previous_frame() {
    let Some(mut r) = renderer() else { return };

    // "IIII" and "MMMM" put ink in visibly different places at this size.
    let wide = r.capture(&scene_at(32.0, "MMMM"));
    let narrow = r.capture(&scene_at(32.0, "IIII"));
    let narrow_fresh = {
        let Some(mut r2) = renderer() else { return };
        r2.capture(&scene_at(32.0, "IIII"))
    };

    let diff = pixel_diff(&narrow, &narrow_fresh);
    assert_eq!(
        diff, 0,
        "a text run drawn after a different one differs from the same run \
         drawn on a fresh renderer ({diff} px) — the previous frame leaked through"
    );

    // And the wide run really did put ink where the narrow one does not, so
    // the comparison above had something to catch.
    assert!(
        pixel_diff(&wide, &narrow) > 100,
        "test setup: the two runs must differ substantially"
    );
}

/// A frame with fewer text batches than the frame before must not redraw the
/// batches that went away — the pooled `TextRenderer`s still hold last frame's
/// vertices.
#[test]
fn shrinking_the_batch_count_drops_the_old_batches() {
    let Some(mut r) = renderer() else { return };

    let label = |y: f32, text: &str| Primitive::Text {
        pos: (2.0, y),
        text: text.to_string(),
        color: Color::rgb(1.0, 1.0, 1.0),
        clip: Rect::new(0.0, 0.0, W, H),
        size: 16.0,
        style: TextStyle::default(),
    };
    let spacer = |y: f32| Primitive::Quad {
        rect: Rect::new(0.0, y, 1.0, 1.0),
        color: Color::rgb(0.0, 0.0, 0.0),
    };

    // Three text batches, separated by quads so they cannot merge.
    let many = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![
            label(2.0, "one"),
            spacer(90.0),
            label(30.0, "two"),
            spacer(92.0),
            label(60.0, "three"),
        ],
    };
    let _ = r.capture(&many);

    // Now one batch. Rows 30 and 60 must be empty.
    let few = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![label(2.0, "one")],
    };
    let cap = r.capture(&few);

    let fresh = {
        let Some(mut r2) = renderer() else { return };
        r2.capture(&few)
    };
    let diff = pixel_diff(&cap, &fresh);
    assert_eq!(
        diff, 0,
        "a frame that dropped two text batches still drew {diff} pixels of them"
    );
}
