//! Software rasterization of the primitive draw commands — polygons,
//! antialiased lines and discs, rounded rects and outlines, conics, gradients,
//! and shadows — onto any SDL `Canvas`.

use sdl2::pixels::Color;
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, RenderTarget};

/// Set the draw color, enabling alpha blending only when the primitive is
/// translucent. Opaque primitives keep `BlendMode::None` so they overwrite —
/// which also preserves the offscreen-canvas contract (an opaque draw sets the
/// canvas pixel fully opaque; the later blit composites it).
pub(super) fn set_draw_color<T: RenderTarget>(canvas: &mut Canvas<T>, r: u8, g: u8, b: u8, a: u8) {
    canvas.set_blend_mode(if a < 255 {
        BlendMode::Blend
    } else {
        BlendMode::None
    });
    canvas.set_draw_color(Color::RGBA(r, g, b, a));
}

/// Fill an arbitrary polygon using the even-odd scanline rule.
/// Handles convex and concave polygons. The draw color must be set by the caller.
pub(super) fn fill_polygon<T: RenderTarget>(canvas: &mut Canvas<T>, points: &[(i32, i32)]) {
    if points.len() < 3 {
        return;
    }

    let min_y = points.iter().map(|p| p.1).min().unwrap();
    let max_y = points.iter().map(|p| p.1).max().unwrap();

    for y in min_y..=max_y {
        // Collect x-intersections of this scanline with every polygon edge.
        let mut crossings: Vec<i32> = Vec::new();
        let n = points.len();
        for i in 0..n {
            let (x1, y1) = points[i];
            let (x2, y2) = points[(i + 1) % n];
            // Skip horizontal edges; they contribute no crossing.
            if y1 == y2 {
                continue;
            }
            let (ya, yb, xa, xb) = if y1 < y2 {
                (y1, y2, x1, x2)
            } else {
                (y2, y1, x2, x1)
            };
            // Half-open interval [ya, yb) avoids double-counting shared vertices.
            if y >= ya && y < yb {
                let t = (y - ya) as f64 / (yb - ya) as f64;
                let x = xa as f64 + t * (xb - xa) as f64;
                crossings.push(x.round() as i32);
            }
        }

        crossings.sort_unstable();
        // Fill spans between consecutive intersection pairs.
        let mut i = 0;
        while i + 1 < crossings.len() {
            let x_start = crossings[i];
            let x_end = crossings[i + 1];
            let _ = canvas.draw_line((x_start, y), (x_end, y));
            i += 2;
        }
    }
}

/// Draw an antialiased line of the given stroke `width` as a distance-field
/// capsule: a pixel's coverage is its distance to the segment against the half-
/// width. This handles thickness and antialiasing together (round caps), and a
/// width-1 line reduces to a soft 1px stroke.
pub(super) fn stroke_line_aa<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    width: u32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    canvas.set_blend_mode(BlendMode::Blend);
    let half = width.max(1) as f64 / 2.0;
    let (ax, ay) = (x1 as f64, y1 as f64);
    let (bx, by) = (x2 as f64, y2 as f64);
    let pad = half.ceil() as i32 + 1;
    let (x0, xe) = (x1.min(x2) - pad, x1.max(x2) + pad);
    let (y0, ye) = (y1.min(y2) - pad, y1.max(y2) + pad);
    for py in y0..=ye {
        for px in x0..=xe {
            let dist = dist_to_segment(px as f64 + 0.5, py as f64 + 0.5, ax, ay, bx, by);
            plot(canvas, px, py, Color::RGBA(r, g, b, a), (half + 0.5 - dist).clamp(0.0, 1.0));
        }
    }
}

