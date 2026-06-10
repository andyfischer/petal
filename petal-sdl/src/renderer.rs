use sdl2::pixels::Color;
use sdl2::rect::Rect;
use sdl2::render::{Canvas, RenderTarget};
use sdl2::surface::Surface;
use sdl2::ttf::Font;
use sdl2::video::Window;

use crate::commands::DrawCommand;

/// Render targets that can also render TTF text. `texture_creator()` is not
/// part of the generic `Canvas<T>` API — it is defined separately for
/// `Canvas<Window>` and `Canvas<Surface>` with distinct context types — so we
/// abstract text rendering behind this trait. Each impl keeps its concrete
/// texture-creator/context type internal.
pub trait TextTarget: RenderTarget + Sized {
    fn render_text(canvas: &mut Canvas<Self>, font: &Font, text: &str, x: i32, y: i32, color: Color);
}

impl TextTarget for Window {
    fn render_text(
        canvas: &mut Canvas<Self>,
        font: &Font,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
    ) {
        let creator = canvas.texture_creator();
        render_text_impl(canvas, &creator, font, text, x, y, color);
    }
}

impl TextTarget for Surface<'_> {
    fn render_text(
        canvas: &mut Canvas<Self>,
        font: &Font,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
    ) {
        // On a software `Canvas<Surface>`, `create_texture_from_surface` /
        // `copy` go through the software renderer. If any step fails it is
        // ignored (text is best-effort); primitives are unaffected.
        let creator = canvas.texture_creator();
        render_text_impl(canvas, &creator, font, text, x, y, color);
    }
}

pub fn render<T: TextTarget>(canvas: &mut Canvas<T>, commands: Vec<DrawCommand>, font: &Font) {
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
                T::render_text(canvas, font, &text, x, y, Color::RGB(r, g, b));
            }
        }
    }
}

