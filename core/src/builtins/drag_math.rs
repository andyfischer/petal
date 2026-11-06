//! The numeric half of `ui/drag`'s `between` and `vary`
//! (docs/proposals/declarative-dragging.md, P2). The drag engine is Petal
//! code, and a native cannot call a Petal closure, so the speculative runs
//! happen in Petal and only numbers come here:
//!
//! - `drag_triangulate(points)`: a Delaunay triangulation of `[[x, y], …]`,
//!   as `[[i, j, k], …]` (counter-clockwise) — `between` over 3+ targets.
//! - `drag_weights(points, tris, x, y)`: the barycentric weights of `(x, y)`
//!   over that triangulation, one per point, clamped to the hull (a point
//!   outside is first moved to the nearest point of the hull). With no
//!   triangles — 2 points, or all on a line — the weights are the linear
//!   projection onto the nearest segment between neighbours on the line.
//! - `drag_solve_step(values, rows, targets, jac, bounds, scale)`: one
//!   Gauss-Newton step for `vary`'s levers ([`crate::dm::lever_step`], the
//!   DM v2 solver's `goal_step` and `Bound::project`).

use crate::dm::lever::{PathLever, lever_step};
use crate::dm::solve::{Bound, GoalRow};
use crate::heap::Heap;
use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::require_args;

fn num(v: Value, what: &str) -> Result<f64, String> {
    match v {
        Value::Int(n) => Ok(n as f64),
        Value::Float(f) => Ok(f),
        other => Err(format!("{what} must be a number, got {}", other.type_name())),
    }
}

fn list(heap: &Heap, v: Value, what: &str) -> Result<Vec<Value>, String> {
    match v {
        Value::List(id) => Ok(heap.get_list(id).to_vec()),
        other => Err(format!("{what} must be a list, got {}", other.type_name())),
    }
}

fn nums(heap: &Heap, v: Value, what: &str) -> Result<Vec<f64>, String> {
    list(heap, v, what)?.into_iter().enumerate().map(|(i, x)| num(x, &format!("{what}[{i}]"))).collect()
}

/// `[[x, y], …]`.
fn points(heap: &Heap, v: Value, name: &str) -> Result<Vec<(f64, f64)>, String> {
    list(heap, v, &format!("{name}(): points"))?
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let xy = nums(heap, p, &format!("{name}(): points[{i}]"))?;
            match xy[..] {
                [x, y] if x.is_finite() && y.is_finite() => Ok((x, y)),
                _ => Err(format!("{name}(): points[{i}] must be [x, y] with finite numbers")),
            }
        })
        .collect()
}

fn floats(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}

// ── Delaunay (Bowyer-Watson) ───────────────────────────────────────────────