/// Distance from point (`px`, `py`) to the segment (`ax`,`ay`)–(`bx`,`by`).
fn dist_to_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((px - ax) * dx + (py - ay) * dy) / len2
    };
    let t = t.clamp(0.0, 1.0);
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Stroke a rectangle outline of the given `width` as four disjoint border
/// bands (so translucent strokes don't double-blend at the corners). `width` is
/// clamped so the bands don't overlap.
pub(super) fn stroke_rect_outline<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    width: u32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    let sw = width.max(1).min(w).min(h);
    set_draw_color(canvas, r, g, b, a);
    let swi = sw as i32;
    let (wi, hi) = (w as i32, h as i32);
    // Top and bottom span the full width; left/right fill only the gap between
    // them so no pixel is painted twice.
    let _ = canvas.fill_rect(Rect::new(x, y, w, sw));
    let _ = canvas.fill_rect(Rect::new(x, y + hi - swi, w, sw));
    let mid_h = h.saturating_sub(2 * sw);
    if mid_h > 0 {
        let _ = canvas.fill_rect(Rect::new(x, y + swi, sw, mid_h));
        let _ = canvas.fill_rect(Rect::new(x + wi - swi, y + swi, sw, mid_h));
    }
}

/// A full turn and a half turn in radians — the sweeps the conic commands are
/// split along so every filled piece stays a simple polygon.
pub(super) const TAU: f64 = std::f64::consts::TAU;
pub(super) const PI: f64 = std::f64::consts::PI;

/// How many segments to approximate a conic of the given size with.
fn conic_segments(size: f64) -> usize {
    ((size * 0.7) as usize + 8).clamp(8, 64)
}

/// Sample an elliptical arc from `a0` to `a1` into a closed point ring — the
/// outline of a filled ellipse or elliptical wedge.
pub(super) fn ellipse_ring(cx: i32, cy: i32, rx: f64, ry: f64, a0: f64, a1: f64) -> Vec<(i32, i32)> {
    let n = conic_segments(rx.max(ry));
    (0..=n)
        .map(|i| {
            let t = a0 + (a1 - a0) * (i as f64) / (n as f64);
            (
                cx + (rx * t.cos()).round() as i32,
                cy + (ry * t.sin()).round() as i32,
            )
        })
        .collect()
}

/// The outline of one annular sector — the outer rim swept forward, the inner
/// rim swept back — which is a *simple* polygon as long as the sweep is at most
/// half a turn. Used for arcs, rounded-outline corners, and (as two halves) a
/// hollow ellipse.
#[allow(clippy::too_many_arguments)]
pub(super) fn annular_sector(
    cx: i32,
    cy: i32,
    rx_in: f64,
    rx_out: f64,
    ry_in: f64,
    ry_out: f64,
    a0: f64,
    a1: f64,
) -> Vec<(i32, i32)> {
    let mut ring = ellipse_ring(cx, cy, rx_out, ry_out, a0, a1);
    if rx_in <= 0.0 || ry_in <= 0.0 {
        ring.push((cx, cy));
    } else {
        let mut inner = ellipse_ring(cx, cy, rx_in, ry_in, a0, a1);
        inner.reverse();
        ring.extend(inner);
    }
    ring
}

/// Stroke a rounded rectangle outline `width` px thick inside the bounds: four
/// straight bands between the corners, plus a quarter annulus at each corner.
/// Genuinely hollow (unlike two stacked rounded fills), which is the whole
/// point of the command.
#[allow(clippy::too_many_arguments)]
pub(super) fn stroke_rounded_rect_outline<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    radius: u32,
    width: u32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    let rad = radius.min(w / 2).min(h / 2) as i32;
    if rad <= 0 {
        stroke_rect_outline(canvas, x, y, w, h, width, r, g, b, a);
        return;
    }
    let t = width.max(1).min(w).min(h);
    let ti = t as i32;
    let (wi, hi) = (w as i32, h as i32);
    set_draw_color(canvas, r, g, b, a);
    let flat_w = (wi - 2 * rad).max(0) as u32;
    let flat_h = (hi - 2 * rad).max(0) as u32;
    if flat_w > 0 {
        let _ = canvas.fill_rect(Rect::new(x + rad, y, flat_w, t));
        let _ = canvas.fill_rect(Rect::new(x + rad, y + hi - ti, flat_w, t));
    }
    if flat_h > 0 {
        let _ = canvas.fill_rect(Rect::new(x, y + rad, t, flat_h));
        let _ = canvas.fill_rect(Rect::new(x + wi - ti, y + rad, t, flat_h));
    }
    let inner = (rad - ti).max(0) as f64;
    let corners = [
        (x + rad, y + rad, PI),
        (x + wi - rad, y + rad, 1.5 * PI),
        (x + wi - rad, y + hi - rad, 0.0),
        (x + rad, y + hi - rad, 0.5 * PI),
    ];
    for (ccx, ccy, start) in corners {
        fill_polygon(
            canvas,
            &annular_sector(
                ccx,
                ccy,
                inner,
                rad as f64,
                inner,
                rad as f64,
                start,
                start + PI / 2.0,
            ),
        );
    }
}

