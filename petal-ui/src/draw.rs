//! Layer 2: the standard draw-command vocabulary.
//!
//! Draw natives don't render — they `emit` a tagged command into the
//! `draw_commands` output buffer; the host drains it after the run with
//! [`take_draw_commands`] and rasterizes. The vocabulary is a shared default,
//! not a ceiling:
//!
//! - Hosts may ignore commands they don't support (e.g. a host without
//!   offscreen render targets skips the canvas ops — which is why
//!   [`register_canvas`] is separate from [`register_draw`]).
//! - Hosts may register extra natives that `emit` their own tags into the
//!   same buffer; those decode as [`DrawCommand::Host`] and keep their place
//!   in the command order.
//!
//! Coordinates are logical pixels, `(0, 0)` at the drawable's top-left.
//! Colors are 0–255 sRGB components.

use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::stack::StackKey;
use petal::value::Value;
use serde::Serialize;

use crate::text::native_text_width;

/// Buffered-output channel carrying draw commands to the renderer.
pub const DRAW_COMMANDS_SYMBOL: &str = "draw_commands";

/// Per-frame offscreen-canvas id counter (ids 1-based; 0 is the framebuffer).
pub const CANVAS_ID_COUNTER: &str = "canvas_id";

/// The font metrics and text measurement subsystem lives in [`crate::text`];
/// it is re-exported here so a host reaches the whole draw contract — the
/// commands and the measurements that feed them — through one path.
pub use crate::text::{
    DEFAULT_TEXT_ADVANCE, DEFAULT_TEXT_SIZE, FontMetrics, REGULAR_WEIGHT, SYM_TEXT_ADVANCE,
    SYM_TEXT_ADVANCES, SYM_TEXT_DEFAULT_FONT, SYM_TEXT_FONTS, TextStyle, bind_default_font_name,
    bind_font_metrics, bind_font_variant_metrics, bind_text_advance_table, bind_text_metrics,
    font_variant_key,
};

/// `skip_serializing_if` predicates that keep the JSON identical to the
/// pre-alpha shape when a primitive is opaque / square-cornered / hairline, so
/// existing draw-command consumers see no change unless a feature is used.
/// (The enum has no `Deserialize`, so no matching `default` fns are needed.)
fn is_opaque(a: &u8) -> bool {
    *a == 255
}
fn is_zero(v: &u32) -> bool {
    *v == 0
}
fn is_one(v: &u32) -> bool {
    *v == 1
}
fn is_regular(w: &u16) -> bool {
    *w == REGULAR_WEIGHT
}
fn is_upright(i: &bool) -> bool {
    !*i
}
fn is_no_spacing(s: &f32) -> bool {
    *s == 0.0
}

