//! petal-ui `DrawCommand`s as flat `pb_draw_cmd` structs.
//!
//! Each command becomes one fixed-size struct with a kind tag; the fields a
//! kind uses are documented on `pb_draw_kind` in the header and everything
//! else is zero. Variable-sized payloads (text, point lists, host-command
//! arguments) live in side buffers owned by the same [`DrawList`].

use std::ffi::{CString, c_char};
use std::ptr;

use petal::heap::Heap;
use petal_ui::draw::DrawCommand;

use crate::ffi::cstring_lossy;
use crate::view::{Names, PbValue, ViewArena};

/// Mirrors `pb_rgba`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PbRgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

fn rgba(r: u8, g: u8, b: u8, a: u8) -> PbRgba {
    PbRgba { r, g, b, a }
}

/// Mirrors `pb_draw_kind`.
#[repr(u32)]
#[derive(Clone, Copy)]
pub enum DrawKind {
    Image = 0,
    Clear,
    Rect,
    RectOutline,
    Line,
    Circle,
    Text,
    Triangle,
    Poly,
    Polygon,
    Fan,
    Polyline,
    Ellipse,
    EllipseOutline,
    Arc,
    RectGradient,
    CircleGradient,
    Shadow,
    Clip,
    ClipNone,
    ClipPush,
    ClipPop,
    CreateCanvas,
    SetTarget,
    DrawCanvas,
    Snapshot,
    BlurCanvas,
    Host,
}

/// Mirrors `pb_draw_cmd`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PbDrawCmd {
    pub kind: u32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
    pub x3: i32,
    pub y3: i32,
    pub cx: i32,
    pub cy: i32,
    pub rx: i32,
    pub ry: i32,
    pub radius: i32,
    pub width: i32,
    pub color: PbRgba,
    pub color2: PbRgba,
    pub r_in: f32,
    pub r_out: f32,
    pub a0: f32,
    pub a1: f32,
    pub angle: f32,
    pub blur: i32,
    pub spread: i32,
    pub dx: i32,
    pub dy: i32,
    pub id: u32,
    pub text: *const c_char,
    pub text_len: usize,
    pub font: *const c_char,
    pub size: u16,
    pub weight: u16,
    pub italic: u8,
    pub spacing: f32,
    pub points: *const i32,
    pub point_count: usize,
    pub data: *const PbValue,
    pub data_count: usize,
}

impl PbDrawCmd {
    fn new(kind: DrawKind) -> Self {
        PbDrawCmd {
            kind: kind as u32,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            x1: 0,
            y1: 0,
            x2: 0,
            y2: 0,
            x3: 0,
            y3: 0,
            cx: 0,
            cy: 0,
            rx: 0,
            ry: 0,
            radius: 0,
            width: 0,
            color: PbRgba::default(),
            color2: PbRgba::default(),
            r_in: 0.0,
            r_out: 0.0,
            a0: 0.0,
            a1: 0.0,
            angle: 0.0,
            blur: 0,
            spread: 0,
            dx: 0,
            dy: 0,
            id: 0,
            text: ptr::null(),
            text_len: 0,
            font: ptr::null(),
            size: 0,
            weight: 0,
            italic: 0,
            spacing: 0.0,
            points: ptr::null(),
            point_count: 0,
            data: ptr::null(),
            data_count: 0,
        }
    }

    fn rect(mut self, x: i32, y: i32, w: u32, h: u32) -> Self {
        self.x = x;
        self.y = y;
        self.w = w as i32;
        self.h = h as i32;
        self
    }
}

/// One frame's decoded draw commands plus the buffers they point into.
/// Every side buffer is its own heap allocation (a `CString`, a boxed slice,
/// an arena's vectors), so pointers taken while building stay valid when the
/// outer vectors grow or the list itself moves.
#[derive(Default)]
pub struct DrawList {
    pub cmds: Vec<PbDrawCmd>,
    strings: Vec<CString>,
    points: Vec<Box<[i32]>>,
    host_data: Vec<ViewArena>,
}

impl DrawList {
    fn keep_str(&mut self, s: &str) -> (*const c_char, usize) {
        let c = cstring_lossy(s);
        let p = c.as_ptr();
        self.strings.push(c);
        (p, s.len())
    }