/// Linear interpolation between two RGBA stops at `t` ∈ [0, 1].
fn lerp_rgba(c0: [u8; 4], c1: [u8; 4], t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
    Color::RGBA(
        mix(c0[0], c1[0]),
        mix(c0[1], c1[1]),
        mix(c0[2], c1[2]),
        mix(c0[3], c1[3]),
    )
}

/// How far a rounded rect's edge is inset at `d` px from the near end of the
/// side — 0 along the straight run, ramping up through the corner arc.
fn corner_inset(d: f64, span: f64, radius: f64) -> f64 {
    // Distance into the nearer corner arc, or 0 between them.
    let into = if d < radius {
        radius - d
    } else if d > span - radius {
        radius - (span - d)
    } else {
        return 0.0;
    };
    radius - (radius * radius - into * into).max(0.0).sqrt()
}

/// Fill a (optionally rounded) rect with a linear gradient, one 1px line at a
/// time along whichever axis the gradient leans toward.
///
/// This is the software fallback, not a shader: the color is evaluated once
/// per line, at the line's midpoint, so a diagonal gradient banks its steps
/// along the dominant axis. Axis-aligned angles — very nearly all of them in
/// practice — are exact. The rounded case narrows each line to the rounded
/// rect's own span at that offset, so the corners are shaped, not clipped.
#[allow(clippy::too_many_arguments)]
pub(super) fn fill_rect_gradient<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    radius: u32,
    c0: [u8; 4],
    c1: [u8; 4],
    angle: f32,
) {
    if w == 0 || h == 0 {
        return;
    }
    canvas.set_blend_mode(BlendMode::Blend);
    let (wf, hf) = (w as f64, h as f64);
    let rad = (radius as f64).min(wf * 0.5).min(hf * 0.5);
    let (dx, dy) = ((angle as f64).cos(), (angle as f64).sin());
    // The gradient spans the rect's whole extent along the axis, so the
    // projection is normalized by the projected width of the rect.
    let extent = dx.abs() * wf + dy.abs() * hf;
    let t_at = |px: f64, py: f64| {
        // Offset from the corner the gradient starts at.
        let ox = if dx >= 0.0 { px } else { wf - px };
        let oy = if dy >= 0.0 { py } else { hf - py };
        (ox * dx.abs() + oy * dy.abs()) / extent.max(1.0)
    };
    if dy.abs() >= dx.abs() {
        for row in 0..h as i32 {
            let inset = corner_inset(row as f64 + 0.5, hf, rad);
            let line_w = wf - 2.0 * inset;
            if line_w <= 0.0 {
                continue;
            }
            canvas.set_draw_color(lerp_rgba(c0, c1, t_at(wf * 0.5, row as f64 + 0.5)));
            let _ = canvas.fill_rect(Rect::new(
                x + inset.round() as i32,
                y + row,
                line_w.round().max(1.0) as u32,
                1,
            ));
        }
    } else {
        for col in 0..w as i32 {
            let inset = corner_inset(col as f64 + 0.5, wf, rad);
            let line_h = hf - 2.0 * inset;
            if line_h <= 0.0 {
                continue;
            }
            canvas.set_draw_color(lerp_rgba(c0, c1, t_at(col as f64 + 0.5, hf * 0.5)));
            let _ = canvas.fill_rect(Rect::new(
                x + col,
                y + inset.round() as i32,
                1,
                line_h.round().max(1.0) as u32,
            ));
        }
    }
}

