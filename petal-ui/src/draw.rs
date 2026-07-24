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

/// Buffered-output channel carrying draw commands to the renderer.
pub const DRAW_COMMANDS_SYMBOL: &str = "draw_commands";

/// Per-frame offscreen-canvas id counter (ids 1-based; 0 is the framebuffer).
pub const CANVAS_ID_COUNTER: &str = "canvas_id";

/// Uniform read by the default `text_width`: monospace advance as a fraction
/// of the font size. See [`bind_text_metrics`].
pub const SYM_TEXT_ADVANCE: &str = "text_advance";

/// Fallback advance ratio when the host hasn't bound one — a typical
/// monospace glyph advances ~0.6× the font size.
pub const DEFAULT_TEXT_ADVANCE: f64 = 0.6;

/// Per-glyph advance table read by `text_width` for proportional fonts:
/// a list of advance-÷-size ratios indexed by Unicode codepoint. When bound,
/// `text_width` sums per-glyph advances instead of `chars × size × ratio`. A
/// codepoint beyond the table's length falls back to [`SYM_TEXT_ADVANCE`].
pub const SYM_TEXT_ADVANCES: &str = "text_advances";

/// Per-font metrics read by `text_width(s, size, font)`: a record keyed by
/// font name, each value a record `{advance: float, advances: [float]}` with
/// the same meaning as the default-font [`SYM_TEXT_ADVANCE`] /
/// [`SYM_TEXT_ADVANCES`] bindings. See [`bind_font_metrics`].
pub const SYM_TEXT_FONTS: &str = "text_fonts";

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