#[derive(Serialize, PartialEq, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DrawCommand {
    /// A bitmap asset scaled into a destination rectangle.
    ///
    /// `source` is a host-resolved asset path or logical asset name.
    Image {
        source: String,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    Clear {
        r: u8,
        g: u8,
        b: u8,
    },
    Rect {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        r: u8,
        g: u8,
        b: u8,
        /// Opacity 0–255 (255 = opaque).
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        /// Corner radius in px; 0 = square corners.
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: u32,
    },
    RectOutline {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        /// Stroke width in px (1 = hairline).
        #[serde(skip_serializing_if = "is_one")]
        width: u32,
    },
    Line {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_one")]
        width: u32,
    },
    Circle {
        cx: i32,
        cy: i32,
        radius: i32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    Text {
        text: String,
        x: i32,
        y: i32,
        size: u16,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        /// The face to render in: a role (`ui`, `mono`, `serif`) or a
        /// CSS-style fallback list (`"Inter, ui"`). `None` = the host's
        /// default font, which is what every pre-typography command means.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        font: Option<String>,
        /// CSS numeric weight, 100–900; 400 is regular, 700 bold. A host with
        /// only one weight renders it as-is.
        #[serde(default, skip_serializing_if = "is_regular")]
        weight: u16,
        #[serde(default, skip_serializing_if = "is_upright")]
        italic: bool,
        /// Letter-spacing in px, added after every glyph (CSS semantics).
        /// Negative tightens.
        #[serde(default, skip_serializing_if = "is_no_spacing")]
        spacing: f32,
    },
    Triangle {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        x3: i32,
        y3: i32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    Poly {
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    /// Restrict subsequent drawing to a rectangle (intersected with the
    /// drawable). Cleared by [`DrawCommand::ClipNone`].
    Clip {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    },
    ClipNone,
    /// Allocate an offscreen canvas (render target) of size `w`×`h`,
    /// identified by `id`. Canvases are transparent until drawn into and are
    /// recreated fresh each frame from the command stream. Optional — hosts
    /// without render targets ignore the three canvas ops.
    CreateCanvas {
        id: u32,
        w: u32,
        h: u32,
    },
    /// Redirect subsequent draw commands to a render target. `id == 0` is
    /// the main framebuffer; any other `id` is an offscreen canvas.
    SetTarget {
        id: u32,
    },
    /// Blit an offscreen canvas onto the current render target at (`x`, `y`).
    DrawCanvas {
        id: u32,
        x: i32,
        y: i32,
    },
    /// A host-registered extension command: an unrecognized tag passes
    /// through in order with its raw args (heap-backed; decode them before
    /// mutating the Env). Not included when serializing a command list.
    Host {
        tag: String,
        #[serde(skip)]
        data: Vec<Value>,
    },
}

/// Read a numeric `Value` (int or float) as i64, or `None` if non-numeric —
/// used for optional trailing draw-command args.
pub(crate) fn num_as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int(n) => Some(*n),
        Value::Float(f) => Some(*f as i64),
        _ => None,
    }
}

/// Read an optional numeric `Value` as f64 — for fractional args (letter
/// spacing), where truncating to an integer would lose the point.
pub(crate) fn num_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// Read a numeric `Value` (int or float) as i64.
fn as_i64(v: &Value) -> Result<i64, String> {
    match v {
        Value::Int(n) => Ok(*n),
        Value::Float(f) => Ok(*f as i64),
        other => Err(format!(
            "expected number in draw command, got {}",
            other.type_name()
        )),
    }
}

impl DrawCommand {
    /// A text command in the host's own font — regular, upright, unspaced:
    /// what every `text` command meant before typography, and what hosts and
    /// tests want when they synthesize one by hand.
    pub fn plain_text(
        text: impl Into<String>,
        x: i32,
        y: i32,
        size: u16,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    ) -> DrawCommand {
        DrawCommand::Text {
            text: text.into(),
            x,
            y,
            size,
            r,
            g,
            b,
            a,
            font: None,
            weight: REGULAR_WEIGHT,
            italic: false,
            spacing: 0.0,
        }
    }

    /// Decode a draw command from a buffered-output `Value`. Native draw
    /// functions push each command as `Value::EnumVariant { tag, data }`
    /// where `data` is a flat list of arguments; this is the inverse mapping
    /// hosts use when draining the buffer. Unknown tags decode as
    /// [`DrawCommand::Host`].
    pub fn from_value(val: &Value, heap: &Heap) -> Result<DrawCommand, String> {
        let (tag, data) = match val {
            Value::EnumVariant { tag, data } => (
                heap.get_string(*tag).to_string(),
                heap.get_list(*data).to_vec(),
            ),
            other => {
                return Err(format!(
                    "draw command must be an enum value, got {}",
                    other.type_name()
                ));
            }
        };

        let arg = |i: usize| -> Result<&Value, String> {
            data.get(i)
                .ok_or_else(|| format!("draw command '{tag}' missing arg {i}"))
        };
        let i32_at = |i: usize| -> Result<i32, String> { Ok(as_i64(arg(i)?)? as i32) };
        let u32_at = |i: usize| -> Result<u32, String> { Ok(as_i64(arg(i)?)? as u32) };
        let u8_at = |i: usize| -> Result<u8, String> { Ok(as_i64(arg(i)?)? as u8) };
        // Optional trailing args (alpha / radius / width) — absent means the
        // caller used the short form, so fall back to the default. This keeps
        // scripts that emit the pre-alpha arg lists working unchanged.
        let opt_u8 = |i: usize, default: u8| -> u8 {
            data.get(i)
                .and_then(num_as_i64)
                .map_or(default, |n| n as u8)
        };
        let opt_u32 = |i: usize, default: u32| -> u32 {
            data.get(i)
                .and_then(num_as_i64)
                .map_or(default, |n| n as u32)
        };

        let cmd = match tag.as_str() {
            "image" => {
                let source = match arg(0)? {
                    Value::String(id) => heap.get_string(*id).to_string(),
                    other => {
                        return Err(format!(
                            "image command needs a source string, got {}",
                            other.type_name()
                        ));
                    }
                };
                DrawCommand::Image {
                    source,
                    x: i32_at(1)?,
                    y: i32_at(2)?,
                    w: u32_at(3)?,
                    h: u32_at(4)?,
                    a: opt_u8(5, 255),
                }
            }
            "clear" => DrawCommand::Clear {
                r: u8_at(0)?,
                g: u8_at(1)?,
                b: u8_at(2)?,
            },
            "rect" => DrawCommand::Rect {
                x: i32_at(0)?,
                y: i32_at(1)?,
                w: u32_at(2)?,
                h: u32_at(3)?,
                r: u8_at(4)?,
                g: u8_at(5)?,
                b: u8_at(6)?,
                a: opt_u8(7, 255),
                radius: opt_u32(8, 0),
            },
            "rect_outline" => DrawCommand::RectOutline {
                x: i32_at(0)?,
                y: i32_at(1)?,
                w: u32_at(2)?,
                h: u32_at(3)?,
                r: u8_at(4)?,
                g: u8_at(5)?,
                b: u8_at(6)?,
                a: opt_u8(7, 255),
                width: opt_u32(8, 1),
            },
            "line" => DrawCommand::Line {
                x1: i32_at(0)?,
                y1: i32_at(1)?,
                x2: i32_at(2)?,
                y2: i32_at(3)?,
                r: u8_at(4)?,
                g: u8_at(5)?,
                b: u8_at(6)?,
                a: opt_u8(7, 255),
                width: opt_u32(8, 1),
            },
            "circle" => DrawCommand::Circle {
                cx: i32_at(0)?,
                cy: i32_at(1)?,
                radius: i32_at(2)?,
                r: u8_at(3)?,
                g: u8_at(4)?,
                b: u8_at(5)?,
                a: opt_u8(6, 255),
            },
            "triangle" => DrawCommand::Triangle {
                x1: i32_at(0)?,
                y1: i32_at(1)?,
                x2: i32_at(2)?,
                y2: i32_at(3)?,
                x3: i32_at(4)?,
                y3: i32_at(5)?,
                r: u8_at(6)?,
                g: u8_at(7)?,
                b: u8_at(8)?,
                a: opt_u8(9, 255),
            },
            "poly" => {
                let points_id = match arg(0)? {
                    Value::List(id) => *id,
                    other => {
                        return Err(format!(
                            "poly points must be a list, got {}",
                            other.type_name()
                        ));
                    }
                };
                let mut points = Vec::new();
                for p in heap.get_list(points_id) {
                    match p {
                        Value::Vec2(x, y) => points.push((*x as i32, *y as i32)),
                        Value::List(pid) => {
                            let coords = heap.get_list(*pid);
                            points.push((as_i64(&coords[0])? as i32, as_i64(&coords[1])? as i32));
                        }
                        other => {
                            return Err(format!(
                                "poly point must be vec2 or [x, y], got {}",
                                other.type_name()
                            ));
                        }
                    }
                }
                DrawCommand::Poly {
                    points,
                    r: u8_at(1)?,
                    g: u8_at(2)?,
                    b: u8_at(3)?,
                    a: opt_u8(4, 255),
                }
            }
            "text" => {
                let text = match arg(0)? {
                    Value::String(id) => heap.get_string(*id).to_string(),
                    other => {
                        return Err(format!(
                            "text command needs a string, got {}",
                            other.type_name()
                        ));
                    }
                };
                // Args 8–11 are the typography extension: a command emitted by
                // a pre-typography script simply stops at the alpha, and every
                // field below keeps its "the host's one font, upright, regular"
                // default — the same thing that command has always meant.
                DrawCommand::Text {
                    text,
                    x: i32_at(1)?,
                    y: i32_at(2)?,
                    size: as_i64(arg(3)?)? as u16,
                    r: u8_at(4)?,
                    g: u8_at(5)?,
                    b: u8_at(6)?,
                    a: opt_u8(7, 255),
                    font: match data.get(8) {
                        Some(Value::String(id)) => Some(heap.get_string(*id).to_string()),
                        _ => None,
                    },
                    weight: data
                        .get(9)
                        .and_then(num_as_i64)
                        .map_or(REGULAR_WEIGHT, |n| n as u16),
                    italic: matches!(data.get(10), Some(Value::Bool(true))),
                    spacing: data.get(11).and_then(num_as_f64).unwrap_or(0.0) as f32,
                }
            }
            "clip" => DrawCommand::Clip {
                x: i32_at(0)?,
                y: i32_at(1)?,
                w: u32_at(2)?,
                h: u32_at(3)?,
            },
            "clip_none" => DrawCommand::ClipNone,
            "create_canvas" => DrawCommand::CreateCanvas {
                id: u32_at(0)?,
                w: u32_at(1)?,
                h: u32_at(2)?,
            },
            "set_target" => DrawCommand::SetTarget { id: u32_at(0)? },
            "draw_canvas" => DrawCommand::DrawCanvas {
                id: u32_at(0)?,
                x: i32_at(1)?,
                y: i32_at(2)?,
            },
            _ => DrawCommand::Host { tag, data },
        };
        Ok(cmd)
    }
}

// ── Host-side: drain / clear ──────────────────────────────────────────────

/// Drain the `draw_commands` output buffer and decode it into a renderable
/// command list. Malformed commands are skipped (logged to stderr).
pub fn take_draw_commands(env: &mut Env) -> Vec<DrawCommand> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer(sym);
    let mut out = Vec::with_capacity(values.len());
    for v in &values {
        match DrawCommand::from_value(v, env.heap()) {
            Ok(cmd) => out.push(cmd),
            Err(e) => eprintln!("[petal-ui draw] {}", e),
        }
    }
    out
}

