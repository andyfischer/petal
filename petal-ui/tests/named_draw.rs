//! The prelude's draw wrappers called by name.
//!
//! Every shape of every primitive — flat, mixed (flat coordinates and a colour
//! record) and record — has parameters a call can name, and a named call must
//! draw exactly what the positional call it spells out draws. Each case below
//! is such a pair, run as two scripts whose command streams are compared.
//!
//! The positional half is what pins the other direction: where two shapes
//! share an argument count, one declaration takes both and tells them apart
//! by an argument's type, and the names it is declared with must not change
//! which shape an all-positional call means.

use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;

const PROLOGUE: &str = "let C = {r: 11, g: 22, b: 33}\n\
                        let D = {r: 44, g: 55, b: 66, a: 77}\n\
                        let R = {x: 1, y: 2, w: 30, h: 40}\n\
                        let P = {x: 5, y: 6}\n\
                        let Q = {x: 15, y: 16}\n\
                        let T = {x: 25, y: 9}\n\
                        let PTS = [{x: 0, y: 0}, {x: 10, y: 0}, {x: 10, y: 10}]\n\
                        let ST = {size: 13, color: C, weight: 700}\n\
                        let cv = canvas(10, 10)\n";

fn draw(call: &str) -> Vec<DrawCommand> {
    let src = format!("{PROLOGUE}{call}\n");
    let mut ui = Headless::new(&src).unwrap_or_else(|e| panic!("`{call}` failed to compile: {e}"));
    ui.frame()
        .unwrap_or_else(|e| panic!("`{call}` failed: {e}"))
        .to_vec()
}

/// Each `(named, positional)` pair draws the same thing, and draws something.
fn same(pairs: &[(&str, &str)]) {
    // What the prologue alone emits (its `canvas`).
    let idle = draw("").len();
    for (named, positional) in pairs {
        let want = draw(positional);
        assert!(want.len() > idle, "`{positional}` drew nothing");
        assert_eq!(draw(named), want, "`{named}` vs `{positional}`");
    }
}

fn error(call: &str) -> String {
    let src = format!("{PROLOGUE}{call}\n");
    match Headless::new(&src) {
        Err(e) => e,
        Ok(mut ui) => ui
            .frame()
            .map(|_| ())
            .expect_err(&format!("`{call}` should fail")),
    }
}

#[test]
fn draw_image_by_name() {
    same(&[
        (
            "draw_image(source: \"i\", x: 1, y: 2, w: 3, h: 4)",
            "draw_image(\"i\", 1, 2, 3, 4)",
        ),
        (
            "draw_image(\"i\", x: 1, y: 2, w: 3, h: 4, a: 200, radius: 6)",
            "draw_image(\"i\", 1, 2, 3, 4, 200, 6)",
        ),
        // `a` skipped to reach `radius`.
        (
            "draw_image(\"i\", 1, 2, 3, 4, radius: 6)",
            "draw_image(\"i\", 1, 2, 3, 4, 255, 6)",
        ),
        ("draw_image(source: \"i\", rect: R)", "draw_image(\"i\", R)"),
        (
            "draw_image(\"i\", rect: R, radius: 7)",
            "draw_image(\"i\", R, 255, 7)",
        ),
        (
            "draw_image(\"i\", R, radius: 7, a: 9)",
            "draw_image(\"i\", R, 9, 7)",
        ),
    ]);
}

#[test]
fn draw_rect_by_name() {
    same(&[
        (
            "draw_rect(x: 1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7)",
            "draw_rect(1, 2, 3, 4, 5, 6, 7)",
        ),
        (
            "draw_rect(x: 1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7, a: 8)",
            "draw_rect(1, 2, 3, 4, 5, 6, 7, 8)",
        ),
        (
            "draw_rect(x: 1, y: 2, w: 3, h: 4, c: C)",
            "draw_rect(1, 2, 3, 4, C)",
        ),
        (
            "draw_rect(h: 4, w: 3, c: C, a: 9, y: 2, x: 1)",
            "draw_rect(1, 2, 3, 4, C, 9)",
        ),
        ("draw_rect(rect: R, c: C)", "draw_rect(R, C)"),
        ("draw_rect(R, c: C, a: 9)", "draw_rect(R, C, 9)"),
    ]);
}