/// Fill an arbitrary polygon using the even-odd scanline rule.
/// Handles convex and concave polygons. The draw color must be set by the caller.
fn fill_polygon<T: RenderTarget>(canvas: &mut Canvas<T>, points: &[(i32, i32)]) {
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

fn draw_filled_circle<T: RenderTarget>(canvas: &mut Canvas<T>, cx: i32, cy: i32, radius: i32) {
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

/// Shared text-rendering body. Generic over the `Canvas` target `T` and the
/// `TextureCreator`'s context `C`; the caller supplies a creator matching the
/// canvas so the produced texture is compatible with `canvas.copy`.
fn render_text_impl<T, C>(
    canvas: &mut Canvas<T>,
    texture_creator: &sdl2::render::TextureCreator<C>,
    font: &Font,
    text: &str,
    x: i32,
    y: i32,
    color: Color,
) where
    T: RenderTarget,
{
    if text.is_empty() {
        return;
    }
    let surface = match font.render(text).blended(color) {
        Ok(s) => s,
        Err(_) => return,
    };
    let texture = match texture_creator.create_texture_from_surface(&surface) {
        Ok(t) => t,
        Err(_) => return,
    };
    let query = texture.query();
    let target = Rect::new(x, y, query.width, query.height);
    let _ = canvas.copy(&texture, None, Some(target));
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl2::pixels::PixelFormatEnum;
    use sdl2::surface::Surface;

    const W: u32 = 64;
    const H: u32 = 64;

    /// Build a black, persistent software surface for accumulation tests.
    fn new_black_surface() -> Surface<'static> {
        let mut surface = Surface::new(W, H, PixelFormatEnum::RGB888).unwrap();
        surface.fill_rect(None, Color::RGB(0, 0, 0)).unwrap();
        surface
    }

    /// Try to load a font; tests pass None if no system font is available so
    /// they remain robust in headless CI. Primitive-only frames don't need it.
    fn load_test_font(
        ttf: &sdl2::ttf::Sdl2TtfContext,
    ) -> Option<sdl2::ttf::Font<'_, '_>> {
        let paths = [
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNSMono.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ];
        for p in &paths {
            if std::path::Path::new(p).exists() {
                if let Ok(f) = ttf.load_font(p, 24) {
                    return Some(f);
                }
            }
        }
        None
    }

    /// Render a frame's commands into the persistent surface via the REAL
    /// `render` entry point, exercising the generic `Canvas<Surface>` target.
    /// Uses the same into_canvas / into_surface dance the game loop uses.
    fn render_frame(
        surface: Surface<'static>,
        commands: Vec<DrawCommand>,
        font: &Font,
    ) -> Surface<'static> {
        let mut sc = surface.into_canvas().unwrap();
        render(&mut sc, commands, font);
        sc.into_surface()
    }

    /// Read back the RGB of a pixel from a locked surface.
    fn pixel_rgb(surface: &Surface, px: u32, py: u32) -> (u8, u8, u8) {
        let pitch = surface.pitch() as usize;
        let bpp = surface.pixel_format_enum().byte_size_per_pixel();
        let mut out = (0u8, 0u8, 0u8);
        surface.with_lock(|pixels: &[u8]| {
            let offset = (py as usize) * pitch + (px as usize) * bpp;
            // RGB888 packs as a 32-bit little-endian value: byte0=B, byte1=G, byte2=R
            let b = pixels[offset];
            let g = pixels[offset + 1];
            let r = pixels[offset + 2];
            out = (r, g, b);
        });
        out
    }

    fn is_white(px: (u8, u8, u8)) -> bool {
        px.0 > 200 && px.1 > 200 && px.2 > 200
    }

    fn is_black(px: (u8, u8, u8)) -> bool {
        px.0 < 50 && px.1 < 50 && px.2 < 50
    }

    #[test]
    fn no_clear_accumulates() {
        let ttf = sdl2::ttf::init().unwrap();
        let font = load_test_font(&ttf).expect("a system font for tests");
        let mut surface = new_black_surface();

        // Frame 1: white rect at (2,2), NO clear.
        surface = render_frame(
            surface,
            vec![DrawCommand::Rect { x: 2, y: 2, w: 4, h: 4, r: 255, g: 255, b: 255 }],
            &font,
        );
        // Frame 2: white rect at (40,40), NO clear — should accumulate.
        surface = render_frame(
            surface,
            vec![DrawCommand::Rect { x: 40, y: 40, w: 4, h: 4, r: 255, g: 255, b: 255 }],
            &font,
        );

        assert!(is_white(pixel_rgb(&surface, 3, 3)), "frame-1 pixel should persist");
        assert!(is_white(pixel_rgb(&surface, 41, 41)), "frame-2 pixel should be drawn");
    }

    #[test]
    fn clear_wipes() {
        let ttf = sdl2::ttf::init().unwrap();
        let font = load_test_font(&ttf).expect("a system font for tests");
        let mut surface = new_black_surface();

        // Frame 1: white rect at (2,2).
        surface = render_frame(
            surface,
            vec![DrawCommand::Rect { x: 2, y: 2, w: 4, h: 4, r: 255, g: 255, b: 255 }],
            &font,
        );
        // Frame 2: Clear(black) then white rect at (40,40).
        surface = render_frame(
            surface,
            vec![
                DrawCommand::Clear { r: 0, g: 0, b: 0 },
                DrawCommand::Rect { x: 40, y: 40, w: 4, h: 4, r: 255, g: 255, b: 255 },
            ],
            &font,
        );

        assert!(is_black(pixel_rgb(&surface, 3, 3)), "frame-1 pixel should be wiped");
        assert!(is_white(pixel_rgb(&surface, 41, 41)), "frame-2 pixel should be drawn");
    }

    #[test]
    fn text_renders_on_software_surface() {
        // Confirms TTF text rendering works on a `Canvas<Surface>` (software
        // renderer path) and is not silently dropped. We scan the text's
        // bounding box for any non-black pixel.
        let ttf = sdl2::ttf::init().unwrap();
        let font = load_test_font(&ttf).expect("a system font for tests");
        let surface = render_frame(
            new_black_surface(),
            vec![DrawCommand::Text {
                text: "Hi".to_string(),
                x: 2,
                y: 2,
                size: 24,
                r: 255,
                g: 255,
                b: 255,
            }],
            &font,
        );

        let mut any_lit = false;
        for py in 0..32 {
            for px in 0..40 {
                if !is_black(pixel_rgb(&surface, px, py)) {
                    any_lit = true;
                }
            }
        }
        assert!(any_lit, "text should draw at least one non-black pixel on the software surface");
    }
}