/// Discard any buffered draw commands (defensive clear at the top of a frame).
pub fn clear_draw_commands(env: &mut Env) {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    env.clear_output_buffer(sym);
}

/// [`take_draw_commands`] for a *forked* stack: a fork's draw commands — and
/// the heap objects (string tags, list args) they reference — live in the
/// fork's context, so both the drain and the decode target `stack_id`'s heap.
pub fn take_draw_commands_for(env: &mut Env, stack_id: StackKey) -> Vec<DrawCommand> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer_for(stack_id, sym);
    let heap = match env.heap_for(stack_id) {
        Some(h) => h,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(values.len());
    for v in &values {
        match DrawCommand::from_value(v, heap) {
            Ok(cmd) => out.push(cmd),
            Err(e) => eprintln!("[petal-ui draw] {}", e),
        }
    }
    out
}

/// Reset the per-frame offscreen-canvas id counter so `create_canvas` hands
/// out stable ids each frame. Call before each run (only needed with
/// [`register_canvas`]).
pub fn reset_canvas_ids(env: &mut Env) {
    let c = env.intern_symbol(CANVAS_ID_COUNTER);
    env.reset_counter(c, 1);
}

// ── Script-side: the standard draw natives ───────────────────────────────

/// Register the core draw natives (everything except the optional offscreen
/// canvas ops — see [`register_canvas`]).
pub fn register_draw(env: &mut Env) {
    env.register_native("draw_image", native_draw_image);
    env.register_native("clear", native_clear);
    env.register_native("draw_rect", native_draw_rect);
    env.register_native("draw_rect_rounded", native_draw_rect_rounded);
    env.register_native("draw_rect_outline", native_draw_rect_outline);
    env.register_native("draw_line", native_draw_line);
    env.register_native("draw_circle", native_draw_circle);
    env.register_native("fill_triangle", native_fill_triangle);
    env.register_native("fill_poly", native_fill_poly);
    env.register_native("draw_text", native_draw_text);
    env.register_native("clip", native_clip);
    env.register_native("clip_none", native_clip_none);
    env.register_native("text_width", native_text_width);
}