#[test]
fn draw_rect_rounded_by_name() {
    same(&[
        (
            "draw_rect_rounded(x: 1, y: 2, w: 3, h: 4, radius: 5, r: 6, g: 7, b: 8)",
            "draw_rect_rounded(1, 2, 3, 4, 5, 6, 7, 8)",
        ),
        (
            "draw_rect_rounded(1, 2, 3, 4, radius: 5, r: 6, g: 7, b: 8, a: 9)",
            "draw_rect_rounded(1, 2, 3, 4, 5, 6, 7, 8, 9)",
        ),
        (
            "draw_rect_rounded(x: 1, y: 2, w: 3, h: 4, radius: 5, c: C)",
            "draw_rect_rounded(1, 2, 3, 4, 5, C)",
        ),
        (
            "draw_rect_rounded(x: 1, y: 2, w: 3, h: 4, radius: 5, c: C, a: 9)",
            "draw_rect_rounded(1, 2, 3, 4, 5, C, 9)",
        ),
        (
            "draw_rect_rounded(rect: R, radius: 5, c: C)",
            "draw_rect_rounded(R, 5, C)",
        ),
        (
            "draw_rect_rounded(R, radius: 5, c: C, a: 9)",
            "draw_rect_rounded(R, 5, C, 9)",
        ),
    ]);
}

#[test]
fn draw_rect_outline_by_name() {
    same(&[
        // The call the merged `c_or_r, a_or_g, width_or_b` names made impossible.
        (
            "draw_rect_outline(x: 0, y: 0, w: 10, h: 4, c: C, a: 255, width: 2)",
            "draw_rect_outline(0, 0, 10, 4, C, 255, 2)",
        ),
        (
            "draw_rect_outline(x: 0, y: 0, w: 10, h: 4, c: C, width: 2)",
            "draw_rect_outline(0, 0, 10, 4, C, 255, 2)",
        ),
        (
            "draw_rect_outline(0, 0, 10, 4, C, width: 2)",
            "draw_rect_outline(0, 0, 10, 4, C, 255, 2)",
        ),
        (
            "draw_rect_outline(x: 0, y: 0, w: 10, h: 4, c: C)",
            "draw_rect_outline(0, 0, 10, 4, C)",
        ),
        (
            "draw_rect_outline(x: 0, y: 0, w: 10, h: 4, c: C, a: 8)",
            "draw_rect_outline(0, 0, 10, 4, C, 8)",
        ),
        // Seven arguments named as the flat shape: the seven-parameter
        // declaration is the mixed one, and the names send this past it.
        (
            "draw_rect_outline(x: 1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7)",
            "draw_rect_outline(1, 2, 3, 4, 5, 6, 7)",
        ),
        (
            "draw_rect_outline(1, 2, 3, 4, 5, 6, b: 7)",
            "draw_rect_outline(1, 2, 3, 4, 5, 6, 7)",
        ),
        (
            "draw_rect_outline(x: 1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7, width: 9)",
            "draw_rect_outline(1, 2, 3, 4, 5, 6, 7, 255, 9)",
        ),
        (
            "draw_rect_outline(x: 1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7, a: 8, width: 9)",
            "draw_rect_outline(1, 2, 3, 4, 5, 6, 7, 8, 9)",
        ),
        (
            "draw_rect_outline(rect: R, c: C)",
            "draw_rect_outline(R, C)",
        ),
        (
            "draw_rect_outline(rect: R, c: C, a: 8)",
            "draw_rect_outline(R, C, 8)",
        ),
        (
            "draw_rect_outline(R, C, width: 9)",
            "draw_rect_outline(R, C, 255, 9)",
        ),
    ]);
}

#[test]
fn draw_line_by_name() {
    same(&[
        (
            "draw_line(x1: 1, y1: 2, x2: 3, y2: 4, r: 5, g: 6, b: 7)",
            "draw_line(1, 2, 3, 4, 5, 6, 7)",
        ),
        (
            "draw_line(x1: 1, y1: 2, x2: 3, y2: 4, r: 5, g: 6, b: 7, a: 8, width: 9)",
            "draw_line(1, 2, 3, 4, 5, 6, 7, 8, 9)",
        ),
        (
            "draw_line(1, 2, 3, 4, 5, 6, 7, width: 9)",
            "draw_line(1, 2, 3, 4, 5, 6, 7, 255, 9)",
        ),
        // Five arguments named as the mixed shape; the five-parameter
        // declaration is the two-point one.
        (
            "draw_line(x1: 1, y1: 2, x2: 3, y2: 4, c: C)",
            "draw_line(1, 2, 3, 4, C)",
        ),
        ("draw_line(1, 2, 3, 4, c: C)", "draw_line(1, 2, 3, 4, C)"),
        (
            "draw_line(x1: 1, y1: 2, x2: 3, y2: 4, c: C, a: 8)",
            "draw_line(1, 2, 3, 4, C, 8)",
        ),
        (
            "draw_line(x1: 1, y1: 2, x2: 3, y2: 4, c: C, a: 8, width: 9)",
            "draw_line(1, 2, 3, 4, C, 8, 9)",
        ),
        (
            "draw_line(1, 2, 3, 4, C, width: 9)",
            "draw_line(1, 2, 3, 4, C, 255, 9)",
        ),
        ("draw_line(p1: P, p2: Q, c: C)", "draw_line(P, Q, C)"),
        (
            "draw_line(p2: Q, p1: P, c: C, a: 8)",
            "draw_line(P, Q, C, 8)",
        ),
        (
            "draw_line(p1: P, p2: Q, c: C, a: 8, width: 9)",
            "draw_line(P, Q, C, 8, 9)",
        ),
        ("draw_line(P, Q, C, width: 9)", "draw_line(P, Q, C, 255, 9)"),
    ]);
}

