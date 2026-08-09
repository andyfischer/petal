//! Pure CPU tessellation of panel geometry into triangle-list vertices.
//!
//! A `panel(...)` sketch emits [`PanelCmd`](garden_script::PanelCmd)s; the
//! renderer only knows solid triangles ([`garden_render::Primitive::Mesh`]).
//! This module bridges the two: each helper appends the triangles for one
//! shape — a filled rect, a thick line, a circle fan, a polygon fan — to a
//! shared vertex buffer, in **absolute (already pane-offset) logical pixels**.
//! It knows nothing about panes, panels, or the GPU, so it is unit-tested
//! without a window (the codebase's pure-core ethos: see `window_nav`,
//! `search`, `frontend/grid`).
//!
//! All shapes share the caller's submission order in one buffer, so a later
//! shape paints over an earlier one (painter's algorithm — there is no depth
//! buffer).

use garden_render::{Color, Vertex};

/// Half-width of a `line`, in logical pixels — gives a ~1px stroke.
const LINE_HALF_WIDTH: f32 = 0.5;

/// Append one triangle (three points, one color).
pub fn triangle(buf: &mut Vec<Vertex>, a: (f32, f32), b: (f32, f32), c: (f32, f32), color: Color) {
    buf.push(Vertex::new(a, color));
    buf.push(Vertex::new(b, color));
    buf.push(Vertex::new(c, color));
}

/// Append a filled axis-aligned rectangle (two triangles).
pub fn rect(buf: &mut Vec<Vertex>, x: f32, y: f32, w: f32, h: f32, color: Color) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (x1, y1) = (x + w, y + h);
    triangle(buf, (x, y), (x1, y), (x, y1), color);
    triangle(buf, (x1, y), (x1, y1), (x, y1), color);
}

/// Append a hollow rectangle as four thin filled edges (`t` px thick).
pub fn rect_outline(buf: &mut Vec<Vertex>, x: f32, y: f32, w: f32, h: f32, t: f32, color: Color) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    rect(buf, x, y, w, t, color); // top
    rect(buf, x, y + h - t, w, t, color); // bottom
    rect(buf, x, y, t, h, color); // left
    rect(buf, x + w - t, y, t, h, color); // right
}

/// Append a straight line as a `width`-px-thick quad between two endpoints
/// (`width` is the full stroke width; a hairline is 1px). A zero-length line
/// degenerates to a tiny square so a single click still shows a dot.
pub fn line(buf: &mut Vec<Vertex>, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: Color) {
    let half = (width * 0.5).max(LINE_HALF_WIDTH);
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON {
        let side = (half * 2.0).max(1.0);
        rect(buf, x1 - half, y1 - half, side, side, color);
        return;
    }
    // Unit normal, scaled to the half width.
    let (nx, ny) = (-dy / len * half, dx / len * half);
    let p0 = (x1 + nx, y1 + ny);
    let p1 = (x1 - nx, y1 - ny);
    let p2 = (x2 + nx, y2 + ny);
    let p3 = (x2 - nx, y2 - ny);
    triangle(buf, p0, p1, p2, color);
    triangle(buf, p1, p3, p2, color);
}

/// Append a filled rectangle with `radius`-px rounded corners: three straight
/// bands (a full-height center column flanked by two side strips) plus a
/// quarter-circle fan at each corner. `radius` is clamped to half the shorter
/// side; a zero (or negative) radius falls back to a square [`rect`].
pub fn rect_rounded(
    buf: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: Color,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let r = radius.min(w * 0.5).min(h * 0.5);
    if r <= 0.0 {
        rect(buf, x, y, w, h, color);
        return;
    }
    // Straight interior: center column (full height) + left/right side strips.
    rect(buf, x + r, y, w - 2.0 * r, h, color);
    rect(buf, x, y + r, r, h - 2.0 * r, color);
    rect(buf, x + w - r, y + r, r, h - 2.0 * r, color);
    // Corner fans, each sweeping a quadrant (screen space, y grows downward).
    let pi = std::f32::consts::PI;
    corner_fan(buf, x + r, y + r, r, pi, color); // top-left
    corner_fan(buf, x + w - r, y + r, r, 1.5 * pi, color); // top-right
    corner_fan(buf, x + w - r, y + h - r, r, 0.0, color); // bottom-right
    corner_fan(buf, x + r, y + h - r, r, 0.5 * pi, color); // bottom-left
}