/// Register the optional offscreen-canvas natives (`create_canvas`,
/// `draw_to`, `draw_to_screen`, `draw_canvas`). Hosts that register these
/// must handle the canvas commands and call [`reset_canvas_ids`] per frame.
pub fn register_canvas(env: &mut Env) {
    env.register_native("create_canvas", native_create_canvas);
    env.register_native("draw_to", native_draw_to);
    env.register_native("draw_to_screen", native_draw_to_screen);
    env.register_native("draw_canvas", native_draw_canvas);
}

/// Emit a draw command into the `draw_commands` output buffer.
pub fn emit_draw(state: &mut PetalCxt, tag: &str, data: Vec<Value>) {
    let sym = state.intern_symbol(DRAW_COMMANDS_SYMBOL);
    state.emit(sym, tag, data);
}

/// Collect the first `n` arguments (1-indexed) as integer `Value`s — the
/// common shape for draw commands whose arguments are all numbers.
fn int_args(state: &PetalCxt, n: usize) -> Result<Vec<Value>, String> {
    (1..=n).map(|i| state.get_int(i).map(Value::Int)).collect()
}

/// Read an optional 1-indexed integer arg, or `default` if the caller omitted
/// it — how the draw natives accept trailing alpha / width without breaking
/// callers that use the short (opaque, hairline) form.
fn opt_int(state: &PetalCxt, index: usize, default: i64) -> Result<i64, String> {
    if state.arg_count() >= index {
        state.get_int(index)
    } else {
        Ok(default)
    }
}

