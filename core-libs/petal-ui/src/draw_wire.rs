//! A compact binary encoding of a frame's [`DrawCommand`]s, for hosts across a
//! language boundary (the browser canvas runtime) where JSON costs both code
//! size and frame time.
//!
//! A frame encodes to two parts:
//!
//! - **numbers**: one flat `Vec<f64>`. Each command is its opcode
//!   ([`op`]) followed by its fields in declaration order. Every field is
//!   always present, at its default when the JSON form would omit it, so a
//!   decoder knows each command's length from its opcode alone. Variable-length
//!   fields (`points`, `rects`) are a count followed by that many entries; an
//!   `Option` is a 0/1 flag followed, when 1, by its values. Every `i32`,
//!   `u32`, `u16`, `u8` and `f32` a command holds is exact in an `f64`.
//! - **strings**: the frame's strings (`text`, `font`, `source`, `tag`), each
//!   referenced from the numbers by its index; an absent `Option<String>` is
//!   `-1`.
//!
//! The decoder is `integrations/petal-web-canvas/src/draw-wire.ts`, which
//! rebuilds the same `{ op, ...fields }` objects the JSON form has, defaults
//! omitted. `test/vitest/fixtures/draw-wire.json` pins the two together: a Rust test
//! here writes (or checks) the encoding and the JSON of one sample of every
//! command, and a vitest test decodes the one and compares it to the other.
//! Changing a command's fields means changing both sides and regenerating the
//! fixture (`PETAL_UPDATE_FIXTURES=1 cargo test -p petal-ui draw_wire`).
//!
//! `Host` commands carry only their `tag`; their `data` is host-specific
//! `Value`s that the JSON form skips too.

use crate::draw::{BatchRect, DrawCommand};

/// Opcodes: a command's first number. Append new ones; never renumber.
pub mod op {
    pub const IMAGE: u8 = 0;
    pub const CLEAR: u8 = 1;
    pub const RECT: u8 = 2;
    pub const RECT_OUTLINE: u8 = 3;
    pub const LINE: u8 = 4;
    pub const CIRCLE: u8 = 5;
    pub const TEXT: u8 = 6;
    pub const TRIANGLE: u8 = 7;
    pub const POLY: u8 = 8;
    pub const POLYGON: u8 = 9;
    pub const FAN: u8 = 10;
    pub const POLYLINE: u8 = 11;
    pub const ELLIPSE: u8 = 12;
    pub const ELLIPSE_OUTLINE: u8 = 13;
    pub const ARC: u8 = 14;
    pub const RECT_GRADIENT: u8 = 15;
    pub const CIRCLE_GRADIENT: u8 = 16;
    pub const SHADOW: u8 = 17;
    pub const CLIP: u8 = 18;
    pub const CLIP_NONE: u8 = 19;
    pub const CLIP_PUSH: u8 = 20;
    pub const CLIP_POP: u8 = 21;
    pub const CREATE_CANVAS: u8 = 22;
    pub const SET_TARGET: u8 = 23;
    pub const DRAW_CANVAS: u8 = 24;
    pub const SNAPSHOT: u8 = 25;
    pub const BLUR_CANVAS: u8 = 26;
    pub const RECTS: u8 = 27;
    pub const LAYER: u8 = 28;
    pub const CACHE_BEGIN: u8 = 29;
    pub const CACHE_END: u8 = 30;
    pub const CACHE_DRAW: u8 = 31;
    pub const MUTE: u8 = 32;
    pub const HOST: u8 = 33;
    /// One past the last opcode.
    pub const COUNT: u8 = 34;
}

/// A frame's commands in wire form: see the module docs.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DrawWire {
    pub numbers: Vec<f64>,
    pub strings: Vec<String>,
}

impl DrawWire {
    /// Encode `cmds`.
    pub fn encode(cmds: &[DrawCommand]) -> Self {
        let mut w = Self {
            numbers: Vec::with_capacity(cmds.len() * 10),
            strings: Vec::new(),
        };
        for cmd in cmds {
            w.command(cmd);
        }
        w
    }

    fn n(&mut self, v: impl Into<f64>) {
        self.numbers.push(v.into());
    }

    fn rgba(&mut self, r: u8, g: u8, b: u8, a: u8) {
        self.numbers.extend([r, g, b, a].map(f64::from));
    }

    fn string(&mut self, s: &str) {
        self.numbers.push(self.strings.len() as f64);
        self.strings.push(s.to_string());
    }

    fn points(&mut self, points: &[(i32, i32)]) {
        self.n(points.len() as u32);
        for &(x, y) in points {
            self.n(x);
            self.n(y);
        }
    }

    fn opt<const N: usize, T: Copy + Into<f64>>(&mut self, v: Option<[T; N]>) {
        match v {
            Some(vals) => {
                self.n(1);
                self.numbers.extend(vals.map(Into::into));
            }
            None => self.n(0),
        }
    }