    fn keep_points(&mut self, pts: &[(i32, i32)]) -> (*const i32, usize) {
        let flat: Box<[i32]> = pts.iter().flat_map(|(x, y)| [*x, *y]).collect();
        let p = flat.as_ptr();
        self.points.push(flat);
        (p, pts.len())
    }

    /// Convert and append one command. `heap`/`names` decode `Host` args.
    pub fn push(&mut self, cmd: &DrawCommand, heap: &Heap, names: &Names) {
        use DrawCommand as D;
        let c = match cmd {
            D::Image {
                source,
                x,
                y,
                w,
                h,
                a,
                radius,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Image).rect(*x, *y, *w, *h);
                (c.text, c.text_len) = self.keep_str(source);
                c.color = rgba(255, 255, 255, *a);
                c.radius = *radius as i32;
                c
            }
            D::Clear { r, g, b } => {
                let mut c = PbDrawCmd::new(DrawKind::Clear);
                c.color = rgba(*r, *g, *b, 255);
                c
            }
            D::Rect {
                x,
                y,
                w,
                h,
                r,
                g,
                b,
                a,
                radius,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Rect).rect(*x, *y, *w, *h);
                c.color = rgba(*r, *g, *b, *a);
                c.radius = *radius as i32;
                c
            }
            D::RectOutline {
                x,
                y,
                w,
                h,
                r,
                g,
                b,
                a,
                width,
                radius,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::RectOutline).rect(*x, *y, *w, *h);
                c.color = rgba(*r, *g, *b, *a);
                c.width = *width as i32;
                c.radius = *radius as i32;
                c
            }
            D::Line {
                x1,
                y1,
                x2,
                y2,
                r,
                g,
                b,
                a,
                width,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Line);
                (c.x1, c.y1, c.x2, c.y2) = (*x1, *y1, *x2, *y2);
                c.color = rgba(*r, *g, *b, *a);
                c.width = *width as i32;
                c
            }
            D::Circle {
                cx,
                cy,
                radius,
                r,
                g,
                b,
                a,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Circle);
                (c.cx, c.cy, c.rx, c.ry, c.radius) = (*cx, *cy, *radius, *radius, *radius);
                c.color = rgba(*r, *g, *b, *a);
                c
            }
            D::Text {
                text,
                x,
                y,
                size,
                r,
                g,
                b,
                a,
                font,
                weight,
                italic,
                spacing,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Text);
                (c.x, c.y) = (*x, *y);
                (c.text, c.text_len) = self.keep_str(text);
                if let Some(f) = font {
                    c.font = self.keep_str(f).0;
                }
                c.size = *size;
                c.weight = *weight;
                c.italic = *italic as u8;
                c.spacing = *spacing;
                c.color = rgba(*r, *g, *b, *a);
                c
            }
            D::Triangle {
                x1,
                y1,
                x2,
                y2,
                x3,
                y3,
                r,
                g,
                b,
                a,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Triangle);
                (c.x1, c.y1, c.x2, c.y2, c.x3, c.y3) = (*x1, *y1, *x2, *y2, *x3, *y3);
                c.color = rgba(*r, *g, *b, *a);
                c
            }
            D::Poly { points, r, g, b, a } | D::Polygon { points, r, g, b, a } => {
                let kind = if matches!(cmd, D::Poly { .. }) {
                    DrawKind::Poly
                } else {
                    DrawKind::Polygon
                };
                let mut c = PbDrawCmd::new(kind);
                (c.points, c.point_count) = self.keep_points(points);
                c.color = rgba(*r, *g, *b, *a);
                c
            }
            D::Fan {
                cx,
                cy,
                points,
                r,
                g,
                b,
                a,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Fan);
                (c.cx, c.cy) = (*cx, *cy);
                (c.points, c.point_count) = self.keep_points(points);
                c.color = rgba(*r, *g, *b, *a);
                c
            }
            D::Polyline {
                points,
                r,
                g,
                b,
                a,
                width,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Polyline);
                (c.points, c.point_count) = self.keep_points(points);
                c.color = rgba(*r, *g, *b, *a);
                c.width = *width as i32;
                c
            }
            D::Ellipse {
                cx,
                cy,
                rx,
                ry,
                r,
                g,
                b,
                a,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Ellipse);
                (c.cx, c.cy, c.rx, c.ry) = (*cx, *cy, *rx, *ry);
                c.color = rgba(*r, *g, *b, *a);
                c
            }
            D::EllipseOutline {
                cx,
                cy,
                rx,
                ry,
                r,
                g,
                b,
                a,
                width,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::EllipseOutline);
                (c.cx, c.cy, c.rx, c.ry) = (*cx, *cy, *rx, *ry);
                c.color = rgba(*r, *g, *b, *a);
                c.width = *width as i32;
                c
            }
            D::Arc {
                cx,
                cy,
                r_in,
                r_out,
                a0,
                a1,
                r,
                g,
                b,
                a,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Arc);
                (c.cx, c.cy) = (*cx, *cy);
                (c.r_in, c.r_out, c.a0, c.a1) = (*r_in, *r_out, *a0, *a1);
                c.color = rgba(*r, *g, *b, *a);
                c
            }
            D::RectGradient {
                x,
                y,
                w,
                h,
                radius,
                r0,
                g0,
                b0,
                a0,
                r1,
                g1,
                b1,
                a1,
                angle,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::RectGradient).rect(*x, *y, *w, *h);
                c.radius = *radius as i32;
                c.color = rgba(*r0, *g0, *b0, *a0);
                c.color2 = rgba(*r1, *g1, *b1, *a1);
                c.angle = *angle;
                c
            }
            D::CircleGradient {
                cx,
                cy,
                radius,
                r0,
                g0,
                b0,
                a0,
                r1,
                g1,
                b1,
                a1,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::CircleGradient);
                (c.cx, c.cy, c.rx, c.ry, c.radius) = (*cx, *cy, *radius, *radius, *radius);
                c.color = rgba(*r0, *g0, *b0, *a0);
                c.color2 = rgba(*r1, *g1, *b1, *a1);
                c
            }
            D::Shadow {
                x,
                y,
                w,
                h,
                radius,
                blur,
                spread,
                dx,
                dy,
                r,
                g,
                b,
                a,
            } => {
                let mut c = PbDrawCmd::new(DrawKind::Shadow).rect(*x, *y, *w, *h);
                c.radius = *radius as i32;
                c.blur = *blur as i32;
                (c.spread, c.dx, c.dy) = (*spread, *dx, *dy);
                c.color = rgba(*r, *g, *b, *a);
                c
            }
            D::Clip { x, y, w, h, radius } => {
                let mut c = PbDrawCmd::new(DrawKind::Clip).rect(*x, *y, *w, *h);
                c.radius = *radius as i32;
                c
            }
            D::ClipNone => PbDrawCmd::new(DrawKind::ClipNone),
            D::ClipPush { x, y, w, h, radius } => {
                let mut c = PbDrawCmd::new(DrawKind::ClipPush).rect(*x, *y, *w, *h);
                c.radius = *radius as i32;
                c
            }
            D::ClipPop => PbDrawCmd::new(DrawKind::ClipPop),
            D::CreateCanvas { id, w, h } => {
                let mut c = PbDrawCmd::new(DrawKind::CreateCanvas).rect(0, 0, *w, *h);
                c.id = *id;
                c
            }
            D::SetTarget { id } => {
                let mut c = PbDrawCmd::new(DrawKind::SetTarget);
                c.id = *id;
                c
            }
            D::DrawCanvas { id, x, y, a, w, h } => {
                let mut c = PbDrawCmd::new(DrawKind::DrawCanvas).rect(*x, *y, *w, *h);
                c.id = *id;
                c.color = rgba(255, 255, 255, *a);
                c
            }
            D::Snapshot { id, x, y } => {
                let mut c = PbDrawCmd::new(DrawKind::Snapshot);
                c.id = *id;
                (c.x, c.y) = (*x, *y);
                c
            }
            D::BlurCanvas { id, radius } => {
                let mut c = PbDrawCmd::new(DrawKind::BlurCanvas);
                c.id = *id;
                c.radius = *radius as i32;
                c
            }
            D::Host { tag, data } => {
                let mut c = PbDrawCmd::new(DrawKind::Host);
                (c.text, c.text_len) = self.keep_str(tag);
                let mut arena = ViewArena::new();
                let first = arena.decode(data, heap, names);
                arena.finish();
                c.data = arena.node_ptr(first);
                c.data_count = data.len();
                self.host_data.push(arena);
                c
            }
        };
        self.cmds.push(c);
    }
}