fn native_clear(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 3)?;
    emit_draw(state, "clear", args);
    state.push_nil();
    Ok(1)
}

// `draw_rect(x, y, w, h, r, g, b, [a])` — trailing alpha is optional (opaque).
/// `draw_image(source, x, y, w, h, [alpha])` — draw a host-resolved bitmap.
fn native_draw_image(state: &mut PetalCxt) -> NativeResult {
    let source = state.get_string(1)?;
    let source = Value::String(state.heap_mut().alloc_string(source));
    let mut args = vec![source];
    for index in 2..=5 {
        args.push(Value::Int(state.get_int(index)?));
    }
    args.push(Value::Int(opt_int(state, 6, 255)?));
    emit_draw(state, "image", args);
    state.push_nil();
    Ok(1)
}

fn native_draw_rect(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 7)?;
    args.push(Value::Int(opt_int(state, 8, 255)?)); // a
    emit_draw(state, "rect", args);
    state.push_nil();
    Ok(1)
}

// `draw_rect_rounded(x, y, w, h, radius, r, g, b, [a])`. Emits a `rect` with a
// corner radius — the same extended variant, so hosts without rounded support
// still draw a square rect.
fn native_draw_rect_rounded(state: &mut PetalCxt) -> NativeResult {
    let x = state.get_int(1)?;
    let y = state.get_int(2)?;
    let w = state.get_int(3)?;
    let h = state.get_int(4)?;
    let radius = state.get_int(5)?;
    let r = state.get_int(6)?;
    let g = state.get_int(7)?;
    let b = state.get_int(8)?;
    let a = opt_int(state, 9, 255)?;
    emit_draw(
        state,
        "rect",
        vec![
            Value::Int(x),
            Value::Int(y),
            Value::Int(w),
            Value::Int(h),
            Value::Int(r),
            Value::Int(g),
            Value::Int(b),
            Value::Int(a),
            Value::Int(radius),
        ],
    );
    state.push_nil();
    Ok(1)
}

// `draw_rect_outline(x, y, w, h, r, g, b, [a], [width])`.
fn native_draw_rect_outline(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 7)?;
    args.push(Value::Int(opt_int(state, 8, 255)?)); // a
    args.push(Value::Int(opt_int(state, 9, 1)?)); // width
    emit_draw(state, "rect_outline", args);
    state.push_nil();
    Ok(1)
}

// `draw_line(x1, y1, x2, y2, r, g, b, [a], [width])`.
fn native_draw_line(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 7)?;
    args.push(Value::Int(opt_int(state, 8, 255)?)); // a
    args.push(Value::Int(opt_int(state, 9, 1)?)); // width
    emit_draw(state, "line", args);
    state.push_nil();
    Ok(1)
}

// `draw_circle(cx, cy, radius, r, g, b, [a])`.
fn native_draw_circle(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 6)?;
    args.push(Value::Int(opt_int(state, 7, 255)?)); // a
    emit_draw(state, "circle", args);
    state.push_nil();
    Ok(1)
}

