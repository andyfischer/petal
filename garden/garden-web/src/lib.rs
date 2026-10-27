//! Garden's Petal panel host in the browser.
//!
//! This is the half of `garden petal-ide` that does not need a GPU: a
//! [`PanelHost`] running a sketch every frame, traced so every drawn shape
//! resolves back to the call that drew it, and checkpointed so a gesture on
//! a shape can be answered by direct manipulation v2 (re-running the frame
//! speculatively and solving for the literals behind it). The page supplies
//! the editor and paints the commands on a `<canvas>`.
//!
//! Every method trades JSON strings, so the page needs no generated types
//! beyond the class itself. Spans are 0-based lines and 0-based character
//! columns, the coordinates of [`garden_script::CodeSpan`].
//!
//! A gesture, as the page drives it (see `PanelHost::begin_adjust_v2`):
//!
//! 1. `pin()` on press, so frames that run before the pointer moves keep the
//!    checkpoint of the frame that was on screen.
//! 2. `begin(index, property, filter)` once the pointer passes the slop: the
//!    one expensive step (discovery and the Jacobian).
//! 3. `preview_text(source, deltas)` on every move: a linear prediction, no
//!    re-runs, applied to the press-time source.
//! 4. `finish(deltas)` on release: verified, rounded, ranked alternatives,
//!    each written with `alternative_text(source, k)`.

use garden_script::dm_hints::{self, HintState};
use garden_script::{
    apply_rewrites_to, cmd_bounds, hit_test, ArgSource, CodeSpan, DragAlternative, DragCandidate, DragFilter,
    DragOutcomeV2, DragProperty, DragSelector, PanelCmd, PanelHost, SourceRewrite,
};
use petal_ui::input::InputEvent;
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

const SKETCH_NAME: &str = "sketch.ptl";

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// A sketch running in the page.
#[wasm_bindgen]
pub struct GardenWeb {
    host: PanelHost,
    width: i32,
    height: i32,
    mono_advances: Option<Vec<f64>>,
    /// The last frame's commands: what hit tests and gestures index into.
    cmds: Vec<PanelCmd>,
    /// The last runtime error, cleared by the next good frame.
    error: Option<String>,
    /// The last gesture's answer, for `alternative_text`.
    alternatives: Vec<DragAlternative>,
}

#[wasm_bindgen]
impl GardenWeb {
    /// Compile `source` and get it ready to run. `Err` is the compile error.
    #[wasm_bindgen(constructor)]
    pub fn new(source: &str) -> Result<GardenWeb, JsValue> {
        let host = new_host(source).map_err(|e| JsValue::from_str(&e))?;
        Ok(GardenWeb {
            host,
            width: 640,
            height: 400,
            mono_advances: None,
            cmds: Vec::new(),
            error: None,
            alternatives: Vec::new(),
        })
    }

    /// The canvas size the sketch sees as `screen_width()` / `screen_height()`.
    pub fn set_size(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        self.host.set_dimensions(width, height);
    }

    /// Advance ratios (advance / font size, by codepoint) of the monospace
    /// face the page paints text in, so `text_width` measures what is drawn.
    pub fn set_mono_advances(&mut self, ratios: Vec<f64>) {
        self.host.set_font_advance_ratios(ratios.clone());
        self.mono_advances = Some(ratios);
    }

    // ── Input ─────────────────────────────────────────────────────────

    pub fn mouse_move(&mut self, x: i32, y: i32) {
        self.host.input_event(InputEvent::MouseMove { x, y });
    }

    /// `button`: 0 left, 1 right, 2 middle (petal-ui's ids).
    pub fn mouse_button(&mut self, button: u8, down: bool) {
        self.host.input_event(if down {
            InputEvent::MouseDown { button }
        } else {
            InputEvent::MouseUp { button }
        });
    }

    pub fn scroll(&mut self, dx: f64, dy: f64) {
        self.host.input_event(InputEvent::Scroll { dx, dy });
    }

    /// `key` is a canonical petal-ui key name (`left`, `space`, `a`).
    pub fn key(&mut self, key: &str, down: bool) {
        let key = key.to_string();
        self.host.input_event(if down {
            InputEvent::KeyDown { key }
        } else {
            InputEvent::KeyUp { key }
        });
    }

    pub fn text(&mut self, text: &str) {
        self.host.input_event(InputEvent::Text { text: text.to_string() });
    }