/// Fill a disc with a radial gradient, center color to rim color, per pixel —
/// the same per-pixel idiom `fill_disc_aa` uses, with the rim antialiased by
/// disc coverage.
pub(super) fn fill_circle_gradient<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    cx: i32,
    cy: i32,
    radius: i32,
    c0: [u8; 4],
    c1: [u8; 4],
) {
    if radius <= 0 {
        return;
    }
    canvas.set_blend_mode(BlendMode::Blend);
    let (fx, fy, fr) = (cx as f64 + 0.5, cy as f64 + 0.5, radius as f64);
    for py in (cy - radius - 1)..=(cy + radius + 1) {
        for px in (cx - radius - 1)..=(cx + radius + 1) {
            let coverage = disc_coverage(px, py, fx, fy, fr);
            if coverage <= 0.0 {
                continue;
            }
            let d = ((px as f64 + 0.5 - fx).powi(2) + (py as f64 + 0.5 - fy).powi(2)).sqrt();
            plot(canvas, px, py, lerp_rgba(c0, c1, d / fr), coverage);
        }
    }
}

/// Draw a soft shadow per pixel, from the signed distance to the (offset,
/// spread) rounded rect: full alpha inside, ramping linearly to 0 at `blur`.
///
/// That is the same falloff [`petal_ui::tess::shadow_mesh`] tessellates for a
/// GPU host, evaluated analytically instead — which on a software canvas is
/// both simpler and exact, and shares the property that matters: every pixel
/// is written once, so the shadow never double-composites over itself.
#[allow(clippy::too_many_arguments)]
pub(super) fn fill_shadow<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    radius: u32,
    blur: u32,
    spread: i32,
    dx: i32,
    dy: i32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    let sw = w as f64 + 2.0 * spread as f64;
    let sh = h as f64 + 2.0 * spread as f64;
    if sw <= 0.0 || sh <= 0.0 || a == 0 {
        return;
    }
    let sx = (x + dx) as f64 - spread as f64;
    let sy = (y + dy) as f64 - spread as f64;
    // The spread grows the corners with the shape, as CSS does.
    let rad = (radius as f64 + spread as f64).clamp(0.0, sw.min(sh) * 0.5);
    let blur = blur as f64;
    let (hx, hy) = (sw * 0.5, sh * 0.5);
    let (mx, my) = (sx + hx, sy + hy);

    canvas.set_blend_mode(BlendMode::Blend);
    let x0 = (sx - blur - 1.0).floor() as i32;
    let x1 = (sx + sw + blur + 1.0).ceil() as i32;
    let y0 = (sy - blur - 1.0).floor() as i32;
    let y1 = (sy + sh + blur + 1.0).ceil() as i32;
    for py in y0..=y1 {
        for px in x0..=x1 {
            // Signed distance to the rounded rect: positive outside.
            let qx = (px as f64 + 0.5 - mx).abs() - hx + rad;
            let qy = (py as f64 + 0.5 - my).abs() - hy + rad;
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let d = outside + qx.max(qy).min(0.0) - rad;
            let falloff = if d <= 0.0 {
                1.0
            } else if blur <= 0.0 {
                0.0
            } else {
                (1.0 - d / blur).max(0.0)
            };
            plot(canvas, px, py, Color::RGBA(r, g, b, a), falloff);
        }
    }
}