#[test]
fn draw_circle_by_name() {
    same(&[
        (
            "draw_circle(cx: 1, cy: 2, radius: 3, r: 4, g: 5, b: 6)",
            "draw_circle(1, 2, 3, 4, 5, 6)",
        ),
        (
            "draw_circle(cx: 1, cy: 2, radius: 3, r: 4, g: 5, b: 6, a: 7)",
            "draw_circle(1, 2, 3, 4, 5, 6, 7)",
        ),
        // Four arguments named as the mixed shape; the four-parameter
        // declaration is the centre-record one.
        (
            "draw_circle(cx: 1, cy: 2, radius: 3, c: C)",
            "draw_circle(1, 2, 3, C)",
        ),
        ("draw_circle(1, 2, 3, c: C)", "draw_circle(1, 2, 3, C)"),
        (
            "draw_circle(cx: 1, cy: 2, radius: 3, c: C, a: 7)",
            "draw_circle(1, 2, 3, C, 7)",
        ),
        (
            "draw_circle(center: P, radius: 3, c: C)",
            "draw_circle(P, 3, C)",
        ),
        (
            "draw_circle(center: P, radius: 3, c: C, a: 7)",
            "draw_circle(P, 3, C, 7)",
        ),
    ]);
}

#[test]
fn draw_rect_rounded_outline_by_name() {
    same(&[
        (
            "draw_rect_rounded_outline(x: 1, y: 2, w: 3, h: 4, radius: 5, r: 6, g: 7, b: 8)",
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, 6, 7, 8)",
        ),
        (
            "draw_rect_rounded_outline(x: 1, y: 2, w: 3, h: 4, radius: 5, r: 6, g: 7, b: 8, a: 9, width: 10)",
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)",
        ),
        (
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, 6, 7, 8, width: 10)",
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, 6, 7, 8, 255, 10)",
        ),
        (
            "draw_rect_rounded_outline(x: 1, y: 2, w: 3, h: 4, radius: 5, c: C)",
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, C)",
        ),
        (
            "draw_rect_rounded_outline(x: 1, y: 2, w: 3, h: 4, radius: 5, c: C, a: 9)",
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, C, 9)",
        ),
        (
            "draw_rect_rounded_outline(x: 1, y: 2, w: 3, h: 4, radius: 5, c: C, a: 9, width: 10)",
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, C, 9, 10)",
        ),
        (
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, C, width: 10)",
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, C, 255, 10)",
        ),
        (
            "draw_rect_rounded_outline(rect: R, radius: 5, c: C)",
            "draw_rect_rounded_outline(R, 5, C)",
        ),
        (
            "draw_rect_rounded_outline(rect: R, radius: 5, c: C, a: 9, width: 10)",
            "draw_rect_rounded_outline(R, 5, C, 9, 10)",
        ),
        (
            "draw_rect_rounded_outline(R, 5, C, width: 10)",
            "draw_rect_rounded_outline(R, 5, C, 255, 10)",
        ),
    ]);
}

