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

fn render_commands(commands: &[DrawCommand], width: u32, height: u32) -> RgbImage {
    let mut img = RgbImage::from_pixel(width, height, Rgb([0, 0, 0]));
    for cmd in commands {
        match cmd {
            DrawCommand::Clear { r, g, b } => {
                for pixel in img.pixels_mut() {
                    *pixel = Rgb([*r, *g, *b]);
                }
            }
            DrawCommand::Rect { x, y, w, h, r, g, b } => {
                fill_rect(&mut img, *x, *y, *w, *h, Rgb([*r, *g, *b]));
            }
            DrawCommand::RectOutline { x, y, w, h, r, g, b } => {
                draw_rect_outline(&mut img, *x, *y, *w, *h, Rgb([*r, *g, *b]));
            }
            DrawCommand::Line { x1, y1, x2, y2, r, g, b } => {
                draw_line(&mut img, *x1, *y1, *x2, *y2, Rgb([*r, *g, *b]));
            }
            DrawCommand::Circle { cx, cy, radius, r, g, b } => {
                fill_circle(&mut img, *cx, *cy, *radius, Rgb([*r, *g, *b]));
            }
            DrawCommand::Triangle { x1, y1, x2, y2, x3, y3, r, g, b } => {
                fill_polygon(
                    &mut img,
                    &[(*x1, *y1), (*x2, *y2), (*x3, *y3)],
                    Rgb([*r, *g, *b]),
                );
            }
            DrawCommand::Poly { points, r, g, b } => {
                fill_polygon(&mut img, points, Rgb([*r, *g, *b]));
            }
            DrawCommand::Text { text, x, y, size, r, g, b } => {
                // Approximate text as a colored rectangle proportional to string length
                let char_w = (*size as u32) * 3 / 5;
                let tw = char_w * text.len() as u32;
                let th = *size as u32;
                fill_rect(&mut img, *x, *y, tw.max(1), th.max(1), Rgb([*r, *g, *b]));
            }
        }
    }
    img
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