// `fill_triangle(x1, y1, x2, y2, x3, y3, r, g, b, [a])`.
fn native_fill_triangle(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 9)?;
    args.push(Value::Int(opt_int(state, 10, 255)?)); // a
    emit_draw(state, "triangle", args);
    state.push_nil();
    Ok(1)
}

fn coord_to_i32(v: &Value) -> Result<i32, String> {
    match v {
        Value::Int(n) => Ok(*n as i32),
        Value::Float(f) => Ok(*f as i32),
        _ => Err("fill_poly() point coords must be numbers".to_string()),
    }
}

fn native_fill_poly(state: &mut PetalCxt) -> NativeResult {
    let points_value = state.get_value(1)?;
    let list_id = match points_value {
        Value::List(id) => id,
        other => {
            return Err(format!(
                "fill_poly() expects a list of points, got {}",
                other.type_name()
            ));
        }
    };

    // Validate the points up front (the renderer re-reads the list on decode).
    let elements: Vec<Value> = state.heap().get_list(list_id).to_vec();
    for el in &elements {
        match el {
            Value::Vec2(_, _) => {}
            Value::List(pid) => {
                let coords = state.heap().get_list(*pid);
                if coords.len() != 2 {
                    return Err(
                        "fill_poly() list points must have exactly 2 coords [x, y]".to_string()
                    );
                }
                coord_to_i32(&coords[0])?;
                coord_to_i32(&coords[1])?;
            }
            other => {
                return Err(format!(
                    "fill_poly() points must be vec2 or [x, y] lists, got {}",
                    other.type_name()
                ));
            }
        }
    }

    if elements.len() < 3 {
        return Err("fill_poly() needs at least 3 points".to_string());
    }

    let r = state.get_int(2)?;
    let g = state.get_int(3)?;
    let b = state.get_int(4)?;
    let a = opt_int(state, 5, 255)?;

    emit_draw(
        state,
        "poly",
        vec![
            points_value,
            Value::Int(r),
            Value::Int(g),
            Value::Int(b),
            Value::Int(a),
        ],
    );
    state.push_nil();
    Ok(1)
}

/// `draw_text(text, x, y, size, r, g, b, [a])` — the flat form every host has
/// always had — or `draw_text(text, x, y, style)`, where `style` is a
/// [`TextStyle`] record naming a face, weight, italic, and letter-spacing.
/// The prelude wraps the styled form as `draw_text(text, pos, style)`.
fn native_draw_text(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let x = state.get_int(2)?;
    let y = state.get_int(3)?;

    let styled = state.arg_count() == 4 && matches!(state.get_value(4)?, Value::Map(_));
    let args = match styled {
        true => {
            let style = TextStyle::from_value(state, &state.get_value(4)?)?;
            style.emit_args(state, text, x, y)
        }
        false => {
            let style = TextStyle {
                size: state.get_int(4)?,
                r: state.get_int(5)? as u8,
                g: state.get_int(6)? as u8,
                b: state.get_int(7)? as u8,
                a: opt_int(state, 8, 255)? as u8,
                ..TextStyle::default()
            };
            style.emit_args(state, text, x, y)
        }
    };
    emit_draw(state, "text", args);
    state.push_nil();
    Ok(1)
}

fn native_clip(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 4)?;
    emit_draw(state, "clip", args);
    state.push_nil();
    Ok(1)
}

fn native_clip_none(state: &mut PetalCxt) -> NativeResult {
    emit_draw(state, "clip_none", vec![]);
    state.push_nil();
    Ok(1)
}

fn native_create_canvas(state: &mut PetalCxt) -> NativeResult {
    let w = state.get_int(1)?;
    let h = state.get_int(2)?;
    let sym = state.intern_symbol(CANVAS_ID_COUNTER);
    let id = state.next_counter(sym) as i64;
    emit_draw(
        state,
        "create_canvas",
        vec![Value::Int(id), Value::Int(w), Value::Int(h)],
    );
    state.push_int(id);
    Ok(1)
}

