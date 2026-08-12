//! Tracing a rendered panel back to the code that drew it.
//!
//! This is the host half of **programming by direct manipulation**: point at a
//! shape on the canvas, get the source span of the `draw_*` call that produced
//! it, and the span of each argument literal that positions it — which is what
//! lets a *drag* state its goals and write the answer back
//! ([`PanelHost::propose_drag_edits`](crate::PanelHost::propose_drag_edits)).
//!
//! # Why this is nearly free
//!
//! It leans entirely on attribution the Petal runtime already computes. The
//! bytecode lowerer stamps every instruction with the term it came from, so an
//! emitting native knows its own call site; with
//! [`Env::enable_emit_trace`](petal::env::Env::enable_emit_trace) on, each draw
//! command carries that term id out of the frame
//! ([`petal_ui::draw::take_draw_commands_traced`]). Nothing re-runs, nothing
//! about the frame changes, and with tracing off — every non-IDE panel — the
//! runtime records nothing at all.
//!
//! Everything richer than "which call" is derived *lazily* from that one id via
//! [`petal::provenance`], on the mouse move that asks, not on the frame that
//! drew. So a 60fps canvas pays a `Copy` push per shape and no more.
//!
//! # What hit-testing means here
//!
//! A panel frame is a flat, ordered command list, painted in order — so the
//! shape a user sees at a point is the **last** one covering it. That is the
//! whole hit-test: a forward scan tracking the clip rect, keeping the last
//! command whose geometry contains the point. No spatial index, because there
//! is nothing to index across frames: the list is rebuilt every frame anyway,
//! and a linear scan of a few hundred commands on one mouse move is not a cost
//! worth a data structure.

use petal::execution_context::EmitSite;
use petal::program::{Program, TermId};
use petal::provenance::{pick_frame, ArgKind, CallSite};
use petal::source_map::{SourceSpan, ENTRY_FILE};

use crate::panel::PanelCmd;

/// The call chain a draw command came from — an opaque handle to the runtime's
/// attribution, resolved to source by
/// [`PanelHost::trace_origin`](crate::PanelHost::trace_origin).
///
/// Opaque so the app layer can carry origins around (one per drawn command)
/// without taking on a direct Petal dependency, matching how [`PanelCmd`] keeps
/// the render vocabulary plain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawOrigin(pub(crate) EmitSite);

impl DrawOrigin {
    /// Whether this command was attributed at all.
    pub fn is_empty(&self) -> bool {
        self.0.chain.is_empty()
    }
}

/// A source range, in the 0-based line / 0-based column coordinates Garden's
/// editor uses. (Petal spans are 1-based on both axes; the conversion happens
/// once, here, so no caller has to remember which convention it holds.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeSpan {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl CodeSpan {
    /// Convert a Petal span. Returns `None` for the zero span — the placeholder
    /// a term with no real source position carries, which would otherwise
    /// highlight the start of line 1 for no reason.
    pub(crate) fn from_petal(span: &SourceSpan) -> Option<CodeSpan> {
        if span.start.line == 0 {
            return None;
        }
        Some(CodeSpan {
            start_line: span.start.line as usize - 1,
            start_col: span.start.column.saturating_sub(1) as usize,
            end_line: span.end.line.max(1) as usize - 1,
            end_col: span.end.column.saturating_sub(1) as usize,
        })
    }
}

/// How directly a traced argument maps onto something an editor may rewrite.
/// Mirrors [`petal::provenance::ArgKind`], re-stated here so the app layer
/// matches on a `garden-script` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSource {
    /// A literal written in the call itself — the safe case: the span belongs
    /// to this call and nothing else reads it.
    Literal,
    /// A name that resolves to a literal defined elsewhere. Rewritable, but the
    /// definition may feed other shapes too.
    Binding,
    /// Computed. One number cannot be edited to move this shape.
    Computed,
}

