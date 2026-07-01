use std::collections::HashMap;

use base64::Engine;
use image::{Rgb, RgbImage};

use crate::commands::DrawCommand;

pub fn render_to_png_base64(commands: &[DrawCommand], width: u32, height: u32) -> String {
    let img = render_commands(commands, width, height);
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encode failed");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

pub fn save_png(commands: &[DrawCommand], width: u32, height: u32, path: &str) -> Result<(), String> {
    let img = render_commands(commands, width, height);
    img.save(path).map_err(|e| e.to_string())
}

/// A render target: an RGB pixel buffer plus a parallel "painted" mask so
/// offscreen canvases can be blitted compositing only the pixels that were
/// actually drawn (transparent regions show the destination through).
struct Target {
    img: RgbImage,
    /// One bool per pixel (row-major); true where a draw command has painted.
    painted: Vec<bool>,
    width: u32,
}

impl Target {
    fn new(width: u32, height: u32) -> Self {
        Target {
            img: RgbImage::from_pixel(width, height, Rgb([0, 0, 0])),
            painted: vec![false; (width as usize) * (height as usize)],
            width,
        }
    }

    fn mark_painted(&mut self, x: i32, y: i32) {
        if x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.img.height() {
            self.painted[(y as usize) * (self.width as usize) + (x as usize)] = true;
        }
    }

    /// Apply a draw command to this target, marking painted pixels. Stream-level
    /// commands (`CreateCanvas`/`SetTarget`/`DrawCanvas`) are handled by the
    /// caller and never reach here.
    fn apply(&mut self, cmd: &DrawCommand) {
        match cmd {
            DrawCommand::Clear { r, g, b } => {
                for pixel in self.img.pixels_mut() {
                    *pixel = Rgb([*r, *g, *b]);
                }
                for p in self.painted.iter_mut() {
                    *p = true;
                }
            }
            DrawCommand::Rect { x, y, w, h, r, g, b } => {
                fill_rect(&mut self.img, *x, *y, *w, *h, Rgb([*r, *g, *b]));
                for py in *y..(*y + *h as i32) {
                    for px in *x..(*x + *w as i32) {
                        self.mark_painted(px, py);
                    }
                }
            }
            DrawCommand::RectOutline { x, y, w, h, r, g, b } => {
                draw_rect_outline(&mut self.img, *x, *y, *w, *h, Rgb([*r, *g, *b]));
                let x1 = *x + *w as i32 - 1;
                let y1 = *y + *h as i32 - 1;
                for px in *x..=x1 {
                    self.mark_painted(px, *y);
                    self.mark_painted(px, y1);
                }
                for py in *y..=y1 {
                    self.mark_painted(*x, py);
                    self.mark_painted(x1, py);
                }
            }
            DrawCommand::Line { x1, y1, x2, y2, r, g, b } => {
                draw_line(&mut self.img, *x1, *y1, *x2, *y2, Rgb([*r, *g, *b]));
                self.mark_line(*x1, *y1, *x2, *y2);
            }
            DrawCommand::Circle { cx, cy, radius, r, g, b } => {
                fill_circle(&mut self.img, *cx, *cy, *radius, Rgb([*r, *g, *b]));
                for py in (*cy - *radius)..=(*cy + *radius) {
                    for px in (*cx - *radius)..=(*cx + *radius) {
                        let dx = px - *cx;
                        let dy = py - *cy;
                        if dx * dx + dy * dy <= *radius * *radius {
                            self.mark_painted(px, py);
                        }
                    }
                }
            }
            DrawCommand::Triangle { x1, y1, x2, y2, x3, y3, r, g, b } => {
                let pts = [(*x1, *y1), (*x2, *y2), (*x3, *y3)];
                fill_polygon(&mut self.img, &pts, Rgb([*r, *g, *b]));
                self.mark_polygon(&pts);
            }
            DrawCommand::Poly { points, r, g, b } => {
                fill_polygon(&mut self.img, points, Rgb([*r, *g, *b]));
                self.mark_polygon(points);
            }
            DrawCommand::Text { text, x, y, size, r, g, b } => {
                // Approximate text as a colored rectangle proportional to string length
                let char_w = (*size as u32) * 3 / 5;
                let tw = (char_w * text.len() as u32).max(1);
                let th = (*size as u32).max(1);
                fill_rect(&mut self.img, *x, *y, tw, th, Rgb([*r, *g, *b]));
                for py in *y..(*y + th as i32) {
                    for px in *x..(*x + tw as i32) {
                        self.mark_painted(px, py);
                    }
                }
            }
            // The screenshot raster approximates text as blocks and doesn't
            // model clip regions; clip commands are ignored here (the real
            // renderer honors them).
            DrawCommand::Clip { .. } | DrawCommand::ClipNone => {}
            DrawCommand::Host { .. } => {}
            DrawCommand::CreateCanvas { .. }
            | DrawCommand::SetTarget { .. }
            | DrawCommand::DrawCanvas { .. } => {}
        }
    }

    fn mark_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            self.mark_painted(x, y);
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

    fn mark_polygon(&mut self, points: &[(i32, i32)]) {
        if points.len() < 3 {
            return;
        }
        let min_y = points.iter().map(|p| p.1).min().unwrap();
        let max_y = points.iter().map(|p| p.1).max().unwrap();
        let n = points.len();
        for y in min_y..=max_y {
            let mut crossings: Vec<i32> = Vec::new();
            for i in 0..n {
                let (x1, y1) = points[i];
                let (x2, y2) = points[(i + 1) % n];
                if y1 == y2 {
                    continue;
                }
                let (ya, yb, xa, xb) = if y1 < y2 { (y1, y2, x1, x2) } else { (y2, y1, x2, x1) };
                if y >= ya && y < yb {
                    let t = (y - ya) as f64 / (yb - ya) as f64;
                    crossings.push((xa as f64 + t * (xb - xa) as f64).round() as i32);
                }
            }
            crossings.sort_unstable();
            let mut i = 0;
            while i + 1 < crossings.len() {
                for px in crossings[i]..=crossings[i + 1] {
                    self.mark_painted(px, y);
                }
                i += 2;
            }
        }
    }

    /// Composite painted pixels of `src` onto this target at offset (`ox`, `oy`).
    fn blit(&mut self, src: &Target, ox: i32, oy: i32) {
        for sy in 0..src.img.height() as i32 {
            for sx in 0..src.width as i32 {
                if !src.painted[(sy as usize) * (src.width as usize) + (sx as usize)] {
                    continue;
                }
                let (dx, dy) = (ox + sx, oy + sy);
                if dx >= 0 && dy >= 0 && (dx as u32) < self.width && (dy as u32) < self.img.height() {
                    let px = *src.img.get_pixel(sx as u32, sy as u32);
                    self.img.put_pixel(dx as u32, dy as u32, px);
                    self.mark_painted(dx, dy);
                }
            }
        }
    }
}

fn render_commands(commands: &[DrawCommand], width: u32, height: u32) -> RgbImage {
    let mut main = Target::new(width, height);
    // Offscreen canvases keyed by id, rebuilt fresh from the command stream.
    let mut offscreen: HashMap<u32, Target> = HashMap::new();
    // Active draw target: 0 = main framebuffer, otherwise an offscreen id.
    let mut target: u32 = 0;

    for cmd in commands {
        match cmd {
            DrawCommand::CreateCanvas { id, w, h } => {
                offscreen.insert(*id, Target::new((*w).max(1), (*h).max(1)));
            }
            DrawCommand::SetTarget { id } => {
                target = *id;
            }
            DrawCommand::DrawCanvas { id, x, y } => {
                if let Some(src) = offscreen.remove(id) {
                    if target == 0 {
                        main.blit(&src, *x, *y);
                    } else if let Some(dst) = offscreen.get_mut(&target) {
                        dst.blit(&src, *x, *y);
                    }
                    offscreen.insert(*id, src);
                }
            }
            other => {
                if target == 0 {
                    main.apply(other);
                } else if let Some(dst) = offscreen.get_mut(&target) {
                    dst.apply(other);
                }
            }
        }
    }
    main.img
}

/// Fill a polygon into the image buffer using the even-odd scanline rule.
fn fill_polygon(img: &mut RgbImage, points: &[(i32, i32)], color: Rgb<u8>) {
    if points.len() < 3 {
        return;
    }
    let min_y = points.iter().map(|p| p.1).min().unwrap();
    let max_y = points.iter().map(|p| p.1).max().unwrap();
    let n = points.len();
    for y in min_y..=max_y {
        let mut crossings: Vec<i32> = Vec::new();
        for i in 0..n {
            let (x1, y1) = points[i];
            let (x2, y2) = points[(i + 1) % n];
            if y1 == y2 {
                continue;
            }
            let (ya, yb, xa, xb) = if y1 < y2 {
                (y1, y2, x1, x2)
            } else {
                (y2, y1, x2, x1)
            };
            if y >= ya && y < yb {
                let t = (y - ya) as f64 / (yb - ya) as f64;
                let x = xa as f64 + t * (xb - xa) as f64;
                crossings.push(x.round() as i32);
            }
        }
        crossings.sort_unstable();
        let mut i = 0;
        while i + 1 < crossings.len() {
            for px in crossings[i]..=crossings[i + 1] {
                set_pixel(img, px, y, color);
            }
            i += 2;
        }
    }
}

fn set_pixel(img: &mut RgbImage, x: i32, y: i32, color: Rgb<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, color);
    }
}