#[test]
fn draw_circle_outline_by_name() {
    same(&[
        (
            "draw_circle_outline(cx: 1, cy: 2, radius: 3, r: 4, g: 5, b: 6)",
            "draw_circle_outline(1, 2, 3, 4, 5, 6)",
        ),
        (
            "draw_circle_outline(cx: 1, cy: 2, radius: 3, r: 4, g: 5, b: 6, a: 7, width: 8)",
            "draw_circle_outline(1, 2, 3, 4, 5, 6, 7, 8)",
        ),
        (
            "draw_circle_outline(1, 2, 3, 4, 5, 6, width: 8)",
            "draw_circle_outline(1, 2, 3, 4, 5, 6, 255, 8)",
        ),
        // The mixed shape shares every count with another; its names reach
        // it at each of them.
        (
            "draw_circle_outline(cx: 1, cy: 2, radius: 3, c: C)",
            "draw_circle_outline(1, 2, 3, C)",
        ),
        (
            "draw_circle_outline(1, 2, 3, c: C)",
            "draw_circle_outline(1, 2, 3, C)",
        ),
        (
            "draw_circle_outline(cx: 1, cy: 2, radius: 3, c: C, a: 7)",
            "draw_circle_outline(1, 2, 3, C, 7)",
        ),
        (
            "draw_circle_outline(1, 2, 3, C, a: 7)",
            "draw_circle_outline(1, 2, 3, C, 7)",
        ),
        (
            "draw_circle_outline(cx: 1, cy: 2, radius: 3, c: C, a: 7, width: 8)",
            "draw_circle_outline(1, 2, 3, C, 7, 8)",
        ),
        (
            "draw_circle_outline(cx: 1, cy: 2, radius: 3, c: C, width: 8)",
            "draw_circle_outline(1, 2, 3, C, 255, 8)",
        ),
        (
            "draw_circle_outline(center: P, radius: 3, c: C)",
            "draw_circle_outline(P, 3, C)",
        ),
        (
            "draw_circle_outline(center: P, radius: 3, c: C, a: 7)",
            "draw_circle_outline(P, 3, C, 7)",
        ),
        (
            "draw_circle_outline(center: P, radius: 3, c: C, a: 7, width: 8)",
            "draw_circle_outline(P, 3, C, 7, 8)",
        ),
        (
            "draw_circle_outline(P, 3, C, width: 8)",
            "draw_circle_outline(P, 3, C, 255, 8)",
        ),
    ]);
}

#[test]
fn draw_ellipse_by_name() {
    same(&[
        (
            "draw_ellipse(cx: 1, cy: 2, rx: 3, ry: 4, r: 5, g: 6, b: 7)",
            "draw_ellipse(1, 2, 3, 4, 5, 6, 7)",
        ),
        (
            "draw_ellipse(cx: 1, cy: 2, rx: 3, ry: 4, r: 5, g: 6, b: 7, a: 8)",
            "draw_ellipse(1, 2, 3, 4, 5, 6, 7, 8)",
        ),
        (
            "draw_ellipse(cx: 1, cy: 2, rx: 3, ry: 4, c: C)",
            "draw_ellipse(1, 2, 3, 4, C)",
        ),
        (
            "draw_ellipse(cx: 1, cy: 2, rx: 3, ry: 4, c: C, a: 8)",
            "draw_ellipse(1, 2, 3, 4, C, 8)",
        ),
        (
            "draw_ellipse(center: P, rx: 3, ry: 4, c: C)",
            "draw_ellipse(P, 3, 4, C)",
        ),
        (
            "draw_ellipse(center: P, rx: 3, ry: 4, c: C, a: 8)",
            "draw_ellipse(P, 3, 4, C, 8)",
        ),
    ]);
}

#[test]
fn draw_ellipse_outline_by_name() {
    same(&[
        (
            "draw_ellipse_outline(cx: 1, cy: 2, rx: 3, ry: 4, r: 5, g: 6, b: 7)",
            "draw_ellipse_outline(1, 2, 3, 4, 5, 6, 7)",
        ),
        (
            "draw_ellipse_outline(cx: 1, cy: 2, rx: 3, ry: 4, r: 5, g: 6, b: 7, a: 8, width: 9)",
            "draw_ellipse_outline(1, 2, 3, 4, 5, 6, 7, 8, 9)",
        ),
        (
            "draw_ellipse_outline(cx: 1, cy: 2, rx: 3, ry: 4, c: C)",
            "draw_ellipse_outline(1, 2, 3, 4, C)",
        ),
        (
            "draw_ellipse_outline(cx: 1, cy: 2, rx: 3, ry: 4, c: C, a: 8)",
            "draw_ellipse_outline(1, 2, 3, 4, C, 8)",
        ),
        (
            "draw_ellipse_outline(1, 2, 3, 4, C, a: 8)",
            "draw_ellipse_outline(1, 2, 3, 4, C, 8)",
        ),
        (
            "draw_ellipse_outline(cx: 1, cy: 2, rx: 3, ry: 4, c: C, a: 8, width: 9)",
            "draw_ellipse_outline(1, 2, 3, 4, C, 8, 9)",
        ),
        (
            "draw_ellipse_outline(cx: 1, cy: 2, rx: 3, ry: 4, c: C, width: 9)",
            "draw_ellipse_outline(1, 2, 3, 4, C, 255, 9)",
        ),
        (
            "draw_ellipse_outline(center: P, rx: 3, ry: 4, c: C)",
            "draw_ellipse_outline(P, 3, 4, C)",
        ),
        (
            "draw_ellipse_outline(center: P, rx: 3, ry: 4, c: C, a: 8)",
            "draw_ellipse_outline(P, 3, 4, C, 8)",
        ),
        (
            "draw_ellipse_outline(center: P, rx: 3, ry: 4, c: C, a: 8, width: 9)",
            "draw_ellipse_outline(P, 3, 4, C, 8, 9)",
        ),
        (
            "draw_ellipse_outline(P, 3, 4, C, width: 9)",
            "draw_ellipse_outline(P, 3, 4, C, 255, 9)",
        ),
    ]);
}