/// One argument of a traced draw call, resolved back to source.
#[derive(Debug, Clone)]
pub struct TracedArg {
    /// 0-based position in the call's argument list.
    pub index: usize,
    /// Where the argument is written in the call, when mapped.
    pub span: Option<CodeSpan>,
    /// The span a rewrite must replace — the literal's own text, which for a
    /// [`ArgSource::Binding`] is at the *definition*, not at this call.
    pub editable_span: Option<CodeSpan>,
    pub source: ArgSource,
    /// The literal value this argument resolves to, when it resolves to one.
    pub value: Option<f64>,
    /// Whether that literal was written as an integer — a rewrite should keep
    /// it one rather than churning `10` into `10.0`.
    pub is_int: bool,
}

/// A handle on the traced call itself — the term the runtime resolved it to.
///
/// Opaque, and deliberately not `Clone`-into-anything-meaningful: it is an
/// *index* into the program that was running when the trace was taken, so it is
/// only valid until the next recompile. Hand it straight back to
/// [`PanelHost::propose_drag_edits`](crate::PanelHost::propose_drag_edits),
/// which range-checks it, rather than storing it across a reload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallRef(pub(crate) TermId);

/// A draw call resolved back to source: what to highlight, and what a drag
/// would have to edit.
#[derive(Debug, Clone)]
pub struct DrawTrace {
    /// The call this trace resolved to, for stating goals against it.
    pub call_ref: CallRef,
    /// The whole call expression — what a hover highlights.
    pub call: Option<CodeSpan>,
    /// The drawing native's name (`draw_circle`, …), when statically known.
    pub callee: Option<String>,
    /// Each argument, in call order. Empty is a legitimate answer (a no-arg
    /// call), not a failure.
    pub args: Vec<TracedArg>,
}

impl DrawTrace {
    /// Resolve `origin` against the program it was recorded in.
    ///
    /// Attributes to the innermost frame in the **sketch's own file**, not the
    /// innermost frame overall: `draw_circle` is a prelude function wrapping a
    /// native, so the leaf of the chain is a line of `petal-ui`, while the line
    /// the user wrote — and the one an editor must highlight — is a frame or two
    /// further out.
    ///
    /// `None` when the chain is empty, or when its ids don't belong to this
    /// program — which is exactly what a stale origin looks like after a live
    /// reload, and why this is checked rather than assumed. Term ids are
    /// indices, so a stale one would resolve happily to unrelated code and point
    /// the user at it with total confidence.
    pub(crate) fn resolve(program: &Program, origin: &DrawOrigin) -> Option<DrawTrace> {
        let term = pick_frame(program, &origin.0.chain, ENTRY_FILE)?;
        let site = CallSite::resolve(program, term)?;
        let args = site
            .args
            .iter()
            .map(|a| TracedArg {
                index: a.index,
                span: a.span.as_ref().and_then(CodeSpan::from_petal),
                editable_span: a
                    .editable_span(program)
                    .as_ref()
                    .and_then(CodeSpan::from_petal),
                source: match a.kind {
                    ArgKind::Literal => ArgSource::Literal,
                    ArgKind::Binding => ArgSource::Binding,
                    ArgKind::Computed => ArgSource::Computed,
                },
                value: a.literal.map(|l| l.value),
                is_int: a.literal.is_some_and(|l| l.is_int),
            })
            .collect();
        Some(DrawTrace {
            call_ref: CallRef(term),
            call: site.span.as_ref().and_then(CodeSpan::from_petal),
            callee: site.callee,
            args,
        })
    }
}

// ── Dragging: which arguments a gesture moves ─────────────────────────────

