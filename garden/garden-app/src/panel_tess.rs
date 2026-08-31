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
    rect_rounded_shaded(buf, x, y, w, h, radius, &|_, _| color);
}

/// A shape's color as a function of position, in the same absolute logical
/// pixels the geometry is in — what turns a flat fill into a gradient.
///
/// The mesh pipeline interpolates vertex color across a triangle affinely, so
/// a shade that is itself affine in position (a linear gradient) is reproduced
/// *exactly*, not approximated: sampling it at the three corners and letting
/// the rasterizer fill in is the same function. That is the whole reason
/// gradients are per-vertex color rather than a band stack — 32 stacked
/// translucent rects both cost more and blend wrong.
pub type Shade<'a> = &'a dyn Fn(f32, f32) -> Color;

/// [`triangle`] with a per-vertex color.
fn triangle_shaded(buf: &mut Vec<Vertex>, a: (f32, f32), b: (f32, f32), c: (f32, f32), shade: Shade) {
    buf.push(Vertex::new(a, shade(a.0, a.1)));
    buf.push(Vertex::new(b, shade(b.0, b.1)));
    buf.push(Vertex::new(c, shade(c.0, c.1)));
}

/// [`rect`] with a per-vertex color.
fn rect_shaded(buf: &mut Vec<Vertex>, x: f32, y: f32, w: f32, h: f32, shade: Shade) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (x1, y1) = (x + w, y + h);
    triangle_shaded(buf, (x, y), (x1, y), (x, y1), shade);
    triangle_shaded(buf, (x1, y), (x1, y1), (x, y1), shade);
}

/// [`rect_rounded`] with a per-vertex color — the one definition of the
/// rounded-rect silhouette, which the flat fill and the gradient both use so
/// they cannot drift apart.
pub fn rect_rounded_shaded(
    buf: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    shade: Shade,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let r = radius.min(w * 0.5).min(h * 0.5);
    if r <= 0.0 {
        rect_shaded(buf, x, y, w, h, shade);
        return;
    }
    // Straight interior: center column (full height) + left/right side strips.
    rect_shaded(buf, x + r, y, w - 2.0 * r, h, shade);
    rect_shaded(buf, x, y + r, r, h - 2.0 * r, shade);
    rect_shaded(buf, x + w - r, y + r, r, h - 2.0 * r, shade);
    // Corner fans, each sweeping a quadrant (screen space, y grows downward).
    let pi = std::f32::consts::PI;
    corner_fan(buf, x + r, y + r, r, pi, shade); // top-left
    corner_fan(buf, x + w - r, y + r, r, 1.5 * pi, shade); // top-right
    corner_fan(buf, x + w - r, y + h - r, r, 0.0, shade); // bottom-right
    corner_fan(buf, x + r, y + h - r, r, 0.5 * pi, shade); // bottom-left
}

