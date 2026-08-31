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
use petal::execution_context::EmitSite;
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
    DEFAULT_TEXT_ADVANCE, DEFAULT_TEXT_SIZE, FontMetrics, FontProvider, FontSource, REGULAR_WEIGHT,
    SYM_TEXT_ADVANCE, SYM_TEXT_ADVANCES, SYM_TEXT_DEFAULT_FONT, SYM_TEXT_FONTS, TextStyle,
    bind_default_font_name, bind_font_metrics, bind_font_variant_metrics, bind_text_advance_table,
    bind_text_metrics, clear_font_cache, font_variant_key, swap_font_provider,
};

// The two natives `register_draw` installs from that module; not re-exported,
// since a host registers them through `register_draw` rather than by hand.
use crate::text::{native_font, native_fonts};

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
fn is_zero_i32(v: &i32) -> bool {
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
        /// Corner radius in px; 0 = square corners. A host that can't round a
        /// bitmap draws the square image, which is what this command has
        /// always meant.
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: u32,
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
        /// Corner radius in px; 0 = square corners. A host that can't round a
        /// stroke draws the square outline, which is what this command has
        /// always meant.
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: u32,
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
    /// A filled **simple** polygon — concave allowed. Where [`Poly`](Self::Poly)
    /// is a fan from the first vertex (correct only for convex outlines), this
    /// is triangulated properly, so a star, an L, or an arrowhead fills the
    /// region its outline encloses.
    Polygon {
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    /// A filled triangle fan from an explicit center through `points` — the
    /// cheap star/pie shape, where every vertex is visible from the center.
    /// The fan is *not* closed: repeat the first point to close it.
    Fan {
        cx: i32,
        cy: i32,
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    /// A stroked open path through `points`, `width` px wide, with round joins
    /// and round caps. The whole path is one translucent shape: unlike N
    /// separate [`Line`](Self::Line)s, overlapping segments don't double up
    /// where they meet, so a translucent stroke reads evenly.
    Polyline {
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_one")]
        width: u32,
    },
    /// Filled axis-aligned ellipse with semi-axes `rx`/`ry` about (`cx`, `cy`).
    Ellipse {
        cx: i32,
        cy: i32,
        rx: i32,
        ry: i32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    /// Hollow axis-aligned ellipse, stroked `width` px wide *inside* the
    /// rx/ry boundary. `draw_circle_outline` emits this with `rx == ry`.
    EllipseOutline {
        cx: i32,
        cy: i32,
        rx: i32,
        ry: i32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_one")]
        width: u32,
    },
    /// A filled annular sector — the donut/pie wedge. Spans radii `r_in`
    /// (0 for a solid pie slice) to `r_out`, between angles `a0` and `a1` in
    /// **radians**, measured clockwise from the +x axis (y grows downward, the
    /// screen convention every other draw command uses). `a1 < a0` sweeps the
    /// other way; a sweep of a full turn or more is a complete ring.
    Arc {
        cx: i32,
        cy: i32,
        r_in: f32,
        r_out: f32,
        a0: f32,
        a1: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    /// A (optionally rounded) rect filled with a **linear gradient** from
    /// color 0 to color 1 along `angle` — radians clockwise from the +x axis
    /// with y growing downward, the same screen convention [`Arc`](Self::Arc)
    /// uses. `0` runs left→right, `PI/2` top→bottom.
    ///
    /// The gradient spans the rect's full extent along that axis: the stop-0
    /// color sits on the corner furthest *against* the axis, stop 1 on the
    /// corner furthest along it, so the whole rect is covered whatever the
    /// angle. Both stops carry their own alpha, so a fade-to-nothing scrim is
    /// one command rather than a stack of translucent rects.
    ///
    /// A host that can't gradient may fill the rect with either stop; a host
    /// without rounded corners draws it square. Neither is silent omission.
    RectGradient {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        /// Corner radius in px; 0 = square corners.
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: u32,
        r0: u8,
        g0: u8,
        b0: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a0: u8,
        r1: u8,
        g1: u8,
        b1: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a1: u8,
        /// Gradient axis in radians, clockwise from +x (screen y-down).
        angle: f32,
    },
    /// A disc filled with a **radial gradient**: color 0 at (`cx`, `cy`)
    /// fading to color 1 at `radius`. The cheap glow / vignette / spotlight,
    /// and the radial sibling of [`RectGradient`](Self::RectGradient) — it
    /// tessellates as the fan [`Fan`](Self::Fan) already draws, with the two
    /// colors interpolated per vertex (center vs. rim), so a host that has
    /// the fan path has this one nearly for free.
    CircleGradient {
        cx: i32,
        cy: i32,
        radius: i32,
        r0: u8,
        g0: u8,
        b0: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a0: u8,
        r1: u8,
        g1: u8,
        b1: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a1: u8,
    },
    /// A CSS `box-shadow` cast by the rounded rect (`x`, `y`, `w`, `h`,
    /// `radius`): offset by (`dx`, `dy`), grown by `spread`, its edge falling
    /// off over `blur` px.
    ///
    /// This is **not** a blur pass. It is meant to be tessellated on the CPU
    /// as a single *non-overlapping* mesh — a solid core plus a ring whose
    /// per-vertex alpha runs from the shape boundary to 0 at `blur` — the
    /// same trick [`Polyline`](Self::Polyline) uses so a translucent shape
    /// composited in one go doesn't double up where its parts meet. A shadow
    /// is translucent by definition, so a mesh that overlapped itself would
    /// show every seam. [`crate::tess::shadow_mesh`] is the shared
    /// tessellator; a host without one may approximate with a few nested
    /// translucent rounded rects, but must not drop the command.
    Shadow {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        /// Corner radius of the *casting shape*, before `spread`.
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: u32,
        /// Falloff distance in px, outward from the (spread) shape boundary.
        blur: u32,
        /// Grow the shape by this many px before blurring; negative shrinks.
        #[serde(default, skip_serializing_if = "is_zero_i32")]
        spread: i32,
        #[serde(default, skip_serializing_if = "is_zero_i32")]
        dx: i32,
        #[serde(default, skip_serializing_if = "is_zero_i32")]
        dy: i32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    /// Restrict subsequent drawing to a rectangle (intersected with the
    /// drawable). Cleared by [`DrawCommand::ClipNone`].
    ///
    /// `Clip` *replaces* whatever clip is active — it does not nest. Use
    /// [`ClipPush`](Self::ClipPush)/[`ClipPop`](Self::ClipPop) to clip inside
    /// an enclosing clip and get it back.
    Clip {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        /// Corner radius in px; 0 = a plain rectangular clip, which is what
        /// this command has always meant. A host that can only scissor a rect
        /// ignores it and clips square.
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: u32,
    },
    ClipNone,
    /// Push a clip that is *intersected* with the enclosing one, so a widget
    /// can clip its own contents without knowing (or destroying) the clip its
    /// caller set. [`ClipPop`](Self::ClipPop) restores the enclosing clip.
    ///
    /// The stack is per-frame and starts empty (the whole drawable). An
    /// unmatched `ClipPop` is a no-op, and any clip left pushed when the
    /// frame ends simply ends with it.
    ClipPush {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: u32,
    },
    /// Restore the clip in force before the matching
    /// [`ClipPush`](Self::ClipPush).
    ClipPop,
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

/// Decode a heap list of points — each a `vec2` or a two-element `[x, y]`
/// list — into integer pixel coordinates. Shared by every point-list command
/// (`poly`, `polygon`, `fan`, `polyline`), so they all accept the same two
/// spellings a script may have written.
fn decode_points(tag: &str, points: &Value, heap: &Heap) -> Result<Vec<(i32, i32)>, String> {
    let list_id = match points {
        Value::List(id) => *id,
        other => {
            return Err(format!(
                "{tag} points must be a list, got {}",
                other.type_name()
            ));
        }
    };
    let mut out = Vec::new();
    for p in heap.get_list(list_id) {
        match p {
            Value::Vec2(x, y) => out.push((*x as i32, *y as i32)),
            Value::List(pid) => {
                let coords = heap.get_list(*pid);
                if coords.len() != 2 {
                    return Err(format!("{tag} list points must have 2 coords [x, y]"));
                }
                out.push((as_i64(&coords[0])? as i32, as_i64(&coords[1])? as i32));
            }
            other => {
                return Err(format!(
                    "{tag} point must be vec2 or [x, y], got {}",
                    other.type_name()
                ));
            }
        }
    }
    Ok(out)
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
                    radius: opt_u32(6, 0),
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
                radius: opt_u32(9, 0),
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
            "poly" => DrawCommand::Poly {
                points: decode_points(&tag, arg(0)?, heap)?,
                r: u8_at(1)?,
                g: u8_at(2)?,
                b: u8_at(3)?,
                a: opt_u8(4, 255),
            },
            "polygon" => DrawCommand::Polygon {
                points: decode_points(&tag, arg(0)?, heap)?,
                r: u8_at(1)?,
                g: u8_at(2)?,
                b: u8_at(3)?,
                a: opt_u8(4, 255),
            },
            "fan" => DrawCommand::Fan {
                cx: i32_at(0)?,
                cy: i32_at(1)?,
                points: decode_points(&tag, arg(2)?, heap)?,
                r: u8_at(3)?,
                g: u8_at(4)?,
                b: u8_at(5)?,
                a: opt_u8(6, 255),
            },
            "polyline" => DrawCommand::Polyline {
                points: decode_points(&tag, arg(0)?, heap)?,
                r: u8_at(1)?,
                g: u8_at(2)?,
                b: u8_at(3)?,
                a: opt_u8(4, 255),
                width: opt_u32(5, 1),
            },
            "ellipse" => DrawCommand::Ellipse {
                cx: i32_at(0)?,
                cy: i32_at(1)?,
                rx: i32_at(2)?,
                ry: i32_at(3)?,
                r: u8_at(4)?,
                g: u8_at(5)?,
                b: u8_at(6)?,
                a: opt_u8(7, 255),
            },
            "ellipse_outline" => DrawCommand::EllipseOutline {
                cx: i32_at(0)?,
                cy: i32_at(1)?,
                rx: i32_at(2)?,
                ry: i32_at(3)?,
                r: u8_at(4)?,
                g: u8_at(5)?,
                b: u8_at(6)?,
                a: opt_u8(7, 255),
                width: opt_u32(8, 1),
            },
            "arc" => {
                let f32_at = |i: usize| -> Result<f32, String> {
                    data.get(i)
                        .and_then(num_as_f64)
                        .map(|v| v as f32)
                        .ok_or_else(|| format!("arc command needs a number at arg {i}"))
                };
                DrawCommand::Arc {
                    cx: i32_at(0)?,
                    cy: i32_at(1)?,
                    r_in: f32_at(2)?,
                    r_out: f32_at(3)?,
                    a0: f32_at(4)?,
                    a1: f32_at(5)?,
                    r: u8_at(6)?,
                    g: u8_at(7)?,
                    b: u8_at(8)?,
                    a: opt_u8(9, 255),
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
            // The gradients and the shadow carry every field positionally —
            // there is no "short form" of them to stay compatible with, since
            // they postdate every existing consumer.
            "rect_gradient" => {
                let angle = data
                    .get(13)
                    .and_then(num_as_f64)
                    .ok_or_else(|| "rect_gradient command needs an angle at arg 13".to_string())?;
                DrawCommand::RectGradient {
                    x: i32_at(0)?,
                    y: i32_at(1)?,
                    w: u32_at(2)?,
                    h: u32_at(3)?,
                    radius: u32_at(4)?,
                    r0: u8_at(5)?,
                    g0: u8_at(6)?,
                    b0: u8_at(7)?,
                    a0: u8_at(8)?,
                    r1: u8_at(9)?,
                    g1: u8_at(10)?,
                    b1: u8_at(11)?,
                    a1: u8_at(12)?,
                    angle: angle as f32,
                }
            }
            "circle_gradient" => DrawCommand::CircleGradient {
                cx: i32_at(0)?,
                cy: i32_at(1)?,
                radius: i32_at(2)?,
                r0: u8_at(3)?,
                g0: u8_at(4)?,
                b0: u8_at(5)?,
                a0: u8_at(6)?,
                r1: u8_at(7)?,
                g1: u8_at(8)?,
                b1: u8_at(9)?,
                a1: u8_at(10)?,
            },
            "shadow" => DrawCommand::Shadow {
                x: i32_at(0)?,
                y: i32_at(1)?,
                w: u32_at(2)?,
                h: u32_at(3)?,
                radius: u32_at(4)?,
                blur: u32_at(5)?,
                spread: i32_at(6)?,
                dx: i32_at(7)?,
                dy: i32_at(8)?,
                r: u8_at(9)?,
                g: u8_at(10)?,
                b: u8_at(11)?,
                a: opt_u8(12, 255),
            },
            "clip" => DrawCommand::Clip {
                x: i32_at(0)?,
                y: i32_at(1)?,
                w: u32_at(2)?,
                h: u32_at(3)?,
                radius: opt_u32(4, 0),
            },
            "clip_none" => DrawCommand::ClipNone,
            "clip_push" => DrawCommand::ClipPush {
                x: i32_at(0)?,
                y: i32_at(1)?,
                w: u32_at(2)?,
                h: u32_at(3)?,
                radius: opt_u32(4, 0),
            },
            "clip_pop" => DrawCommand::ClipPop,
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

/// [`take_draw_commands`], but each command paired with the call chain that drew
/// it — the source attribution behind hit-testing a rendered scene back to the
/// code that produced it.
///
/// The chain is empty for any command the runtime could not attribute, and for
/// *every* command unless the host turned tracing on first
/// (`env.enable_emit_trace(true)`); this is a drain, not a switch, so a host that
/// forgets gets an untraced list rather than an error.
///
/// It is a chain rather than a single site because most `draw_*` names in the
/// `ui` prelude are Petal functions wrapping the natives — so the innermost call
/// site is prelude code, not the sketch. Pick the frame you want with
/// [`petal::provenance::pick_frame`], then resolve it with
/// [`petal::provenance::CallSite`]. Resolve against the program that was
/// *running* when the frame drew: an id survives neither a recompile nor a hot
/// reload, which is why `CallSite::resolve` rejects an out-of-range id rather
/// than guessing.
pub fn take_draw_commands_traced(env: &mut Env) -> Vec<(DrawCommand, EmitSite)> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer(sym);
    let mut origins = env.take_output_origins(sym);
    let mut out = Vec::with_capacity(values.len());
    for (i, v) in values.iter().enumerate() {
        match DrawCommand::from_value(v, env.heap()) {
            // Index into `origins` rather than zipping: a command that fails to
            // decode is skipped, and zipping would silently shift every later
            // command's attribution onto the wrong call site.
            Ok(cmd) => {
                let site = origins.get_mut(i).map(std::mem::take).unwrap_or_default();
                out.push((cmd, site));
            }
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
    env.register_native(
        "draw_rect_rounded_outline",
        native_draw_rect_rounded_outline,
    );
    env.register_native("draw_rect_gradient", native_draw_rect_gradient);
    env.register_native(
        "draw_rect_gradient_rounded",
        native_draw_rect_gradient_rounded,
    );
    env.register_native("draw_circle_gradient", native_draw_circle_gradient);
    env.register_native("draw_shadow", native_draw_shadow);
    env.register_native("draw_line", native_draw_line);
    env.register_native("draw_polyline", native_draw_polyline);
    env.register_native("draw_circle", native_draw_circle);
    env.register_native("draw_circle_outline", native_draw_circle_outline);
    env.register_native("draw_ellipse", native_draw_ellipse);
    env.register_native("draw_ellipse_outline", native_draw_ellipse_outline);
    env.register_native("fill_arc", native_fill_arc);
    env.register_native("fill_triangle", native_fill_triangle);
    env.register_native("fill_poly", native_fill_poly);
    env.register_native("fill_polygon", native_fill_polygon);
    env.register_native("fill_fan", native_fill_fan);
    env.register_native("draw_text", native_draw_text);
    env.register_native("clip", native_clip);
    env.register_native("clip_none", native_clip_none);
    env.register_native("clip_push", native_clip_push);
    env.register_native("clip_pop", native_clip_pop);
    env.register_native("text_width", native_text_width);
    env.register_native("font", native_font);
    env.register_native("fonts", native_fonts);
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
/// `draw_image(source, x, y, w, h, [alpha], [radius])` — draw a host-resolved
/// bitmap, optionally with rounded corners (an avatar, a card thumbnail).
fn native_draw_image(state: &mut PetalCxt) -> NativeResult {
    let source = state.get_string(1)?;
    let source = Value::String(state.heap_mut().alloc_string(source));
    let mut args = vec![source];
    for index in 2..=5 {
        args.push(Value::Int(state.get_int(index)?));
    }
    args.push(Value::Int(opt_int(state, 6, 255)?));
    args.push(Value::Int(opt_int(state, 7, 0)?)); // radius
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
    args.push(Value::Int(0)); // radius: square corners
    emit_draw(state, "rect_outline", args);
    state.push_nil();
    Ok(1)
}

/// `draw_rect_rounded_outline(x, y, w, h, radius, r, g, b, [a], [width])` —
/// the stroked sibling of `draw_rect_rounded`. Emits the same `rect_outline`
/// command with a corner radius, so a host that only draws square outlines
/// still draws the frame (rather than nothing).
fn native_draw_rect_rounded_outline(state: &mut PetalCxt) -> NativeResult {
    let x = state.get_int(1)?;
    let y = state.get_int(2)?;
    let w = state.get_int(3)?;
    let h = state.get_int(4)?;
    let radius = state.get_int(5)?;
    let r = state.get_int(6)?;
    let g = state.get_int(7)?;
    let b = state.get_int(8)?;
    let a = opt_int(state, 9, 255)?;
    let width = opt_int(state, 10, 1)?;
    emit_draw(
        state,
        "rect_outline",
        vec![
            Value::Int(x),
            Value::Int(y),
            Value::Int(w),
            Value::Int(h),
            Value::Int(r),
            Value::Int(g),
            Value::Int(b),
            Value::Int(a),
            Value::Int(width),
            Value::Int(radius),
        ],
    );
    state.push_nil();
    Ok(1)
}

/// Build the flat `rect_gradient` arg list. Shared by the square and rounded
/// natives so the two can never drift in field order.
#[allow(clippy::too_many_arguments)]
fn rect_gradient_args(
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    radius: i64,
    c0: [i64; 4],
    c1: [i64; 4],
    angle: f64,
) -> Vec<Value> {
    vec![
        Value::Int(x),
        Value::Int(y),
        Value::Int(w),
        Value::Int(h),
        Value::Int(radius),
        Value::Int(c0[0]),
        Value::Int(c0[1]),
        Value::Int(c0[2]),
        Value::Int(c0[3]),
        Value::Int(c1[0]),
        Value::Int(c1[1]),
        Value::Int(c1[2]),
        Value::Int(c1[3]),
        Value::Float(angle),
    ]
}

/// Read the two gradient stops (`r0 g0 b0 a0 r1 g1 b1 a1`) starting at
/// 1-indexed argument `first`.
fn gradient_stops(state: &PetalCxt, first: usize) -> Result<([i64; 4], [i64; 4]), String> {
    let stop = |i: usize| -> Result<[i64; 4], String> {
        Ok([
            state.get_int(i)?,
            state.get_int(i + 1)?,
            state.get_int(i + 2)?,
            state.get_int(i + 3)?,
        ])
    };
    Ok((stop(first)?, stop(first + 4)?))
}

/// `draw_rect_gradient(x, y, w, h, r0, g0, b0, a0, r1, g1, b1, a1, angle)` —
/// a rect filled with a linear gradient along `angle` (radians, clockwise from
/// +x, screen y-down). Both stops carry an alpha, so the fade-to-transparent
/// scrim under a caption is one command.
fn native_draw_rect_gradient(state: &mut PetalCxt) -> NativeResult {
    let x = state.get_int(1)?;
    let y = state.get_int(2)?;
    let w = state.get_int(3)?;
    let h = state.get_int(4)?;
    let (c0, c1) = gradient_stops(state, 5)?;
    let angle = get_num(state, 13, "draw_rect_gradient")?;
    let args = rect_gradient_args(x, y, w, h, 0, c0, c1, angle);
    emit_draw(state, "rect_gradient", args);
    state.push_nil();
    Ok(1)
}

/// `draw_rect_gradient_rounded(x, y, w, h, radius, r0, g0, b0, a0, r1, g1, b1,
/// a1, angle)` — [`native_draw_rect_gradient`] with rounded corners, the same
/// way `draw_rect_rounded` relates to `draw_rect`.
fn native_draw_rect_gradient_rounded(state: &mut PetalCxt) -> NativeResult {
    let x = state.get_int(1)?;
    let y = state.get_int(2)?;
    let w = state.get_int(3)?;
    let h = state.get_int(4)?;
    let radius = state.get_int(5)?;
    let (c0, c1) = gradient_stops(state, 6)?;
    let angle = get_num(state, 14, "draw_rect_gradient_rounded")?;
    let args = rect_gradient_args(x, y, w, h, radius, c0, c1, angle);
    emit_draw(state, "rect_gradient", args);
    state.push_nil();
    Ok(1)
}

/// `draw_circle_gradient(cx, cy, radius, r0, g0, b0, a0, r1, g1, b1, a1)` — a
/// disc shading from the center color to the rim color: the glow, the vignette,
/// the soft spotlight.
fn native_draw_circle_gradient(state: &mut PetalCxt) -> NativeResult {
    let cx = state.get_int(1)?;
    let cy = state.get_int(2)?;
    let radius = state.get_int(3)?;
    let (c0, c1) = gradient_stops(state, 4)?;
    emit_draw(
        state,
        "circle_gradient",
        vec![
            Value::Int(cx),
            Value::Int(cy),
            Value::Int(radius),
            Value::Int(c0[0]),
            Value::Int(c0[1]),
            Value::Int(c0[2]),
            Value::Int(c0[3]),
            Value::Int(c1[0]),
            Value::Int(c1[1]),
            Value::Int(c1[2]),
            Value::Int(c1[3]),
        ],
    );
    state.push_nil();
    Ok(1)
}

/// `draw_shadow(x, y, w, h, radius, blur, spread, dx, dy, r, g, b, [a])` — the
/// CSS box-shadow of the rounded rect (`x`, `y`, `w`, `h`, `radius`). The
/// argument order is CSS's, with the casting shape first.
fn native_draw_shadow(state: &mut PetalCxt) -> NativeResult {
    let x = state.get_int(1)?;
    let y = state.get_int(2)?;
    let w = state.get_int(3)?;
    let h = state.get_int(4)?;
    let radius = state.get_int(5)?;
    let blur = state.get_int(6)?;
    let spread = state.get_int(7)?;
    let dx = state.get_int(8)?;
    let dy = state.get_int(9)?;
    let r = state.get_int(10)?;
    let g = state.get_int(11)?;
    let b = state.get_int(12)?;
    let a = opt_int(state, 13, 255)?;
    emit_draw(
        state,
        "shadow",
        vec![
            Value::Int(x),
            Value::Int(y),
            Value::Int(w),
            Value::Int(h),
            Value::Int(radius),
            Value::Int(blur.max(0)),
            Value::Int(spread),
            Value::Int(dx),
            Value::Int(dy),
            Value::Int(r),
            Value::Int(g),
            Value::Int(b),
            Value::Int(a),
        ],
    );
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

// `draw_circle_outline(cx, cy, radius, r, g, b, [a], [width])` — a hollow
// circle, stroked `width` px inward from `radius`. Emitted as the `rx == ry`
// case of `ellipse_outline` so a host implements one stroked-conic path.
fn native_draw_circle_outline(state: &mut PetalCxt) -> NativeResult {
    let cx = state.get_int(1)?;
    let cy = state.get_int(2)?;
    let radius = state.get_int(3)?;
    // Read r/g/b as named bindings rather than a loop: the stdlib doc
    // extractor recovers a native's signature from `let <name> = state.get_*`
    // bindings, and a loop index leaves it with no name to report.
    let r = state.get_int(4)?;
    let g = state.get_int(5)?;
    let b = state.get_int(6)?;
    let mut args = vec![
        Value::Int(cx),
        Value::Int(cy),
        Value::Int(radius),
        Value::Int(radius),
        Value::Int(r),
        Value::Int(g),
        Value::Int(b),
    ];
    args.push(Value::Int(opt_int(state, 7, 255)?)); // a
    args.push(Value::Int(opt_int(state, 8, 1)?)); // width
    emit_draw(state, "ellipse_outline", args);
    state.push_nil();
    Ok(1)
}

// `draw_ellipse(cx, cy, rx, ry, r, g, b, [a])`.
fn native_draw_ellipse(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 7)?;
    args.push(Value::Int(opt_int(state, 8, 255)?)); // a
    emit_draw(state, "ellipse", args);
    state.push_nil();
    Ok(1)
}

// `draw_ellipse_outline(cx, cy, rx, ry, r, g, b, [a], [width])`.
fn native_draw_ellipse_outline(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 7)?;
    args.push(Value::Int(opt_int(state, 8, 255)?)); // a
    args.push(Value::Int(opt_int(state, 9, 1)?)); // width
    emit_draw(state, "ellipse_outline", args);
    state.push_nil();
    Ok(1)
}

/// Read a 1-indexed argument as f64 — for the angle/radius args of `fill_arc`,
/// where an int would quantize a sweep to whole radians.
fn get_num(state: &PetalCxt, index: usize, name: &str) -> Result<f64, String> {
    match state.get_value(index)? {
        Value::Int(n) => Ok(n as f64),
        Value::Float(f) => Ok(f),
        other => Err(format!(
            "{name}() arg {index} must be a number, got {}",
            other.type_name()
        )),
    }
}

// `fill_arc(cx, cy, r_in, r_out, a0, a1, r, g, b, [a])` — one annular sector:
// the donut wedge that every pie/gauge chart otherwise hand-rolls as a quad
// fan. Angles are radians, clockwise from +x (screen y-down). `r_in = 0` is a
// solid pie slice; a sweep of `TAU` is a full ring.
fn native_fill_arc(state: &mut PetalCxt) -> NativeResult {
    let cx = state.get_int(1)?;
    let cy = state.get_int(2)?;
    let r_in = get_num(state, 3, "fill_arc")?;
    let r_out = get_num(state, 4, "fill_arc")?;
    let a0 = get_num(state, 5, "fill_arc")?;
    let a1 = get_num(state, 6, "fill_arc")?;
    let r = state.get_int(7)?;
    let g = state.get_int(8)?;
    let b = state.get_int(9)?;
    let a = opt_int(state, 10, 255)?;
    emit_draw(
        state,
        "arc",
        vec![
            Value::Int(cx),
            Value::Int(cy),
            Value::Float(r_in),
            Value::Float(r_out),
            Value::Float(a0),
            Value::Float(a1),
            Value::Int(r),
            Value::Int(g),
            Value::Int(b),
            Value::Int(a),
        ],
    );
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

/// Read a point-list argument, validating it up front (the host re-reads the
/// same list when it decodes the command, so a bad point must be rejected here
/// where the script's own call site is still on the stack). `min` is the
/// smallest point count the shape means anything at.
fn point_list_arg(state: &PetalCxt, index: usize, name: &str, min: usize) -> Result<Value, String> {
    let points_value = state.get_value(index)?;
    let list_id = match points_value {
        Value::List(id) => id,
        other => {
            return Err(format!(
                "{name}() expects a list of points, got {}",
                other.type_name()
            ));
        }
    };

    let elements: Vec<Value> = state.heap().get_list(list_id).to_vec();
    for el in &elements {
        match el {
            Value::Vec2(_, _) => {}
            Value::List(pid) => {
                let coords = state.heap().get_list(*pid);
                if coords.len() != 2 {
                    return Err(format!(
                        "{name}() list points must have exactly 2 coords [x, y]"
                    ));
                }
                coord_to_i32(&coords[0])?;
                coord_to_i32(&coords[1])?;
            }
            other => {
                return Err(format!(
                    "{name}() points must be vec2 or [x, y] lists, got {}",
                    other.type_name()
                ));
            }
        }
    }

    if elements.len() < min {
        return Err(format!("{name}() needs at least {min} points"));
    }
    Ok(points_value)
}

fn native_fill_poly(state: &mut PetalCxt) -> NativeResult {
    let points_value = point_list_arg(state, 1, "fill_poly", 3)?;
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

/// `fill_polygon(points, r, g, b, [a])` — fill a **simple** polygon, concave
/// allowed. `fill_poly` fans from the first vertex, which only fills a convex
/// outline correctly; this triangulates properly, so a star fills as a star
/// instead of needing to be hand-fanned into triangles by the caller.
fn native_fill_polygon(state: &mut PetalCxt) -> NativeResult {
    let points_value = point_list_arg(state, 1, "fill_polygon", 3)?;
    let r = state.get_int(2)?;
    let g = state.get_int(3)?;
    let b = state.get_int(4)?;
    let a = opt_int(state, 5, 255)?;
    emit_draw(
        state,
        "polygon",
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

/// `fill_fan(cx, cy, points, r, g, b, [a])` — a triangle fan from an explicit
/// center. The cheap star/pie/gauge shape for an outline every vertex of which
/// the center can see; `fill_polygon` is the general case.
fn native_fill_fan(state: &mut PetalCxt) -> NativeResult {
    let cx = state.get_int(1)?;
    let cy = state.get_int(2)?;
    let points_value = point_list_arg(state, 3, "fill_fan", 2)?;
    let r = state.get_int(4)?;
    let g = state.get_int(5)?;
    let b = state.get_int(6)?;
    let a = opt_int(state, 7, 255)?;
    emit_draw(
        state,
        "fan",
        vec![
            Value::Int(cx),
            Value::Int(cy),
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

/// `draw_polyline(points, r, g, b, [a], [width])` — one stroked open path with
/// round joins and caps. The point of it being a single command rather than N
/// `draw_line` calls is translucency: the whole stroke is composited once, so
/// overlapping segments and joins don't darken where they meet.
fn native_draw_polyline(state: &mut PetalCxt) -> NativeResult {
    let points_value = point_list_arg(state, 1, "draw_polyline", 1)?;
    let r = state.get_int(2)?;
    let g = state.get_int(3)?;
    let b = state.get_int(4)?;
    let a = opt_int(state, 5, 255)?;
    let width = opt_int(state, 6, 1)?;
    emit_draw(
        state,
        "polyline",
        vec![
            points_value,
            Value::Int(r),
            Value::Int(g),
            Value::Int(b),
            Value::Int(a),
            Value::Int(width),
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

/// `clip(x, y, w, h, [radius])` — *replace* the active clip. `clip_none`
/// clears it.
fn native_clip(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 4)?;
    args.push(Value::Int(opt_int(state, 5, 0)?)); // radius
    emit_draw(state, "clip", args);
    state.push_nil();
    Ok(1)
}

fn native_clip_none(state: &mut PetalCxt) -> NativeResult {
    emit_draw(state, "clip_none", vec![]);
    state.push_nil();
    Ok(1)
}

/// `clip_push(x, y, w, h, [radius])` — clip *inside* the enclosing clip
/// (intersected with it), restored by `clip_pop()`. The composable form: a
/// widget that clips its own contents no longer has to know, or destroy, the
/// clip its caller set.
fn native_clip_push(state: &mut PetalCxt) -> NativeResult {
    let mut args = int_args(state, 4)?;
    args.push(Value::Int(opt_int(state, 5, 0)?)); // radius
    emit_draw(state, "clip_push", args);
    state.push_nil();
    Ok(1)
}

fn native_clip_pop(state: &mut PetalCxt) -> NativeResult {
    emit_draw(state, "clip_pop", vec![]);
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
                h: 40,
                radius: 0
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
                    radius: 0,
                },
                DrawCommand::Image {
                    source: "assets/glow.png".into(),
                    x: 5,
                    y: 6,
                    w: 70,
                    h: 80,
                    a: 128,
                    radius: 0,
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
    fn polyline_decodes_points_alpha_and_width() {
        let mut env = Env::new();
        register_draw(&mut env);
        env.run_source("draw_polyline([[0, 0], [10, 5], [20, 0]], 1, 2, 3, 128, 6)")
            .expect("run");
        assert_eq!(
            take_draw_commands(&mut env),
            vec![DrawCommand::Polyline {
                points: vec![(0, 0), (10, 5), (20, 0)],
                r: 1,
                g: 2,
                b: 3,
                a: 128,
                width: 6,
            }]
        );
        // Short form: opaque hairline, the same defaults draw_line has.
        env.run_source("draw_polyline([[0, 0], [1, 1]], 4, 5, 6)")
            .expect("run");
        assert!(matches!(
            take_draw_commands(&mut env)[0],
            DrawCommand::Polyline {
                a: 255,
                width: 1,
                ..
            }
        ));
    }

    #[test]
    fn ellipse_circle_outline_and_arc_decode() {
        let mut env = Env::new();
        register_draw(&mut env);
        env.run_source(
            "draw_ellipse(50, 40, 30, 10, 200, 100, 50)\n\
             draw_ellipse_outline(50, 40, 30, 10, 200, 100, 50, 128, 4)\n\
             draw_circle_outline(8, 9, 7, 1, 2, 3)\n\
             fill_arc(60, 60, 20.0, 40.0, 0.0, 1.5, 9, 8, 7, 200)",
        )
        .expect("run");
        let cmds = take_draw_commands(&mut env);
        assert_eq!(
            cmds[0],
            DrawCommand::Ellipse {
                cx: 50,
                cy: 40,
                rx: 30,
                ry: 10,
                r: 200,
                g: 100,
                b: 50,
                a: 255,
            }
        );
        assert_eq!(
            cmds[1],
            DrawCommand::EllipseOutline {
                cx: 50,
                cy: 40,
                rx: 30,
                ry: 10,
                r: 200,
                g: 100,
                b: 50,
                a: 128,
                width: 4,
            }
        );
        // A circle outline is the rx == ry case of the same command.
        assert_eq!(
            cmds[2],
            DrawCommand::EllipseOutline {
                cx: 8,
                cy: 9,
                rx: 7,
                ry: 7,
                r: 1,
                g: 2,
                b: 3,
                a: 255,
                width: 1,
            }
        );
        assert_eq!(
            cmds[3],
            DrawCommand::Arc {
                cx: 60,
                cy: 60,
                r_in: 20.0,
                r_out: 40.0,
                a0: 0.0,
                a1: 1.5,
                r: 9,
                g: 8,
                b: 7,
                a: 200,
            }
        );
    }

    #[test]
    fn arc_angles_keep_their_fraction() {
        // Ints are accepted, but a fractional sweep must survive as a float —
        // truncating it would quantize every pie chart to whole radians.
        let mut env = Env::new();
        register_draw(&mut env);
        env.run_source("fill_arc(0, 0, 0, 10, 0.25, 2.75, 1, 2, 3)")
            .expect("run");
        match take_draw_commands(&mut env)[0] {
            DrawCommand::Arc { a0, a1, r_in, .. } => {
                assert!((a0 - 0.25).abs() < 1e-6, "a0 was {a0}");
                assert!((a1 - 2.75).abs() < 1e-6, "a1 was {a1}");
                assert_eq!(r_in, 0.0, "an int radius still decodes");
            }
            ref other => panic!("expected an arc, got {other:?}"),
        }
    }

    #[test]
    fn rounded_outline_carries_radius_and_plain_outline_does_not() {
        let mut env = Env::new();
        register_draw(&mut env);
        env.run_source(
            "draw_rect_outline(0, 0, 10, 10, 1, 2, 3, 128, 2)\n\
             draw_rect_rounded_outline(0, 0, 10, 10, 4, 1, 2, 3, 128, 2)",
        )
        .expect("run");
        let cmds = take_draw_commands(&mut env);
        assert!(matches!(
            cmds[0],
            DrawCommand::RectOutline {
                a: 128,
                width: 2,
                radius: 0,
                ..
            }
        ));
        assert!(matches!(
            cmds[1],
            DrawCommand::RectOutline {
                a: 128,
                width: 2,
                radius: 4,
                ..
            }
        ));
        // The square outline must still serialize to its pre-radius JSON.
        let json = serde_json::to_string(&cmds[0]).unwrap();
        assert!(!json.contains("radius"), "{json}");
    }

    #[test]
    fn concave_fill_and_fan_decode() {
        let mut env = Env::new();
        register_draw(&mut env);
        env.run_source(
            "fill_polygon([[0, 0], [10, 10], [20, 0], [10, 30]], 1, 2, 3, 90)\n\
             fill_fan(5, 5, [[0, 0], [10, 0], [10, 10]], 4, 5, 6)",
        )
        .expect("run");
        let cmds = take_draw_commands(&mut env);
        assert_eq!(
            cmds[0],
            DrawCommand::Polygon {
                points: vec![(0, 0), (10, 10), (20, 0), (10, 30)],
                r: 1,
                g: 2,
                b: 3,
                a: 90,
            }
        );
        assert_eq!(
            cmds[1],
            DrawCommand::Fan {
                cx: 5,
                cy: 5,
                points: vec![(0, 0), (10, 0), (10, 10)],
                r: 4,
                g: 5,
                b: 6,
                a: 255,
            }
        );
    }

    #[test]
    fn point_lists_are_validated_at_the_call_site() {
        let mut env = Env::new();
        register_draw(&mut env);
        // A non-list, a too-short list, and a malformed point each fail at the
        // call rather than being silently dropped when the host decodes.
        assert!(env.run_source("fill_polygon(7, 1, 2, 3)").is_err());
        assert!(
            env.run_source("fill_polygon([[0, 0], [1, 1]], 1, 2, 3)")
                .is_err()
        );
        assert!(
            env.run_source("draw_polyline([[0, 0, 0]], 1, 2, 3)")
                .is_err()
        );
        // vec2 points are the other accepted spelling.
        env.run_source("draw_polyline([vec2(1, 2), vec2(3, 4)], 1, 2, 3)")
            .expect("vec2 points");
        assert_eq!(
            take_draw_commands(&mut env),
            vec![DrawCommand::Polyline {
                points: vec![(1, 2), (3, 4)],
                r: 1,
                g: 2,
                b: 3,
                a: 255,
                width: 1,
            }]
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