/// Append a quarter-circle triangle fan of radius `r` centered at (`cx`, `cy`),
/// sweeping `FRAC_PI_2` from `start` radians — one rounded corner.
fn corner_fan(buf: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, start: f32, color: Color) {
    let segments = (circle_segments(r) / 4).max(2);
    let center = (cx, cy);
    let mut prev = (cx + r * start.cos(), cy + r * start.sin());
    for i in 1..=segments {
        let theta = start + std::f32::consts::FRAC_PI_2 * (i as f32) / (segments as f32);
        let p = (cx + r * theta.cos(), cy + r * theta.sin());
        triangle(buf, center, prev, p, color);
        prev = p;
    }
}

/// Number of segments to approximate a circle of the given radius — more for
/// bigger circles, capped so a huge radius doesn't explode the vertex count.
fn circle_segments(radius: f32) -> usize {
    ((radius * 0.7) as usize + 8).clamp(8, 64)
}

/// Append a filled circle as a triangle fan around its center.
pub fn circle(buf: &mut Vec<Vertex>, cx: f32, cy: f32, radius: f32, color: Color) {
    if radius <= 0.0 {
        return;
    }
    let segments = circle_segments(radius);
    let center = (cx, cy);
    let mut prev = (cx + radius, cy);
    for i in 1..=segments {
        let theta = std::f32::consts::TAU * (i as f32) / (segments as f32);
        let p = (cx + radius * theta.cos(), cy + radius * theta.sin());
        triangle(buf, center, prev, p, color);
        prev = p;
    }
}

