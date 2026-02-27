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
            DrawCommand::Text { text, x, y, size: _, r, g, b } => {
                render_text(canvas, font, &text, x, y, Color::RGB(r, g, b));
            }
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