/// Coverage of the pixel at (`px`, `py`) by a disc of `radius` centered at
/// (`cx`, `cy`) — 1 inside, 0 outside, a smooth ramp across the 1px edge band.
/// This is the antialiasing kernel shared by circles and rounded-rect corners.
fn disc_coverage(px: i32, py: i32, cx: f64, cy: f64, radius: f64) -> f64 {
    let dx = px as f64 + 0.5 - cx;
    let dy = py as f64 + 0.5 - cy;
    let dist = (dx * dx + dy * dy).sqrt();
    (radius + 0.5 - dist).clamp(0.0, 1.0)
}

/// Write one pixel of `color` with its alpha scaled by `coverage` — the
/// per-pixel step of every antialiased primitive. A pixel that rounds to fully
/// transparent is skipped. The caller sets `BlendMode::Blend`.
fn plot<T: RenderTarget>(canvas: &mut Canvas<T>, px: i32, py: i32, color: Color, coverage: f64) {
    let pa = (coverage * color.a as f64).round() as u8;
    if pa > 0 {
        canvas.set_draw_color(Color::RGBA(color.r, color.g, color.b, pa));
        let _ = canvas.draw_point((px, py));
    }
}

/// Draw an antialiased filled disc. Edge pixels are drawn with partial alpha
/// (coverage × the fill alpha), which smooths the outline instead of the
/// stair-stepped midpoint circle.
pub(super) fn fill_disc_aa<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    cx: f64,
    cy: f64,
    radius: f64,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    if radius <= 0.0 {
        return;
    }
    canvas.set_blend_mode(BlendMode::Blend);
    let x0 = (cx - radius - 1.0).floor() as i32;
    let x1 = (cx + radius + 1.0).ceil() as i32;
    let y0 = (cy - radius - 1.0).floor() as i32;
    let y1 = (cy + radius + 1.0).ceil() as i32;
    for py in y0..=y1 {
        for px in x0..=x1 {
            plot(canvas, px, py, Color::RGBA(r, g, b, a), disc_coverage(px, py, cx, cy, radius));
        }
    }
}

/// Fill a rounded rectangle: fast solid bands for the body (top strip, bottom
/// strip, full-width middle) plus four antialiased quarter-disc corners. The
/// bands and corner boxes are disjoint, so nothing is painted twice. `radius`
/// is clamped to half the smaller side.
pub(super) fn fill_rounded_rect<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    radius: u32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    let rad = radius.min(w / 2).min(h / 2);
    if rad == 0 {
        set_draw_color(canvas, r, g, b, a);
        let _ = canvas.fill_rect(Rect::new(x, y, w, h));
        return;
    }
    let radi = rad as i32;
    let (wi, hi) = (w as i32, h as i32);

    // Solid body: the middle band spans the full width; the top/bottom bands
    // span only between the corner columns.
    set_draw_color(canvas, r, g, b, a);
    let _ = canvas.fill_rect(Rect::new(x, y + radi, w, h - 2 * rad));
    let mid_w = w - 2 * rad;
    let _ = canvas.fill_rect(Rect::new(x + radi, y, mid_w, rad));
    let _ = canvas.fill_rect(Rect::new(x + radi, y + hi - radi, mid_w, rad));

    // Antialiased corners: each rad×rad box is a quarter disc about the inner
    // corner point (`bx`, `by` is the box's top-left).
    canvas.set_blend_mode(BlendMode::Blend);
    let radf = rad as f64;
    let corners = [
        (x as f64 + radf, y as f64 + radf, x, y),
        (
            x as f64 + wi as f64 - radf,
            y as f64 + radf,
            x + wi - radi,
            y,
        ),
        (
            x as f64 + radf,
            y as f64 + hi as f64 - radf,
            x,
            y + hi - radi,
        ),
        (
            x as f64 + wi as f64 - radf,
            y as f64 + hi as f64 - radf,
            x + wi - radi,
            y + hi - radi,
        ),
    ];
    for (ccx, ccy, bx, by) in corners {
        for py in by..by + radi {
            for px in bx..bx + radi {
                let coverage = disc_coverage(px, py, ccx, ccy, radf);
                plot(canvas, px, py, Color::RGBA(r, g, b, a), coverage);
            }
        }
    }
}