    // ── Frames and source ─────────────────────────────────────────────

    /// Run one frame at `time` seconds: `{cmds, error, skipped}`. On a runtime
    /// error the last good frame's commands come back with the error.
    /// (`frame_count` is an `f64` so JS passes a plain number, not a BigInt.)
    pub fn frame(&mut self, time: f64, dt: f64, frame_count: f64) -> String {
        self.host.set_clock(time);
        match self.host.frame(dt, frame_count as i64) {
            Ok(cmds) => {
                self.cmds = cmds;
                // A gate-skipped frame repeats the last run, error and all.
                if !self.host.last_frame_skipped() {
                    self.error = None;
                }
            }
            Err(e) => self.error = Some(e),
        }
        json!({
            "cmds": self.cmds.iter().map(cmd_json).collect::<Vec<_>>(),
            "error": self.error,
            "skipped": self.host.last_frame_skipped(),
        })
        .to_string()
    }

    /// Whether the next frame would run (the frame gate): a still sketch
    /// with no new input costs nothing to skip.
    pub fn invalidate(&mut self) {
        self.host.invalidate_frame();
    }

    /// Hot-reload `source`, keeping `state`. `{ok}` or `{error}`; on an error
    /// the running program is left as it was.
    pub fn reload(&mut self, source: &str) -> String {
        self.cancel();
        match self.host.reload_source(source) {
            Ok(()) => json!({ "ok": true }),
            Err(e) => json!({ "error": e }),
        }
        .to_string()
    }

    /// Start `source` again from scratch, discarding `state`.
    pub fn reset(&mut self, source: &str) -> String {
        match new_host(source) {
            Ok(mut host) => {
                host.set_dimensions(self.width, self.height);
                if let Some(r) = &self.mono_advances {
                    host.set_font_advance_ratios(r.clone());
                }
                self.host = host;
                self.cmds.clear();
                self.alternatives.clear();
                json!({ "ok": true })
            }
            Err(e) => json!({ "error": e }),
        }
        .to_string()
    }

    /// Lines the sketch printed since the last call.
    pub fn take_output(&mut self) -> String {
        json!(self.host.take_output()).to_string()
    }

    /// The sketch's `state` variables.
    pub fn state_json(&self) -> String {
        Value::Object(self.host.state_json()).to_string()
    }

    // ── Tracing ───────────────────────────────────────────────────────

    /// The topmost shape at a canvas point, or -1.
    pub fn hit(&self, x: i32, y: i32) -> i32 {
        hit_test(&self.cmds, x, y).map_or(-1, |i| i as i32)
    }

    /// The command at `index` resolved to source: `{call, callee, args,
    /// bounds, draggable}`, or `null` when it cannot be attributed.
    pub fn trace(&self, index: usize) -> String {
        let Some(cmd) = self.cmds.get(index) else {
            return "null".into();
        };
        let trace = self.host.origin_at(index).and_then(|o| self.host.trace_origin(o));
        let bounds = cmd_bounds(cmd).map(|(x, y, w, h)| json!([x, y, w, h]));
        match trace {
            None => json!({ "call": null, "bounds": bounds }),
            Some(t) => json!({
                "call": t.call.map(span_json),
                "callee": t.callee,
                "bounds": bounds,
                "args": t.args.iter().map(|a| json!({
                    "index": a.index,
                    "span": a.span.map(span_json),
                    "editable": a.editable_span.map(span_json),
                    "source": match a.source {
                        ArgSource::Literal => "literal",
                        ArgSource::Binding => "binding",
                        ArgSource::Computed => "computed",
                    },
                    "value": a.value,
                })).collect::<Vec<_>>(),
            }),
        }
        .to_string()
    }

    /// The call span of every command of the last frame (`null` where
    /// unattributed), and its bounds: what an editor hover needs to light up
    /// every shape a line drew.
    pub fn trace_all(&self) -> String {
        let rows: Vec<Value> = (0..self.cmds.len())
            .map(|i| {
                let call = self
                    .host
                    .origin_at(i)
                    .and_then(|o| self.host.trace_origin(o))
                    .and_then(|t| t.call);
                json!({
                    "call": call.map(span_json),
                    "bounds": cmd_bounds(&self.cmds[i]).map(|(x, y, w, h)| json!([x, y, w, h])),
                })
            })
            .collect();
        Value::Array(rows).to_string()
    }