    fn command(&mut self, cmd: &DrawCommand) {
        use DrawCommand as C;
        match cmd {
            C::Image {
                source,
                x,
                y,
                w,
                h,
                a,
                radius,
                tint,
                src,
                slice,
            } => {
                self.n(op::IMAGE);
                self.string(source);
                self.numbers
                    .extend([*x as f64, *y as f64, *w as f64, *h as f64]);
                self.n(*a);
                self.n(*radius);
                self.opt(*tint);
                self.opt(*src);
                self.opt(*slice);
            }
            C::Clear { r, g, b } => {
                self.n(op::CLEAR);
                self.numbers.extend([*r, *g, *b].map(f64::from));
            }
            C::Rect {
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
                self.n(op::RECT);
                self.numbers
                    .extend([*x as f64, *y as f64, *w as f64, *h as f64]);
                self.rgba(*r, *g, *b, *a);
                self.n(*radius);
            }
            C::RectOutline {
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
                self.n(op::RECT_OUTLINE);
                self.numbers
                    .extend([*x as f64, *y as f64, *w as f64, *h as f64]);
                self.rgba(*r, *g, *b, *a);
                self.n(*width);
                self.n(*radius);
            }
            C::Line {
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
                self.n(op::LINE);
                self.numbers.extend([*x1, *y1, *x2, *y2].map(f64::from));
                self.rgba(*r, *g, *b, *a);
                self.n(*width);
            }
            C::Circle {
                cx,
                cy,
                radius,
                r,
                g,
                b,
                a,
            } => {
                self.n(op::CIRCLE);
                self.numbers.extend([*cx, *cy, *radius].map(f64::from));
                self.rgba(*r, *g, *b, *a);
            }
            C::Text {
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
                self.n(op::TEXT);
                self.string(text);
                self.n(*x);
                self.n(*y);
                self.n(*size);
                self.rgba(*r, *g, *b, *a);
                match font {
                    Some(f) => self.string(f),
                    None => self.n(-1),
                }
                self.n(*weight);
                self.n(u8::from(*italic));
                self.n(*spacing);
            }
            C::Triangle {
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
                self.n(op::TRIANGLE);
                self.numbers
                    .extend([*x1, *y1, *x2, *y2, *x3, *y3].map(f64::from));
                self.rgba(*r, *g, *b, *a);
            }
            C::Poly { points, r, g, b, a } => {
                self.n(op::POLY);
                self.points(points);
                self.rgba(*r, *g, *b, *a);
            }
            C::Polygon { points, r, g, b, a } => {
                self.n(op::POLYGON);
                self.points(points);
                self.rgba(*r, *g, *b, *a);
            }
            C::Fan {
                cx,
                cy,
                points,
                r,
                g,
                b,
                a,
            } => {
                self.n(op::FAN);
                self.n(*cx);
                self.n(*cy);
                self.points(points);
                self.rgba(*r, *g, *b, *a);
            }
            C::Polyline {
                points,
                r,
                g,
                b,
                a,
                width,
            } => {
                self.n(op::POLYLINE);
                self.points(points);
                self.rgba(*r, *g, *b, *a);
                self.n(*width);
            }
            C::Ellipse {
                cx,
                cy,
                rx,
                ry,
                r,
                g,
                b,
                a,
            } => {
                self.n(op::ELLIPSE);
                self.numbers.extend([*cx, *cy, *rx, *ry].map(f64::from));
                self.rgba(*r, *g, *b, *a);
            }
            C::EllipseOutline {
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
                self.n(op::ELLIPSE_OUTLINE);
                self.numbers.extend([*cx, *cy, *rx, *ry].map(f64::from));
                self.rgba(*r, *g, *b, *a);
                self.n(*width);
            }
            C::Arc {
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
                self.n(op::ARC);
                self.n(*cx);
                self.n(*cy);
                self.numbers
                    .extend([*r_in, *r_out, *a0, *a1].map(f64::from));
                self.rgba(*r, *g, *b, *a);
            }
            C::RectGradient {
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
                self.n(op::RECT_GRADIENT);
                self.numbers
                    .extend([*x as f64, *y as f64, *w as f64, *h as f64]);
                self.n(*radius);
                self.rgba(*r0, *g0, *b0, *a0);
                self.rgba(*r1, *g1, *b1, *a1);
                self.n(*angle);
            }
            C::CircleGradient {
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
                self.n(op::CIRCLE_GRADIENT);
                self.numbers.extend([*cx, *cy, *radius].map(f64::from));
                self.rgba(*r0, *g0, *b0, *a0);
                self.rgba(*r1, *g1, *b1, *a1);
            }
            C::Shadow {
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
                self.n(op::SHADOW);
                self.numbers
                    .extend([*x as f64, *y as f64, *w as f64, *h as f64]);
                self.n(*radius);
                self.n(*blur);
                self.numbers.extend([*spread, *dx, *dy].map(f64::from));
                self.rgba(*r, *g, *b, *a);
            }
            C::Clip { x, y, w, h, radius } => {
                self.n(op::CLIP);
                self.numbers
                    .extend([*x as f64, *y as f64, *w as f64, *h as f64]);
                self.n(*radius);
            }
            C::ClipNone => self.n(op::CLIP_NONE),
            C::ClipPush { x, y, w, h, radius } => {
                self.n(op::CLIP_PUSH);
                self.numbers
                    .extend([*x as f64, *y as f64, *w as f64, *h as f64]);
                self.n(*radius);
            }
            C::ClipPop => self.n(op::CLIP_POP),
            C::CreateCanvas { id, w, h } => {
                self.n(op::CREATE_CANVAS);
                self.numbers.extend([*id, *w, *h].map(f64::from));
            }
            C::SetTarget { id } => {
                self.n(op::SET_TARGET);
                self.n(*id);
            }
            C::DrawCanvas { id, x, y, a, w, h } => {
                self.n(op::DRAW_CANVAS);
                self.n(*id);
                self.n(*x);
                self.n(*y);
                self.n(*a);
                self.n(*w);
                self.n(*h);
            }
            C::Snapshot { id, x, y } => {
                self.n(op::SNAPSHOT);
                self.n(*id);
                self.n(*x);
                self.n(*y);
            }
            C::BlurCanvas { id, radius } => {
                self.n(op::BLUR_CANVAS);
                self.n(*id);
                self.n(*radius);
            }
            C::Rects { rects } => {
                self.n(op::RECTS);
                self.n(rects.len() as u32);
                for &BatchRect {
                    x,
                    y,
                    w,
                    h,
                    r,
                    g,
                    b,
                    a,
                } in rects
                {
                    self.numbers
                        .extend([x as f64, y as f64, w as f64, h as f64]);
                    self.rgba(r, g, b, a);
                }
            }
            C::Layer { n } => {
                self.n(op::LAYER);
                self.n(*n);
            }
            C::CacheBegin { id, w, h } => {
                self.n(op::CACHE_BEGIN);
                self.numbers.extend([*id, *w, *h].map(f64::from));
            }
            C::CacheEnd { id } => {
                self.n(op::CACHE_END);
                self.n(*id);
            }
            C::CacheDraw { id, x, y } => {
                self.n(op::CACHE_DRAW);
                self.n(*id);
                self.n(*x);
                self.n(*y);
            }
            C::Mute { on } => {
                self.n(op::MUTE);
                self.n(u8::from(*on));
            }
            C::Host { tag, .. } => {
                self.n(op::HOST);
                self.string(tag);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every command, each with its optional fields once at a
    /// non-default value and (where it has any) once at the defaults, so the
    /// fixture exercises both the "present" and "omitted" paths of the decoder.
    #[rustfmt::skip]
    fn samples() -> Vec<DrawCommand> {
        use DrawCommand as C;
        let pts = vec![(1, 2), (-3, 4), (5, -6)];
        vec![
            C::Image { source: "a.png".into(), x: 1, y: -2, w: 30, h: 40, a: 128, radius: 3, tint: Some([1, 2, 3]), src: Some([0, 1, -2, 3]), slice: Some([4, 5, 6, 7]) },
            C::Image { source: "b".into(), x: 0, y: 0, w: 1, h: 1, a: 255, radius: 0, tint: None, src: None, slice: None },
            C::Clear { r: 10, g: 20, b: 30 },
            C::Rect { x: -1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7, a: 8, radius: 9 },
            C::Rect { x: 0, y: 0, w: 0, h: 0, r: 0, g: 0, b: 0, a: 255, radius: 0 },
            C::RectOutline { x: 1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7, a: 8, width: 2, radius: 4 },
            C::RectOutline { x: 1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7, a: 255, width: 1, radius: 0 },
            C::Line { x1: 1, y1: 2, x2: 3, y2: -4, r: 5, g: 6, b: 7, a: 8, width: 3 },
            C::Line { x1: 1, y1: 2, x2: 3, y2: 4, r: 5, g: 6, b: 7, a: 255, width: 1 },
            C::Circle { cx: 1, cy: 2, radius: -3, r: 4, g: 5, b: 6, a: 7 },
            C::Text { text: "héllo ✓".into(), x: 1, y: 2, size: 14, r: 1, g: 2, b: 3, a: 4, font: Some("mono, serif".into()), weight: 700, italic: true, spacing: 1.5 },
            C::Text { text: "".into(), x: 0, y: 0, size: 12, r: 0, g: 0, b: 0, a: 255, font: None, weight: crate::text::REGULAR_WEIGHT, italic: false, spacing: 0.0 },
            C::Triangle { x1: 1, y1: 2, x2: 3, y2: 4, x3: 5, y3: 6, r: 7, g: 8, b: 9, a: 10 },
            C::Poly { points: pts.clone(), r: 1, g: 2, b: 3, a: 255 },
            C::Polygon { points: pts.clone(), r: 1, g: 2, b: 3, a: 4 },
            C::Fan { cx: 7, cy: 8, points: pts.clone(), r: 1, g: 2, b: 3, a: 4 },
            C::Polyline { points: vec![], r: 1, g: 2, b: 3, a: 4, width: 5 },
            C::Ellipse { cx: 1, cy: 2, rx: 3, ry: 4, r: 5, g: 6, b: 7, a: 8 },
            C::EllipseOutline { cx: 1, cy: 2, rx: 3, ry: 4, r: 5, g: 6, b: 7, a: 255, width: 2 },
            C::Arc { cx: 1, cy: 2, r_in: 0.5, r_out: 10.25, a0: -1.5, a1: 3.0, r: 5, g: 6, b: 7, a: 8 },
            C::RectGradient { x: 1, y: 2, w: 3, h: 4, radius: 5, r0: 1, g0: 2, b0: 3, a0: 4, r1: 5, g1: 6, b1: 7, a1: 255, angle: 0.75 },
            C::CircleGradient { cx: 1, cy: 2, radius: 3, r0: 1, g0: 2, b0: 3, a0: 255, r1: 5, g1: 6, b1: 7, a1: 8 },
            C::Shadow { x: 1, y: 2, w: 3, h: 4, radius: 5, blur: 6, spread: -7, dx: 8, dy: -9, r: 1, g: 2, b: 3, a: 4 },
            C::Shadow { x: 1, y: 2, w: 3, h: 4, radius: 0, blur: 6, spread: 0, dx: 0, dy: 0, r: 1, g: 2, b: 3, a: 255 },
            C::Clip { x: 1, y: 2, w: 3, h: 4, radius: 5 },
            C::ClipNone,
            C::ClipPush { x: 1, y: 2, w: 3, h: 4, radius: 0 },
            C::ClipPop,
            C::CreateCanvas { id: 1, w: 2, h: 3 },
            C::SetTarget { id: 4 },
            C::DrawCanvas { id: 1, x: -2, y: 3, a: 100, w: 5, h: 6 },
            C::DrawCanvas { id: 1, x: 2, y: 3, a: 255, w: 0, h: 0 },
            C::Snapshot { id: 1, x: 2, y: 3 },
            C::BlurCanvas { id: 1, radius: 8 },
            C::Rects { rects: vec![BatchRect { x: 1, y: 2, w: 3, h: 4, r: 5, g: 6, b: 7, a: 8 }, BatchRect { x: -1, y: 0, w: 1, h: 1, r: 0, g: 0, b: 0, a: 255 }] },
            C::Layer { n: -2 },
            C::CacheBegin { id: 1, w: 2, h: 3 },
            C::CacheEnd { id: 1 },
            C::CacheDraw { id: 1, x: 2, y: 3 },
            C::Mute { on: true },
            C::Host { tag: "sound".into(), data: vec![] },
        ]
    }

    #[test]
    fn samples_cover_every_opcode() {
        let mut seen = [false; op::COUNT as usize];
        for cmd in samples() {
            let w = DrawWire::encode(std::slice::from_ref(&cmd));
            seen[w.numbers[0] as usize] = true;
        }
        let missing: Vec<usize> = (0..seen.len()).filter(|&i| !seen[i]).collect();
        assert!(missing.is_empty(), "no sample for opcodes {missing:?}");
    }

    /// `test/vitest/fixtures/draw-wire.json`: the encoding of [`samples`] beside
    /// their JSON form, which `test/vitest/draw-wire.test.ts` decodes and
    /// compares. Fails when it is stale; `PETAL_UPDATE_FIXTURES=1` rewrites it.
    #[test]
    fn fixture_is_current() {
        let cmds = samples();
        let wire = DrawWire::encode(&cmds);
        let fixture = serde_json::json!({
            "_comment": "Generated by core-libs/petal-ui/src/draw_wire.rs (fixture_is_current). Regenerate: PETAL_UPDATE_FIXTURES=1 cargo test -p petal-ui draw_wire",
            "numbers": wire.numbers,
            "strings": wire.strings,
            "commands": cmds,
        });
        let text = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/vitest/fixtures/draw-wire.json");
        if std::env::var_os("PETAL_UPDATE_FIXTURES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
            return;
        }
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            current == text,
            "{} is stale; regenerate with PETAL_UPDATE_FIXTURES=1 cargo test -p petal-ui draw_wire",
            path.display()
        );
    }
}