fn fill_rect(img: &mut RgbImage, x: i32, y: i32, w: u32, h: u32, color: Rgb<u8>) {
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = ((x as i64 + w as i64) as u32).min(img.width());
    let y1 = ((y as i64 + h as i64) as u32).min(img.height());
    for py in y0..y1 {
        for px in x0..x1 {
            img.put_pixel(px, py, color);
        }
    }
}

fn draw_rect_outline(img: &mut RgbImage, x: i32, y: i32, w: u32, h: u32, color: Rgb<u8>) {
    let x1 = x + w as i32 - 1;
    let y1 = y + h as i32 - 1;
    draw_line(img, x, y, x1, y, color);
    draw_line(img, x, y1, x1, y1, color);
    draw_line(img, x, y, x, y1, color);
    draw_line(img, x1, y, x1, y1, color);
}

fn draw_line(img: &mut RgbImage, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb<u8>) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;

    loop {
        set_pixel(img, x, y, color);
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

fn fill_circle(img: &mut RgbImage, cx: i32, cy: i32, radius: i32, color: Rgb<u8>) {
    let mut x = radius;
    let mut y = 0i32;
    let mut err = 1 - radius;

    while x >= y {
        for px in (cx - x)..=(cx + x) {
            set_pixel(img, px, cy + y, color);
            set_pixel(img, px, cy - y, color);
        }
        for px in (cx - y)..=(cx + y) {
            set_pixel(img, px, cy + x, color);
            set_pixel(img, px, cy - x, color);
        }
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}
