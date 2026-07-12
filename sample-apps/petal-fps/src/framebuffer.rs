//! Software framebuffer with color + depth buffers and triangle rasterization.
//!
//! The renderer works the same way in windowed mode, headless mode, and for
//! PNG screenshots — all three paths call into this module. That uniformity is
//! deliberate: what an agent sees in a `--screenshot` PNG is the exact same
//! pixels a human sees in the live window.

use crate::commands::DrawCommand;
use crate::font::{FONT_HEIGHT, FONT_WIDTH, glyph};

pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// RGB triplets, row-major.
    pub color: Vec<u8>,
    /// Depth buffer: smaller = nearer. Initialised to +infinity. One per pixel.
    pub depth: Vec<f32>,
}

const FAR: f32 = 1.0e9;

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width * height) as usize;
        Self {
            width,
            height,
            color: vec![0; n * 3],
            depth: vec![FAR; n],
        }
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        for px in self.color.chunks_exact_mut(3) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
        }
        for d in self.depth.iter_mut() {
            *d = FAR;
        }
    }

    /// Fill with a vertical gradient (top → bottom) at depth = FAR so all
    /// 3D geometry draws over the sky.
    pub fn sky_gradient(&mut self, rt: u8, gt: u8, bt: u8, rb: u8, gb: u8, bb: u8) {
        let h = self.height as i32;
        for y in 0..h {
            let t = y as f32 / (h - 1).max(1) as f32;
            let r = lerp_u8(rt, rb, t);
            let g = lerp_u8(gt, gb, t);
            let b = lerp_u8(bt, bb, t);
            let row = (y as u32) * self.width * 3;
            for x in 0..self.width {
                let i = (row + x * 3) as usize;
                self.color[i] = r;
                self.color[i + 1] = g;
                self.color[i + 2] = b;
            }
        }
        for d in self.depth.iter_mut() {
            *d = FAR;
        }
    }

    #[inline]
    fn put_pixel(&mut self, x: i32, y: i32, r: u8, g: u8, b: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = ((y as u32) * self.width + x as u32) as usize * 3;
        self.color[i] = r;
        self.color[i + 1] = g;
        self.color[i + 2] = b;
    }

    #[inline]
    fn put_pixel_z(&mut self, x: i32, y: i32, z: f32, r: u8, g: u8, b: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let pi = (y as u32) * self.width + x as u32;
        let di = pi as usize;
        if z < self.depth[di] {
            self.depth[di] = z;
            let ci = di * 3;
            self.color[ci] = r;
            self.color[ci + 1] = g;
            self.color[ci + 2] = b;
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, r: u8, g: u8, b: u8) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = ((x as i64) + (w as i64)).min(self.width as i64) as i32;
        let y1 = ((y as i64) + (h as i64)).min(self.height as i64) as i32;
        for py in y0..y1 {
            for px in x0..x1 {
                let i = ((py as u32) * self.width + px as u32) as usize * 3;
                self.color[i] = r;
                self.color[i + 1] = g;
                self.color[i + 2] = b;
            }
        }
    }

    pub fn draw_line2d(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: u8, g: u8, b: u8) {
        bresenham(x0, y0, x1, y1, |x, y| self.put_pixel(x, y, r, g, b));
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, r: u8, g: u8, b: u8) {
        if radius <= 0 {
            return;
        }
        let r2 = radius * radius;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= r2 {
                    self.put_pixel(cx + dx, cy + dy, r, g, b);
                }
            }
        }
    }

    pub fn draw_text(&mut self, text: &str, x: i32, y: i32, scale: u16, r: u8, g: u8, b: u8) {
        let s = (scale as i32).max(1);
        let gw = FONT_WIDTH as i32;
        let gh = FONT_HEIGHT as i32;
        let mut cx = x;
        for ch in text.chars() {
            let bits = glyph(ch);
            for row in 0..gh {
                let row_bits = bits[row as usize];
                for col in 0..gw {
                    if row_bits & (1 << (gw - 1 - col)) != 0 {
                        self.fill_rect(cx + col * s, y + row * s, s as u32, s as u32, r, g, b);
                    }
                }
            }
            cx += (gw + 1) * s; // 1-pixel gap between glyphs (scaled)
        }
    }

    /// Rasterize a z-buffered 3D triangle. `z` is treated as a scalar depth
    /// value interpolated linearly in screen space — good enough for FPS-style
    /// rendering at the scales we use, and keeps the inner loop fast.
    pub fn draw_triangle_3d(
        &mut self,
        x1: f32,
        y1: f32,
        z1: f32,
        x2: f32,
        y2: f32,
        z2: f32,
        x3: f32,
        y3: f32,
        z3: f32,
        r: u8,
        g: u8,
        b: u8,
    ) {
        // Reject triangles with any vertex behind the near plane.
        if z1 <= 0.0 || z2 <= 0.0 || z3 <= 0.0 {
            return;
        }
        self.triangle_inner(
            x1, y1, z1, r, g, b, x2, y2, z2, r, g, b, x3, y3, z3, r, g, b, false,
        );
    }

    /// Gouraud-shaded triangle (per-vertex colors).
    pub fn draw_triangle_shaded(
        &mut self,
        x1: f32,
        y1: f32,
        z1: f32,
        r1: u8,
        g1: u8,
        b1: u8,
        x2: f32,
        y2: f32,
        z2: f32,
        r2: u8,
        g2: u8,
        b2: u8,
        x3: f32,
        y3: f32,
        z3: f32,
        r3: u8,
        g3: u8,
        b3: u8,
    ) {
        if z1 <= 0.0 || z2 <= 0.0 || z3 <= 0.0 {
            return;
        }
        self.triangle_inner(
            x1, y1, z1, r1, g1, b1, x2, y2, z2, r2, g2, b2, x3, y3, z3, r3, g3, b3, true,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn triangle_inner(
        &mut self,
        mut x1: f32,
        mut y1: f32,
        mut z1: f32,
        mut r1: u8,
        mut g1: u8,
        mut b1: u8,
        mut x2: f32,
        mut y2: f32,
        mut z2: f32,
        mut r2: u8,
        mut g2: u8,
        mut b2: u8,
        mut x3: f32,
        mut y3: f32,
        mut z3: f32,
        mut r3: u8,
        mut g3: u8,
        mut b3: u8,
        shaded: bool,
    ) {
        // Sort vertices by y ascending so v1.y <= v2.y <= v3.y.
        if y2 < y1 {
            std::mem::swap(&mut x1, &mut x2);
            std::mem::swap(&mut y1, &mut y2);
            std::mem::swap(&mut z1, &mut z2);
            std::mem::swap(&mut r1, &mut r2);
            std::mem::swap(&mut g1, &mut g2);
            std::mem::swap(&mut b1, &mut b2);
        }
        if y3 < y1 {
            std::mem::swap(&mut x1, &mut x3);
            std::mem::swap(&mut y1, &mut y3);
            std::mem::swap(&mut z1, &mut z3);
            std::mem::swap(&mut r1, &mut r3);
            std::mem::swap(&mut g1, &mut g3);
            std::mem::swap(&mut b1, &mut b3);
        }
        if y3 < y2 {
            std::mem::swap(&mut x2, &mut x3);
            std::mem::swap(&mut y2, &mut y3);
            std::mem::swap(&mut z2, &mut z3);
            std::mem::swap(&mut r2, &mut r3);
            std::mem::swap(&mut g2, &mut g3);
            std::mem::swap(&mut b2, &mut b3);
        }

        let total_h = y3 - y1;
        if total_h < 0.5 {
            return;
        }

        // Clip to screen y range.
        let y_start = y1.floor().max(0.0) as i32;
        let y_end = y3.ceil().min(self.height as f32) as i32;

        for y in y_start..y_end {
            let yf = y as f32 + 0.5;
            if yf < y1 || yf > y3 {
                continue;
            }
            let second_half = yf > y2 || (y2 - y1).abs() < 0.001;
            let seg_h = if second_half { y3 - y2 } else { y2 - y1 };
            if seg_h.abs() < 0.001 {
                continue;
            }
            // alpha along 1→3, beta along the current segment (1→2 or 2→3)
            let alpha = (yf - y1) / total_h;
            let beta = if second_half {
                (yf - y2) / seg_h
            } else {
                (yf - y1) / seg_h
            };

            // Point A on long edge (v1→v3).
            let ax = x1 + (x3 - x1) * alpha;
            let az = z1 + (z3 - z1) * alpha;
            // Point B on the short edge (v1→v2 or v2→v3).
            let (bx, bz, ar, ag, ab, br_, bg, bb_);
            if second_half {
                bx = x2 + (x3 - x2) * beta;
                bz = z2 + (z3 - z2) * beta;
                ar = lerp_u8(r1, r3, alpha);
                ag = lerp_u8(g1, g3, alpha);
                ab = lerp_u8(b1, b3, alpha);
                br_ = lerp_u8(r2, r3, beta);
                bg = lerp_u8(g2, g3, beta);
                bb_ = lerp_u8(b2, b3, beta);
            } else {
                bx = x1 + (x2 - x1) * beta;
                bz = z1 + (z2 - z1) * beta;
                ar = lerp_u8(r1, r3, alpha);
                ag = lerp_u8(g1, g3, alpha);
                ab = lerp_u8(b1, b3, alpha);
                br_ = lerp_u8(r1, r2, beta);
                bg = lerp_u8(g1, g2, beta);
                bb_ = lerp_u8(b1, b2, beta);
            }

            let (lx, lz, lr, lg, lb, rx, rz, rr, rg, rb);
            if ax <= bx {
                lx = ax;
                lz = az;
                lr = ar;
                lg = ag;
                lb = ab;
                rx = bx;
                rz = bz;
                rr = br_;
                rg = bg;
                rb = bb_;
            } else {
                lx = bx;
                lz = bz;
                lr = br_;
                lg = bg;
                lb = bb_;
                rx = ax;
                rz = az;
                rr = ar;
                rg = ag;
                rb = ab;
            }

            let x_start = lx.floor().max(0.0) as i32;
            let x_end = rx.ceil().min(self.width as f32) as i32;
            let span = (rx - lx).max(0.001);
            for x in x_start..x_end {
                let xf = x as f32 + 0.5;
                if xf < lx || xf > rx {
                    continue;
                }
                let t = (xf - lx) / span;
                let z = lz + (rz - lz) * t;
                let (pr, pg, pb) = if shaded {
                    (lerp_u8(lr, rr, t), lerp_u8(lg, rg, t), lerp_u8(lb, rb, t))
                } else {
                    (lr, lg, lb)
                };
                self.put_pixel_z(x, y, z, pr, pg, pb);
            }
        }
    }

    /// z-tested 3D line via Bresenham with linear z interpolation.
    pub fn draw_line_3d(
        &mut self,
        x1: f32,
        y1: f32,
        z1: f32,
        x2: f32,
        y2: f32,
        z2: f32,
        r: u8,
        g: u8,
        b: u8,
    ) {
        if z1 <= 0.0 || z2 <= 0.0 {
            return;
        }
        let dx = (x2 - x1).abs();
        let dy = (y2 - y1).abs();
        let steps = dx.max(dy).ceil() as i32;
        if steps <= 0 {
            self.put_pixel_z(x1 as i32, y1 as i32, z1, r, g, b);
            return;
        }
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = x1 + (x2 - x1) * t;
            let y = y1 + (y2 - y1) * t;
            let z = z1 + (z2 - z1) * t;
            self.put_pixel_z(x as i32, y as i32, z, r, g, b);
        }
    }

    pub fn execute(&mut self, commands: &[DrawCommand]) {
        for cmd in commands {
            match cmd {
                DrawCommand::Clear3d { r, g, b } => self.clear(*r, *g, *b),
                DrawCommand::SkyGradient {
                    r_top,
                    g_top,
                    b_top,
                    r_bot,
                    g_bot,
                    b_bot,
                } => {
                    self.sky_gradient(*r_top, *g_top, *b_top, *r_bot, *g_bot, *b_bot);
                }
                DrawCommand::Triangle3d {
                    x1,
                    y1,
                    z1,
                    x2,
                    y2,
                    z2,
                    x3,
                    y3,
                    z3,
                    r,
                    g,
                    b,
                } => {
                    self.draw_triangle_3d(*x1, *y1, *z1, *x2, *y2, *z2, *x3, *y3, *z3, *r, *g, *b);
                }
                DrawCommand::Triangle3dShaded {
                    x1,
                    y1,
                    z1,
                    r1,
                    g1,
                    b1,
                    x2,
                    y2,
                    z2,
                    r2,
                    g2,
                    b2,
                    x3,
                    y3,
                    z3,
                    r3,
                    g3,
                    b3,
                } => {
                    self.draw_triangle_shaded(
                        *x1, *y1, *z1, *r1, *g1, *b1, *x2, *y2, *z2, *r2, *g2, *b2, *x3, *y3, *z3,
                        *r3, *g3, *b3,
                    );
                }
                DrawCommand::Line3d {
                    x1,
                    y1,
                    z1,
                    x2,
                    y2,
                    z2,
                    r,
                    g,
                    b,
                } => {
                    self.draw_line_3d(*x1, *y1, *z1, *x2, *y2, *z2, *r, *g, *b);
                }
                DrawCommand::Rect2d {
                    x,
                    y,
                    w,
                    h,
                    r,
                    g,
                    b,
                } => {
                    self.fill_rect(*x, *y, *w, *h, *r, *g, *b);
                }
                DrawCommand::Line2d {
                    x1,
                    y1,
                    x2,
                    y2,
                    r,
                    g,
                    b,
                } => {
                    self.draw_line2d(*x1, *y1, *x2, *y2, *r, *g, *b);
                }
                DrawCommand::Circle2d {
                    cx,
                    cy,
                    radius,
                    r,
                    g,
                    b,
                } => {
                    self.fill_circle(*cx, *cy, *radius, *r, *g, *b);
                }
                DrawCommand::Text2d {
                    text,
                    x,
                    y,
                    size,
                    r,
                    g,
                    b,
                } => {
                    self.draw_text(text, *x, *y, *size, *r, *g, *b);
                }
            }
        }
    }
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (a as f32 + (b as f32 - a as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn bresenham(x0: i32, y0: i32, x1: i32, y1: i32, mut plot: impl FnMut(i32, i32)) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        plot(x, y);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            if x == x1 {
                break;
            }
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            if y == y1 {
                break;
            }
            err += dx;
            y += sy;
        }
    }
}