#[test]
fn fills_by_name() {
    same(&[
        (
            "fill_arc(cx: 1, cy: 2, r_in: 3, r_out: 4, a0: 0.5, a1: 1.5, r: 7, g: 8, b: 9)",
            "fill_arc(1, 2, 3, 4, 0.5, 1.5, 7, 8, 9)",
        ),
        (
            "fill_arc(cx: 1, cy: 2, r_in: 3, r_out: 4, a0: 0.5, a1: 1.5, r: 7, g: 8, b: 9, a: 10)",
            "fill_arc(1, 2, 3, 4, 0.5, 1.5, 7, 8, 9, 10)",
        ),
        (
            "fill_arc(cx: 1, cy: 2, r_in: 3, r_out: 4, a0: 0.5, a1: 1.5, c: C)",
            "fill_arc(1, 2, 3, 4, 0.5, 1.5, C)",
        ),
        (
            "fill_arc(cx: 1, cy: 2, r_in: 3, r_out: 4, a0: 0.5, a1: 1.5, c: C, a: 10)",
            "fill_arc(1, 2, 3, 4, 0.5, 1.5, C, 10)",
        ),
        (
            "fill_arc(center: P, r_in: 3, r_out: 4, a0: 0.5, a1: 1.5, c: C)",
            "fill_arc(P, 3, 4, 0.5, 1.5, C)",
        ),
        (
            "fill_arc(center: P, r_in: 3, r_out: 4, a0: 0.5, a1: 1.5, c: C, a: 10)",
            "fill_arc(P, 3, 4, 0.5, 1.5, C, 10)",
        ),
        (
            "fill_triangle(x1: 1, y1: 2, x2: 3, y2: 4, x3: 5, y3: 6, r: 7, g: 8, b: 9)",
            "fill_triangle(1, 2, 3, 4, 5, 6, 7, 8, 9)",
        ),
        (
            "fill_triangle(x1: 1, y1: 2, x2: 3, y2: 4, x3: 5, y3: 6, r: 7, g: 8, b: 9, a: 10)",
            "fill_triangle(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)",
        ),
        (
            "fill_triangle(x1: 1, y1: 2, x2: 3, y2: 4, x3: 5, y3: 6, c: C)",
            "fill_triangle(1, 2, 3, 4, 5, 6, C)",
        ),
        (
            "fill_triangle(x1: 1, y1: 2, x2: 3, y2: 4, x3: 5, y3: 6, c: C, a: 10)",
            "fill_triangle(1, 2, 3, 4, 5, 6, C, 10)",
        ),
        (
            "fill_triangle(p1: P, p2: Q, p3: T, c: C)",
            "fill_triangle(P, Q, T, C)",
        ),
        (
            "fill_triangle(p1: P, p2: Q, p3: T, c: C, a: 10)",
            "fill_triangle(P, Q, T, C, 10)",
        ),
        (
            "fill_poly(points: PTS, r: 1, g: 2, b: 3)",
            "fill_poly(PTS, 1, 2, 3)",
        ),
        (
            "fill_poly(points: PTS, r: 1, g: 2, b: 3, a: 4)",
            "fill_poly(PTS, 1, 2, 3, 4)",
        ),
        ("fill_poly(points: PTS, c: C)", "fill_poly(PTS, C)"),
        ("fill_poly(PTS, c: C, a: 4)", "fill_poly(PTS, C, 4)"),
        (
            "fill_polygon(points: PTS, r: 1, g: 2, b: 3)",
            "fill_polygon(PTS, 1, 2, 3)",
        ),
        (
            "fill_polygon(points: PTS, r: 1, g: 2, b: 3, a: 4)",
            "fill_polygon(PTS, 1, 2, 3, 4)",
        ),
        ("fill_polygon(points: PTS, c: C)", "fill_polygon(PTS, C)"),
        ("fill_polygon(PTS, c: C, a: 4)", "fill_polygon(PTS, C, 4)"),
        (
            "fill_fan(cx: 1, cy: 2, points: PTS, r: 4, g: 5, b: 6)",
            "fill_fan(1, 2, PTS, 4, 5, 6)",
        ),
        (
            "fill_fan(cx: 1, cy: 2, points: PTS, r: 4, g: 5, b: 6, a: 7)",
            "fill_fan(1, 2, PTS, 4, 5, 6, 7)",
        ),
        (
            "fill_fan(cx: 1, cy: 2, points: PTS, c: C)",
            "fill_fan(1, 2, PTS, C)",
        ),
        (
            "fill_fan(cx: 1, cy: 2, points: PTS, c: C, a: 7)",
            "fill_fan(1, 2, PTS, C, 7)",
        ),
        (
            "fill_fan(center: P, points: PTS, c: C)",
            "fill_fan(P, PTS, C)",
        ),
        (
            "fill_fan(center: P, points: PTS, c: C, a: 7)",
            "fill_fan(P, PTS, C, 7)",
        ),
    ]);
}

