use sdl2::pixels::Color;
use sdl2::rect::Rect;
use sdl2::render::Canvas;
use sdl2::ttf::Font;
use sdl2::video::Window;

use crate::commands::DrawCommand;

pub fn render(canvas: &mut Canvas<Window>, commands: Vec<DrawCommand>, font: &Font) {
    for cmd in commands {
        match cmd {
            DrawCommand::Clear { r, g, b } => {
                canvas.set_draw_color(Color::RGB(r, g, b));
                canvas.clear();
            }
            DrawCommand::Rect { x, y, w, h, r, g, b } => {
                canvas.set_draw_color(Color::RGB(r, g, b));
                let _ = canvas.fill_rect(Rect::new(x, y, w, h));
            }
            DrawCommand::RectOutline { x, y, w, h, r, g, b } => {
                canvas.set_draw_color(Color::RGB(r, g, b));
                let _ = canvas.draw_rect(Rect::new(x, y, w, h));
            }
            DrawCommand::Line { x1, y1, x2, y2, r, g, b } => {
                canvas.set_draw_color(Color::RGB(r, g, b));
                let _ = canvas.draw_line((x1, y1), (x2, y2));
            }
            DrawCommand::Circle { cx, cy, radius, r, g, b } => {
                canvas.set_draw_color(Color::RGB(r, g, b));
                draw_filled_circle(canvas, cx, cy, radius);
            }
            DrawCommand::Triangle { x1, y1, x2, y2, x3, y3, r, g, b } => {
                canvas.set_draw_color(Color::RGB(r, g, b));
                fill_polygon(canvas, &[(x1, y1), (x2, y2), (x3, y3)]);
            }
            DrawCommand::Poly { points, r, g, b } => {
                canvas.set_draw_color(Color::RGB(r, g, b));
                fill_polygon(canvas, &points);
            }
            DrawCommand::Text { text, x, y, size: _, r, g, b } => {
                render_text(canvas, font, &text, x, y, Color::RGB(r, g, b));
            }
        }
    }
}

/// Fill an arbitrary polygon using the even-odd scanline rule.
/// Handles convex and concave polygons. The draw color must be set by the caller.
fn fill_polygon(canvas: &mut Canvas<Window>, points: &[(i32, i32)]) {
    if points.len() < 3 {
        return;
    }

    let min_y = points.iter().map(|p| p.1).min().unwrap();
    let max_y = points.iter().map(|p| p.1).max().unwrap();

    for y in min_y..=max_y {
        // Collect x-intersections of this scanline with every polygon edge.
        let mut crossings: Vec<i32> = Vec::new();
        let n = points.len();
        for i in 0..n {
            let (x1, y1) = points[i];
            let (x2, y2) = points[(i + 1) % n];
            // Skip horizontal edges; they contribute no crossing.
            if y1 == y2 {
                continue;
            }
            let (ya, yb, xa, xb) = if y1 < y2 {
                (y1, y2, x1, x2)
            } else {
                (y2, y1, x2, x1)
            };
            // Half-open interval [ya, yb) avoids double-counting shared vertices.
            if y >= ya && y < yb {
                let t = (y - ya) as f64 / (yb - ya) as f64;
                let x = xa as f64 + t * (xb - xa) as f64;
                crossings.push(x.round() as i32);
            }
        }

        crossings.sort_unstable();
        // Fill spans between consecutive intersection pairs.
        let mut i = 0;
        while i + 1 < crossings.len() {
            let x_start = crossings[i];
            let x_end = crossings[i + 1];
            let _ = canvas.draw_line((x_start, y), (x_end, y));
            i += 2;
        }
    }
}

fn draw_filled_circle(canvas: &mut Canvas<Window>, cx: i32, cy: i32, radius: i32) {
    // Midpoint circle algorithm - draw horizontal scanlines for filled circle
    let mut x = radius;
    let mut y = 0i32;
    let mut err = 1 - radius;

    while x >= y {
        // Draw horizontal lines for each octant pair
        let _ = canvas.draw_line((cx - x, cy + y), (cx + x, cy + y));
        let _ = canvas.draw_line((cx - x, cy - y), (cx + x, cy - y));
        let _ = canvas.draw_line((cx - y, cy + x), (cx + y, cy + x));
        let _ = canvas.draw_line((cx - y, cy - x), (cx + y, cy - x));

        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

fn render_text(
    canvas: &mut Canvas<Window>,
    font: &Font,
    text: &str,
    x: i32,
    y: i32,
    color: Color,
) {
    if text.is_empty() {
        return;
    }
    let surface = match font.render(text).blended(color) {
        Ok(s) => s,
        Err(_) => return,
    };
    let texture_creator = canvas.texture_creator();
    let texture = match texture_creator.create_texture_from_surface(&surface) {
        Ok(t) => t,
        Err(_) => return,
    };
    let query = texture.query();
    let target = Rect::new(x, y, query.width, query.height);
    let _ = canvas.copy(&texture, None, Some(target));
}