fn native_draw_to(state: &mut PetalCxt) -> NativeResult {
    let id = state.get_int(1)?;
    emit_draw(state, "set_target", vec![Value::Int(id)]);
    state.push_nil();
    Ok(1)
}

fn native_draw_to_screen(state: &mut PetalCxt) -> NativeResult {
    emit_draw(state, "set_target", vec![Value::Int(0)]);
    state.push_nil();
    Ok(1)
}

fn native_draw_canvas(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 3)?;
    emit_draw(state, "draw_canvas", args);
    state.push_nil();
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_and_unknown_tags_decode() {
        let mut env = Env::new();
        register_draw(&mut env);
        env.register_native("host_marker", |state| {
            emit_draw(state, "marker", vec![Value::Int(7)]);
            state.push_nil();
            Ok(1)
        });
        env.run_source("clip(1, 2, 30, 40)\nhost_marker()\nclip_none()")
            .expect("run_source");
        let cmds = take_draw_commands(&mut env);
        assert_eq!(cmds.len(), 3);
        assert_eq!(
            cmds[0],
            DrawCommand::Clip {
                x: 1,
                y: 2,
                w: 30,
                h: 40
            }
        );
        match &cmds[1] {
            DrawCommand::Host { tag, data } => {
                assert_eq!(tag, "marker");
                assert_eq!(data, &vec![Value::Int(7)]);
            }
            other => panic!("expected Host command, got {other:?}"),
        }
        assert_eq!(cmds[2], DrawCommand::ClipNone);
    }

    #[test]
    fn image_source_geometry_and_alpha_decode() {
        let mut env = Env::new();
        register_draw(&mut env);
        env.run_source(
            "draw_image(\"assets/gauge.png\", 1, 2, 30, 40)\n\
             draw_image(\"assets/glow.png\", 5, 6, 70, 80, 128)",
        )
        .expect("run");
        assert_eq!(
            take_draw_commands(&mut env),
            vec![
                DrawCommand::Image {
                    source: "assets/gauge.png".into(),
                    x: 1,
                    y: 2,
                    w: 30,
                    h: 40,
                    a: 255,
                },
                DrawCommand::Image {
                    source: "assets/glow.png".into(),
                    x: 5,
                    y: 6,
                    w: 70,
                    h: 80,
                    a: 128,
                },
            ]
        );
    }

    #[test]
    fn rect_alpha_and_radius_decode() {
        let mut env = Env::new();
        register_draw(&mut env);
        // Opaque short form: no alpha, square corners.
        env.run_source("draw_rect(0, 0, 10, 10, 1, 2, 3)")
            .expect("run");
        // Translucent long form.
        env.run_source("draw_rect(0, 0, 10, 10, 1, 2, 3, 128)")
            .expect("run");
        // Rounded via the convenience native (radius 6, alpha 200).
        env.run_source("draw_rect_rounded(0, 0, 10, 10, 6, 1, 2, 3, 200)")
            .expect("run");
        let cmds = take_draw_commands(&mut env);
        assert_eq!(
            cmds[0],
            DrawCommand::Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                r: 1,
                g: 2,
                b: 3,
                a: 255,
                radius: 0
            }
        );
        assert_eq!(
            cmds[1],
            DrawCommand::Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                r: 1,
                g: 2,
                b: 3,
                a: 128,
                radius: 0
            }
        );
        assert_eq!(
            cmds[2],
            DrawCommand::Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                r: 1,
                g: 2,
                b: 3,
                a: 200,
                radius: 6
            }
        );
    }

    #[test]
    fn opaque_defaults_are_not_serialized() {
        // An opaque, square, hairline primitive must serialize to the exact
        // pre-alpha JSON shape (no `a`/`radius`/`width`) so existing consumers
        // and the protocol docs stay valid.
        let cmd = DrawCommand::Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
            r: 5,
            g: 6,
            b: 7,
            a: 255,
            radius: 0,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"op":"rect","x":1,"y":2,"w":3,"h":4,"r":5,"g":6,"b":7}"#
        );
        // But a translucent rounded rect includes the extra fields.
        let cmd = DrawCommand::Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
            r: 5,
            g: 6,
            b: 7,
            a: 128,
            radius: 8,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(
            json.contains(r#""a":128"#) && json.contains(r#""radius":8"#),
            "{json}"
        );
    }
}