#[test]
fn polylines_by_name() {
    for f in ["draw_polyline", "draw_polygon_outline"] {
        let pairs = [
            ("(points: PTS, r: 1, g: 2, b: 3)", "(PTS, 1, 2, 3)"),
            (
                "(points: PTS, r: 1, g: 2, b: 3, a: 4, width: 5)",
                "(PTS, 1, 2, 3, 4, 5)",
            ),
            ("(PTS, 1, 2, 3, width: 5)", "(PTS, 1, 2, 3, 255, 5)"),
            ("(points: PTS, c: C)", "(PTS, C)"),
            ("(points: PTS, c: C, a: 4)", "(PTS, C, 4)"),
            ("(points: PTS, c: C, a: 4, width: 5)", "(PTS, C, 4, 5)"),
            ("(PTS, C, width: 5)", "(PTS, C, 255, 5)"),
        ];
        for (named, positional) in pairs {
            same(&[(&format!("{f}{named}"), &format!("{f}{positional}"))]);
        }
    }
}

#[test]
fn draw_text_by_name() {
    same(&[
        (
            "draw_text(text: \"t\", x: 1, y: 2, size: 12, r: 5, g: 6, b: 7)",
            "draw_text(\"t\", 1, 2, 12, 5, 6, 7)",
        ),
        (
            "draw_text(\"t\", x: 1, y: 2, size: 12, r: 5, g: 6, b: 7, a: 8)",
            "draw_text(\"t\", 1, 2, 12, 5, 6, 7, 8)",
        ),
        // Five arguments named as the mixed shape; the five-parameter
        // declaration is the `pos` one.
        (
            "draw_text(\"t\", x: 1, y: 2, size: 12, c: C)",
            "draw_text(\"t\", 1, 2, 12, C)",
        ),
        (
            "draw_text(\"t\", x: 1, y: 2, size: 12, c: C, a: 8)",
            "draw_text(\"t\", 1, 2, 12, C, 8)",
        ),
        // Four arguments named as the `pos` shape; the four-parameter
        // declaration is the styled one with flat coordinates.
        (
            "draw_text(\"t\", pos: P, size: 12, c: C)",
            "draw_text(\"t\", P, 12, C)",
        ),
        (
            "draw_text(text: \"t\", pos: P, size: 12, c: C, a: 8)",
            "draw_text(\"t\", P, 12, C, 8)",
        ),
        (
            "draw_text(\"t\", x: 1, y: 2, style: ST)",
            "draw_text(\"t\", 1, 2, ST)",
        ),
        (
            "draw_text(\"t\", pos: P, style: ST)",
            "draw_text(\"t\", P, ST)",
        ),
    ]);
}

#[test]
fn clips_by_name() {
    for f in ["clip", "clip_push"] {
        let pairs = [
            ("(x: 1, y: 2, w: 3, h: 4)", "(1, 2, 3, 4)"),
            ("(x: 1, y: 2, w: 3, h: 4, radius: 5)", "(1, 2, 3, 4, 5)"),
            ("(rect: R)", "(R)"),
            ("(rect: R, radius: 5)", "(R, 5)"),
        ];
        for (named, positional) in pairs {
            same(&[(&format!("{f}{named}"), &format!("{f}{positional}"))]);
        }
    }
}