fn orient(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Whether `p` is strictly inside the circumcircle of the counter-clockwise
/// triangle `a b c`.
fn in_circle(a: (f64, f64), b: (f64, f64), c: (f64, f64), p: (f64, f64)) -> bool {
    let (ax, ay) = (a.0 - p.0, a.1 - p.1);
    let (bx, by) = (b.0 - p.0, b.1 - p.1);
    let (cx, cy) = (c.0 - p.0, c.1 - p.1);
    let det = (ax * ax + ay * ay) * (bx * cy - cx * by) - (bx * bx + by * by) * (ax * cy - cx * ay)
        + (cx * cx + cy * cy) * (ax * by - bx * ay);
    det > 1e-9 * (ax.abs() + ay.abs() + bx.abs() + by.abs() + cx.abs() + cy.abs()).powi(4).max(1e-300)
}

/// The Delaunay triangles of `pts` (indices, counter-clockwise). A point
/// equal to an earlier one is left out; collinear points give none.
pub fn triangulate(pts: &[(f64, f64)]) -> Vec<[usize; 3]> {
    let mut uniq: Vec<usize> = Vec::new();
    for (i, p) in pts.iter().enumerate() {
        if !uniq.iter().any(|&j| (pts[j].0 - p.0).abs() < 1e-9 && (pts[j].1 - p.1).abs() < 1e-9) {
            uniq.push(i);
        }
    }
    if uniq.len() < 3 {
        return Vec::new();
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &i in &uniq {
        x0 = x0.min(pts[i].0);
        y0 = y0.min(pts[i].1);
        x1 = x1.max(pts[i].0);
        y1 = y1.max(pts[i].1);
    }
    let d = (x1 - x0).max(y1 - y0).max(1.0) * 20.0;
    let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    // The super-triangle's corners sit after the real points.
    let n = pts.len();
    let mut all: Vec<(f64, f64)> = pts.to_vec();
    all.push((mx - d, my - d));
    all.push((mx + d, my - d));
    all.push((mx, my + d));
    let mut tris: Vec<[usize; 3]> = vec![[n, n + 1, n + 2]];
    for &i in &uniq {
        let p = all[i];
        let (bad, keep): (Vec<[usize; 3]>, Vec<[usize; 3]>) =
            tris.into_iter().partition(|t| in_circle(all[t[0]], all[t[1]], all[t[2]], p));
        tris = keep;
        // The boundary of the cavity: edges of exactly one bad triangle.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for t in &bad {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                if let Some(k) = edges.iter().position(|&(u, v)| u == b && v == a) {
                    edges.swap_remove(k);
                } else {
                    edges.push((a, b));
                }
            }
        }
        for (a, b) in edges {
            if orient(all[a], all[b], p).abs() > 1e-12 {
                tris.push([a, b, i]);
            }
        }
    }
    tris.retain(|t| t.iter().all(|&v| v < n));
    for t in &mut tris {
        if orient(pts[t[0]], pts[t[1]], pts[t[2]]) < 0.0 {
            t.swap(1, 2);
        }
    }
    tris.retain(|t| orient(pts[t[0]], pts[t[1]], pts[t[2]]) > 1e-9);
    tris.sort();
    tris
}

// ── barycentric weights, clamped to the hull ───────────────────────────────

fn bary(a: (f64, f64), b: (f64, f64), c: (f64, f64), p: (f64, f64)) -> Option<[f64; 3]> {
    let det = orient(a, b, c);
    if det.abs() < 1e-12 {
        return None;
    }
    let wa = orient(b, c, p) / det;
    let wb = orient(c, a, p) / det;
    Some([wa, wb, 1.0 - wa - wb])
}

/// `t` in [0, 1] of the point of segment `a b` nearest `p`, and its distance².
fn on_segment(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    (t, (p.0 - qx).powi(2) + (p.1 - qy).powi(2))
}

/// One weight per point of `pts` for `p` (see the module doc).
pub fn weights(pts: &[(f64, f64)], tris: &[[usize; 3]], p: (f64, f64)) -> Vec<f64> {
    let n = pts.len();
    let mut w = vec![0.0; n];
    if n == 0 {
        return w;
    }
    if n == 1 {
        w[0] = 1.0;
        return w;
    }
    let tris: Vec<[usize; 3]> = tris.iter().copied().filter(|t| t.iter().all(|&i| i < n)).collect();
    for t in &tris {
        if let Some(b) = bary(pts[t[0]], pts[t[1]], pts[t[2]], p)
            && b.iter().all(|&x| x >= -1e-9)
        {
            for k in 0..3 {
                w[t[k]] += b[k].max(0.0);
            }
            let s: f64 = w.iter().sum();
            return w.iter().map(|x| x / s).collect();
        }
    }
    // Outside (or no triangles): the nearest point of the candidate edges.
    let edges: Vec<(usize, usize)> = if tris.is_empty() {
        // On a line: neighbours along its direction.
        let far = (1..n)
            .max_by(|&a, &b| {
                let da = (pts[a].0 - pts[0].0).powi(2) + (pts[a].1 - pts[0].1).powi(2);
                let db = (pts[b].0 - pts[0].0).powi(2) + (pts[b].1 - pts[0].1).powi(2);
                da.total_cmp(&db)
            })
            .unwrap_or(1);
        let (dx, dy) = (pts[far].0 - pts[0].0, pts[far].1 - pts[0].1);
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| (pts[a].0 * dx + pts[a].1 * dy).total_cmp(&(pts[b].0 * dx + pts[b].1 * dy)));
        order.windows(2).map(|w| (w[0], w[1])).collect()
    } else {
        tris.iter().flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]).collect()
    };
    let mut best = (f64::INFINITY, 0usize, 1usize, 0.0f64);
    for (a, b) in edges {
        let (t, d2) = on_segment(pts[a], pts[b], p);
        if d2 < best.0 - 1e-12 {
            best = (d2, a, b, t);
        }
    }
    let (_, a, b, t) = best;
    w[a] += 1.0 - t;
    w[b] += t;
    w
}