/// Which arguments of a traced `draw_*` call follow the pointer, and what each
/// one is worth *right now*.
///
/// The current values are read off the **drawn command's own geometry**, not
/// off the argument literals, which is what makes a drag work on a computed
/// argument (`base_y - bh`): the trace can't tell you what that expression came
/// to, but the rectangle it produced is right there in the command list. Adding
/// the drag delta to it states the goal in the only terms that are always
/// known — where the shape actually is.
#[derive(Debug, Clone, PartialEq)]
pub struct DragHandle {
    /// `(argument index, current value)` for each argument the horizontal
    /// component of a drag moves. A line has two.
    pub x_args: Vec<(usize, f64)>,
    /// The same for the vertical component.
    pub y_args: Vec<(usize, f64)>,
}

impl DragHandle {
    fn xy(x: (usize, f64), y: (usize, f64)) -> DragHandle {
        DragHandle {
            x_args: vec![x],
            y_args: vec![y],
        }
    }
}

/// The drag handle for the command at a hit index, given the callee that drew
/// it and how many arguments that call was written with.
///
/// `None` — no drag — for a shape whose position isn't a pair of plain
/// arguments: the `draw_rect(rect, color)` overloads (the position lives inside
/// a `Rect` value, one argument for both axes), polygons (variadic), and
/// anything Garden draws that no `draw_*` name owns. Refusing is the honest
/// answer; a drag that rewrote the wrong argument would be worse than a drag
/// that doesn't start.
///
/// The arity check is what separates the overloads: `draw_circle(cx, cy, r, …)`
/// carries at least six arguments, `draw_circle(center, radius, color)` three.
pub fn drag_handle(cmd: &PanelCmd, callee: &str, arity: usize) -> Option<DragHandle> {
    match (callee, cmd) {
        ("draw_circle", PanelCmd::Circle { cx, cy, .. }) if arity >= 6 => {
            Some(DragHandle::xy((0, *cx as f64), (1, *cy as f64)))
        }
        ("draw_rect" | "draw_rect_rounded", PanelCmd::Rect { x, y, .. }) if arity >= 7 => {
            Some(DragHandle::xy((0, *x as f64), (1, *y as f64)))
        }
        ("draw_rect_outline", PanelCmd::RectOutline { x, y, .. }) if arity >= 7 => {
            Some(DragHandle::xy((0, *x as f64), (1, *y as f64)))
        }
        // `draw_rect_rounded_outline(x, y, w, h, radius, r, g, b, …)` — the
        // radius pushes the color args along, so its arity floor is one higher
        // than the square outline's.
        ("draw_rect_rounded_outline", PanelCmd::RectOutline { x, y, .. }) if arity >= 8 => {
            Some(DragHandle::xy((0, *x as f64), (1, *y as f64)))
        }
        ("draw_circle_outline", PanelCmd::EllipseOutline { cx, cy, .. }) if arity >= 6 => {
            Some(DragHandle::xy((0, *cx as f64), (1, *cy as f64)))
        }
        (
            "draw_ellipse" | "draw_ellipse_outline",
            PanelCmd::Ellipse { cx, cy, .. } | PanelCmd::EllipseOutline { cx, cy, .. },
        ) if arity >= 7 => Some(DragHandle::xy((0, *cx as f64), (1, *cy as f64))),
        ("fill_arc", PanelCmd::Arc { cx, cy, .. }) if arity >= 9 => {
            Some(DragHandle::xy((0, *cx as f64), (1, *cy as f64)))
        }
        ("fill_fan", PanelCmd::Fan { cx, cy, .. }) if arity >= 6 => {
            Some(DragHandle::xy((0, *cx as f64), (1, *cy as f64)))
        }
        ("draw_text", PanelCmd::Text { x, y, .. }) if arity >= 7 => {
            Some(DragHandle::xy((1, *x as f64), (2, *y as f64)))
        }
        ("draw_image", PanelCmd::Image { x, y, .. }) if arity >= 5 => {
            Some(DragHandle::xy((1, *x as f64), (2, *y as f64)))
        }
        // A line moves as a whole: both endpoints take the same delta, which is
        // four goals stated in one gesture — the case `propose_edits_batch`
        // exists for.
        ("draw_line", PanelCmd::Line { x1, y1, x2, y2, .. }) if arity >= 7 => Some(DragHandle {
            x_args: vec![(0, *x1 as f64), (2, *x2 as f64)],
            y_args: vec![(1, *y1 as f64), (3, *y2 as f64)],
        }),
        _ => None,
    }
}