    // ── Direct manipulation v2 ────────────────────────────────────────

    /// A press: keep the checkpoint of the frame on screen for the gesture.
    pub fn pin(&mut self) {
        self.host.pin_dm_checkpoint();
    }

    /// Open a gesture on command `index`. `property` is `position`, `size`
    /// or `color`; `filter` is `{allow: [selector], deny: [selector]}` in
    /// `:DragAllow` syntax (`gap`, `line:3`, `fn:place`, `config`).
    ///
    /// `{ok: true, axes, candidates, runs}` or `{refused, candidates}` or
    /// `{stale: true}`.
    pub fn begin(&mut self, index: usize, property: &str, filter: &str) -> String {
        let property = match property {
            "size" => DragProperty::Size,
            "color" => DragProperty::Color,
            _ => DragProperty::Position,
        };
        match self.host.begin_adjust_v2(index, property, &parse_filter(filter)) {
            Ok(start) => json!({
                "ok": true,
                "axes": start.axes,
                "candidates": start.candidates.iter().map(candidate_json).collect::<Vec<_>>(),
                "runs": start.runs,
            })
            .to_string(),
            Err(outcome) => self.outcome_json(outcome),
        }
    }

    /// The open gesture's linear prediction at `deltas` (one per axis),
    /// applied to `source`, the text the gesture started from:
    /// `{text, rewrites}`. The text is `source` unchanged when nothing is
    /// predicted to move.
    pub fn preview_text(&self, source: &str, deltas: Vec<f64>) -> String {
        let rewrites: Vec<SourceRewrite> = self
            .host
            .preview_adjust_v2(&deltas)
            .into_iter()
            .filter(|r| r.file.is_none())
            .collect();
        let text = apply_rewrites_to(source, &rewrites).unwrap_or_else(|_| source.to_string());
        json!({ "text": text, "rewrites": rewrites.iter().map(rewrite_json).collect::<Vec<_>>() }).to_string()
    }

    /// Release the open gesture at `deltas`: `{alternatives, candidates}`
    /// (best first), `{refused, candidates}` or `{stale: true}`.
    pub fn finish(&mut self, deltas: Vec<f64>) -> String {
        let outcome = self.host.finish_adjust_v2(&deltas);
        self.outcome_json(outcome)
    }

    /// Abandon an open gesture or a press that never became one.
    pub fn cancel(&mut self) {
        self.host.cancel_drag_v2();
    }

    /// Alternative `k` of the last release written into `source` (the text
    /// the gesture started from), or `null` when it does not apply.
    pub fn alternative_text(&self, source: &str, k: usize) -> Option<String> {
        let alt = self.alternatives.get(k)?;
        let rewrites: Vec<SourceRewrite> = alt.rewrites.iter().filter(|r| r.file.is_none()).cloned().collect();
        apply_rewrites_to(source, &rewrites).ok()
    }

