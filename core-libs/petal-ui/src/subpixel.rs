//! Sub-pixel draw commands: fractional coordinates for a host that opts in.
//!
//! Every [`DrawCommand`] holds whole pixels, and a host built against it keeps
//! working unchanged. A host that can draw at fractional positions (a canvas,
//! an anti-aliasing rasterizer) opts in with
//! [`register_draw_with`](crate::draw::register_draw_with)`(env, DrawOptions
//! { subpixel: true, .. })` and drains with [`take_frame_commands`]:
//!
//! - The geometry natives put a script's numbers into the buffer as they are:
//!   an int stays an int, a float stays a float. Colours, alpha, ids and text
//!   sizes are still whole numbers.
//! - [`take_frame_commands`] hands back [`FrameCommand`]s: a
//!   [`FrameCommand::Sub`] (a [`SubCommand`], `f32` geometry) for each
//!   primitive the [`PixelSnap`] mode in force leaves unsnapped, and a
//!   [`FrameCommand::Px`] — exactly what [`take_draw_commands`] returns for
//!   it — for everything else.
//! - By default the rect family stays snapped (truncated, as ever), so rects
//!   that tile still meet without an anti-aliased seam, and curves, strokes
//!   and polygons go sub-pixel. A script says otherwise with `pixel_snap(true)`
//!   (snap everything: pixel art) or `pixel_snap(false)` (snap nothing);
//!   `pixel_snap(nil)` goes back to the default. The mode starts at the
//!   default on every frame and applies to what is drawn after the call.
//! - The whole-pixel drains ([`take_draw_commands`] and its `_traced` / `_for`
//!   siblings) still work on such an env and return exactly what a default
//!   env returns: they truncate a fractional number the way the default
//!   natives do. The `pixel_snap` marker never reaches any drain.
//!
//! [`DrawWire::encode_frame`](crate::draw_wire::DrawWire::encode_frame) puts a
//! frame of `FrameCommand`s on the wire: a `SubCommand` uses its whole-pixel
//! twin's opcode and layout, with fractional numbers.
//!
//! [`take_draw_commands`]: crate::draw::take_draw_commands

use petal::env::Env;
use petal::execution_context::EmitSite;
use petal::heap::Heap;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::stack::StackKey;
use petal::value::Value;
use serde::Serialize;

use crate::compose::{compose_items, frame_command};
use crate::draw::{
    DRAW, DRAW_COMMANDS_SYMBOL, DrawCommand, TextStyle, color_arg, decode_buffer, emit_draw,
    get_num, gradient_stops, image_tail, num_as_f64, opt_int, point_list_arg, record_point,
    site_of, take_sites, text_decorations,
};
use crate::params::register;
use crate::text::REGULAR_WEIGHT;

// ── Snap mode ─────────────────────────────────────────────────────────────

/// Which primitives a subpixel host gets whole pixels for: the mode a
/// script sets with `pixel_snap`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PixelSnap {
    /// The default: the rect family (`rect`, `rect_outline`, `rect_gradient`,
    /// `shadow`, `image`, `draw_canvas`, `text`'s position; `rects` batches
    /// and clips always) is snapped, everything else is sub-pixel.
    #[default]
    Rects,
    /// `pixel_snap(true)`: everything is snapped. Pixel art.
    All,
    /// `pixel_snap(false)`: nothing that has a [`SubCommand`] is snapped.
    Off,
}

impl PixelSnap {
    /// The number the `pixel_snap` marker (and a retained layer's kept
    /// content) carries for this mode.
    pub(crate) fn code(self) -> i64 {
        match self {
            PixelSnap::Rects => 0,
            PixelSnap::All => 1,
            PixelSnap::Off => 2,
        }
    }

    pub(crate) fn from_code(code: i64) -> PixelSnap {
        match code {
            1 => PixelSnap::All,
            2 => PixelSnap::Off,
            _ => PixelSnap::Rects,
        }
    }

    /// The mode `pixel_snap(on)` asks for.
    pub(crate) fn from_arg(on: &Value) -> Result<PixelSnap, String> {
        match on {
            Value::Bool(true) => Ok(PixelSnap::All),
            Value::Bool(false) => Ok(PixelSnap::Off),
            Value::Nil => Ok(PixelSnap::Rects),
            other => Err(format!(
                "pixel_snap() expects true (snap everything), false (snap nothing) or nil (the default: snap rects), got {}",
                other.type_name()
            )),
        }
    }

    /// Whether `cmd` is drawn in whole pixels under this mode.
    pub fn snaps(self, cmd: &DrawCommand) -> bool {
        match self {
            PixelSnap::All => true,
            PixelSnap::Off => false,
            PixelSnap::Rects => matches!(
                cmd,
                DrawCommand::Image { .. }
                    | DrawCommand::Rect { .. }
                    | DrawCommand::RectOutline { .. }
                    | DrawCommand::RectGradient { .. }
                    | DrawCommand::Shadow { .. }
                    | DrawCommand::DrawCanvas { .. }
                    | DrawCommand::Text { .. }
            ),
        }
    }
}

/// The tag of the marker `pixel_snap` emits on a subpixel host.
const PIXEL_SNAP_TAG: &str = "pixel_snap";

/// The mode a `pixel_snap` marker sets, or `None` for any other value.
pub(crate) fn snap_marker(v: &Value, heap: &Heap) -> Option<PixelSnap> {
    let Value::EnumVariant { tag, data } = v else {
        return None;
    };
    if heap.tag_str(*tag) != PIXEL_SNAP_TAG {
        return None;
    }
    let code = match heap.get_list(*data).first() {
        Some(Value::Int(n)) => *n,
        _ => 0,
    };
    Some(PixelSnap::from_code(code))
}

// ── Commands ──────────────────────────────────────────────────────────────

fn is_opaque(a: &u8) -> bool {
    *a == 255
}
fn is_zero(v: &f32) -> bool {
    *v == 0.0
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}
fn is_regular(w: &u16) -> bool {
    *w == REGULAR_WEIGHT
}
fn is_upright(i: &bool) -> bool {
    !*i
}