/// One text replacement a drag resolved to: the runtime's answer to "this shape
/// should have been *there*".
///
/// Mirrors [`petal::direct_manipulation::EditProposal`] in Garden's own
/// coordinates, minus the term id — a host applies the text and re-traces, it
/// never re-resolves an id that the edit has already made stale.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRewrite {
    /// The range to replace, in the editor's 0-based coordinates.
    pub span: CodeSpan,
    pub new_text: String,
    /// The binding this edits, or `None` for a literal written in the call.
    pub variable: Option<String>,
    /// Whether other code reads the edited binding — the edit moves more than
    /// the shape that was grabbed.
    pub shared: bool,
    /// The edited binding was declared `config let`: the source itself named it
    /// a tuning knob.
    pub config: bool,
    /// Human-readable summary ("set `offset` to 12 (line 3)").
    pub description: String,
}

/// What a drag gesture's goals resolved to.
#[derive(Debug, Clone, PartialEq)]
pub enum DragOutcome {
    /// Apply these, and the shape is where the pointer put it.
    Edits(Vec<SourceRewrite>),
    /// Nothing can move, and why — a message worth showing the user, because
    /// "the drag did nothing" is otherwise indistinguishable from a bug.
    Refused(String),
    /// The trace no longer describes the running program. The normal state for
    /// one frame after an edit (term ids are indices, so the drag's own rewrite
    /// invalidates them); skip the frame rather than reporting anything.
    Stale,
}

/// The topmost drawn command containing the panel-local point (`x`, `y`), as an
/// index into `cmds`, or `None` if the point is bare canvas.
///
/// `cmds` is the frame's command list in paint order; "topmost" therefore means
/// *last*, which is why this scans forward and keeps the final match rather
/// than scanning backwards and returning early — the clip rect is stateful, and
/// reading it backwards would apply each clip to the wrong commands.
///
/// Whole-canvas fills (`Clear`) never match: a background that answers every
/// query would make every miss look like a hit on the line that cleared.
pub fn hit_test(cmds: &[PanelCmd], x: i32, y: i32) -> Option<usize> {
    let mut clip: Option<(i32, i32, i32, i32)> = None;
    let mut hit = None;
    for (i, cmd) in cmds.iter().enumerate() {
        match cmd {
            PanelCmd::Clip { x: cx, y: cy, w, h } => {
                clip = Some((*cx, *cy, *cx + *w as i32, *cy + *h as i32));
                continue;
            }
            PanelCmd::ClipNone => {
                clip = None;
                continue;
            }
            _ => {}
        }
        if let Some((x0, y0, x1, y1)) = clip {
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                continue;
            }
        }
        if contains(cmd, x, y) {
            hit = Some(i);
        }
    }
    hit
}