pub(super) fn native_drag_triangulate(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "drag_triangulate")?;
    let pts = points(state.heap(), state.get_value(1)?, "drag_triangulate")?;
    let tris = triangulate(&pts);
    let items: Vec<Value> = tris
        .iter()
        .map(|t| Value::List(state.heap_mut().alloc_list(t.iter().map(|&i| Value::Int(i as i64)).collect())))
        .collect();
    state.push_list(items);
    Ok(1)
}

pub(super) fn native_drag_weights(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 4, "drag_weights")?;
    let pts = points(state.heap(), state.get_value(1)?, "drag_weights")?;
    let tris: Vec<[usize; 3]> = list(state.heap(), state.get_value(2)?, "drag_weights(): tris")?
        .into_iter()
        .map(|t| {
            let ix = nums(state.heap(), t, "drag_weights(): a triangle")?;
            match ix[..] {
                [a, b, c] if [a, b, c].iter().all(|&v| v >= 0.0 && v.fract() == 0.0) => {
                    Ok([a as usize, b as usize, c as usize])
                }
                _ => Err("drag_weights(): each triangle must be [i, j, k] point indices".to_string()),
            }
        })
        .collect::<Result<_, _>>()?;
    let x = num(state.get_value(3)?, "drag_weights(): x")?;
    let y = num(state.get_value(4)?, "drag_weights(): y")?;
    let w = weights(&pts, &tris, (x, y));
    state.push_list(floats(&w));
    Ok(1)
}