/// The `f32` twin of each [`DrawCommand`] with geometry: the same fields under
/// the same names, in the same order, with positions, sizes, radii, stroke
/// widths and point lists fractional. Colours, alpha, ids, text sizes and an
/// image's own-pixel `src` / `slice` stay whole. The JSON form is the twin's,
/// tagged by `op` and omitting the same defaults, so a consumer of the JSON
/// reads either.
///
/// Every field means what it means on the twin; see [`DrawCommand`].
#[derive(Serialize, PartialEq, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SubCommand {
    Image {
        source: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_zero")]
        radius: f32,
        #[serde(skip_serializing_if = "Option::is_none")]
        tint: Option<[u8; 3]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        src: Option<[i32; 4]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        slice: Option<[u32; 4]>,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_zero")]
        radius: f32,
    },
    RectOutline {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_one")]
        width: f32,
        #[serde(skip_serializing_if = "is_zero")]
        radius: f32,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_one")]
        width: f32,
    },
    Circle {
        cx: f32,
        cy: f32,
        radius: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    Text {
        text: String,
        x: f32,
        y: f32,
        size: u16,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        font: Option<String>,
        #[serde(skip_serializing_if = "is_regular")]
        weight: u16,
        #[serde(skip_serializing_if = "is_upright")]
        italic: bool,
        #[serde(skip_serializing_if = "is_zero")]
        spacing: f32,
    },
    Triangle {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x3: f32,
        y3: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    Poly {
        points: Vec<(f32, f32)>,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    Polygon {
        points: Vec<(f32, f32)>,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    Fan {
        cx: f32,
        cy: f32,
        points: Vec<(f32, f32)>,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    Polyline {
        points: Vec<(f32, f32)>,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_one")]
        width: f32,
    },
    Ellipse {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    EllipseOutline {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_one")]
        width: f32,
    },
    Arc {
        cx: f32,
        cy: f32,
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
    RectGradient {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(skip_serializing_if = "is_zero")]
        radius: f32,
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
        angle: f32,
    },
    CircleGradient {
        cx: f32,
        cy: f32,
        radius: f32,
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
    Shadow {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(skip_serializing_if = "is_zero")]
        radius: f32,
        blur: f32,
        #[serde(skip_serializing_if = "is_zero")]
        spread: f32,
        #[serde(skip_serializing_if = "is_zero")]
        dx: f32,
        #[serde(skip_serializing_if = "is_zero")]
        dy: f32,
        r: u8,
        g: u8,
        b: u8,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
    },
    DrawCanvas {
        id: u32,
        x: f32,
        y: f32,
        #[serde(skip_serializing_if = "is_opaque")]
        a: u8,
        #[serde(skip_serializing_if = "is_zero")]
        w: f32,
        #[serde(skip_serializing_if = "is_zero")]
        h: f32,
    },
}

/// One command of a frame drained by [`take_frame_commands`]: whole pixels,
/// or sub-pixel. Serializes as the command it holds.
#[derive(Serialize, PartialEq, Debug, Clone)]
#[serde(untagged)]
pub enum FrameCommand {
    Px(DrawCommand),
    Sub(SubCommand),
}

impl FrameCommand {
    /// The command magnified by `s` for a HiDPI drawable: a `Px` by
    /// [`crate::compose::scale`] (which rounds), a `Sub` by
    /// [`SubCommand::scale`] (which does not).
    pub fn scale(self, s: f32) -> FrameCommand {
        match self {
            FrameCommand::Px(cmd) => FrameCommand::Px(crate::compose::scale(cmd, s)),
            FrameCommand::Sub(sub) => FrameCommand::Sub(sub.scale(s)),
        }
    }

    /// The command in whole pixels: a `Px` as it is, a `Sub` truncated the
    /// way [`take_draw_commands`](crate::draw::take_draw_commands) truncates.
    pub fn to_px(&self) -> DrawCommand {
        match self {
            FrameCommand::Px(cmd) => cmd.clone(),
            FrameCommand::Sub(sub) => sub.to_px(),
        }
    }
}

fn points_f32(points: &[(i32, i32)]) -> Vec<(f32, f32)> {
    points.iter().map(|&(x, y)| (x as f32, y as f32)).collect()
}

/// A point list decoded with its fractions — the decode
/// [`DrawCommand::from_value`] did, without the truncation. `None` when it is
/// not one (which a command that decoded never has).
fn decode_points(points: &Value, heap: &Heap) -> Option<Vec<(f32, f32)>> {
    let Value::List(id) = points else {
        return None;
    };
    let mut out = Vec::new();
    for p in heap.get_list(*id) {
        let (x, y) = match p {
            Value::Vec2(id) => heap.get_vec2(*id),
            Value::List(pid) => match heap.get_list(*pid) {
                [x, y] => (num_as_f64(x)?, num_as_f64(y)?),
                _ => return None,
            },
            Value::Map(mid) => {
                let (x, y) = record_point("", *mid, heap).ok()?;
                (num_as_f64(&x)?, num_as_f64(&y)?)
            }
            _ => return None,
        };
        out.push((x as f32, y as f32));
    }
    Some(out)
}

impl SubCommand {
    /// Decode `value` — the raw command `px` decoded from — keeping its
    /// fractions. `px` supplies everything that is not geometry (colours,
    /// text, the image's own-pixel rects), so the two agree on all of it.
    /// `None` for a command with no sub-pixel twin.
    pub(crate) fn decode(px: &DrawCommand, value: &Value, heap: &Heap) -> Option<SubCommand> {
        let Value::EnumVariant { data, .. } = value else {
            return None;
        };
        let data = heap.get_list(*data);
        // A required field (the decode of `px` already found a number there)
        // or an optional one at its default.
        let f = |i: usize| data.get(i).and_then(num_as_f64).unwrap_or(0.0) as f32;
        let of = |i: usize, default: f32| data.get(i).and_then(num_as_f64).map_or(default, |v| v as f32);
        use DrawCommand as C;
        use SubCommand as S;
        Some(match px {
            C::Image { source, a, tint, src, slice, .. } => S::Image {
                source: source.clone(),
                x: f(1),
                y: f(2),
                w: f(3),
                h: f(4),
                a: *a,
                radius: of(6, 0.0),
                tint: *tint,
                src: *src,
                slice: *slice,
            },
            C::Rect { r, g, b, a, .. } => S::Rect {
                x: f(0),
                y: f(1),
                w: f(2),
                h: f(3),
                r: *r,
                g: *g,
                b: *b,
                a: *a,
                radius: of(8, 0.0),
            },
            C::RectOutline { r, g, b, a, .. } => S::RectOutline {
                x: f(0),
                y: f(1),
                w: f(2),
                h: f(3),
                r: *r,
                g: *g,
                b: *b,
                a: *a,
                width: of(8, 1.0),
                radius: of(9, 0.0),
            },
            C::Line { r, g, b, a, .. } => S::Line {
                x1: f(0),
                y1: f(1),
                x2: f(2),
                y2: f(3),
                r: *r,
                g: *g,
                b: *b,
                a: *a,
                width: of(8, 1.0),
            },
            C::Circle { r, g, b, a, .. } => S::Circle {
                cx: f(0),
                cy: f(1),
                radius: f(2),
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            C::Text { text, size, r, g, b, a, font, weight, italic, spacing, .. } => S::Text {
                text: text.clone(),
                x: f(1),
                y: f(2),
                size: *size,
                r: *r,
                g: *g,
                b: *b,
                a: *a,
                font: font.clone(),
                weight: *weight,
                italic: *italic,
                spacing: *spacing,
            },
            C::Triangle { r, g, b, a, .. } => S::Triangle {
                x1: f(0),
                y1: f(1),
                x2: f(2),
                y2: f(3),
                x3: f(4),
                y3: f(5),
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            C::Poly { r, g, b, a, .. } => S::Poly {
                points: decode_points(data.first()?, heap)?,
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            C::Polygon { r, g, b, a, .. } => S::Polygon {
                points: decode_points(data.first()?, heap)?,
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            C::Fan { r, g, b, a, .. } => S::Fan {
                cx: f(0),
                cy: f(1),
                points: decode_points(data.get(2)?, heap)?,
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            C::Polyline { r, g, b, a, .. } => S::Polyline {
                points: decode_points(data.first()?, heap)?,
                r: *r,
                g: *g,
                b: *b,
                a: *a,
                width: of(5, 1.0),
            },
            C::Ellipse { r, g, b, a, .. } => S::Ellipse {
                cx: f(0),
                cy: f(1),
                rx: f(2),
                ry: f(3),
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            C::EllipseOutline { r, g, b, a, .. } => S::EllipseOutline {
                cx: f(0),
                cy: f(1),
                rx: f(2),
                ry: f(3),
                r: *r,
                g: *g,
                b: *b,
                a: *a,
                width: of(8, 1.0),
            },
            C::Arc { r_in, r_out, a0, a1, r, g, b, a, .. } => S::Arc {
                cx: f(0),
                cy: f(1),
                r_in: *r_in,
                r_out: *r_out,
                a0: *a0,
                a1: *a1,
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            C::RectGradient { r0, g0, b0, a0, r1, g1, b1, a1, angle, .. } => S::RectGradient {
                x: f(0),
                y: f(1),
                w: f(2),
                h: f(3),
                radius: f(4),
                r0: *r0,
                g0: *g0,
                b0: *b0,
                a0: *a0,
                r1: *r1,
                g1: *g1,
                b1: *b1,
                a1: *a1,
                angle: *angle,
            },
            C::CircleGradient { r0, g0, b0, a0, r1, g1, b1, a1, .. } => S::CircleGradient {
                cx: f(0),
                cy: f(1),
                radius: f(2),
                r0: *r0,
                g0: *g0,
                b0: *b0,
                a0: *a0,
                r1: *r1,
                g1: *g1,
                b1: *b1,
                a1: *a1,
            },
            C::Shadow { r, g, b, a, .. } => S::Shadow {
                x: f(0),
                y: f(1),
                w: f(2),
                h: f(3),
                radius: f(4),
                blur: f(5),
                spread: f(6),
                dx: f(7),
                dy: f(8),
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            C::DrawCanvas { id, a, .. } => S::DrawCanvas {
                id: *id,
                x: f(1),
                y: f(2),
                a: *a,
                w: of(4, 0.0),
                h: of(5, 0.0),
            },
            _ => return None,
        })
    }

    /// `cmd` as a sub-pixel command with the same, whole, numbers — so a host
    /// can draw a whole frame through one path. `None` for a command with no
    /// sub-pixel twin (clips, canvases, markers, `Host`).
    pub fn from_px(cmd: &DrawCommand) -> Option<SubCommand> {
        use DrawCommand as C;
        use SubCommand as S;
        let f = |v: i32| v as f32;
        let u = |v: u32| v as f32;
        Some(match cmd.clone() {
            C::Image { source, x, y, w, h, a, radius, tint, src, slice } => S::Image {
                source,
                x: f(x),
                y: f(y),
                w: u(w),
                h: u(h),
                a,
                radius: u(radius),
                tint,
                src,
                slice,
            },
            C::Rect { x, y, w, h, r, g, b, a, radius } => {
                S::Rect { x: f(x), y: f(y), w: u(w), h: u(h), r, g, b, a, radius: u(radius) }
            }
            C::RectOutline { x, y, w, h, r, g, b, a, width, radius } => S::RectOutline {
                x: f(x),
                y: f(y),
                w: u(w),
                h: u(h),
                r,
                g,
                b,
                a,
                width: u(width),
                radius: u(radius),
            },
            C::Line { x1, y1, x2, y2, r, g, b, a, width } => S::Line {
                x1: f(x1),
                y1: f(y1),
                x2: f(x2),
                y2: f(y2),
                r,
                g,
                b,
                a,
                width: u(width),
            },
            C::Circle { cx, cy, radius, r, g, b, a } => {
                S::Circle { cx: f(cx), cy: f(cy), radius: f(radius), r, g, b, a }
            }
            C::Text { text, x, y, size, r, g, b, a, font, weight, italic, spacing } => S::Text {
                text,
                x: f(x),
                y: f(y),
                size,
                r,
                g,
                b,
                a,
                font,
                weight,
                italic,
                spacing,
            },
            C::Triangle { x1, y1, x2, y2, x3, y3, r, g, b, a } => S::Triangle {
                x1: f(x1),
                y1: f(y1),
                x2: f(x2),
                y2: f(y2),
                x3: f(x3),
                y3: f(y3),
                r,
                g,
                b,
                a,
            },
            C::Poly { points, r, g, b, a } => S::Poly { points: points_f32(&points), r, g, b, a },
            C::Polygon { points, r, g, b, a } => {
                S::Polygon { points: points_f32(&points), r, g, b, a }
            }
            C::Fan { cx, cy, points, r, g, b, a } => {
                S::Fan { cx: f(cx), cy: f(cy), points: points_f32(&points), r, g, b, a }
            }
            C::Polyline { points, r, g, b, a, width } => {
                S::Polyline { points: points_f32(&points), r, g, b, a, width: u(width) }
            }
            C::Ellipse { cx, cy, rx, ry, r, g, b, a } => {
                S::Ellipse { cx: f(cx), cy: f(cy), rx: f(rx), ry: f(ry), r, g, b, a }
            }
            C::EllipseOutline { cx, cy, rx, ry, r, g, b, a, width } => S::EllipseOutline {
                cx: f(cx),
                cy: f(cy),
                rx: f(rx),
                ry: f(ry),
                r,
                g,
                b,
                a,
                width: u(width),
            },
            C::Arc { cx, cy, r_in, r_out, a0, a1, r, g, b, a } => {
                S::Arc { cx: f(cx), cy: f(cy), r_in, r_out, a0, a1, r, g, b, a }
            }
            C::RectGradient { x, y, w, h, radius, r0, g0, b0, a0, r1, g1, b1, a1, angle } => {
                S::RectGradient {
                    x: f(x),
                    y: f(y),
                    w: u(w),
                    h: u(h),
                    radius: u(radius),
                    r0,
                    g0,
                    b0,
                    a0,
                    r1,
                    g1,
                    b1,
                    a1,
                    angle,
                }
            }
            C::CircleGradient { cx, cy, radius, r0, g0, b0, a0, r1, g1, b1, a1 } => {
                S::CircleGradient {
                    cx: f(cx),
                    cy: f(cy),
                    radius: f(radius),
                    r0,
                    g0,
                    b0,
                    a0,
                    r1,
                    g1,
                    b1,
                    a1,
                }
            }
            C::Shadow { x, y, w, h, radius, blur, spread, dx, dy, r, g, b, a } => S::Shadow {
                x: f(x),
                y: f(y),
                w: u(w),
                h: u(h),
                radius: u(radius),
                blur: u(blur),
                spread: f(spread),
                dx: f(dx),
                dy: f(dy),
                r,
                g,
                b,
                a,
            },
            C::DrawCanvas { id, x, y, a, w, h } => {
                S::DrawCanvas { id, x: f(x), y: f(y), a, w: u(w), h: u(h) }
            }
            _ => return None,
        })
    }

    /// The command in whole pixels, truncated toward zero as the whole-pixel
    /// drains truncate: `Sub` → `Px` for a host that draws a command in whole
    /// pixels after all. For a command whose numbers are whole this is
    /// exactly the inverse of [`from_px`](Self::from_px).
    pub fn to_px(&self) -> DrawCommand {
        use DrawCommand as C;
        use SubCommand as S;
        let i = |v: f32| v as i64 as i32;
        let u = |v: f32| v as i64 as u32;
        let pts = |p: &[(f32, f32)]| p.iter().map(|&(x, y)| (i(x), i(y))).collect();
        match self.clone() {
            S::Image { source, x, y, w, h, a, radius, tint, src, slice } => C::Image {
                source,
                x: i(x),
                y: i(y),
                w: u(w),
                h: u(h),
                a,
                radius: u(radius),
                tint,
                src,
                slice,
            },
            S::Rect { x, y, w, h, r, g, b, a, radius } => {
                C::Rect { x: i(x), y: i(y), w: u(w), h: u(h), r, g, b, a, radius: u(radius) }
            }
            S::RectOutline { x, y, w, h, r, g, b, a, width, radius } => C::RectOutline {
                x: i(x),
                y: i(y),
                w: u(w),
                h: u(h),
                r,
                g,
                b,
                a,
                width: u(width),
                radius: u(radius),
            },
            S::Line { x1, y1, x2, y2, r, g, b, a, width } => C::Line {
                x1: i(x1),
                y1: i(y1),
                x2: i(x2),
                y2: i(y2),
                r,
                g,
                b,
                a,
                width: u(width),
            },
            S::Circle { cx, cy, radius, r, g, b, a } => {
                C::Circle { cx: i(cx), cy: i(cy), radius: i(radius), r, g, b, a }
            }
            S::Text { text, x, y, size, r, g, b, a, font, weight, italic, spacing } => C::Text {
                text,
                x: i(x),
                y: i(y),
                size,
                r,
                g,
                b,
                a,
                font,
                weight,
                italic,
                spacing,
            },
            S::Triangle { x1, y1, x2, y2, x3, y3, r, g, b, a } => C::Triangle {
                x1: i(x1),
                y1: i(y1),
                x2: i(x2),
                y2: i(y2),
                x3: i(x3),
                y3: i(y3),
                r,
                g,
                b,
                a,
            },
            S::Poly { points, r, g, b, a } => C::Poly { points: pts(&points), r, g, b, a },
            S::Polygon { points, r, g, b, a } => C::Polygon { points: pts(&points), r, g, b, a },
            S::Fan { cx, cy, points, r, g, b, a } => {
                C::Fan { cx: i(cx), cy: i(cy), points: pts(&points), r, g, b, a }
            }
            S::Polyline { points, r, g, b, a, width } => {
                C::Polyline { points: pts(&points), r, g, b, a, width: u(width) }
            }
            S::Ellipse { cx, cy, rx, ry, r, g, b, a } => {
                C::Ellipse { cx: i(cx), cy: i(cy), rx: i(rx), ry: i(ry), r, g, b, a }
            }
            S::EllipseOutline { cx, cy, rx, ry, r, g, b, a, width } => C::EllipseOutline {
                cx: i(cx),
                cy: i(cy),
                rx: i(rx),
                ry: i(ry),
                r,
                g,
                b,
                a,
                width: u(width),
            },
            S::Arc { cx, cy, r_in, r_out, a0, a1, r, g, b, a } => {
                C::Arc { cx: i(cx), cy: i(cy), r_in, r_out, a0, a1, r, g, b, a }
            }
            S::RectGradient { x, y, w, h, radius, r0, g0, b0, a0, r1, g1, b1, a1, angle } => {
                C::RectGradient {
                    x: i(x),
                    y: i(y),
                    w: u(w),
                    h: u(h),
                    radius: u(radius),
                    r0,
                    g0,
                    b0,
                    a0,
                    r1,
                    g1,
                    b1,
                    a1,
                    angle,
                }
            }
            S::CircleGradient { cx, cy, radius, r0, g0, b0, a0, r1, g1, b1, a1 } => {
                C::CircleGradient {
                    cx: i(cx),
                    cy: i(cy),
                    radius: i(radius),
                    r0,
                    g0,
                    b0,
                    a0,
                    r1,
                    g1,
                    b1,
                    a1,
                }
            }
            S::Shadow { x, y, w, h, radius, blur, spread, dx, dy, r, g, b, a } => C::Shadow {
                x: i(x),
                y: i(y),
                w: u(w),
                h: u(h),
                radius: u(radius),
                blur: u(blur),
                spread: i(spread),
                dx: i(dx),
                dy: i(dy),
                r,
                g,
                b,
                a,
            },
            S::DrawCanvas { id, x, y, a, w, h } => {
                C::DrawCanvas { id, x: i(x), y: i(y), a, w: u(w), h: u(h) }
            }
        }
    }

    /// `self` moved by (`dx`, `dy`), in `f32`.
    pub fn translate(mut self, dx: f32, dy: f32) -> SubCommand {
        use SubCommand::*;
        if dx == 0.0 && dy == 0.0 {
            return self;
        }
        let mv = |points: &mut Vec<(f32, f32)>| {
            for p in points.iter_mut() {
                p.0 += dx;
                p.1 += dy;
            }
        };
        match &mut self {
            Image { x, y, .. }
            | Rect { x, y, .. }
            | RectOutline { x, y, .. }
            | Text { x, y, .. }
            | RectGradient { x, y, .. }
            | Shadow { x, y, .. }
            | DrawCanvas { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Line { x1, y1, x2, y2, .. } => {
                *x1 += dx;
                *y1 += dy;
                *x2 += dx;
                *y2 += dy;
            }
            Triangle { x1, y1, x2, y2, x3, y3, .. } => {
                *x1 += dx;
                *y1 += dy;
                *x2 += dx;
                *y2 += dy;
                *x3 += dx;
                *y3 += dy;
            }
            Circle { cx, cy, .. }
            | Ellipse { cx, cy, .. }
            | EllipseOutline { cx, cy, .. }
            | Arc { cx, cy, .. }
            | CircleGradient { cx, cy, .. } => {
                *cx += dx;
                *cy += dy;
            }
            Poly { points, .. } | Polygon { points, .. } | Polyline { points, .. } => mv(points),
            Fan { cx, cy, points, .. } => {
                *cx += dx;
                *cy += dy;
                mv(points);
            }
        }
        self
    }

    /// `self` magnified by `s` about the origin — the sub-pixel counterpart of
    /// [`crate::compose::scale`], for a HiDPI drawable. Nothing is rounded;
    /// a text size is, as there (`size` is whole), and is at least 1.
    pub fn scale(mut self, s: f32) -> SubCommand {
        use SubCommand::*;
        if s == 1.0 {
            return self;
        }
        let sc = |points: &mut Vec<(f32, f32)>| {
            for p in points.iter_mut() {
                p.0 *= s;
                p.1 *= s;
            }
        };
        match &mut self {
            Image { x, y, w, h, radius, .. }
            | Rect { x, y, w, h, radius, .. }
            | RectGradient { x, y, w, h, radius, .. } => {
                for v in [x, y, w, h, radius] {
                    *v *= s;
                }
            }
            RectOutline { x, y, w, h, width, radius, .. } => {
                for v in [x, y, w, h, width, radius] {
                    *v *= s;
                }
            }
            Shadow { x, y, w, h, radius, blur, spread, dx, dy, .. } => {
                for v in [x, y, w, h, radius, blur, spread, dx, dy] {
                    *v *= s;
                }
            }
            Text { x, y, size, spacing, .. } => {
                *x *= s;
                *y *= s;
                *size = ((*size as f32 * s).round() as u16).max(1);
                *spacing *= s;
            }
            Line { x1, y1, x2, y2, width, .. } => {
                for v in [x1, y1, x2, y2, width] {
                    *v *= s;
                }
            }
            Triangle { x1, y1, x2, y2, x3, y3, .. } => {
                for v in [x1, y1, x2, y2, x3, y3] {
                    *v *= s;
                }
            }
            Circle { cx, cy, radius, .. } | CircleGradient { cx, cy, radius, .. } => {
                for v in [cx, cy, radius] {
                    *v *= s;
                }
            }
            Ellipse { cx, cy, rx, ry, .. } => {
                for v in [cx, cy, rx, ry] {
                    *v *= s;
                }
            }
            EllipseOutline { cx, cy, rx, ry, width, .. } => {
                for v in [cx, cy, rx, ry, width] {
                    *v *= s;
                }
            }
            Arc { cx, cy, r_in, r_out, .. } => {
                for v in [cx, cy, r_in, r_out] {
                    *v *= s;
                }
            }
            Poly { points, .. } | Polygon { points, .. } => sc(points),
            Polyline { points, width, .. } => {
                sc(points);
                *width *= s;
            }
            Fan { cx, cy, points, .. } => {
                *cx *= s;
                *cy *= s;
                sc(points);
            }
            DrawCanvas { x, y, w, h, .. } => {
                // 0 = the canvas's own size, which is already scaled.
                for v in [x, y, w, h] {
                    *v *= s;
                }
            }
        }
        self
    }
}

// ── Drains ────────────────────────────────────────────────────────────────

/// Drain the `draw_commands` buffer as a subpixel host draws it: the
/// [`take_draw_commands`](crate::draw::take_draw_commands) frame — layers
/// sorted, retained layers resolved, batches lowered, the same commands in
/// the same order — with each primitive the [`PixelSnap`] mode leaves
/// unsnapped as a [`FrameCommand::Sub`] carrying the script's own numbers.
/// A retained layer's content is moved to its `cache_draw` in `f32`.
///
/// Meant for an env registered with [`DrawOptions::subpixel`](crate::draw::DrawOptions::subpixel);
/// on any other env the natives have already truncated, so a `Sub` only
/// differs from its `Px` in a point list's fractions.
pub fn take_frame_commands(env: &mut Env) -> Vec<FrameCommand> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer(sym);
    let d = decode_buffer(&values, env.heap());
    let items = compose_items(env, None, d.cmds, d.values, &d.snaps);
    let heap = env.heap();
    items.into_iter().map(|item| frame_command(item, heap)).collect()
}

/// [`take_frame_commands`] with each command's call chain, as
/// [`take_draw_commands_traced`](crate::draw::take_draw_commands_traced)
/// gives them: the two attribute every command alike.
pub fn take_frame_commands_traced(env: &mut Env) -> Vec<(FrameCommand, EmitSite)> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer(sym);
    let mut origins = env.take_output_origins(sym);
    let d = decode_buffer(&values, env.heap());
    let sites = take_sites(&mut origins, &d.at);
    let items = compose_items(env, None, d.cmds, d.values, &d.snaps);
    let heap = env.heap();
    items
        .into_iter()
        .map(|item| {
            let site = site_of(&sites, item.from);
            (frame_command(item, heap), site)
        })
        .collect()
}

/// [`take_frame_commands`] with the [`PixelSnap`] mode each command was drawn
/// under, for a host whose rendering depends on it beyond the geometry: a
/// `Px` image is whole pixels both by default and under `pixel_snap(true)`,
/// but only the second asks for pixel art (no resampling smoothing). A
/// command with no script origin (a clip a layer made) reports the default.
pub fn take_frame_commands_snapped(env: &mut Env) -> Vec<(FrameCommand, PixelSnap)> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer(sym);
    let d = decode_buffer(&values, env.heap());
    let items = compose_items(env, None, d.cmds, d.values, &d.snaps);
    let heap = env.heap();
    items
        .into_iter()
        .map(|item| {
            let snap = item.src.as_ref().map(|s| s.snap).unwrap_or_default();
            (frame_command(item, heap), snap)
        })
        .collect()
}

/// [`take_frame_commands`] for a *forked* stack, as
/// [`take_draw_commands_for`](crate::draw::take_draw_commands_for).
pub fn take_frame_commands_for(env: &mut Env, stack_id: StackKey) -> Vec<FrameCommand> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer_for(stack_id, sym);
    let d = match env.heap_for(stack_id) {
        Some(heap) => decode_buffer(&values, heap),
        None => return Vec::new(),
    };
    let items = compose_items(env, Some(stack_id), d.cmds, d.values, &d.snaps);
    let Some(heap) = env.heap_for(stack_id) else {
        return Vec::new();
    };
    items.into_iter().map(|item| frame_command(item, heap)).collect()
}

// ── The natives, sub-pixel ────────────────────────────────────────────────
//
// Each is its whole-pixel twin in `crate::draw` with the geometry read by
// `num` instead of `get_int`: the number goes into the buffer as the script
// wrote it, and a non-number fails with the twin's message. Everything else
// — colours, alpha, ids, sizes, the argument shapes, the emitted arg lists —
// is the twin's, so a whole-pixel drain decodes either the same.

/// Replace the geometry natives with their sub-pixel twins, and `pixel_snap`
/// with the one that emits its marker. Part of
/// [`register_draw_with`](crate::draw::register_draw_with).
pub(crate) fn register_subpixel(env: &mut Env) {
    register(env, "draw_image", native_draw_image, DRAW);
    register(env, "draw_rect", native_draw_rect, DRAW);
    register(env, "draw_rect_rounded", native_draw_rect_rounded, DRAW);
    register(env, "draw_rect_outline", native_draw_rect_outline, DRAW);
    register(env, "draw_rect_rounded_outline", native_draw_rect_rounded_outline, DRAW);
    register(env, "draw_rect_gradient", native_draw_rect_gradient, DRAW);
    register(env, "draw_rect_gradient_rounded", native_draw_rect_gradient_rounded, DRAW);
    register(env, "draw_circle_gradient", native_draw_circle_gradient, DRAW);
    register(env, "draw_shadow", native_draw_shadow, DRAW);
    register(env, "draw_line", native_draw_line, DRAW);
    register(env, "draw_polyline", native_draw_polyline, DRAW);
    register(env, "draw_circle", native_draw_circle, DRAW);
    register(env, "draw_circle_outline", native_draw_circle_outline, DRAW);
    register(env, "draw_ellipse", native_draw_ellipse, DRAW);
    register(env, "draw_ellipse_outline", native_draw_ellipse_outline, DRAW);
    register(env, "fill_arc", native_fill_arc, DRAW);
    register(env, "fill_triangle", native_fill_triangle, DRAW);
    register(env, "fill_fan", native_fill_fan, DRAW);
    register(env, "draw_text", native_draw_text, DRAW);
    register(env, "draw_canvas", native_draw_canvas, DRAW);
    register(env, "pixel_snap", native_pixel_snap, DRAW);
}

/// A geometry argument: an int or a float, as it is.
fn num(state: &PetalCxt, index: usize) -> Result<Value, String> {
    match state.get_value(index)? {
        v @ (Value::Int(_) | Value::Float(_)) => Ok(v),
        other => Err(format!(
            "Expected int at arg {index}, got {}",
            other.type_name()
        )),
    }
}

/// Arguments `first..=last` as geometry.
fn nums(state: &PetalCxt, first: usize, last: usize) -> Result<Vec<Value>, String> {
    (first..=last).map(|i| num(state, i)).collect()
}

/// Arguments `first..=last` as whole numbers, as the twin reads them.
fn ints(state: &PetalCxt, first: usize, last: usize) -> Result<Vec<Value>, String> {
    (first..=last).map(|i| state.get_int(i).map(Value::Int)).collect()
}

/// An optional geometry argument, or `default`.
fn opt_num(state: &PetalCxt, index: usize, default: i64) -> Result<Value, String> {
    if state.arg_count() >= index {
        num(state, index)
    } else {
        Ok(Value::Int(default))
    }
}

/// An optional whole-number argument, or `default`.
fn opt_i(state: &PetalCxt, index: usize, default: i64) -> Result<Value, String> {
    opt_int(state, index, default).map(Value::Int)
}

fn done(state: &mut PetalCxt, tag: &str, args: Vec<Value>) -> NativeResult {
    emit_draw(state, tag, args);
    state.push_nil();
    Ok(1)
}

/// `v` moved by a whole `d` px. A fractional `v` keeps its fraction, unless
/// that would truncate to something other than `trunc(v) + d` (a move across
/// zero): then it is the whole number, so a whole-pixel drain still sees
/// exactly what the whole-pixel native would have emitted.
fn shift(v: Value, d: i64) -> Value {
    match v {
        Value::Float(f) => {
            let moved = f + d as f64;
            let whole = f as i64 + d;
            if moved as i64 == whole { Value::Float(moved) } else { Value::Int(whole) }
        }
        Value::Int(n) => Value::Int(n + d),
        other => other,
    }
}

fn native_pixel_snap(state: &mut PetalCxt) -> NativeResult {
    let on = state.get_value(1)?;
    let mode = PixelSnap::from_arg(&on)?;
    done(state, PIXEL_SNAP_TAG, vec![Value::Int(mode.code())])
}

fn native_draw_image(state: &mut PetalCxt) -> NativeResult {
    let source = state.get_string(1)?;
    let source = Value::String(state.heap_mut().alloc_string(source));
    let mut args = vec![source];
    args.extend(nums(state, 2, 5)?);
    args.push(opt_i(state, 6, 255)?);
    args.push(opt_num(state, 7, 0)?); // radius
    image_tail(state, &mut args)?;
    done(state, "image", args)
}

/// The `{x, y, w, h}` record at `index`, its fields as they are.
fn rect_nums(state: &PetalCxt, index: usize, who: &str) -> Result<Vec<Value>, String> {
    let v = state.get_value(index)?;
    let Value::Map(id) = v else {
        return Err(format!(
            "{who}() expects a rect record at arg {index}, got {}",
            v.type_name()
        ));
    };
    let map = state.heap().get_map(id);
    ["x", "y", "w", "h"]
        .iter()
        .map(|name| match map.get(*name) {
            Some(v @ (Value::Int(_) | Value::Float(_))) => Ok(*v),
            Some(v) => Err(format!(
                "{who}() expects a number for rect field '{name}', got {}",
                v.type_name()
            )),
            None => Err(format!("{who}() expects a rect with field '{name}'")),
        })
        .collect()
}

fn native_draw_rect(state: &mut PetalCxt) -> NativeResult {
    let n = state.arg_count();
    let args = if n >= 7 {
        let mut args = nums(state, 1, 4)?;
        args.extend(ints(state, 5, 7)?);
        args.push(opt_i(state, 8, 255)?);
        args
    } else if n >= 5 {
        let mut args = nums(state, 1, 4)?;
        args.extend(color_arg(state, 5, "draw_rect")?);
        args.push(opt_i(state, 6, 255)?);
        args
    } else if n >= 2 {
        let mut args = rect_nums(state, 1, "draw_rect")?;
        args.extend(color_arg(state, 2, "draw_rect")?);
        args.push(opt_i(state, 3, 255)?);
        args
    } else {
        return Err(format!(
            "draw_rect() takes (rect, c, [a]), (x, y, w, h, c, [a]) or (x, y, w, h, r, g, b, [a]); got {n} arguments"
        ));
    };
    done(state, "rect", args)
}

fn native_draw_rect_rounded(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 4)?;
    let radius = num(state, 5)?;
    args.extend(ints(state, 6, 8)?);
    args.push(opt_i(state, 9, 255)?);
    args.push(radius);
    done(state, "rect", args)
}

fn native_draw_rect_outline(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 4)?;
    args.extend(ints(state, 5, 7)?);
    args.push(opt_i(state, 8, 255)?); // a
    args.push(opt_num(state, 9, 1)?); // width
    args.push(Value::Int(0)); // radius: square corners
    done(state, "rect_outline", args)
}

fn native_draw_rect_rounded_outline(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 4)?;
    let radius = num(state, 5)?;
    args.extend(ints(state, 6, 8)?);
    args.push(opt_i(state, 9, 255)?);
    args.push(opt_num(state, 10, 1)?);
    args.push(radius);
    done(state, "rect_outline", args)
}

/// The `rect_gradient` arg list: `x, y, w, h, radius`, the two stops, the
/// angle — the order of `crate::draw`'s builder.
fn rect_gradient(
    state: &mut PetalCxt,
    radius: Option<usize>,
    stops: usize,
    angle: usize,
    who: &str,
) -> NativeResult {
    let mut args = nums(state, 1, 4)?;
    args.push(match radius {
        Some(i) => num(state, i)?,
        None => Value::Int(0),
    });
    let (c0, c1) = gradient_stops(state, stops)?;
    let angle = get_num(state, angle, who)?;
    args.extend(c0.iter().chain(&c1).map(|&c| Value::Int(c)));
    args.push(Value::Float(angle));
    done(state, "rect_gradient", args)
}

fn native_draw_rect_gradient(state: &mut PetalCxt) -> NativeResult {
    rect_gradient(state, None, 5, 13, "draw_rect_gradient")
}

fn native_draw_rect_gradient_rounded(state: &mut PetalCxt) -> NativeResult {
    rect_gradient(state, Some(5), 6, 14, "draw_rect_gradient_rounded")
}

fn native_draw_circle_gradient(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 3)?;
    let (c0, c1) = gradient_stops(state, 4)?;
    args.extend(c0.iter().chain(&c1).map(|&c| Value::Int(c)));
    done(state, "circle_gradient", args)
}

fn native_draw_shadow(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 9)?;
    // blur, at least 0 (the twin's `blur.max(0)`).
    args[5] = match args[5] {
        Value::Int(n) => Value::Int(n.max(0)),
        Value::Float(f) => Value::Float(f.max(0.0)),
        other => other,
    };
    args.extend(ints(state, 10, 12)?);
    args.push(opt_i(state, 13, 255)?);
    done(state, "shadow", args)
}

fn native_draw_line(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 4)?;
    args.extend(ints(state, 5, 7)?);
    args.push(opt_i(state, 8, 255)?); // a
    args.push(opt_num(state, 9, 1)?); // width
    done(state, "line", args)
}

fn native_draw_polyline(state: &mut PetalCxt) -> NativeResult {
    let points_value = point_list_arg(state, 1, "draw_polyline", 1)?;
    let n = state.arg_count();
    let one_colour = n < 4 || (n == 4 && matches!(state.get_value(2)?, Value::Map(_)));
    let (rgb, next) = if one_colour {
        (color_arg(state, 2, "draw_polyline")?, 3)
    } else {
        (
            [
                Value::Int(state.get_int(2)?),
                Value::Int(state.get_int(3)?),
                Value::Int(state.get_int(4)?),
            ],
            5,
        )
    };
    let a = opt_i(state, next, 255)?;
    let width = opt_num(state, next + 1, 1)?;
    let [r, g, b] = rgb;
    done(state, "polyline", vec![points_value, r, g, b, a, width])
}

fn native_draw_circle(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 3)?;
    args.extend(ints(state, 4, 6)?);
    args.push(opt_i(state, 7, 255)?); // a
    done(state, "circle", args)
}

fn native_draw_circle_outline(state: &mut PetalCxt) -> NativeResult {
    let cx = num(state, 1)?;
    let cy = num(state, 2)?;
    let radius = num(state, 3)?;
    let mut args = vec![cx, cy, radius, radius];
    args.extend(ints(state, 4, 6)?);
    args.push(opt_i(state, 7, 255)?); // a
    args.push(opt_num(state, 8, 1)?); // width
    done(state, "ellipse_outline", args)
}

fn native_draw_ellipse(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 4)?;
    args.extend(ints(state, 5, 7)?);
    args.push(opt_i(state, 8, 255)?); // a
    done(state, "ellipse", args)
}

fn native_draw_ellipse_outline(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 4)?;
    args.extend(ints(state, 5, 7)?);
    args.push(opt_i(state, 8, 255)?); // a
    args.push(opt_num(state, 9, 1)?); // width
    done(state, "ellipse_outline", args)
}

fn native_fill_arc(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 2)?;
    for i in 3..=6 {
        args.push(Value::Float(get_num(state, i, "fill_arc")?));
    }
    args.extend(ints(state, 7, 9)?);
    args.push(opt_i(state, 10, 255)?);
    done(state, "arc", args)
}