#[test]
fn gradients_and_shadows_by_name() {
    same(&[
        (
            "draw_rect_gradient(rect: R, c0: C, c1: D, angle: 0.5)",
            "draw_rect_gradient(R, C, D, 0.5)",
        ),
        (
            "draw_rect_gradient(rect: R, c0: C, c1: D, angle: 0.5, a0: 100, a1: 200)",
            "draw_rect_gradient(R, C, D, 0.5, 100, 200)",
        ),
        // One stop's alpha overridden; the other keeps the stop's own.
        (
            "draw_rect_gradient(R, C, D, 0.5, a1: 200)",
            "draw_rect_gradient(R, C, D, 0.5, 255, 200)",
        ),
        (
            "draw_rect_gradient(R, C, D, 0.5, a0: 100)",
            "draw_rect_gradient(R, C, D, 0.5, 100, 77)",
        ),
        (
            "draw_rect_gradient(x: 1, y: 2, w: 3, h: 4, c0: C, c1: D, angle: 0.5)",
            "draw_rect_gradient(1, 2, 3, 4, C, D, 0.5)",
        ),
        (
            "draw_rect_gradient_rounded(rect: R, radius: 6, c0: C, c1: D, angle: 0.5)",
            "draw_rect_gradient_rounded(R, 6, C, D, 0.5)",
        ),
        (
            "draw_rect_gradient_rounded(rect: R, radius: 6, c0: C, c1: D, angle: 0.5, a0: 100, a1: 200)",
            "draw_rect_gradient_rounded(R, 6, C, D, 0.5, 100, 200)",
        ),
        (
            "draw_rect_gradient_rounded(x: 1, y: 2, w: 3, h: 4, radius: 6, c0: C, c1: D, angle: 0.5)",
            "draw_rect_gradient_rounded(1, 2, 3, 4, 6, C, D, 0.5)",
        ),
        (
            "draw_circle_gradient(center: P, radius: 9, c0: C, c1: D)",
            "draw_circle_gradient(P, 9, C, D)",
        ),
        (
            "draw_circle_gradient(center: P, radius: 9, c0: C, c1: D, a0: 100, a1: 200)",
            "draw_circle_gradient(P, 9, C, D, 100, 200)",
        ),
        (
            "draw_circle_gradient(cx: 1, cy: 2, radius: 9, c0: C, c1: D)",
            "draw_circle_gradient(1, 2, 9, C, D)",
        ),
        (
            "linear_gradient(rect: R, stops: [C, D, C], angle: 0.0)",
            "linear_gradient(R, [C, D, C], 0.0)",
        ),
        (
            "linear_gradient(rect: R, stops: [C, D, C], angle: 0.0, radius: 4)",
            "linear_gradient(R, [C, D, C], 0.0, 4)",
        ),
        (
            "draw_shadow(rect: R, radius: 8, blur: 16, c: D)",
            "draw_shadow(R, 8, 16, D)",
        ),
        (
            "draw_shadow(rect: R, radius: 8, blur: 16, c: D, a: 99)",
            "draw_shadow(R, 8, 16, D, 99)",
        ),
        (
            "draw_shadow(rect: R, opts: {radius: 8, blur: 16})",
            "draw_shadow(R, {radius: 8, blur: 16})",
        ),
    ]);
}

#[test]
fn draw_canvas_by_name() {
    same(&[
        ("draw_canvas(id: cv, x: 1, y: 2)", "draw_canvas(cv, 1, 2)"),
        (
            "draw_canvas(id: cv, x: 1, y: 2, a: 3)",
            "draw_canvas(cv, 1, 2, 3)",
        ),
        (
            "draw_canvas(cv, 1, 2, w: 4, h: 5)",
            "draw_canvas(cv, 1, 2, 255, 4, 5)",
        ),
        ("draw_canvas(id: cv, at: P)", "draw_canvas(cv, P)"),
        (
            "draw_canvas(id: cv, at: R, opts: {a: 9})",
            "draw_canvas(cv, R, {a: 9})",
        ),
    ]);
}

#[test]
fn a_name_no_shape_has_is_an_error() {
    let e = error("draw_rect_outline(x: 0, y: 0, w: 10, h: 4, colour: C)");
    assert!(e.contains("draw_rect_outline()"), "{e}");
    assert!(e.contains("colour"), "{e}");
    // Two shapes' names mixed in one call fit neither.
    let e = error("draw_circle(center: P, cy: 2, radius: 3, c: C)");
    assert!(e.contains("draw_circle()"), "{e}");
}