/// `drag_solve_step(values, rows, targets, jac, bounds, scale)`:
///
/// - `values`: the levers' current values (`k`);
/// - `rows` / `targets`: each goal's current value and where it should be
///   (`m`; for a dragged point, `[x, y]` and the pointer's);
/// - `jac`: `m` rows of `k` sensitivities `∂row/∂lever`;
/// - `bounds`: nil, or one entry per lever: nil, or `[lo, hi]` (either nil
///   for an open end);
/// - `scale`: the step's length, 1 for a full step (a backtrack halves it).
///
/// Returns the next values. Goals are equalities with a 0.5 tolerance; each
/// lever's cost is `1 / max(|v|, 1)` (DM v2's `d_i / s_i` with `d_i = 1`), so
/// a step that has a choice moves the levers in proportion to their size.
pub(super) fn native_drag_solve_step(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 6, "drag_solve_step")?;
    let heap = state.heap();
    let values = nums(heap, state.get_value(1)?, "drag_solve_step(): values")?;
    let rows = nums(heap, state.get_value(2)?, "drag_solve_step(): rows")?;
    let targets = nums(heap, state.get_value(3)?, "drag_solve_step(): targets")?;
    let k = values.len();
    let m = rows.len();
    if targets.len() != m {
        return Err(format!("drag_solve_step(): {m} rows but {} targets", targets.len()));
    }
    let jac: Vec<Vec<f64>> = list(heap, state.get_value(4)?, "drag_solve_step(): jac")?
        .into_iter()
        .map(|r| nums(heap, r, "drag_solve_step(): a jac row"))
        .collect::<Result<_, _>>()?;
    if jac.len() != m || jac.iter().any(|r| r.len() != k) {
        return Err(format!("drag_solve_step(): jac must be {m} rows of {k} numbers"));
    }
    let bounds: Vec<Bound> = match state.get_value(5)? {
        Value::Nil => vec![Bound::default(); k],
        v => {
            let bs = list(heap, v, "drag_solve_step(): bounds")?;
            if bs.len() != k {
                return Err(format!("drag_solve_step(): {k} values but {} bounds", bs.len()));
            }
            bs.into_iter()
                .enumerate()
                .map(|(i, b)| match b {
                    Value::Nil => Ok(Bound::default()),
                    Value::List(id) => {
                        let ends = heap.get_list(id);
                        let end = |j: usize| match ends.get(j) {
                            None | Some(Value::Nil) => Ok(None),
                            Some(v) => num(*v, &format!("drag_solve_step(): bounds[{i}][{j}]")).map(Some),
                        };
                        if ends.len() != 2 {
                            return Err(format!("drag_solve_step(): bounds[{i}] must be [lo, hi]"));
                        }
                        Ok(Bound { min: end(0)?, max: end(1)?, step: None, int: false })
                    }
                    other => Err(format!("drag_solve_step(): bounds[{i}] must be nil or [lo, hi], got {}", other.type_name())),
                })
                .collect::<Result<_, _>>()?
        }
    };
    let scale = num(state.get_value(6)?, "drag_solve_step(): scale")?;
    let levers: Vec<PathLever> = values.iter().zip(&bounds).map(|(&value, &bound)| PathLever { value, bound }).collect();
    let goals: Vec<GoalRow> = targets.iter().map(|&t| GoalRow::eq(t, 0.5, 1.0)).collect();
    let cur: Vec<Option<f64>> = rows.iter().map(|&r| Some(r)).collect();
    let cost: Vec<f64> = values.iter().map(|v| 1.0 / v.abs().max(1.0)).collect();
    let next = if values.iter().chain(&rows).chain(&targets).all(|v| v.is_finite())
        && jac.iter().flatten().all(|v| v.is_finite())
    {
        lever_step(&levers, &goals, &cur, &jac, &cost, scale)
    } else {
        levers.iter().map(|l| l.bound.project(l.value)).collect()
    };
    state.push_list(floats(&next));
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_is_two_triangles_and_its_centre_is_shared() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let t = triangulate(&pts);
        assert_eq!(t.len(), 2, "{t:?}");
        let w = weights(&pts, &t, (5.0, 5.0));
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        // A corner gets all of the weight.
        let w = weights(&pts, &t, (10.0, 0.0));
        assert!((w[1] - 1.0).abs() < 1e-9, "{w:?}");
    }

    #[test]
    fn outside_the_hull_is_clamped_to_its_edge() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        let t = triangulate(&pts);
        assert_eq!(t.len(), 1);
        // Below the bottom edge, a quarter of the way along it.
        let w = weights(&pts, &t, (2.5, -7.0));
        assert!((w[0] - 0.75).abs() < 1e-9 && (w[1] - 0.25).abs() < 1e-9 && w[2] == 0.0, "{w:?}");
        // Inside: barycentric, reproducing the point.
        let w = weights(&pts, &t, (2.0, 3.0));
        let x: f64 = w.iter().zip(&pts).map(|(w, p)| w * p.0).sum();
        let y: f64 = w.iter().zip(&pts).map(|(w, p)| w * p.1).sum();
        assert!((x - 2.0).abs() < 1e-9 && (y - 3.0).abs() < 1e-9);
    }

    #[test]
    fn two_points_and_a_line_are_linear() {
        let pts = [(0.0, 0.0), (100.0, 0.0)];
        assert!(triangulate(&pts).is_empty());
        assert_eq!(weights(&pts, &[], (25.0, 40.0)), vec![0.75, 0.25]);
        assert_eq!(weights(&pts, &[], (-50.0, 0.0)), vec![1.0, 0.0]);
        // Three in a row, given out of order: neighbours on the line blend.
        let pts = [(0.0, 0.0), (200.0, 0.0), (100.0, 0.0)];
        assert!(triangulate(&pts).is_empty());
        let w = weights(&pts, &[], (150.0, 5.0));
        assert_eq!(w, vec![0.0, 0.5, 0.5]);
    }

    #[test]
    fn delaunay_avoids_the_thin_diagonal() {
        // A flat rhombus: the short diagonal (1-3) is the Delaunay one.
        let pts = [(0.0, 0.0), (50.0, -10.0), (100.0, 0.0), (50.0, 10.0)];
        let t = triangulate(&pts);
        assert_eq!(t.len(), 2);
        assert!(t.iter().all(|t| t.contains(&1) && t.contains(&3)), "{t:?}");
    }

    #[test]
    fn duplicates_are_left_out() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (0.0, 0.0), (0.0, 10.0)];
        let t = triangulate(&pts);
        assert_eq!(t.len(), 1);
        assert!(!t[0].contains(&2));
    }
}