    fn outcome_json(&mut self, outcome: DragOutcomeV2) -> String {
        match outcome {
            DragOutcomeV2::Alternatives { alternatives, candidates } => {
                let out = json!({
                    "alternatives": alternatives.iter().map(|a| json!({
                        "verified": a.verified,
                        "movesOthers": a.moves_others,
                        "stateful": a.stateful,
                        "configOnly": a.config_only,
                        "score": a.score,
                        "description": a.description,
                        "rewrites": a.rewrites.iter().map(rewrite_json).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                    "candidates": candidates.iter().map(candidate_json).collect::<Vec<_>>(),
                });
                self.alternatives = alternatives;
                out
            }
            DragOutcomeV2::Refused { why, candidates } => {
                self.alternatives.clear();
                json!({
                    "refused": why,
                    "candidates": candidates.iter().map(candidate_json).collect::<Vec<_>>(),
                })
            }
            DragOutcomeV2::Stale => {
                self.alternatives.clear();
                json!({ "stale": true })
            }
        }
        .to_string()
    }
}

// ── Hints: `tunable(…)` / `rigid(…)` as text ──────────────────────────

/// The numeric literals of one line: `[{start, end, text, state, bound}]`,
/// `state` being `free`, `tunable` or `rigid`, and `outer` the columns a
/// click on it covers (the whole hint call, when it has one).
#[wasm_bindgen]
pub fn hint_literals(line: &str) -> String {
    let lits: Vec<Value> = dm_hints::literals(line)
        .iter()
        .map(|l| {
            let (os, oe) = l.outer();
            json!({
                "start": l.start, "end": l.end, "text": l.text,
                "state": l.state().name(), "bound": l.bound, "outer": [os, oe],
            })
        })
        .collect();
    Value::Array(lits).to_string()
}

/// Mark the literal at `col` of `line` as `to` (`free`, `tunable`, `rigid`;
/// empty cycles free → tunable → rigid → free). The rewritten line, or
/// `null` when no literal is there.
#[wasm_bindgen]
pub fn mark_hint(line: &str, col: usize, to: &str) -> Option<String> {
    let lit = dm_hints::literal_at(line, col)?;
    if lit.bound {
        return None;
    }
    let to = HintState::parse(to).unwrap_or_else(|| lit.state().next());
    Some(dm_hints::apply(line, &dm_hints::mark(&lit, to)))
}

// ── Conversions ───────────────────────────────────────────────────────

fn new_host(source: &str) -> Result<PanelHost, String> {
    let mut host = PanelHost::from_source(SKETCH_NAME, source)?;
    host.set_trace_origins(true);
    host.set_dm_checkpoint(true);
    host.set_frame_gating(true);
    Ok(host)
}

fn parse_filter(filter: &str) -> DragFilter {
    let v: Value = serde_json::from_str(filter).unwrap_or(Value::Null);
    let list = |k: &str| -> Vec<DragSelector> {
        v.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).filter_map(DragSelector::parse).collect())
            .unwrap_or_default()
    };
    DragFilter { allow: list("allow"), deny: list("deny") }
}

fn span_json(s: CodeSpan) -> Value {
    json!([s.start_line, s.start_col, s.end_line, s.end_col])
}

fn rewrite_json(r: &SourceRewrite) -> Value {
    json!({
        "span": span_json(r.span),
        "text": r.new_text,
        "file": r.file.as_ref().map(|p| p.display().to_string()),
        "variable": r.variable,
        "shared": r.shared,
        "config": r.config,
        "description": r.description,
    })
}

fn candidate_json(c: &DragCandidate) -> Value {
    json!({
        "label": c.label,
        "variable": c.variable,
        "function": c.function,
        "span": c.span.map(span_json),
        "file": c.file.as_ref().map(|p| p.display().to_string()),
        "value": c.value,
        "config": c.config,
        "tunable": c.tunable,
        "rigid": c.rigid,
        "eligible": c.eligible,
        "whyNot": c.why_not,
        "moves": c.moves,
        "movesAxes": c.moves_axes,
        "selector": c.selector.label(),
    })
}

/// A command in the `{op, …}` shape the page's renderer paints. Alpha,
/// radius and width are omitted at their defaults.
fn cmd_json(cmd: &PanelCmd) -> Value {
    let rgba = |r: u8, g: u8, b: u8, a: u8| {
        let mut m = serde_json::Map::new();
        m.insert("r".into(), r.into());
        m.insert("g".into(), g.into());
        m.insert("b".into(), b.into());
        if a != 255 {
            m.insert("a".into(), a.into());
        }
        m
    };
    let with = |op: &str, mut m: serde_json::Map<String, Value>, rest: Value| {
        m.insert("op".into(), op.into());
        if let Value::Object(o) = rest {
            for (k, v) in o {
                if !v.is_null() {
                    m.insert(k, v);
                }
            }
        }
        Value::Object(m)
    };
    let pts = |p: &[(i32, i32)]| p.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>();
    let nz = |v: u32| (v != 0).then_some(v);
    match cmd {
        PanelCmd::Clear { r, g, b } => with("clear", rgba(*r, *g, *b, 255), Value::Null),
        PanelCmd::Rect { x, y, w, h, r, g, b, a, radius } => with(
            "rect",
            rgba(*r, *g, *b, *a),
            json!({ "x": x, "y": y, "w": w, "h": h, "radius": nz(*radius) }),
        ),
        PanelCmd::RectOutline { x, y, w, h, r, g, b, a, width, radius } => with(
            "rect_outline",
            rgba(*r, *g, *b, *a),
            json!({ "x": x, "y": y, "w": w, "h": h, "width": width, "radius": nz(*radius) }),
        ),
        PanelCmd::Line { x1, y1, x2, y2, r, g, b, a, width } => with(
            "line",
            rgba(*r, *g, *b, *a),
            json!({ "x1": x1, "y1": y1, "x2": x2, "y2": y2, "width": width }),
        ),
        PanelCmd::Circle { cx, cy, radius, r, g, b, a } => {
            with("circle", rgba(*r, *g, *b, *a), json!({ "cx": cx, "cy": cy, "radius": radius }))
        }
        PanelCmd::Ellipse { cx, cy, rx, ry, r, g, b, a } => with(
            "ellipse",
            rgba(*r, *g, *b, *a),
            json!({ "cx": cx, "cy": cy, "rx": rx, "ry": ry }),
        ),
        PanelCmd::EllipseOutline { cx, cy, rx, ry, r, g, b, a, width } => with(
            "ellipse_outline",
            rgba(*r, *g, *b, *a),
            json!({ "cx": cx, "cy": cy, "rx": rx, "ry": ry, "width": width }),
        ),
        PanelCmd::Triangle { x1, y1, x2, y2, x3, y3, r, g, b, a } => with(
            "triangle",
            rgba(*r, *g, *b, *a),
            json!({ "x1": x1, "y1": y1, "x2": x2, "y2": y2, "x3": x3, "y3": y3 }),
        ),
        PanelCmd::Poly { points, r, g, b, a } | PanelCmd::Polygon { points, r, g, b, a } => {
            with("poly", rgba(*r, *g, *b, *a), json!({ "points": pts(points) }))
        }
        PanelCmd::Fan { cx, cy, points, r, g, b, a } => with(
            "fan",
            rgba(*r, *g, *b, *a),
            json!({ "cx": cx, "cy": cy, "points": pts(points) }),
        ),
        PanelCmd::Polyline { points, r, g, b, a, width } => with(
            "polyline",
            rgba(*r, *g, *b, *a),
            json!({ "points": pts(points), "width": width }),
        ),
        PanelCmd::Arc { cx, cy, r_in, r_out, a0, a1, r, g, b, a } => with(
            "arc",
            rgba(*r, *g, *b, *a),
            json!({ "cx": cx, "cy": cy, "r_in": r_in, "r_out": r_out, "a0": a0, "a1": a1 }),
        ),
        PanelCmd::Text { text, x, y, size, r, g, b, a, font, weight, italic, .. } => with(
            "text",
            rgba(*r, *g, *b, *a),
            json!({
                "text": text, "x": x, "y": y, "size": size, "font": font,
                "weight": (*weight != 400).then_some(*weight), "italic": italic.then_some(true),
            }),
        ),
        PanelCmd::RectGradient { x, y, w, h, radius, r0, g0, b0, a0, r1, g1, b1, a1, angle } => json!({
            "op": "rect_gradient", "x": x, "y": y, "w": w, "h": h, "radius": radius, "angle": angle,
            "c0": [r0, g0, b0, a0], "c1": [r1, g1, b1, a1],
        }),
        PanelCmd::CircleGradient { cx, cy, radius, r0, g0, b0, a0, r1, g1, b1, a1 } => json!({
            "op": "circle_gradient", "cx": cx, "cy": cy, "radius": radius,
            "c0": [r0, g0, b0, a0], "c1": [r1, g1, b1, a1],
        }),
        PanelCmd::Shadow { x, y, w, h, radius, blur, spread, dx, dy, r, g, b, a } => with(
            "shadow",
            rgba(*r, *g, *b, *a),
            json!({ "x": x, "y": y, "w": w, "h": h, "radius": radius, "blur": blur, "spread": spread, "dx": dx, "dy": dy }),
        ),
        PanelCmd::Clip { x, y, w, h, radius } => json!({ "op": "clip", "x": x, "y": y, "w": w, "h": h, "radius": radius }),
        PanelCmd::ClipPush { x, y, w, h, radius } => {
            json!({ "op": "clip_push", "x": x, "y": y, "w": w, "h": h, "radius": radius })
        }
        PanelCmd::ClipPop => json!({ "op": "clip_pop" }),
        PanelCmd::ClipNone => json!({ "op": "clip_none" }),
        // Offscreen canvases, images and the editor-only text views have no
        // painter in the page yet: kept in the list so indices still line up
        // with the host's, but drawn as nothing.
        _ => json!({ "op": "unsupported" }),
    }
}