/// Append a quarter-circle triangle fan of radius `r` centered at (`cx`, `cy`),
/// sweeping `FRAC_PI_2` from `start` radians — one rounded corner.
fn corner_fan(buf: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, start: f32, shade: Shade) {
    let segments = (circle_segments(r) / 4).max(2);
    let center = (cx, cy);
    let mut prev = (cx + r * start.cos(), cy + r * start.sin());
    for i in 1..=segments {
        let theta = start + std::f32::consts::FRAC_PI_2 * (i as f32) / (segments as f32);
        let p = (cx + r * theta.cos(), cy + r * theta.sin());
        triangle_shaded(buf, center, prev, p, shade);
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
/// [`polygon`] is the concave-correct filler.
pub fn poly(buf: &mut Vec<Vertex>, points: &[(f32, f32)], color: Color) {
    if points.len() < 3 {
        return;
    }
    let a = points[0];
    for w in points[1..].windows(2) {
        triangle(buf, a, w[0], w[1], color);
    }
}

/// Append a filled triangle fan from an explicit center — the star/pie shape
/// where every vertex is visible from `(cx, cy)`. The fan is open: repeat the
/// first point to close the ring.
pub fn fan(buf: &mut Vec<Vertex>, cx: f32, cy: f32, points: &[(f32, f32)], color: Color) {
    for w in points.windows(2) {
        triangle(buf, (cx, cy), w[0], w[1], color);
    }
}

/// Twice the signed area of the polygon (the shoelace sum): positive when the
/// outline winds one way, negative the other. Used both to reject degenerate
/// outlines and to normalize winding before ear clipping.
fn signed_area2(points: &[(f32, f32)]) -> f32 {
    let mut sum = 0.0;
    for i in 0..points.len() {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % points.len()];
        sum += x0 * y1 - x1 * y0;
    }
    sum
}

/// Twice the signed area of one triangle.
fn cross(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Is `p` inside (or on) triangle `a b c`? Used to reject an ear that would
/// swallow another vertex.
fn point_in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let d1 = cross(a, b, p);
    let d2 = cross(b, c, p);
    let d3 = cross(c, a, p);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Append a filled **simple** polygon — concave allowed — triangulated by ear
/// clipping. This is what [`poly`]'s first-vertex fan cannot do: a star, an L,
/// or an arrowhead fills the region its outline encloses instead of spilling
/// across the reflex vertices.
///
/// O(n²) in the vertex count, which is the right trade for the shapes a panel
/// draws (tens of points, not thousands). Self-intersecting outlines have no
/// well-defined fill; the loop terminates on them regardless (it gives up on a
/// pass that clips no ear and fans the remainder), so bad input degrades to
/// [`poly`]'s old behaviour rather than hanging.
pub fn polygon(buf: &mut Vec<Vertex>, points: &[(f32, f32)], color: Color) {
    if points.len() < 3 {
        return;
    }
    let area2 = signed_area2(points);
    if area2.abs() < f32::EPSILON {
        return; // zero-area outline (collinear, or a doubled-back path)
    }
    if points.len() == 3 {
        triangle(buf, points[0], points[1], points[2], color);
        return;
    }
    // Ear clipping assumes one winding; flip the copy rather than the caller's
    // list so a clockwise and a counter-clockwise star fill identically.
    let mut remaining: Vec<(f32, f32)> = points.to_vec();
    if area2 < 0.0 {
        remaining.reverse();
    }

    while remaining.len() > 3 {
        let n = remaining.len();
        let mut clipped = false;
        for i in 0..n {
            let prev = remaining[(i + n - 1) % n];
            let cur = remaining[i];
            let next = remaining[(i + 1) % n];
            // Convex corner? (A reflex corner is never an ear.)
            if cross(prev, cur, next) <= 0.0 {
                continue;
            }
            // …and empty: no other vertex may fall inside the candidate ear.
            let contains_other = (0..n)
                .filter(|&j| j != i && j != (i + n - 1) % n && j != (i + 1) % n)
                .any(|j| point_in_triangle(remaining[j], prev, cur, next));
            if contains_other {
                continue;
            }
            triangle(buf, prev, cur, next, color);
            remaining.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Not a simple polygon (or numerically degenerate). Fan whatever is
            // left so *something* is drawn, and stop.
            poly(buf, &remaining, color);
            return;
        }
    }
    triangle(buf, remaining[0], remaining[1], remaining[2], color);
}

/// Distance between two points.
fn seg_len(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

/// Where two infinite lines — each a point and a direction — cross. `None`
/// when they are parallel, which for two stroke rims means the path went
/// straight through and there is nothing to trim.
fn line_intersection(
    a: (f32, f32),
    da: (f32, f32),
    b: (f32, f32),
    db: (f32, f32),
) -> Option<(f32, f32)> {
    let denom = da.0 * db.1 - da.1 * db.0;
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = ((b.0 - a.0) * db.1 - (b.1 - a.1) * db.0) / denom;
    Some((a.0 + t * da.0, a.1 + t * da.1))
}

/// Append a stroked open path `width` px wide, with round joins and round caps.
///
/// The reason this exists rather than N [`line`] calls is **translucency**.
/// Alpha blending is not idempotent: draw two 50%-alpha shapes over the same
/// pixel and it comes out 75%. A stroke assembled from per-segment lines
/// double-blends its whole join area, and one assembled from a circle per
/// mouse sample (what a paint app does without this) double-blends nearly
/// everything — which is why a translucent brush comes out mottled and dark.
///
/// So this tessellates a stroke whose pieces **do not overlap**: one quad per
/// segment, and at each interior vertex only the pie *wedge* that fills the gap
/// on the outside of the turn (never a whole disc, which would lie on top of
/// both segments) while both quads are trimmed back to their crossing point on
/// the inside, plus a half-disc cap at each end. Every pixel of the stroke is
/// covered exactly once, so the mesh composites as a single flat shape at the
/// requested alpha.
///
/// The one case that still overlaps is a path that **crosses its own stroke** —
/// a loop, or a doubling back within the stroke width. Nothing short of a
/// stencil or coverage pass fixes that, and it is not what a brush stroke does.
pub fn polyline(buf: &mut Vec<Vertex>, points: &[(f32, f32)], width: f32, color: Color) {
    let half = (width * 0.5).max(LINE_HALF_WIDTH);
    match points.len() {
        0 => return,
        // A one-point path is a dot — the single click a paint app must still
        // register as a mark.
        1 => {
            circle(buf, points[0].0, points[0].1, half, color);
            return;
        }
        _ => {}
    }

    // Unit direction of each segment, with coincident points dropped: a
    // zero-length segment has no normal, and leaving it in would put a NaN
    // through every join angle downstream.
    let mut path: Vec<(f32, f32)> = Vec::with_capacity(points.len());
    let mut dirs: Vec<(f32, f32)> = Vec::with_capacity(points.len());
    path.push(points[0]);
    for &p in &points[1..] {
        let last = *path.last().unwrap();
        let (dx, dy) = (p.0 - last.0, p.1 - last.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            continue;
        }
        dirs.push((dx / len, dy / len));
        path.push(p);
    }
    if dirs.is_empty() {
        circle(buf, points[0].0, points[0].1, half, color);
        return;
    }

    // Each segment's offset rim points, before the joins adjust them: `[start,
    // end]` on the +normal side and on the -normal side.
    let normals: Vec<(f32, f32)> = dirs
        .iter()
        .map(|(dx, dy)| (-dy * half, dx * half))
        .collect();
    let mut plus: Vec<[(f32, f32); 2]> = Vec::with_capacity(dirs.len());
    let mut minus: Vec<[(f32, f32); 2]> = Vec::with_capacity(dirs.len());
    for i in 0..dirs.len() {
        let (a, b) = (path[i], path[i + 1]);
        let (nx, ny) = normals[i];
        plus.push([(a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny)]);
        minus.push([(a.0 - nx, a.1 - ny), (b.0 - nx, b.1 - ny)]);
    }

    // On the **inside** of a turn the two segment quads would overlap — and
    // overlap is exactly what a translucent stroke must not have. So both are
    // trimmed back to where their inner rims cross: one shared corner, no
    // double coverage. On a turn too sharp (or segments too short) for that
    // crossing to lie on both segments, they meet at the vertex itself, which
    // leaves a sliver unpainted rather than painting it twice.
    for i in 1..path.len() - 1 {
        let (d0, d1) = (dirs[i - 1], dirs[i]);
        let turn = d0.0 * d1.1 - d0.1 * d1.0;
        if turn.abs() < 1e-6 {
            continue; // straight through: the rims already line up
        }
        let inner_is_plus = turn > 0.0;
        let (r0, r1) = if inner_is_plus {
            (plus[i - 1][1], plus[i][0])
        } else {
            (minus[i - 1][1], minus[i][0])
        };
        let reach = seg_len(path[i - 1], path[i]).min(seg_len(path[i], path[i + 1]));
        let corner = match line_intersection(r0, d0, r1, d1) {
            Some(x) if seg_len(x, path[i]) <= reach => x,
            _ => path[i],
        };
        if inner_is_plus {
            plus[i - 1][1] = corner;
            plus[i][0] = corner;
        } else {
            minus[i - 1][1] = corner;
            minus[i][0] = corner;
        }
    }

    for i in 0..dirs.len() {
        triangle(buf, plus[i][0], minus[i][0], plus[i][1], color);
        triangle(buf, minus[i][0], minus[i][1], plus[i][1], color);
    }

    // Below ~1px a join wedge and a cap are sub-pixel: all they buy is
    // vertices, and a paint stroke is exactly where that cost would show.
    if half <= LINE_HALF_WIDTH {
        return;
    }

    // Round joins: at each interior vertex, the wedge between the two segments'
    // outer edges — the gap the turn opens up, and nothing more. (Never a full
    // disc: that would lie on top of both segments.)
    for i in 1..path.len() - 1 {
        let (inx, iny) = dirs[i - 1];
        let (onx, ony) = dirs[i];
        // Which side is the outside of the turn? The cross product's sign.
        let turn = inx * ony - iny * onx;
        let side = if turn > 0.0 { -1.0 } else { 1.0 };
        let start = (side * inx).atan2(-side * iny);
        let end = (side * onx).atan2(-side * ony);
        // Sweep the short way: the exterior angle is always under half a turn.
        let mut delta = end - start;
        let tau = std::f32::consts::TAU;
        while delta > std::f32::consts::PI {
            delta -= tau;
        }
        while delta < -std::f32::consts::PI {
            delta += tau;
        }
        arc(
            buf,
            path[i].0,
            path[i].1,
            0.0,
            half,
            start,
            start + delta,
            color,
        );
    }

    // Round caps: the half-disc behind the first point and beyond the last.
    let (fx, fy) = dirs[0];
    let first = path[0];
    let start = (fx).atan2(-fy);
    arc(
        buf,
        first.0,
        first.1,
        0.0,
        half,
        start,
        start + std::f32::consts::PI,
        color,
    );
    let (lx, ly) = dirs[dirs.len() - 1];
    let last = path[path.len() - 1];
    let start = (lx).atan2(-ly);
    arc(
        buf,
        last.0,
        last.1,
        0.0,
        half,
        start,
        start - std::f32::consts::PI,
        color,
    );
}

/// Append a filled axis-aligned ellipse as a triangle fan around its center.
/// A circle is the `rx == ry` case; the segment count follows the larger
/// semi-axis so a wide flat ellipse is not under-tessellated.
pub fn ellipse(buf: &mut Vec<Vertex>, cx: f32, cy: f32, rx: f32, ry: f32, color: Color) {
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let segments = circle_segments(rx.max(ry));
    let center = (cx, cy);
    let mut prev = (cx + rx, cy);
    for i in 1..=segments {
        let theta = std::f32::consts::TAU * (i as f32) / (segments as f32);
        let p = (cx + rx * theta.cos(), cy + ry * theta.sin());
        triangle(buf, center, prev, p, color);
        prev = p;
    }
}

/// Append a hollow axis-aligned ellipse: a `t`-px-thick ring stroked *inside*
/// the rx/ry boundary, as a quad strip between the outer and inner rims. A
/// thickness at or past the semi-axis fills solid (the inner rim collapses),
/// which is what a caller asking for a 20px stroke on a 10px circle means.
pub fn ellipse_outline(
    buf: &mut Vec<Vertex>,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    t: f32,
    color: Color,
) {
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let t = t.max(1.0);
    let (ix, iy) = ((rx - t).max(0.0), (ry - t).max(0.0));
    if ix <= 0.0 || iy <= 0.0 {
        ellipse(buf, cx, cy, rx, ry, color);
        return;
    }
    let segments = circle_segments(rx.max(ry));
    let at = |theta: f32, ax: f32, ay: f32| (cx + ax * theta.cos(), cy + ay * theta.sin());
    for i in 0..segments {
        let t0 = std::f32::consts::TAU * (i as f32) / (segments as f32);
        let t1 = std::f32::consts::TAU * ((i + 1) as f32) / (segments as f32);
        let (o0, o1) = (at(t0, rx, ry), at(t1, rx, ry));
        let (i0, i1) = (at(t0, ix, iy), at(t1, ix, iy));
        triangle(buf, o0, o1, i0, color);
        triangle(buf, i0, o1, i1, color);
    }
}

/// Append a filled annular sector — the donut/pie wedge. Spans radii
/// `r_in`..`r_out` between angles `a0`..`a1` (radians, clockwise from +x with
/// screen y down); `r_in = 0` degenerates to a solid pie slice (the fan's inner
/// rim collapses to the center, which the quad split handles without a special
/// case). A sweep of a full turn or more is clamped to exactly one ring, so a
/// chart that accumulates fractions past 1.0 doesn't overdraw itself.
pub fn arc(
    buf: &mut Vec<Vertex>,
    cx: f32,
    cy: f32,
    r_in: f32,
    r_out: f32,
    a0: f32,
    a1: f32,
    color: Color,
) {
    let r_in = r_in.max(0.0);
    let r_out = r_out.max(0.0);
    if r_out <= 0.0 || r_out <= r_in {
        return;
    }
    let sweep = (a1 - a0).clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
    if sweep.abs() < 1e-6 {
        return;
    }
    // Segment count from the outer rim's arc length, so a thin 5° wedge costs
    // two triangles and a full ring costs a full circle's worth.
    let full = circle_segments(r_out) as f32;
    let segments =
        ((full * sweep.abs() / std::f32::consts::TAU).ceil() as usize).clamp(1, full as usize);
    let at = |theta: f32, r: f32| (cx + r * theta.cos(), cy + r * theta.sin());
    for i in 0..segments {
        let t0 = a0 + sweep * (i as f32) / (segments as f32);
        let t1 = a0 + sweep * ((i + 1) as f32) / (segments as f32);
        let (o0, o1) = (at(t0, r_out), at(t1, r_out));
        let (i0, i1) = (at(t0, r_in), at(t1, r_in));
        triangle(buf, i0, o0, o1, color);
        // With no hole the inner rim is the single center point, so the second
        // triangle of the quad would be degenerate — a pie slice is a fan.
        if r_in > 0.0 {
            triangle(buf, i0, o1, i1, color);
        }
    }
}

/// Append a hollow rounded rectangle: a `t`-px-thick frame stroked *inside* the
/// bounds, with `radius`-px rounded corners. Built as the region between an
/// outer and an inner rounded rect, so unlike two stacked rounded fills it is
/// genuinely hollow (whatever is behind it shows through the middle) and it
/// costs one shape instead of two.
///
/// A radius at or below zero falls back to the square [`rect_outline`]; a
/// thickness that swallows the inner rect fills solid.
pub fn rect_rounded_outline(
    buf: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    t: f32,
    color: Color,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let t = t.max(1.0);
    let r = radius.min(w * 0.5).min(h * 0.5);
    if r <= 0.0 {
        rect_outline(buf, x, y, w, h, t, color);
        return;
    }
    let (iw, ih) = (w - 2.0 * t, h - 2.0 * t);
    if iw <= 0.0 || ih <= 0.0 {
        rect_rounded(buf, x, y, w, h, r, color);
        return;
    }
    // The four straight bands, each spanning the flat run between two corners.
    rect(buf, x + r, y, w - 2.0 * r, t, color); // top
    rect(buf, x + r, y + h - t, w - 2.0 * r, t, color); // bottom
    rect(buf, x, y + r, t, h - 2.0 * r, color); // left
    rect(buf, x + w - t, y + r, t, h - 2.0 * r, color); // right
                                                        // …and a quarter-ring at each corner, between the outer radius `r` and the
                                                        // inner radius `r - t` (clamped: a stroke thicker than the radius makes the
                                                        // corner solid, which arc() gets right with an inner radius of 0).
    let inner = (r - t).max(0.0);
    let pi = std::f32::consts::PI;
    let quarter = std::f32::consts::FRAC_PI_2;
    let corners = [
        (x + r, y + r, pi),           // top-left
        (x + w - r, y + r, 1.5 * pi), // top-right
        (x + w - r, y + h - r, 0.0),  // bottom-right
        (x + r, y + h - r, 0.5 * pi), // bottom-left
    ];
    for (ccx, ccy, start) in corners {
        arc(buf, ccx, ccy, inner, r, start, start + quarter, color);
    }
}

/// Linear interpolation between two colors, component-wise including alpha.
///
/// In sRGB space, deliberately: the renderer composites gamma-encoded values
/// (see [`garden_render::Color`]), and a gradient that interpolated in linear
/// light would not match the ramp CSS, Figma or Core Graphics draw between the
/// same two hex codes.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let l = |x: f32, y: f32| x + (y - x) * t;
    Color::rgba(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b), l(a.a, b.a))
}

/// Append a rectangle — optionally `radius`-rounded — filled with a two-stop
/// linear gradient from `c0` to `c1` along `angle` (radians, clockwise from
/// +x with screen y down, the convention the whole draw protocol uses).
///
/// The gradient axis runs through the rect's center and is scaled so `c0`
/// lands exactly on the first corner the axis reaches and `c1` on the last —
/// CSS's `linear-gradient` geometry. Because the color is affine in position
/// and the rasterizer interpolates affinely, the ramp is exact everywhere, not
/// banded: this is one rounded rect's worth of triangles, not a stack of them.
///
/// Multi-stop gradients arrive from the prelude already subdivided into
/// adjacent bands, each one a call to this, so two stops is the whole
/// primitive.
pub fn rect_gradient(
    buf: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    c0: Color,
    c1: Color,
    angle: f32,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (dx, dy) = (angle.cos(), angle.sin());
    let (cx, cy) = (x + w * 0.5, y + h * 0.5);
    // Length of the rect's projection onto the gradient axis: the axis spans
    // the whole shape, so t is 0 at one extreme corner and 1 at the other.
    let len = (w * dx).abs() + (h * dy).abs();
    if len <= f32::EPSILON {
        rect_rounded_shaded(buf, x, y, w, h, radius, &|_, _| c0);
        return;
    }
    rect_rounded_shaded(buf, x, y, w, h, radius, &|px, py| {
        mix(c0, c1, 0.5 + ((px - cx) * dx + (py - cy) * dy) / len)
    });
}

/// Append a disc filled with a radial gradient: `c0` at the center fading to
/// `c1` at `radius`. The fan's hub carries `c0` and every rim vertex `c1`, so
/// the ramp is one non-overlapping layer — a translucent glow composites once
/// rather than accumulating over itself the way stacked discs would.
pub fn circle_gradient(buf: &mut Vec<Vertex>, cx: f32, cy: f32, radius: f32, c0: Color, c1: Color) {
    if radius <= 0.0 {
        return;
    }
    let segments = circle_segments(radius);
    let center = (cx, cy);
    let mut prev = (cx + radius, cy);
    for i in 1..=segments {
        let theta = std::f32::consts::TAU * (i as f32) / (segments as f32);
        let p = (cx + radius * theta.cos(), cy + radius * theta.sin());
        buf.push(Vertex::new(center, c0));
        buf.push(Vertex::new(prev, c1));
        buf.push(Vertex::new(p, c1));
        prev = p;
    }
}

/// How many concentric rings the shadow's falloff is sampled at. The alpha
/// ramp is smooth (below), so it has to be *sampled*: within one ring the
/// rasterizer interpolates linearly, and 12 chords are enough that the
/// piecewise-linear stand-in is indistinguishable from the curve at the blur
/// radii a UI actually uses.
const SHADOW_RINGS: usize = 12;

/// Append a soft drop shadow as **one** non-overlapping mesh with per-vertex
/// alpha: a solid core (the rect offset by `dx`/`dy` and grown by `spread`,
/// with its corner radius grown to match) surrounded by a ring that fades to
/// fully transparent `blur` px further out.
///
/// Non-overlap is what makes this composite correctly. A shadow is translucent
/// by construction, so the naive implementation — N nested translucent rects —
/// blends each one over the last and the middle of the falloff comes out far
/// darker than either end, while the whole thing costs N draws' worth of
/// overdraw. Here every ring is the region *between* two expansions of the
/// same silhouette, sampled at corresponding points, so the rings tile the
/// falloff exactly: each pixel is covered once, and one alpha-blended pass
/// puts the shadow down.
///
/// The falloff is a smoothstep rather than a straight line. A real box-shadow
/// is a Gaussian blur of the silhouette, whose profile has zero slope at both
/// ends; a linear ramp has a visible crease where it meets the solid core and
/// another where it reaches zero. `3u^2 - 2u^3` has neither and costs nothing,
/// since the ring alphas are computed once on the CPU.
pub fn shadow(
    buf: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    blur: f32,
    spread: f32,
    dx: f32,
    dy: f32,
    color: Color,
) {
    // The core: the source rect displaced and grown. A spread that eats the
    // rect entirely leaves nothing to cast.
    let (cw, ch) = (w + 2.0 * spread, h + 2.0 * spread);
    if cw <= 0.0 || ch <= 0.0 {
        return;
    }
    let (cx0, cy0) = (x + dx - spread, y + dy - spread);
    // Corner radius grows with the spread, the way CSS's does, and is clamped
    // so it never exceeds half the (grown) box.
    let r = (radius + spread).max(0.0).min(cw * 0.5).min(ch * 0.5);
    // The four corner centers stay fixed as the silhouette is expanded, which
    // is what makes ring i and ring i+1 correspond point for point.
    let centers = [
        (cx0 + r, cy0 + r),                // top-left
        (cx0 + cw - r, cy0 + r),           // top-right
        (cx0 + cw - r, cy0 + ch - r),      // bottom-right
        (cx0 + r, cy0 + ch - r),           // bottom-left
    ];
    let blur = blur.max(0.0);
    // Segments per corner, sized from the outermost silhouette so the widest
    // ring is smooth; every ring uses the same count, since they must pair up.
    let seg = (circle_segments(r + blur) / 4).max(2);

    // The solid core, as a fan from the box's center out to the silhouette.
    let hub = (cx0 + cw * 0.5, cy0 + ch * 0.5);
    let mut inner = silhouette(&centers, r, seg);
    for i in 0..inner.len() {
        let (a, b) = (inner[i], inner[(i + 1) % inner.len()]);
        buf.push(Vertex::new(hub, color));
        buf.push(Vertex::new(a, color));
        buf.push(Vertex::new(b, color));
    }
    if blur <= 0.0 {
        return;
    }

    // …then the falloff, ring by ring.
    let mut inner_alpha = 1.0;
    for i in 1..=SHADOW_RINGS {
        let u = i as f32 / SHADOW_RINGS as f32;
        let outer = silhouette(&centers, r + u * blur, seg);
        let outer_alpha = 1.0 - u * u * (3.0 - 2.0 * u);
        let ci = Color::rgba(color.r, color.g, color.b, color.a * inner_alpha);
        let co = Color::rgba(color.r, color.g, color.b, color.a * outer_alpha);
        for j in 0..inner.len() {
            let k = (j + 1) % inner.len();
            buf.push(Vertex::new(inner[j], ci));
            buf.push(Vertex::new(outer[j], co));
            buf.push(Vertex::new(outer[k], co));
            buf.push(Vertex::new(inner[j], ci));
            buf.push(Vertex::new(outer[k], co));
            buf.push(Vertex::new(inner[k], ci));
        }
        inner = outer;
        inner_alpha = outer_alpha;
    }
}

/// The outline of a rounded rect at corner radius `r`, walked clockwise from
/// the top-left corner, with `seg` samples per corner.
///
/// `centers` are the four corner centers in clockwise order starting top-left;
/// they do **not** move with `r`, so two calls differing only in `r` return
/// point lists that correspond one-to-one — the property [`shadow`] leans on
/// to tile its falloff without a gap or an overlap.
fn silhouette(centers: &[(f32, f32); 4], r: f32, seg: usize) -> Vec<(f32, f32)> {
    let quarter = std::f32::consts::FRAC_PI_2;
    let mut out = Vec::with_capacity(4 * seg);
    for (corner, &(ccx, ccy)) in centers.iter().enumerate() {
        // Screen space, y down: the top-left corner's arc runs 180°→270°.
        let start = std::f32::consts::PI + quarter * corner as f32;
        for i in 0..seg {
            let theta = start + quarter * (i as f32) / ((seg - 1).max(1) as f32);
            out.push((ccx + r * theta.cos(), ccy + r * theta.sin()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gradient's stops land on the extreme corners of the shape, so a
    /// caller's `c0` and `c1` are actually reachable rather than being the
    /// midpoints of the first and last band.
    #[test]
    fn a_linear_gradient_reaches_both_stops() {
        let (a, b) = (Color::rgb(0.0, 0.0, 0.0), Color::rgb(1.0, 1.0, 1.0));
        let mut buf = Vec::new();
        rect_gradient(&mut buf, 0.0, 0.0, 100.0, 40.0, 0.0, a, b, 0.0);
        let left = buf.iter().find(|v| v.pos.0 == 0.0).expect("a left edge vertex");
        let right = buf.iter().find(|v| v.pos.0 == 100.0).expect("a right edge vertex");
        assert_eq!(left.color, a);
        assert_eq!(right.color, b);
    }

    /// …and the angle actually turns it: at 90° (screen y down) the ramp runs
    /// top-to-bottom, so the two top corners agree with each other and differ
    /// from the bottom ones.
    #[test]
    fn a_linear_gradient_follows_its_angle() {
        let (a, b) = (Color::rgb(0.0, 0.0, 0.0), Color::rgb(1.0, 1.0, 1.0));
        let mut buf = Vec::new();
        rect_gradient(
            &mut buf,
            0.0,
            0.0,
            100.0,
            40.0,
            0.0,
            a,
            b,
            std::f32::consts::FRAC_PI_2,
        );
        let at = |x: f32, y: f32| {
            buf.iter()
                .find(|v| v.pos == (x, y))
                .unwrap_or_else(|| panic!("no vertex at {x},{y}"))
                .color
                .r
        };
        // `cos(FRAC_PI_2)` is not exactly zero, so the endpoints land a
        // rounding error short of the stops rather than on them.
        assert!(at(0.0, 0.0) < 1e-5);
        assert!(at(100.0, 0.0) < 1e-5);
        assert!(at(0.0, 40.0) > 1.0 - 1e-5);
    }

    /// The radial gradient is a fan, so its hub is the only vertex carrying
    /// `c0` and every rim vertex carries `c1` — one non-overlapping layer, not
    /// a stack of translucent discs.
    #[test]
    fn a_radial_gradient_runs_hub_to_rim() {
        let (a, b) = (Color::rgba(1.0, 1.0, 1.0, 1.0), Color::rgba(1.0, 1.0, 1.0, 0.0));
        let mut buf = Vec::new();
        circle_gradient(&mut buf, 50.0, 50.0, 20.0, a, b);
        assert!(buf.len() >= 3 && buf.len() % 3 == 0);
        for tri in buf.chunks_exact(3) {
            assert_eq!(tri[0].pos, (50.0, 50.0), "the hub leads every triangle");
            assert_eq!(tri[0].color, a);
            assert_eq!(tri[1].color, b);
            assert_eq!(tri[2].color, b);
        }
    }

    /// The shadow's falloff must reach fully transparent, or the mesh's outer
    /// boundary is a visible hard edge — the exact artifact the ring stack
    /// exists to avoid.
    #[test]
    fn the_shadow_fades_to_nothing_at_the_blur_radius() {
        let mut buf = Vec::new();
        let c = Color::rgba(0.0, 0.0, 0.0, 0.5);
        shadow(&mut buf, 20.0, 20.0, 80.0, 40.0, 8.0, 16.0, 0.0, 0.0, 0.0, c);
        let alphas: Vec<f32> = buf.iter().map(|v| v.color.a).collect();
        let max = alphas.iter().cloned().fold(0.0f32, f32::max);
        let min = alphas.iter().cloned().fold(1.0f32, f32::min);
        assert!((max - 0.5).abs() < 1e-6, "the core keeps the full alpha");
        assert!(min.abs() < 1e-6, "the outer rim reaches zero");
    }

    /// The falloff is a smoothstep, which is what makes it read as a blur
    /// rather than a cone: the ramp's midpoint sits at half alpha and its two
    /// ends flatten out, so neither the join with the solid core nor the outer
    /// rim shows a crease.
    #[test]
    fn the_shadow_falloff_is_smooth_at_both_ends() {
        let mut buf = Vec::new();
        let c = Color::rgba(0.0, 0.0, 0.0, 1.0);
        shadow(&mut buf, 0.0, 0.0, 60.0, 60.0, 0.0, 20.0, 0.0, 0.0, 0.0, c);
        // The distinct ring alphas, brightest first.
        let mut levels: Vec<f32> = buf.iter().map(|v| v.color.a).collect();
        levels.sort_by(|a, b| b.partial_cmp(a).expect("no NaN alphas"));
        levels.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        assert_eq!(levels.len(), SHADOW_RINGS + 1, "one alpha per ring boundary");
        // Halfway out, a smoothstep is at exactly 0.5; a straight line would
        // be too, so what distinguishes them is the ends — the first step down
        // from 1.0 must be smaller than the step across the middle.
        let mid = levels[SHADOW_RINGS / 2];
        assert!((mid - 0.5).abs() < 1e-6, "midpoint is half alpha, got {mid}");
        let first_step = levels[0] - levels[1];
        let middle_step = levels[SHADOW_RINGS / 2] - levels[SHADOW_RINGS / 2 + 1];
        assert!(
            first_step < middle_step * 0.5,
            "the ramp should flatten near the core ({first_step} vs {middle_step})"
        );
    }

    /// Ring `i`'s outer boundary is ring `i+1`'s inner boundary, point for
    /// point. That is the whole reason a translucent shadow composites evenly:
    /// overlapping rings would double-blend and darken the middle of the
    /// falloff, gaps would show the background through it.
    #[test]
    fn the_shadow_rings_tile_without_gap_or_overlap() {
        let centers = [(10.0, 10.0), (50.0, 10.0), (50.0, 40.0), (10.0, 40.0)];
        let inner = silhouette(&centers, 6.0, 5);
        let outer = silhouette(&centers, 9.0, 5);
        assert_eq!(inner.len(), outer.len());
        for (a, b) in inner.iter().zip(&outer) {
            // Corresponding points differ by exactly the radius step, along
            // the same ray from the same corner center.
            let d = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            assert!((d - 3.0).abs() < 1e-4, "expected a 3px step, got {d}");
        }
    }

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

    /// Total covered area of a triangle list, counting overlaps once per
    /// triangle — good enough to tell a correct fill from a spilled one.
    fn area(buf: &[Vertex]) -> f32 {
        buf.chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (t[0].pos, t[1].pos, t[2].pos);
                ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs() / 2.0
            })
            .sum()
    }

    /// Is the point covered by any triangle in the list? Zero-area triangles
    /// are skipped: they cover nothing, but every sidedness test reads zero on
    /// them, which would otherwise make them answer "inside" for every point.
    fn covers(buf: &[Vertex], p: (f32, f32)) -> bool {
        buf.chunks_exact(3)
            .filter(|t| {
                let (a, b, c) = (t[0].pos, t[1].pos, t[2].pos);
                ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs() > 1e-6
            })
            .any(|t| {
                let side = |a: (f32, f32), b: (f32, f32)| {
                    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
                };
                let (d1, d2, d3) = (
                    side(t[0].pos, t[1].pos),
                    side(t[1].pos, t[2].pos),
                    side(t[2].pos, t[0].pos),
                );
                !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
            })
    }

    /// A "C" opening to the right — concave, and (unlike an arrowhead, whose
    /// tip can see every other vertex) *not* star-shaped from its first vertex,
    /// which is exactly the case a first-vertex fan gets wrong.
    fn c_shape() -> Vec<(f32, f32)> {
        vec![
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 40.0),
            (40.0, 40.0),
            (40.0, 60.0),
            (100.0, 60.0),
            (100.0, 100.0),
            (0.0, 100.0),
        ]
    }

    /// A point in the C's mouth: outside the shape, inside its convex hull.
    const MOUTH: (f32, f32) = (70.0, 50.0);

    #[test]
    fn concave_polygon_fills_its_own_area_where_the_fan_spills() {
        let pts = c_shape();
        let mut eared = Vec::new();
        polygon(&mut eared, &pts, col());
        // Shoelace area of the C, which is what a correct fill covers.
        let expected = signed_area2(&pts).abs() / 2.0;
        assert!(
            (area(&eared) - expected).abs() < 1.0,
            "ear-clipped area {} != polygon area {expected}",
            area(&eared)
        );

        // The mouth is outside the shape: the concave filler leaves it alone,
        // while the first-vertex fan paints it (the bug this command fixes).
        assert!(
            !covers(&eared, MOUTH),
            "concave fill spilled into the mouth"
        );
        let mut fanned = Vec::new();
        poly(&mut fanned, &pts, col());
        assert!(
            covers(&fanned, MOUTH),
            "the fan is supposed to be the broken one — test premise is wrong"
        );
    }

    #[test]
    fn concave_polygon_fills_the_same_either_winding() {
        let mut cw = Vec::new();
        let mut ccw = Vec::new();
        let pts = c_shape();
        let mut reversed = pts.clone();
        reversed.reverse();
        polygon(&mut cw, &pts, col());
        polygon(&mut ccw, &reversed, col());
        assert!((area(&cw) - area(&ccw)).abs() < 1.0);
    }

    #[test]
    fn a_five_point_star_fills_only_the_star() {
        // The shape the vector editor hand-fanned into 10 triangles.
        let mut pts = Vec::new();
        for i in 0..10 {
            let r = if i % 2 == 0 { 50.0 } else { 20.0 };
            let t = std::f32::consts::TAU * (i as f32) / 10.0 - std::f32::consts::FRAC_PI_2;
            pts.push((100.0 + r * t.cos(), 100.0 + r * t.sin()));
        }
        let mut buf = Vec::new();
        polygon(&mut buf, &pts, col());
        assert_eq!(buf.len(), (pts.len() - 2) * 3, "n-2 triangles for n points");
        assert!(covers(&buf, (100.0, 100.0)), "the middle must be filled");
        // A point between two arms, outside the star's outline, must not be.
        let gap = (
            100.0 + 45.0_f32 * (-std::f32::consts::FRAC_PI_2 + 0.628).cos(),
            100.0 + 45.0_f32 * (-std::f32::consts::FRAC_PI_2 + 0.628).sin(),
        );
        assert!(!covers(&buf, gap), "fill leaked between the arms");
    }

    #[test]
    fn self_intersecting_outlines_terminate() {
        // A pentagram traced as one self-crossing loop has no well-defined
        // fill; the requirement is only that ear clipping gives up and draws
        // *something* rather than looping forever.
        let pts: Vec<(f32, f32)> = (0..5)
            .map(|i| {
                let t = 4.0 * std::f32::consts::PI * (i as f32) / 5.0;
                (50.0 * t.cos(), 50.0 * t.sin())
            })
            .collect();
        let mut buf = Vec::new();
        polygon(&mut buf, &pts, col());
        assert!(!buf.is_empty());

        // A zero-area outline is the one case that draws nothing at all.
        let mut flat = Vec::new();
        polygon(&mut flat, &[(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)], col());
        assert!(flat.is_empty());
    }

    /// How many triangles cover this point? The number that matters for a
    /// translucent stroke: two overlapping pieces blend twice and read darker.
    fn coverage_count(buf: &[Vertex], p: (f32, f32)) -> usize {
        buf.chunks_exact(3)
            .filter(|t| {
                let (a, b, c) = (t[0].pos, t[1].pos, t[2].pos);
                if ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)).abs() <= 1e-6 {
                    return false;
                }
                let side = |u: (f32, f32), v: (f32, f32)| {
                    (v.0 - u.0) * (p.1 - u.1) - (v.1 - u.1) * (p.0 - u.0)
                };
                let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
                // Strict, so a point exactly on a shared edge isn't counted twice.
                (d1 > 0.0 && d2 > 0.0 && d3 > 0.0) || (d1 < 0.0 && d2 < 0.0 && d3 < 0.0)
            })
            .count()
    }

    #[test]
    fn polyline_fills_the_outside_of_a_join_without_a_disc() {
        let mut buf = Vec::new();
        polyline(
            &mut buf,
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            4.0,
            col(),
        );
        // The outside of the right-angle turn is covered — no notch there.
        assert!(covers(&buf, (11.2, -1.2)), "the join wedge is missing");
        // …and so is the body of each segment.
        assert!(covers(&buf, (5.0, 0.0)));
        assert!(covers(&buf, (10.0, 5.0)));
        // The round caps run past each end.
        assert!(covers(&buf, (-1.0, 0.0)), "start cap");
        assert!(covers(&buf, (10.0, 11.0)), "end cap");
        assert!(!covers(&buf, (-3.0, 0.0)), "…but only by the half-width");
    }

    /// The whole point of the primitive: a translucent stroke must not darken
    /// where its pieces meet, so no pixel may be covered twice.
    #[test]
    fn a_polyline_never_covers_a_pixel_twice() {
        let mut buf = Vec::new();
        // A path that turns hard both ways but never doubles back onto its own
        // stroke — the case a stroker can get exactly right.
        let path = [
            (0.0, 0.0),
            (40.0, 0.0),
            (60.0, 30.0),
            (20.0, 50.0),
            (0.0, 44.0),
        ];
        polyline(&mut buf, &path, 9.0, col());
        for gx in -10..80 {
            for gy in -10..65 {
                let p = (gx as f32 + 0.5, gy as f32 + 0.5);
                assert!(
                    coverage_count(&buf, p) <= 1,
                    "point {p:?} is covered {} times — a translucent stroke would blotch there",
                    coverage_count(&buf, p)
                );
            }
        }

        // For contrast, the per-segment `line` stroke this replaces *does*
        // double-cover its joins — which is the bug being fixed, so if this
        // ever stops holding the test above has stopped proving anything.
        let mut lines = Vec::new();
        for w in path.windows(2) {
            line(&mut lines, w[0].0, w[0].1, w[1].0, w[1].1, 9.0, col());
        }
        assert!(
            (-10..80).any(|gx| (-10..65)
                .any(|gy| { coverage_count(&lines, (gx as f32 + 0.5, gy as f32 + 0.5)) > 1 })),
            "per-segment lines were supposed to overlap at the joins"
        );
    }

    #[test]
    fn a_polyline_drops_repeated_points() {
        // A paint app samples the mouse; a stalled pointer emits the same
        // position twice, which must not produce a zero-length segment.
        let mut buf = Vec::new();
        polyline(
            &mut buf,
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (20.0, 0.0)],
            4.0,
            col(),
        );
        for v in &buf {
            assert!(v.pos.0.is_finite() && v.pos.1.is_finite(), "{:?}", v.pos);
        }
        assert!(covers(&buf, (15.0, 0.0)));
        // …and a path of nothing but repeats is still the dot.
        let mut dot = Vec::new();
        polyline(&mut dot, &[(3.0, 3.0), (3.0, 3.0)], 6.0, col());
        assert!(covers(&dot, (3.0, 3.0)));
    }

    #[test]
    fn a_one_point_polyline_is_a_dot() {
        let mut buf = Vec::new();
        polyline(&mut buf, &[(5.0, 5.0)], 6.0, col());
        assert!(!buf.is_empty(), "a single click must still leave a mark");
        assert!(covers(&buf, (5.0, 5.0)));
        assert!(!covers(&buf, (5.0, 20.0)));
    }

    #[test]
    fn a_hairline_polyline_skips_the_joins_and_caps() {
        // At 1px a join wedge is sub-pixel; paying vertices for it is the
        // difference between a paint stroke being cheap and not.
        let mut buf = Vec::new();
        polyline(
            &mut buf,
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            1.0,
            col(),
        );
        assert_eq!(buf.len(), 2 * 2 * 3);
    }

    #[test]
    fn ellipse_respects_both_semi_axes() {
        let mut buf = Vec::new();
        ellipse(&mut buf, 100.0, 50.0, 40.0, 10.0, col());
        for v in &buf {
            let (dx, dy) = ((v.pos.0 - 100.0) / 40.0, (v.pos.1 - 50.0) / 10.0);
            assert!(
                dx * dx + dy * dy <= 1.0 + 1e-3,
                "vertex {:?} outside",
                v.pos
            );
        }
        assert!(covers(&buf, (135.0, 50.0)), "wide axis is filled");
        assert!(!covers(&buf, (100.0, 65.0)), "past the short axis is not");
    }

    #[test]
    fn ellipse_outline_is_hollow_but_a_fat_stroke_fills() {
        let mut ring = Vec::new();
        ellipse_outline(&mut ring, 0.0, 0.0, 30.0, 30.0, 5.0, col());
        assert!(covers(&ring, (28.0, 0.0)), "the rim is drawn");
        assert!(!covers(&ring, (0.0, 0.0)), "the middle stays hollow");

        // A stroke wider than the radius has no inner rim left: fill solid
        // rather than drawing nothing.
        let mut solid = Vec::new();
        ellipse_outline(&mut solid, 0.0, 0.0, 10.0, 10.0, 20.0, col());
        assert!(covers(&solid, (0.0, 0.0)));
    }

    #[test]
    fn circle_outline_is_the_equal_axis_case() {
        let mut buf = Vec::new();
        ellipse_outline(&mut buf, 0.0, 0.0, 20.0, 20.0, 3.0, col());
        for v in &buf {
            let r = (v.pos.0 * v.pos.0 + v.pos.1 * v.pos.1).sqrt();
            assert!(r <= 20.0 + 1e-3 && r >= 17.0 - 1e-3, "vertex {:?}", v.pos);
        }
    }

    #[test]
    fn arc_covers_its_wedge_and_nothing_else() {
        let mut buf = Vec::new();
        // A quarter donut sweeping the +x→+y quadrant (screen y down).
        arc(
            &mut buf,
            0.0,
            0.0,
            20.0,
            40.0,
            0.0,
            std::f32::consts::FRAC_PI_2,
            col(),
        );
        assert!(covers(&buf, (30.0, 1.0)), "inside the sweep, in the band");
        assert!(covers(&buf, (21.0, 21.0)), "mid-sweep, in the band");
        assert!(!covers(&buf, (10.0, 1.0)), "inside the hole");
        assert!(!covers(&buf, (45.0, 1.0)), "past the outer rim");
        assert!(!covers(&buf, (30.0, -10.0)), "outside the sweep");
    }

    #[test]
    fn a_full_turn_arc_is_one_ring_not_two() {
        let mut once = Vec::new();
        let mut twice = Vec::new();
        arc(
            &mut once,
            0.0,
            0.0,
            10.0,
            20.0,
            0.0,
            std::f32::consts::TAU,
            col(),
        );
        // A chart that accumulates fractions past 1.0 must not overdraw.
        arc(
            &mut twice,
            0.0,
            0.0,
            10.0,
            20.0,
            0.0,
            3.0 * std::f32::consts::TAU,
            col(),
        );
        assert_eq!(once.len(), twice.len());
    }

    #[test]
    fn a_zero_sweep_arc_draws_nothing() {
        let mut buf = Vec::new();
        arc(&mut buf, 0.0, 0.0, 10.0, 20.0, 1.0, 1.0, col());
        assert!(buf.is_empty());
        // …and so does an inside-out radius pair.
        arc(&mut buf, 0.0, 0.0, 20.0, 10.0, 0.0, 1.0, col());
        assert!(buf.is_empty());
    }

    #[test]
    fn a_thin_wedge_costs_far_less_than_a_ring() {
        let mut wedge = Vec::new();
        let mut ring = Vec::new();
        arc(&mut wedge, 0.0, 0.0, 30.0, 40.0, 0.0, 0.1, col());
        arc(
            &mut ring,
            0.0,
            0.0,
            30.0,
            40.0,
            0.0,
            std::f32::consts::TAU,
            col(),
        );
        assert!(
            wedge.len() * 4 < ring.len(),
            "segment count must follow the sweep: {} vs {}",
            wedge.len(),
            ring.len()
        );
    }

    #[test]
    fn rounded_outline_is_hollow_and_stays_in_bounds() {
        let mut buf = Vec::new();
        rect_rounded_outline(&mut buf, 10.0, 20.0, 100.0, 60.0, 8.0, 2.0, col());
        for v in &buf {
            assert!(
                v.pos.0 >= 10.0 - 1e-3 && v.pos.0 <= 110.0 + 1e-3,
                "{:?}",
                v.pos
            );
            assert!(
                v.pos.1 >= 20.0 - 1e-3 && v.pos.1 <= 80.0 + 1e-3,
                "{:?}",
                v.pos
            );
        }
        assert!(covers(&buf, (60.0, 21.0)), "top band");
        assert!(covers(&buf, (11.0, 50.0)), "left band");
        assert!(!covers(&buf, (60.0, 50.0)), "the middle is hollow");
        // The squared-off corner is *not* painted — that is the difference
        // from `rect_outline`.
        assert!(!covers(&buf, (10.5, 20.5)), "corner is rounded away");
    }

    #[test]
    fn rounded_outline_at_radius_one_still_leaves_a_hole() {
        // The reported bug: at radius 1 the two-stacked-fills workaround
        // degenerates. A real stroke must stay hollow at any radius.
        let mut buf = Vec::new();
        rect_rounded_outline(&mut buf, 0.0, 0.0, 40.0, 40.0, 1.0, 1.0, col());
        assert!(covers(&buf, (20.0, 0.5)), "the border is drawn");
        assert!(!covers(&buf, (20.0, 20.0)), "the middle is hollow");
    }

    #[test]
    fn rounded_outline_degenerates_predictably() {
        // Zero radius is the square outline, exactly.
        let mut round = Vec::new();
        let mut square = Vec::new();
        rect_rounded_outline(&mut round, 0.0, 0.0, 20.0, 20.0, 0.0, 2.0, col());
        rect_outline(&mut square, 0.0, 0.0, 20.0, 20.0, 2.0, col());
        assert_eq!(round, square);
        // A stroke thicker than the box fills it solid rather than vanishing.
        let mut fat = Vec::new();
        rect_rounded_outline(&mut fat, 0.0, 0.0, 20.0, 20.0, 4.0, 40.0, col());
        assert!(covers(&fat, (10.0, 10.0)));
    }

    #[test]
    fn fan_spokes_from_the_center() {
        let mut buf = Vec::new();
        fan(
            &mut buf,
            0.0,
            0.0,
            &[(10.0, -10.0), (14.0, 0.0), (10.0, 10.0)],
            col(),
        );
        assert_eq!(buf.len(), 2 * 3, "n-1 triangles for n rim points");
        assert!(covers(&buf, (8.0, 0.0)));
        assert!(!covers(&buf, (-8.0, 0.0)), "the fan is open, not a disc");
    }
}
