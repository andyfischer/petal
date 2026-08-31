//! Shared CPU tessellation for the draw commands a host can't rasterize with
//! a single fill.
//!
//! Only [`DrawCommand::Shadow`](crate::draw::DrawCommand::Shadow) needs it so
//! far. A soft shadow is *translucent*, and the obvious constructions for one
//! are both wrong here:
//!
//! - A real blur pass costs a render target and two passes, which several
//!   petal-ui hosts (the panel renderer, the SDL software path) don't have.
//! - A stack of concentric translucent rounded rects double-composites every
//!   ring over the ones inside it, so the falloff is wrong and every seam
//!   shows.
//!
//! So the shadow is tessellated the way `draw_polyline` already handles its
//! joins: as **one non-overlapping mesh**, drawn in a single composite. A
//! solid core covers the (offset, spread) shape at full alpha, and a ring of
//! quads around it carries a per-vertex alpha falling from 1 at the shape
//! boundary to 0 at `blur` px outside. Because the two rings are the *same*
//! rounded rect sampled at radius `r` and `r + blur`, their vertices
//! correspond one-to-one: the quads tile the ring exactly, with no gap to leak
//! background and no overlap to darken.
//!
//! The output is deliberately renderer-agnostic — positions in logical pixels
//! plus an alpha *multiplier* — so each host folds in its own color type and
//! vertex layout.

/// One tessellated vertex: a position in logical pixels and the alpha
/// multiplier to apply to the shadow color there (1 = full, 0 = invisible).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowVertex {
    pub x: f32,
    pub y: f32,
    pub alpha: f32,
}

/// Arc samples per corner of the shadow outline. 8 is enough that the falloff
/// ring reads as a curve at the corner radii UI actually uses, and keeps the
/// mesh at a few hundred vertices.
pub const CORNER_SEGMENTS: usize = 8;

/// The four corner arc sweeps of a rounded rect, in the screen convention
/// (clockwise from +x, y down): each is `(center_selector, start_angle)` for a
/// quarter turn. Order runs top-left → top-right → bottom-right → bottom-left
/// so the ring comes out as one closed loop.
const CORNERS: [(f32, f32, f32); 4] = [
    // (unit x offset of the corner center, unit y offset, start angle)
    (0.0, 0.0, std::f32::consts::PI),
    (1.0, 0.0, -std::f32::consts::FRAC_PI_2),
    (1.0, 1.0, 0.0),
    (0.0, 1.0, std::f32::consts::FRAC_PI_2),
];

/// Sample the outline of a rounded rect at `radius`, and again at
/// `radius + offset`, about the *same* corner centers — so sample `i` of one
/// ring pairs with sample `i` of the other. Returns `(inner, outer)`.
fn ring_pair(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    offset: f32,
) -> (Vec<(f32, f32)>, Vec<(f32, f32)>) {
    let radius = radius.max(0.0).min(w.min(h) * 0.5);
    let n = CORNER_SEGMENTS;
    let mut inner = Vec::with_capacity(4 * (n + 1));
    let mut outer = Vec::with_capacity(4 * (n + 1));
    for (ux, uy, a0) in CORNERS {
        // The corner's arc center: inset from that corner of the rect by the
        // radius. With radius 0 it degenerates to the corner itself, and the
        // whole quarter-arc collapses to a point — which is exactly right:
        // the outer ring then fans a quarter-disc of `offset` around a square
        // corner, and the zero-area inner triangles cost nothing.
        let cx = x + ux * w + (1.0 - 2.0 * ux) * radius;
        let cy = y + uy * h + (1.0 - 2.0 * uy) * radius;
        for i in 0..=n {
            let a = a0 + std::f32::consts::FRAC_PI_2 * (i as f32 / n as f32);
            let (s, c) = a.sin_cos();
            inner.push((cx + c * radius, cy + s * radius));
            outer.push((cx + c * (radius + offset), cy + s * (radius + offset)));
        }
    }
    (inner, outer)
}