#[derive(Serialize, PartialEq, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DrawCommand {
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
fn num_as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int(n) => Some(*n),
        Value::Float(f) => Some(*f as i64),
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
                DrawCommand::Text {
                    text,
                    x: i32_at(1)?,
                    y: i32_at(2)?,
                    size: as_i64(arg(3)?)? as u16,
                    r: u8_at(4)?,
                    g: u8_at(5)?,
                    b: u8_at(6)?,
                    a: opt_u8(7, 255),
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

/// Bind the monospace text metric read by the default `text_width` native:
/// the glyph advance as a fraction of the font size (a typical monospace at
/// size 14 advances 8.4 px → ratio 0.6). Hosts with real text shaping can
/// instead register their own `text_width` native before [`register_draw`].
pub fn bind_text_metrics(env: &mut Env, advance_ratio: f64) {
    let s = env.intern_symbol(SYM_TEXT_ADVANCE);
    env.set_binding(s, Value::Float(advance_ratio));
}

/// Bind the per-glyph advance table read by the proportional `text_width`:
/// `ratios[codepoint]` is that glyph's advance as a fraction of the font size,
/// measured by the host from its actual font. Codepoints past the table's end
/// fall back to the uniform [`bind_text_metrics`] ratio. Binding this is what
/// lets a script measure a proportional glyph run correctly (centered /
/// right-aligned layout), instead of assuming monospace.
pub fn bind_text_advance_table(env: &mut Env, ratios: &[f64]) {
    let list: Vec<Value> = ratios.iter().map(|r| Value::Float(*r)).collect();
    let id = env.heap_mut().alloc_list(list);
    let s = env.intern_symbol(SYM_TEXT_ADVANCES);
    env.set_binding(s, Value::List(id));
}

/// Measurement data for one font, as ratios of the font size (so one table
/// serves every size — glyph advance scales linearly with size).
#[derive(Clone, Debug, PartialEq)]
pub struct FontMetrics {
    /// Advance ratio used for codepoints the table doesn't cover.
    pub advance: f64,
    /// `advances[codepoint]` = that glyph's advance ÷ font size. May be empty
    /// (a monospace font is fully described by `advance` alone).
    pub advances: Vec<f64>,
}

impl Default for FontMetrics {
    /// A typical monospace face: every glyph advances 0.6× the size.
    fn default() -> Self {
        FontMetrics {
            advance: DEFAULT_TEXT_ADVANCE,
            advances: Vec::new(),
        }
    }
}

impl FontMetrics {
    /// A proportional font described by a codepoint-indexed advance table,
    /// with `advance` covering codepoints past the table's end.
    pub fn proportional(advances: Vec<f64>, advance: f64) -> Self {
        FontMetrics { advance, advances }
    }

    /// A monospace font: one advance ratio for every glyph.
    pub fn monospace(advance: f64) -> Self {
        FontMetrics {
            advance,
            advances: Vec::new(),
        }
    }

    fn width_of(&self, text: &str, size: f64) -> f64 {
        text.chars()
            .map(|c| {
                self.advances
                    .get(c as usize)
                    .copied()
                    .unwrap_or(self.advance)
                    * size
            })
            .sum()
    }
}

/// Bind measurement data for a *named* font, so a script can measure text in a
/// face other than the host's default: `text_width(s, size, "mono")`. Hosts
/// register one entry per face they can render, under the role names scripts
/// select by (`ui`, `mono`, `serif`) and/or concrete family names. The
/// unnamed default font stays with [`bind_text_metrics`] /
/// [`bind_text_advance_table`]; a name the host never bound falls back to it,
/// so a script asking for a face this host lacks degrades instead of breaking.
pub fn bind_font_metrics(env: &mut Env, font: &str, metrics: &FontMetrics) {
    let advances: Vec<Value> = metrics.advances.iter().map(|r| Value::Float(*r)).collect();
    let advances_id = env.heap_mut().alloc_list(advances);
    let mut entry = indexmap::IndexMap::new();
    entry.insert("advance".to_string(), Value::Float(metrics.advance));
    entry.insert("advances".to_string(), Value::List(advances_id));
    let entry_id = env.heap_mut().alloc_map(entry);

    let sym = env.intern_symbol(SYM_TEXT_FONTS);
    let mut fonts = match env.binding(sym) {
        Some(Value::Map(id)) => env.heap().get_map(id).clone(),
        _ => indexmap::IndexMap::new(),
    };
    fonts.insert(font.to_string(), Value::Map(entry_id));
    let fonts_id = env.heap_mut().alloc_map(fonts);
    env.set_binding(sym, Value::Map(fonts_id));
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

// `draw_text(text, x, y, size, r, g, b, [a])`.
fn native_draw_text(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let a = opt_int(state, 8, 255)?;
    let args = vec![
        Value::String(state.heap_mut().alloc_string(text)),
        Value::Int(state.get_int(2)?),
        Value::Int(state.get_int(3)?),
        Value::Int(state.get_int(4)?),
        Value::Int(state.get_int(5)?),
        Value::Int(state.get_int(6)?),
        Value::Int(state.get_int(7)?),
        Value::Int(a),
    ];
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

/// Read a numeric `Value` as f64, or `default` if it isn't a number.
fn num_or(v: &Value, default: f64) -> f64 {
    match v {
        Value::Float(f) => *f,
        Value::Int(n) => *n as f64,
        _ => default,
    }
}

/// Decode a list `Value` of advance ratios into a table.
fn advance_list(state: &mut PetalCxt, v: &Value, uniform: f64) -> Option<Vec<f64>> {
    match v {
        Value::List(id) => Some(
            state
                .heap()
                .get_list(*id)
                .iter()
                .map(|r| num_or(r, uniform))
                .collect(),
        ),
        _ => None,
    }
}

/// The host's default-font metrics — the [`bind_text_metrics`] /
/// [`bind_text_advance_table`] bindings every host has always used.
fn default_font_metrics(state: &mut PetalCxt) -> FontMetrics {
    let uniform = num_or(&state.binding_named(SYM_TEXT_ADVANCE), DEFAULT_TEXT_ADVANCE);
    let table = state.binding_named(SYM_TEXT_ADVANCES);
    let advances = advance_list(state, &table, uniform);
    FontMetrics {
        advance: uniform,
        advances: advances.unwrap_or_default(),
    }
}

/// Resolve a font spec — a name or a CSS-style fallback list (`"Inter, ui"`) —
/// against the [`bind_font_metrics`] registry. The first name the host bound
/// wins; if none did, the caller falls back to the default font.
fn named_font_metrics(state: &mut PetalCxt, spec: &str) -> Option<FontMetrics> {
    let fonts = match state.binding_named(SYM_TEXT_FONTS) {
        Value::Map(id) => state.heap().get_map(id).clone(),
        _ => return None,
    };
    for name in spec.split(',') {
        let Some(Value::Map(entry_id)) = fonts.get(name.trim()) else {
            continue;
        };
        let entry = state.heap().get_map(*entry_id).clone();
        let advance = entry
            .get("advance")
            .map_or(DEFAULT_TEXT_ADVANCE, |v| num_or(v, DEFAULT_TEXT_ADVANCE));
        let advances = entry
            .get("advances")
            .cloned()
            .and_then(|v| advance_list(state, &v, advance));
        return Some(FontMetrics {
            advance,
            advances: advances.unwrap_or_default(),
        });
    }
    None
}

/// `text_width(s, size, [font]) -> int`: width in logical px of `s` at font
/// `size`. If the host bound a per-glyph advance table
/// ([`bind_text_advance_table`]), the width is the sum of each glyph's advance
/// × `size` — correct for proportional fonts. Otherwise it falls back to the
/// monospace model `chars × size × ratio`, with the ratio from
/// [`bind_text_metrics`] (default 0.6).
///
/// The optional `font` selects a face registered with [`bind_font_metrics`],
/// by role name or CSS-style fallback list (`"Inter, ui"`). A face this host
/// doesn't offer measures with the default font.
fn native_text_width(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let size = state.get_int(2)? as f64;

    let metrics = match state.arg_count() >= 3 {
        true => {
            let spec = state.get_string(3)?;
            named_font_metrics(state, &spec).unwrap_or_else(|| default_font_metrics(state))
        }
        false => default_font_metrics(state),
    };

    state.push_int(metrics.width_of(&text, size).round() as i64);
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
    fn text_width_uses_bound_ratio() {
        let mut env = Env::new();
        register_draw(&mut env);
        // Default ratio 0.6: 5 chars at size 10 → 30.
        let v = env.run_source("text_width(\"hello\", 10)").expect("run");
        assert_eq!(v, Value::Int(30));
        // Typical monospace metric: ratio 0.6 at size 14 → 8.4 px/char.
        bind_text_metrics(&mut env, 0.6);
        let v = env.run_source("text_width(\"abc\", 14)").expect("run");
        assert_eq!(v, Value::Int(25)); // 3 × 14 × 0.6 = 25.2 → 25
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

    #[test]
    fn text_width_uses_advance_table_when_bound() {
        let mut env = Env::new();
        register_draw(&mut env);
        // A proportional table: 'i' is narrow, 'W' is wide; everything else 0.6.
        let mut ratios = vec![0.6f64; 128];
        ratios['i' as usize] = 0.2;
        ratios['W' as usize] = 0.9;
        bind_text_advance_table(&mut env, &ratios);

        // Per-glyph sum, not chars × uniform: 3 × 10 × 0.2 = 6, 3 × 10 × 0.9 = 27.
        let narrow = env.run_source("text_width(\"iii\", 10)").expect("run");
        let wide = env.run_source("text_width(\"WWW\", 10)").expect("run");
        assert_eq!(narrow, Value::Int(6));
        assert_eq!(wide, Value::Int(27));
        assert!(
            narrow != wide,
            "a proportional font must measure 'iii' and 'WWW' differently"
        );
    }

    #[test]
    fn text_width_measures_a_named_font() {
        let mut env = Env::new();
        register_draw(&mut env);
        // Default font: proportional, narrow 'i'. A second face, "mono", is
        // registered by name.
        let mut ratios = vec![0.6f64; 128];
        ratios['i' as usize] = 0.2;
        bind_text_advance_table(&mut env, &ratios);
        bind_font_metrics(&mut env, "mono", &FontMetrics::monospace(0.5));

        // Two args → default (proportional) font: 3 × 10 × 0.2 = 6.
        let v = env.run_source("text_width(\"iii\", 10)").expect("run");
        assert_eq!(v, Value::Int(6));
        // Three args → the named face: 3 × 10 × 0.5 = 15.
        let v = env.run_source("text_width(\"iii\", 10, \"mono\")").expect("run");
        assert_eq!(v, Value::Int(15));
        // A fallback list picks the first registered name.
        let v = env
            .run_source("text_width(\"iii\", 10, \"Inter, mono\")")
            .expect("run");
        assert_eq!(v, Value::Int(15));
        // A face this host never bound degrades to the default font.
        let v = env.run_source("text_width(\"iii\", 10, \"serif\")").expect("run");
        assert_eq!(v, Value::Int(6));
    }

    #[test]
    fn named_fonts_accumulate_and_carry_tables() {
        let mut env = Env::new();
        register_draw(&mut env);
        let mut ui = vec![0.6f64; 128];
        ui['W' as usize] = 1.0;
        bind_font_metrics(&mut env, "ui", &FontMetrics::proportional(ui, 0.6));
        // A second registration must not drop the first.
        bind_font_metrics(&mut env, "mono", &FontMetrics::monospace(0.5));

        let v = env.run_source("text_width(\"WW\", 10, \"ui\")").expect("run");
        assert_eq!(v, Value::Int(20));
        let v = env.run_source("text_width(\"WW\", 10, \"mono\")").expect("run");
        assert_eq!(v, Value::Int(10));
    }

    #[test]
    fn text_width_advance_table_falls_back_for_untabled_chars() {
        let mut env = Env::new();
        register_draw(&mut env);
        // Table only covers a few ASCII slots; a char beyond its length uses the
        // uniform ratio (default 0.6).
        let ratios = vec![0.3f64; 65]; // covers up to 'A' - 1
        bind_text_advance_table(&mut env, &ratios);
        // 'Z' (0x5A) is past the table → uniform 0.6: 2 × 10 × 0.6 = 12.
        let v = env.run_source("text_width(\"ZZ\", 10)").expect("run");
        assert_eq!(v, Value::Int(12));
    }
}