/// Whether one command's painted geometry covers the point.
fn contains(cmd: &PanelCmd, x: i32, y: i32) -> bool {
    match cmd {
        PanelCmd::Rect {
            x: rx, y: ry, w, h, ..
        }
        | PanelCmd::Image {
            x: rx, y: ry, w, h, ..
        } => in_rect(*rx, *ry, *w, *h, x, y),

        // An outline is hit on its stroke, not its hollow middle — clicking the
        // empty centre of a frame should reach whatever is drawn inside it.
        PanelCmd::RectOutline {
            x: rx,
            y: ry,
            w,
            h,
            width,
            ..
        } => {
            let t = (*width).max(1) as i32;
            in_rect(*rx, *ry, *w, *h, x, y)
                && !(x >= rx + t && x < rx + *w as i32 - t && y >= ry + t && y < ry + *h as i32 - t)
        }

        PanelCmd::Circle { cx, cy, radius, .. } => {
            let (dx, dy) = ((x - cx) as f64, (y - cy) as f64);
            dx * dx + dy * dy <= (*radius as f64) * (*radius as f64)
        }

        // A line is a zero-area shape, so it needs a pick tolerance or it would
        // be unhittable: at least a few px, wider for a thick stroke.
        PanelCmd::Line {
            x1,
            y1,
            x2,
            y2,
            width,
            ..
        } => {
            let tol = ((*width).max(1) as f64 / 2.0).max(LINE_PICK_TOLERANCE);
            point_segment_distance(
                x as f64, y as f64, *x1 as f64, *y1 as f64, *x2 as f64, *y2 as f64,
            ) <= tol
        }

        PanelCmd::Triangle {
            x1,
            y1,
            x2,
            y2,
            x3,
            y3,
            ..
        } => in_polygon(&[(*x1, *y1), (*x2, *y2), (*x3, *y3)], x, y),

        PanelCmd::Poly { points, .. } | PanelCmd::Polygon { points, .. } => {
            in_polygon(points, x, y)
        }

        // A fan covers the polygon center→p0→…→pn, so the same even-odd test
        // applies once the center is spliced in.
        PanelCmd::Fan { cx, cy, points, .. } => {
            let mut ring = Vec::with_capacity(points.len() + 1);
            ring.push((*cx, *cy));
            ring.extend_from_slice(points);
            in_polygon(&ring, x, y)
        }

        // Like a line, a polyline is a zero-area shape and needs a pick
        // tolerance: it is hit anywhere along the stroke.
        PanelCmd::Polyline { points, width, .. } => {
            let tol = ((*width).max(1) as f64 / 2.0).max(LINE_PICK_TOLERANCE);
            match points.as_slice() {
                [] => false,
                [(px, py)] => {
                    let (dx, dy) = ((x - px) as f64, (y - py) as f64);
                    (dx * dx + dy * dy).sqrt() <= tol
                }
                pts => pts.windows(2).any(|w| {
                    point_segment_distance(
                        x as f64,
                        y as f64,
                        w[0].0 as f64,
                        w[0].1 as f64,
                        w[1].0 as f64,
                        w[1].1 as f64,
                    ) <= tol
                }),
            }
        }

        PanelCmd::Ellipse { cx, cy, rx, ry, .. } => in_ellipse(*cx, *cy, *rx, *ry, x, y),

        // An outline is hit on its ring, not its hollow middle — same rule the
        // rect outline follows, so clicking through a hollow shape reaches what
        // is drawn inside it.
        PanelCmd::EllipseOutline {
            cx,
            cy,
            rx,
            ry,
            width,
            ..
        } => {
            let t = (*width).max(1);
            in_ellipse(*cx, *cy, *rx, *ry, x, y)
                && !in_ellipse(*cx, *cy, *rx - t as i32, *ry - t as i32, x, y)
        }

        // An annular sector: inside the radius band *and* inside the angular
        // sweep. Angles are normalized into [0, TAU) relative to the start, so
        // a wedge that crosses the +x axis (or sweeps backwards) still tests
        // correctly.
        PanelCmd::Arc {
            cx,
            cy,
            r_in,
            r_out,
            a0,
            a1,
            ..
        } => {
            let (dx, dy) = ((x - cx) as f32, (y - cy) as f32);
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < *r_in || dist > *r_out {
                return false;
            }
            let tau = std::f32::consts::TAU;
            let sweep = (a1 - a0).clamp(-tau, tau);
            let rel = (dy.atan2(dx) - a0).rem_euclid(tau);
            if sweep >= 0.0 {
                rel <= sweep
            } else {
                rel >= tau + sweep || rel == 0.0
            }
        }

        // Text is hit over its measured box. The host knows the real advance
        // width; approximating it here from the size would mis-hit proportional
        // faces, so this uses the conservative monospace estimate the panel
        // vocabulary already assumes for layout.
        PanelCmd::Text {
            text,
            x: tx,
            y: ty,
            size,
            ..
        } => {
            let w = (text.chars().count() as f64 * *size as f64 * TEXT_ADVANCE_RATIO) as u32;
            in_rect(*tx, *ty, w, *size as u32, x, y)
        }

        // Everything else is either not a painted shape (region declarations,
        // scroll/wrap actions) or is the background itself.
        _ => false,
    }
}