/// The one reading the names cannot fix (see the note at the head of the
/// prelude's draw section): flat coordinates passed positionally, with alpha
/// skipped by naming `width`, has the count and the name of the centre-record
/// shape's longest declaration, so `width` lands in the mixed shape's alpha.
#[test]
fn positional_coordinates_with_a_named_width_read_width_as_alpha() {
    assert_eq!(
        draw("draw_circle_outline(1, 2, 3, C, width: 8)"),
        draw("draw_circle_outline(1, 2, 3, C, 8)"),
    );
    assert_eq!(
        draw("draw_ellipse_outline(1, 2, 3, 4, C, width: 9)"),
        draw("draw_ellipse_outline(1, 2, 3, 4, C, 9)"),
    );
    // Naming the coordinates, or passing the alpha, says which is meant.
    assert_eq!(
        draw("draw_circle_outline(1, 2, 3, C, 255, width: 8)"),
        draw("draw_circle_outline(1, 2, 3, C, 255, 8)"),
    );
}

/// Same-count shapes, all positional: the type of the telling argument still
/// picks the shape, whatever the shared declaration's parameters are called.
#[test]
fn positional_calls_still_dispatch_on_type_where_two_shapes_share_a_count() {
    same(&[
        (
            "draw_rect_outline(1, 2, 3, 4, 5, 6, 7)",
            "draw_rect_outline(1, 2, 3, 4, {r: 5, g: 6, b: 7})",
        ),
        (
            "draw_line(1, 2, 3, 4, 5, 6, 7)",
            "draw_line(1, 2, 3, 4, {r: 5, g: 6, b: 7})",
        ),
        (
            "draw_line(1, 2, 3, 4, C)",
            "draw_line({x: 1, y: 2}, {x: 3, y: 4}, C)",
        ),
        ("draw_circle(1, 2, 3, C)", "draw_circle({x: 1, y: 2}, 3, C)"),
        (
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, 6, 7, 8)",
            "draw_rect_rounded_outline(1, 2, 3, 4, 5, {r: 6, g: 7, b: 8})",
        ),
        (
            "draw_circle_outline(1, 2, 3, 4, 5, 6)",
            "draw_circle_outline(1, 2, 3, {r: 4, g: 5, b: 6})",
        ),
        (
            "draw_circle_outline(1, 2, 3, C)",
            "draw_circle_outline({x: 1, y: 2}, 3, C)",
        ),
        (
            "draw_circle_outline(1, 2, 3, C, 7)",
            "draw_circle_outline({x: 1, y: 2}, 3, C, 7)",
        ),
        (
            "draw_ellipse(1, 2, 3, 4, C)",
            "draw_ellipse({x: 1, y: 2}, 3, 4, C)",
        ),
        (
            "draw_ellipse_outline(1, 2, 3, 4, 5, 6, 7)",
            "draw_ellipse_outline(1, 2, 3, 4, {r: 5, g: 6, b: 7})",
        ),
        (
            "draw_ellipse_outline(1, 2, 3, 4, C)",
            "draw_ellipse_outline({x: 1, y: 2}, 3, 4, C)",
        ),
        (
            "draw_ellipse_outline(1, 2, 3, 4, C, 8)",
            "draw_ellipse_outline({x: 1, y: 2}, 3, 4, C, 8)",
        ),
        (
            "fill_arc(1, 2, 3, 4, 0.5, 1.5, C)",
            "fill_arc({x: 1, y: 2}, 3, 4, 0.5, 1.5, C)",
        ),
        ("fill_fan(1, 2, PTS, C)", "fill_fan({x: 1, y: 2}, PTS, C)"),
        (
            "draw_polyline(PTS, 1, 2, 3)",
            "draw_polyline(PTS, {r: 1, g: 2, b: 3})",
        ),
        (
            "draw_polygon_outline(PTS, 1, 2, 3)",
            "draw_polygon_outline(PTS, {r: 1, g: 2, b: 3})",
        ),
        (
            "draw_text(\"t\", 1, 2, 12, C)",
            "draw_text(\"t\", {x: 1, y: 2}, 12, C)",
        ),
        (
            "draw_text(\"t\", {x: 1, y: 2}, 12, C)",
            "draw_text(\"t\", 1, 2, 12, 11, 22, 33)",
        ),
        ("draw_canvas(cv, 1, 2)", "draw_canvas(cv, {x: 1, y: 2})"),
    ]);
}