/// Append a filled polygon as a triangle fan from its first vertex. Correct for
/// convex polygons (petal-sdl's `fill_poly` contract); a concave polygon
/// renders its convex hull-ish fan, matching petal-sdl's own simple filler.
pub fn poly(buf: &mut Vec<Vertex>, points: &[(f32, f32)], color: Color) {
    if points.len() < 3 {
        return;
    }
    let a = points[0];
    for w in points[1..].windows(2) {
        triangle(buf, a, w[0], w[1], color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col() -> Color {
        Color::rgb(1.0, 0.0, 0.0)
    }

    #[test]
    fn rect_is_two_triangles_covering_the_corners() {
        let mut buf = Vec::new();
        rect(&mut buf, 10.0, 20.0, 4.0, 6.0, col());
        assert_eq!(buf.len(), 6);
        // All four corners appear among the vertices.
        let pts: Vec<_> = buf.iter().map(|v| v.pos).collect();
        for corner in [(10.0, 20.0), (14.0, 20.0), (10.0, 26.0), (14.0, 26.0)] {
            assert!(pts.contains(&corner), "missing corner {corner:?}");
        }
    }

    #[test]
    fn degenerate_rect_emits_nothing() {
        let mut buf = Vec::new();
        rect(&mut buf, 0.0, 0.0, 0.0, 10.0, col());
        rect(&mut buf, 0.0, 0.0, 10.0, -1.0, col());
        assert!(buf.is_empty());
    }

    #[test]
    fn outline_is_four_edges() {
        let mut buf = Vec::new();
        rect_outline(&mut buf, 0.0, 0.0, 10.0, 10.0, 1.0, col());
        assert_eq!(buf.len(), 4 * 6); // 4 rects × 2 tris × 3 verts
    }

    #[test]
    fn line_quad_straddles_the_axis() {
        let mut buf = Vec::new();
        // Horizontal line: normal is vertical, so y spans ±half-width.
        line(&mut buf, 0.0, 5.0, 10.0, 5.0, 1.0, col());
        assert_eq!(buf.len(), 6);
        let ys: Vec<f32> = buf.iter().map(|v| v.pos.1).collect();
        assert!(ys.iter().any(|&y| (y - 4.5).abs() < 1e-5));
        assert!(ys.iter().any(|&y| (y - 5.5).abs() < 1e-5));
    }

    #[test]
    fn zero_length_line_is_a_dot() {
        let mut buf = Vec::new();
        line(&mut buf, 3.0, 3.0, 3.0, 3.0, 1.0, col());
        assert_eq!(buf.len(), 6); // a 1×1 rect
    }

    #[test]
    fn thick_line_straddles_by_half_width() {
        let mut buf = Vec::new();
        // Width 4 → half-width 2, so a horizontal line spans y ∈ [3, 7].
        line(&mut buf, 0.0, 5.0, 10.0, 5.0, 4.0, col());
        let ys: Vec<f32> = buf.iter().map(|v| v.pos.1).collect();
        assert!(ys.iter().any(|&y| (y - 3.0).abs() < 1e-5));
        assert!(ys.iter().any(|&y| (y - 7.0).abs() < 1e-5));
    }

    #[test]
    fn rounded_rect_stays_within_bounds_and_rounds() {
        let mut buf = Vec::new();
        rect_rounded(&mut buf, 10.0, 20.0, 40.0, 30.0, 6.0, col());
        assert!(!buf.is_empty());
        // Every vertex is inside the rect's bounding box…
        for v in &buf {
            assert!(
                v.pos.0 >= 10.0 - 1e-3 && v.pos.0 <= 50.0 + 1e-3,
                "x {:?}",
                v.pos
            );
            assert!(
                v.pos.1 >= 20.0 - 1e-3 && v.pos.1 <= 50.0 + 1e-3,
                "y {:?}",
                v.pos
            );
        }
        // …and no vertex lands in the clipped corner square (radius 6 from the
        // top-left corner at (10, 20)): rounding removed that triangle.
        let in_tl_corner = buf.iter().any(|v| v.pos.0 < 10.5 && v.pos.1 < 20.5);
        assert!(!in_tl_corner, "a vertex sits in the squared-off corner");
    }

    #[test]
    fn rounded_rect_zero_radius_is_a_plain_rect() {
        let mut round = Vec::new();
        let mut plain = Vec::new();
        rect_rounded(&mut round, 0.0, 0.0, 10.0, 10.0, 0.0, col());
        rect(&mut plain, 0.0, 0.0, 10.0, 10.0, col());
        assert_eq!(round, plain);
    }

    #[test]
    fn circle_fans_into_triangles() {
        let mut buf = Vec::new();
        circle(&mut buf, 0.0, 0.0, 10.0, col());
        let segs = circle_segments(10.0);
        assert_eq!(buf.len(), segs * 3);
        // Every vertex is within the radius (center + on-circle points).
        for v in &buf {
            let r = (v.pos.0 * v.pos.0 + v.pos.1 * v.pos.1).sqrt();
            assert!(r <= 10.0 + 1e-3, "vertex {:?} outside radius", v.pos);
        }
    }

    #[test]
    fn circle_segment_count_grows_and_caps() {
        assert_eq!(circle_segments(0.0), 8);
        assert!(circle_segments(20.0) > 8);
        assert_eq!(circle_segments(1000.0), 64);
    }

    #[test]
    fn poly_fan_triangulates() {
        let mut buf = Vec::new();
        // A quad → 2 triangles.
        poly(
            &mut buf,
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            col(),
        );
        assert_eq!(buf.len(), 6);
    }

    #[test]
    fn poly_needs_three_points() {
        let mut buf = Vec::new();
        poly(&mut buf, &[(0.0, 0.0), (10.0, 0.0)], col());
        assert!(buf.is_empty());
    }
}