/// Pick tolerance in px for hair-thin lines — how close the pointer must come
/// to a 1px stroke to count as on it.
const LINE_PICK_TOLERANCE: f64 = 3.0;

/// Width-to-size ratio for the monospace face panels lay text out with, matching
/// `panel::TEXT_ADVANCE_RATIO`.
const TEXT_ADVANCE_RATIO: f64 = 0.6;

fn in_rect(rx: i32, ry: i32, w: u32, h: u32, x: i32, y: i32) -> bool {
    x >= rx && x < rx + w as i32 && y >= ry && y < ry + h as i32
}

/// Is the point inside the axis-aligned ellipse? A zero (or negative) semi-axis
/// is an empty shape — which is what makes the hollow-ellipse test below fall
/// out for free once a thick stroke eats the inner rim.
fn in_ellipse(cx: i32, cy: i32, rx: i32, ry: i32, x: i32, y: i32) -> bool {
    if rx <= 0 || ry <= 0 {
        return false;
    }
    let (dx, dy) = ((x - cx) as f64 / rx as f64, (y - cy) as f64 / ry as f64);
    dx * dx + dy * dy <= 1.0
}

/// Even-odd point-in-polygon. Handles the concave polygons `fill_poly` allows,
/// which a convexity-assuming half-plane test would get wrong.
fn in_polygon(points: &[(i32, i32)], x: i32, y: i32) -> bool {
    if points.len() < 3 {
        return false;
    }
    let (px, py) = (x as f64, y as f64);
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = (points[i].0 as f64, points[i].1 as f64);
        let (xj, yj) = (points[j].0 as f64, points[j].1 as f64);
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Distance from a point to a line segment.
fn point_segment_distance(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len_sq = dx * dx + dy * dy;
    // A degenerate segment is a point; fall through to the point distance
    // rather than dividing by zero.
    let t = if len_sq > 0.0 {
        (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (x1 + t * dx, y1 + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> PanelCmd {
        PanelCmd::Rect {
            x,
            y,
            w,
            h,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            radius: 0,
        }
    }

    fn circle(cx: i32, cy: i32, radius: i32) -> PanelCmd {
        PanelCmd::Circle {
            cx,
            cy,
            radius,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }
    }

    #[test]
    fn finds_the_shape_under_the_point() {
        let cmds = vec![rect(10, 10, 50, 50), circle(200, 200, 20)];
        assert_eq!(hit_test(&cmds, 20, 20), Some(0));
        assert_eq!(hit_test(&cmds, 200, 205), Some(1));
        assert_eq!(hit_test(&cmds, 500, 500), None);
    }

    /// Overlapping shapes resolve to the one actually visible — the last
    /// painted. Getting this backwards is the bug a user notices immediately.
    #[test]
    fn overlapping_shapes_resolve_to_the_topmost() {
        let cmds = vec![rect(0, 0, 100, 100), rect(20, 20, 20, 20)];
        assert_eq!(hit_test(&cmds, 25, 25), Some(1), "the shape on top wins");
        assert_eq!(hit_test(&cmds, 5, 5), Some(0), "outside it, the one below");
    }

    /// A full-canvas `clear` is background, not a shape: a miss must read as a
    /// miss rather than as a hit on whatever line cleared the screen.
    #[test]
    fn a_clear_is_never_a_hit() {
        let cmds = vec![PanelCmd::Clear { r: 0, g: 0, b: 0 }, rect(10, 10, 10, 10)];
        assert_eq!(hit_test(&cmds, 300, 300), None);
        assert_eq!(hit_test(&cmds, 12, 12), Some(1));
    }

    /// A clipped shape is only hittable where it is actually drawn.
    #[test]
    fn clipping_bounds_the_hit_area() {
        let cmds = vec![
            PanelCmd::Clip {
                x: 0,
                y: 0,
                w: 30,
                h: 30,
            },
            rect(0, 0, 100, 100),
            PanelCmd::ClipNone,
            rect(200, 0, 10, 10),
        ];
        assert_eq!(hit_test(&cmds, 10, 10), Some(1), "inside the clip");
        assert_eq!(hit_test(&cmds, 50, 50), None, "clipped away");
        assert_eq!(hit_test(&cmds, 205, 5), Some(3), "after ClipNone");
    }

    /// A rect outline is picked on its stroke; its hollow middle belongs to
    /// whatever is drawn inside it.
    #[test]
    fn an_outline_is_hit_on_its_stroke_not_its_middle() {
        let cmds = vec![PanelCmd::RectOutline {
            x: 0,
            y: 0,
            w: 40,
            h: 40,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            width: 2,
            radius: 0,
        }];
        assert_eq!(hit_test(&cmds, 1, 20), Some(0), "on the left edge");
        assert_eq!(hit_test(&cmds, 20, 20), None, "through the middle");
    }

    /// The circle test is radial, not its bounding box — a corner of the box is
    /// outside the shape.
    #[test]
    fn a_circle_is_round() {
        let cmds = vec![circle(50, 50, 20)];
        assert_eq!(hit_test(&cmds, 50, 50), Some(0));
        assert_eq!(hit_test(&cmds, 69, 50), Some(0), "just inside the edge");
        assert_eq!(hit_test(&cmds, 35, 35), None, "the bounding box's corner");
    }

    /// A zero-area line still has to be pickable, within a small tolerance.
    #[test]
    fn a_line_is_pickable_near_its_stroke() {
        let cmds = vec![PanelCmd::Line {
            x1: 0,
            y1: 0,
            x2: 100,
            y2: 0,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            width: 1,
        }];
        assert_eq!(hit_test(&cmds, 50, 1), Some(0), "within tolerance");
        assert_eq!(hit_test(&cmds, 50, 40), None, "well off the line");
    }

    /// A concave polygon's notch is outside it — the reason this uses even-odd
    /// rather than a convex half-plane test.
    #[test]
    fn a_concave_polygon_excludes_its_notch() {
        // An arrowhead-ish chevron with a deep notch in the bottom middle.
        let cmds = vec![PanelCmd::Poly {
            points: vec![(0, 0), (100, 0), (100, 100), (50, 20), (0, 100)],
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }];
        assert_eq!(hit_test(&cmds, 50, 10), Some(0), "in the solid top");
        assert_eq!(hit_test(&cmds, 50, 90), None, "in the notch");
    }

    /// A zero-length span (a term with no real source position) must not
    /// resolve to "the top of line 1", which would highlight arbitrary code.
    #[test]
    fn the_placeholder_span_does_not_resolve() {
        assert_eq!(CodeSpan::from_petal(&petal::source_map::ZERO_SPAN), None);
    }

    /// Petal spans are 1-based on both axes; Garden's editor is 0-based on both.
    #[test]
    fn spans_convert_to_editor_coordinates() {
        use petal::source_map::{SourcePosition, SourceSpan};
        let span = SourceSpan {
            start: SourcePosition {
                line: 3,
                column: 5,
                offset: 0,
            },
            end: SourcePosition {
                line: 3,
                column: 12,
                offset: 0,
            },
            file: petal::source_map::ENTRY_FILE,
        };
        let converted = CodeSpan::from_petal(&span).unwrap();
        assert_eq!(converted.start_line, 2);
        assert_eq!(converted.start_col, 4);
        assert_eq!(converted.end_line, 2);
        assert_eq!(converted.end_col, 11);
    }
}