/// Tessellate a [`DrawCommand::Shadow`](crate::draw::DrawCommand::Shadow) into
/// a triangle list: the casting shape is the rounded rect (`x`, `y`, `w`, `h`,
/// `radius`), grown by `spread`, offset by (`dx`, `dy`), with its edge fading
/// out over `blur` px.
///
/// Vertices come back as a flat triangle list (every three make one triangle),
/// in logical pixels, each carrying the alpha multiplier for the shadow color
/// at that point. The mesh never overlaps itself, so a host draws it in one
/// composite with ordinary alpha blending and gets an even falloff.
///
/// An empty result means there is nothing to draw (a shape shrunk away by a
/// negative `spread`).
#[allow(clippy::too_many_arguments)]
pub fn shadow_mesh(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    blur: f32,
    spread: f32,
    dx: f32,
    dy: f32,
) -> Vec<ShadowVertex> {
    let sx = x + dx - spread;
    let sy = y + dy - spread;
    let sw = w + 2.0 * spread;
    let sh = h + 2.0 * spread;
    if sw <= 0.0 || sh <= 0.0 {
        return Vec::new();
    }
    // The spread grows the corners with the shape, the way CSS does.
    let radius = (radius + spread).max(0.0);
    let blur = blur.max(0.0);

    let (inner, outer) = ring_pair(sx, sy, sw, sh, radius, blur);
    let mut out = Vec::with_capacity(inner.len() * 9);

    // Solid core: a fan from the shape's center over the inner ring. Convex,
    // so the fan covers it exactly once.
    let (mx, my) = (sx + sw * 0.5, sy + sh * 0.5);
    let solid = |p: (f32, f32)| ShadowVertex {
        x: p.0,
        y: p.1,
        alpha: 1.0,
    };
    let faded = |p: (f32, f32)| ShadowVertex {
        x: p.0,
        y: p.1,
        alpha: 0.0,
    };
    for i in 0..inner.len() {
        let j = (i + 1) % inner.len();
        out.push(solid((mx, my)));
        out.push(solid(inner[i]));
        out.push(solid(inner[j]));
    }

    // Falloff ring: one quad per outline segment, alpha 1 on the shape
    // boundary and 0 at `blur` px out. Skipped entirely for a hard shadow.
    if blur > 0.0 {
        for i in 0..inner.len() {
            let j = (i + 1) % inner.len();
            out.push(solid(inner[i]));
            out.push(faded(outer[i]));
            out.push(faded(outer[j]));
            out.push(solid(inner[i]));
            out.push(faded(outer[j]));
            out.push(solid(inner[j]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signed area of the triangle list — a mesh that overlapped itself would
    /// count the overlap twice, so comparing against the analytic area of the
    /// shape plus its ring is a direct check of the non-overlap property.
    fn area(mesh: &[ShadowVertex]) -> f32 {
        mesh.chunks(3)
            .map(|t| {
                ((t[1].x - t[0].x) * (t[2].y - t[0].y) - (t[2].x - t[0].x) * (t[1].y - t[0].y)).abs()
                    * 0.5
            })
            .sum()
    }

    #[test]
    fn mesh_covers_shape_plus_ring_exactly_once() {
        let (w, h, blur) = (100.0, 60.0, 10.0);
        let mesh = shadow_mesh(0.0, 0.0, w, h, 0.0, blur, 0.0, 0.0, 0.0);
        // Square-cornered shape (w·h) grown by `blur` on all sides: the ring
        // adds one band per edge plus four quarter-discs at the corners. The
        // discs are polygonal (n chords each), so the exact expected area is
        // the inscribed-polygon one, a hair under πr².
        let n = CORNER_SEGMENTS as f32;
        let corners = 4.0 * n * 0.5 * blur * blur * (std::f32::consts::FRAC_PI_2 / n).sin();
        let expected = w * h + 2.0 * blur * (w + h) + corners;
        let got = area(&mesh);
        assert!(
            (got - expected).abs() < 0.5,
            "area {got} != {expected} — the mesh has a gap or an overlap"
        );
    }

    #[test]
    fn alpha_is_one_inside_and_zero_at_the_blur_edge() {
        let mesh = shadow_mesh(10.0, 10.0, 40.0, 40.0, 8.0, 12.0, 0.0, 0.0, 0.0);
        assert!(!mesh.is_empty());
        assert!(mesh.iter().all(|v| v.alpha == 0.0 || v.alpha == 1.0));
        // Every faded vertex sits `blur` outside the shape, so none of them
        // can be inside the (10,10)–(50,50) box.
        for v in mesh.iter().filter(|v| v.alpha == 0.0) {
            let inside = v.x > 10.5 && v.x < 49.5 && v.y > 10.5 && v.y < 49.5;
            assert!(!inside, "faded vertex {v:?} landed inside the shape");
        }
    }

    #[test]
    fn offset_and_spread_move_and_grow_the_shape() {
        let base = shadow_mesh(0.0, 0.0, 40.0, 40.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let moved = shadow_mesh(0.0, 0.0, 40.0, 40.0, 0.0, 0.0, 0.0, 5.0, 7.0);
        assert_eq!(base.len(), moved.len());
        for (b, m) in base.iter().zip(&moved) {
            assert_eq!((m.x - b.x, m.y - b.y), (5.0, 7.0));
        }
        // Spread grows the covered area; a spread that eats the shape yields
        // nothing rather than an inside-out mesh.
        assert!(area(&shadow_mesh(0.0, 0.0, 40.0, 40.0, 0.0, 0.0, 4.0, 0.0, 0.0)) > area(&base));
        assert!(shadow_mesh(0.0, 0.0, 40.0, 40.0, 0.0, 0.0, -30.0, 0.0, 0.0).is_empty());
    }

    #[test]
    fn hard_shadow_has_no_falloff_ring() {
        let hard = shadow_mesh(0.0, 0.0, 20.0, 20.0, 4.0, 0.0, 0.0, 0.0, 0.0);
        assert!(hard.iter().all(|v| v.alpha == 1.0));
    }
}
