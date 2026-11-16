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
//! solid core covers the (offset, spread) shape at full alpha, and
//! [`SHADOW_RINGS`] concentric bands around it carry a per-vertex alpha that
//! falls from 1 at the shape boundary to 0 at `blur` px outside. Every band is
//! the region between two expansions of the *same* rounded rect about the
//! same corner centers, so their vertices correspond one-to-one: the bands
//! tile the falloff exactly, with no gap to leak background and no overlap to
//! darken.
//!
//! The falloff is [`shadow_falloff`], a smoothstep rather than a straight
//! line. A real box-shadow is a Gaussian blur of the silhouette, whose profile
//! has zero slope at both ends; a linear ramp shows a crease where it meets
//! the core and another where it reaches zero. Hosts that rasterize a shadow
//! per pixel instead (the SDL software renderer) evaluate the same function on
//! the signed distance, so every host draws the same shadow.
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

/// How many concentric bands the falloff is sampled at. Within one band the
/// rasterizer interpolates alpha linearly, and 12 chords are enough that the
/// piecewise-linear stand-in is indistinguishable from the curve at the blur
/// radii a UI uses.
pub const SHADOW_RINGS: usize = 12;

/// The shadow's alpha multiplier at `t` of the way out through the blur band
/// (0 = the shape's edge, 1 = `blur` px outside it): 1 at the edge, 0 at the
/// rim, a smoothstep between. Values outside 0..=1 clamp.
pub fn shadow_falloff(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Fewest samples per corner arc. A square corner (radius 0) still takes
/// them: the inner arc collapses to a point while the outer bands fan a
/// quarter-disc around it.
pub const MIN_CORNER_SEGMENTS: usize = 2;

/// The furthest a corner arc's chord may fall inside the true outermost
/// curve, in px.
const MAX_SAGITTA: f32 = 0.25;

/// Samples per corner arc for a silhouette whose outermost band has radius
/// `outer_radius`: enough that no chord strays more than a quarter pixel
/// inside the curve, capped so a huge blur stays cheap.
pub fn corner_segments(outer_radius: f32) -> usize {
    let ramp = ((outer_radius * 0.7) as usize + 8).clamp(8, 64) / 4;
    let by_error = if outer_radius > MAX_SAGITTA {
        // A chord across angle `a` sits `r · (1 − cos(a / 2))` inside the rim;
        // a quarter turn needs (π/2) / a of them.
        let half_angle = (1.0 - MAX_SAGITTA / outer_radius).acos();
        (std::f32::consts::FRAC_PI_4 / half_angle).ceil() as usize
    } else {
        0
    };
    ramp.max(by_error).clamp(MIN_CORNER_SEGMENTS, 128)
}

/// The outline of a rounded rect with corner radius `r` about the fixed
/// corner `centers` (clockwise from top-left), `n` chords per corner. The
/// centers do **not** move with `r`, so two calls differing only in `r`
/// return point lists that correspond one-to-one.
fn silhouette(centers: &[(f32, f32); 4], r: f32, n: usize) -> Vec<(f32, f32)> {
    let quarter = std::f32::consts::FRAC_PI_2;
    let mut out = Vec::with_capacity(4 * (n + 1));
    for (corner, &(cx, cy)) in centers.iter().enumerate() {
        // Screen space, y down: the top-left corner's arc runs 180°→270°.
        let start = std::f32::consts::PI + quarter * corner as f32;
        for i in 0..=n {
            let a = start + quarter * (i as f32 / n as f32);
            let (s, c) = a.sin_cos();
            out.push((cx + c * r, cy + s * r));
        }
    }
    out
}

/// Tessellate a [`DrawCommand::Shadow`](crate::draw::DrawCommand::Shadow) into
/// a triangle list: the casting shape is the rounded rect (`x`, `y`, `w`, `h`,
/// `radius`), grown by `spread`, offset by (`dx`, `dy`), with its edge fading
/// out over `blur` px along [`shadow_falloff`].
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
    // The spread grows the corners with the shape, the way CSS does, clamped
    // so they never exceed half the (grown) box.
    let r = (radius + spread).max(0.0).min(sw * 0.5).min(sh * 0.5);
    let blur = blur.max(0.0);
    let centers = [
        (sx + r, sy + r),           // top-left
        (sx + sw - r, sy + r),      // top-right
        (sx + sw - r, sy + sh - r), // bottom-right
        (sx + r, sy + sh - r),      // bottom-left
    ];
    // Every band uses the same count, since they must pair up; size it from
    // the outermost one so the widest curve is smooth.
    let n = corner_segments(r + blur);
    let v = |p: (f32, f32), alpha: f32| ShadowVertex { x: p.0, y: p.1, alpha };

    let mut inner = silhouette(&centers, r, n);
    let rings = if blur > 0.0 { SHADOW_RINGS } else { 0 };
    let mut out = Vec::with_capacity(inner.len() * (3 + 6 * rings));

    // Solid core: a fan from the shape's center over the inner outline.
    // Convex, so the fan covers it exactly once.
    let hub = (sx + sw * 0.5, sy + sh * 0.5);
    for i in 0..inner.len() {
        let j = (i + 1) % inner.len();
        out.push(v(hub, 1.0));
        out.push(v(inner[i], 1.0));
        out.push(v(inner[j], 1.0));
    }

    // The falloff, band by band. Skipped entirely for a hard shadow.
    let mut inner_alpha = 1.0;
    for ring in 1..=rings {
        let t = ring as f32 / rings as f32;
        let outer = silhouette(&centers, r + t * blur, n);
        let outer_alpha = shadow_falloff(t);
        for i in 0..inner.len() {
            let j = (i + 1) % inner.len();
            out.push(v(inner[i], inner_alpha));
            out.push(v(outer[i], outer_alpha));
            out.push(v(outer[j], outer_alpha));
            out.push(v(inner[i], inner_alpha));
            out.push(v(outer[j], outer_alpha));
            out.push(v(inner[j], inner_alpha));
        }
        inner = outer;
        inner_alpha = outer_alpha;
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
                ((t[1].x - t[0].x) * (t[2].y - t[0].y) - (t[2].x - t[0].x) * (t[1].y - t[0].y))
                    .abs()
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
        let n = corner_segments(blur) as f32;
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
        let max = mesh.iter().map(|v| v.alpha).fold(0.0f32, f32::max);
        let min = mesh.iter().map(|v| v.alpha).fold(1.0f32, f32::min);
        assert_eq!(max, 1.0, "the core keeps the full alpha");
        assert!(min.abs() < 1e-6, "the outer rim reaches zero");
        // Every fully faded vertex sits `blur` outside the shape, so none of
        // them can be inside the (10,10)–(50,50) box.
        for v in mesh.iter().filter(|v| v.alpha < 1e-6) {
            let inside = v.x > 10.5 && v.x < 49.5 && v.y > 10.5 && v.y < 49.5;
            assert!(!inside, "faded vertex {v:?} landed inside the shape");
        }
    }

    /// The falloff is a smoothstep, which is what makes it read as a blur
    /// rather than a cone: its midpoint is half alpha and both ends flatten,
    /// so neither the join with the core nor the rim shows a crease.
    #[test]
    fn falloff_is_smooth_at_both_ends() {
        let mesh = shadow_mesh(0.0, 0.0, 60.0, 60.0, 0.0, 20.0, 0.0, 0.0, 0.0);
        let mut levels: Vec<f32> = mesh.iter().map(|v| v.alpha).collect();
        levels.sort_by(|a, b| b.partial_cmp(a).expect("no NaN alphas"));
        levels.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        assert_eq!(levels.len(), SHADOW_RINGS + 1, "one alpha per band edge");
        let mid = levels[SHADOW_RINGS / 2];
        assert!((mid - 0.5).abs() < 1e-6, "midpoint is half alpha, got {mid}");
        let first_step = levels[0] - levels[1];
        let middle_step = levels[SHADOW_RINGS / 2] - levels[SHADOW_RINGS / 2 + 1];
        assert!(
            first_step < middle_step * 0.5,
            "the ramp should flatten near the core ({first_step} vs {middle_step})"
        );
        assert_eq!(shadow_falloff(0.0), 1.0);
        assert_eq!(shadow_falloff(1.0), 0.0);
        assert_eq!(shadow_falloff(-1.0), 1.0);
    }

    /// Band `i`'s outer edge is band `i+1`'s inner edge, point for point —
    /// overlapping bands would double-blend, gaps would show the background.
    #[test]
    fn bands_share_their_edges() {
        let centers = [(10.0, 10.0), (50.0, 10.0), (50.0, 40.0), (10.0, 40.0)];
        let inner = silhouette(&centers, 6.0, 5);
        let outer = silhouette(&centers, 9.0, 5);
        assert_eq!(inner.len(), outer.len());
        for (a, b) in inner.iter().zip(&outer) {
            let d = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            assert!((d - 3.0).abs() < 1e-4, "expected a 3px step, got {d}");
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
