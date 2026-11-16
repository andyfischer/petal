//! `compose::scale`: a frame drawn in logical points, magnified to the
//! physical pixels of a HiDPI drawable.

use petal_ui::compose::scale;
use petal_ui::draw::DrawCommand;

fn rect(x: i32, y: i32, w: u32, h: u32) -> DrawCommand {
    DrawCommand::Rect { x, y, w, h, r: 1, g: 2, b: 3, a: 255, radius: 4 }
}

#[test]
fn scale_one_is_identity() {
    let cmds = [
        rect(3, 5, 7, 9),
        DrawCommand::plain_text("hi", 1, 2, 13, 255, 255, 255, 255),
        DrawCommand::Line { x1: 1, y1: 2, x2: 3, y2: 4, r: 0, g: 0, b: 0, a: 255, width: 1 },
    ];
    for c in cmds {
        assert_eq!(scale(c.clone(), 1.0), c);
    }
}

#[test]
fn scale_two_doubles_geometry_and_text_size() {
    assert_eq!(scale(rect(3, 5, 7, 9), 2.0), DrawCommand::Rect {
        x: 6, y: 10, w: 14, h: 18, r: 1, g: 2, b: 3, a: 255, radius: 8
    });
    match scale(DrawCommand::plain_text("hi", 1, 2, 13, 255, 255, 255, 255), 2.0) {
        DrawCommand::Text { x, y, size, text, .. } => {
            assert_eq!((x, y, size, text.as_str()), (2, 4, 26, "hi"));
        }
        other => panic!("{other:?}"),
    }
    match scale(
        DrawCommand::Line { x1: 1, y1: 2, x2: 3, y2: 4, r: 0, g: 0, b: 0, a: 255, width: 1 },
        2.0,
    ) {
        DrawCommand::Line { x1, y1, x2, y2, width, .. } => {
            assert_eq!((x1, y1, x2, y2, width), (2, 4, 6, 8, 2));
        }
        other => panic!("{other:?}"),
    }
}

/// At a fractional scale, neighbours that shared an edge in points still
/// share one in pixels: no seams, no overlaps.
#[test]
fn fractional_scale_keeps_rects_tiled() {
    let s = 1.5;
    let mut edge = 0;
    for i in 0..20 {
        let DrawCommand::Rect { x, w, .. } = scale(rect(i * 3, 0, 3, 1), s) else { unreachable!() };
        assert_eq!(x, edge, "rect {i} starts where the last ended");
        edge = x + w as i32;
    }
    assert_eq!(edge, 90);
}

#[test]
fn hairlines_survive_shrinking() {
    let DrawCommand::RectOutline { width, w, .. } = scale(
        DrawCommand::RectOutline { x: 0, y: 0, w: 1, h: 1, r: 0, g: 0, b: 0, a: 255, width: 1, radius: 0 },
        0.4,
    ) else {
        unreachable!()
    };
    assert_eq!((width, w), (1, 1));
}

#[test]
fn canvas_own_size_stays_implicit() {
    let DrawCommand::DrawCanvas { x, y, w, h, .. } =
        scale(DrawCommand::DrawCanvas { id: 1, x: 5, y: 6, a: 255, w: 0, h: 0 }, 2.0)
    else {
        unreachable!()
    };
    assert_eq!((x, y, w, h), (10, 12, 0, 0));
}