fn native_fill_triangle(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 6)?;
    args.extend(ints(state, 7, 9)?);
    args.push(opt_i(state, 10, 255)?); // a
    done(state, "triangle", args)
}

fn native_fill_fan(state: &mut PetalCxt) -> NativeResult {
    let mut args = nums(state, 1, 2)?;
    args.push(point_list_arg(state, 3, "fill_fan", 2)?);
    args.extend(ints(state, 4, 6)?);
    args.push(opt_i(state, 7, 255)?);
    done(state, "fan", args)
}

fn native_draw_text(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let x = num(state, 2)?;
    let y = num(state, 3)?;
    // The arg list the twin emits, then the position put back as written.
    let at = |mut args: Vec<Value>, dx: i64, dy: i64| {
        args[1] = shift(x, dx);
        args[2] = shift(y, dy);
        args
    };
    let styled = state.arg_count() == 4 && matches!(state.get_value(4)?, Value::Map(_));
    let args = if styled {
        let style_value = state.get_value(4)?;
        let style = TextStyle::from_value(state, &style_value)?;
        for (dx, dy, deco) in text_decorations(state, &style_value, &style) {
            let args = deco.emit_args(state, text.clone(), 0, 0);
            emit_draw(state, "text", at(args, dx, dy));
        }
        at(style.emit_args(state, text, 0, 0), 0, 0)
    } else {
        let style = TextStyle {
            size: state.get_int(4)?,
            r: state.get_int(5)? as u8,
            g: state.get_int(6)? as u8,
            b: state.get_int(7)? as u8,
            a: opt_int(state, 8, 255)? as u8,
            ..TextStyle::default()
        };
        at(style.emit_args(state, text, 0, 0), 0, 0)
    };
    done(state, "text", args)
}

fn native_draw_canvas(state: &mut PetalCxt) -> NativeResult {
    let mut args = vec![Value::Int(state.get_int(1)?)];
    args.extend(nums(state, 2, 3)?);
    args.push(opt_i(state, 4, 255)?);
    args.push(opt_num(state, 5, 0)?);
    args.push(opt_num(state, 6, 0)?);
    done(state, "draw_canvas", args)
}
