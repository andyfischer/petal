//! PNG screenshots rendered through the **real** renderer.
//!
//! Rather than re-implement a second software rasterizer (which approximated
//! text as colored blocks and diverged from the window), we render the command
//! stream into an SDL `Canvas<Surface>` via [`crate::renderer::render`] — the
//! exact path the live window uses — then read the pixels back into a PNG. So a
//! screenshot has real glyphs, honored clip regions, and correct offscreen-
//! canvas compositing, pixel-identical to what's on screen.

use base64::Engine;
use image::{Rgb, RgbImage};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::surface::Surface;

use crate::commands::DrawCommand;
use crate::font::FontLadder;
use crate::renderer;

pub fn render_to_png_base64(
    commands: &[DrawCommand],
    width: u32,
    height: u32,
    fonts: &FontLadder,
) -> String {
    let img = render_commands(commands, width, height, fonts);
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encode failed");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

pub fn save_png(
    commands: &[DrawCommand],
    width: u32,
    height: u32,
    path: &str,
    fonts: &FontLadder,
) -> Result<(), String> {
    let img = render_commands(commands, width, height, fonts);
    img.save(path).map_err(|e| e.to_string())
}

/// Render the command stream through the real renderer into an RGB image.
fn render_commands(
    commands: &[DrawCommand],
    width: u32,
    height: u32,
    fonts: &FontLadder,
) -> RgbImage {
    // A software surface, same pixel format as the live framebuffer. Frames
    // usually `clear()` first; default to black for the (rare) frame that
    // doesn't so the readback is well-defined.
    let mut surface = Surface::new(width.max(1), height.max(1), PixelFormatEnum::RGB888)
        .expect("create screenshot surface");
    let _ = surface.fill_rect(None, Color::RGB(0, 0, 0));

    let mut sc = surface.into_canvas().expect("screenshot surface into canvas");
    renderer::render(&mut sc, commands.to_vec(), fonts);
    surface_to_image(&sc.into_surface())
}

/// Copy an RGB888 SDL surface into an `image::RgbImage`.
fn surface_to_image(surface: &Surface) -> RgbImage {
    let (w, h) = surface.size();
    let pitch = surface.pitch() as usize;
    let bpp = surface.pixel_format_enum().byte_size_per_pixel();
    let mut img = RgbImage::new(w, h);
    surface.with_lock(|pixels: &[u8]| {
        for y in 0..h {
            for x in 0..w {
                let offset = (y as usize) * pitch + (x as usize) * bpp;
                // RGB888 packs as a 32-bit little-endian value: byte0=B, byte1=G, byte2=R.
                let b = pixels[offset];
                let g = pixels[offset + 1];
                let r = pixels[offset + 2];
                img.put_pixel(x, y, Rgb([r, g, b]));
            }
        }
    });
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::DEFAULT_LADDER;

    fn test_ladder(ttf: &sdl2::ttf::Sdl2TtfContext) -> Option<FontLadder<'_>> {
        FontLadder::load_system(ttf, DEFAULT_LADDER).ok()
    }

    /// A screenshot of text must contain real glyphs — lit strokes with black
    /// gaps between and inside letters — not a solid filled rectangle the way
    /// the old block approximation produced.
    #[test]
    fn text_screenshot_has_glyphs_not_a_block() {
        let ttf = sdl2::ttf::init().unwrap();
        let fonts = test_ladder(&ttf).expect("a system font for tests");

        let commands = vec![
            DrawCommand::Clear { r: 0, g: 0, b: 0 },
            DrawCommand::Text {
                text: "Hello".to_string(),
                x: 10,
                y: 10,
                size: 40,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
        ];
        let img = render_commands(&commands, 200, 80, &fonts);

        // Inspect the text's rough bounding box.
        let (mut lit, mut dark, mut total) = (0u32, 0u32, 0u32);
        for y in 10..55 {
            for x in 10..170 {
                let px = img.get_pixel(x, y);
                total += 1;
                if px[0] > 128 {
                    lit += 1;
                } else {
                    dark += 1;
                }
            }
        }

        assert!(lit > 0, "text should paint lit glyph pixels");
        assert!(
            dark > 0,
            "a real glyph run leaves black gaps between letters — a solid block would have none"
        );
        // Glyph coverage of a bounding box is well under half; a block is ~100%.
        assert!(
            lit * 2 < total,
            "lit coverage {lit}/{total} looks like a filled block, not glyphs"
        );
    }

    /// Primitives read back with the correct color at the correct place.
    #[test]
    fn rect_screenshot_reads_back_color() {
        let ttf = sdl2::ttf::init().unwrap();
        let fonts = test_ladder(&ttf).expect("a system font for tests");
        let commands = vec![
            DrawCommand::Clear { r: 0, g: 0, b: 0 },
            DrawCommand::Rect { x: 5, y: 5, w: 20, h: 20, r: 10, g: 200, b: 40, a: 255, radius: 0 },
        ];
        let img = render_commands(&commands, 64, 64, &fonts);
        let px = img.get_pixel(15, 15);
        assert_eq!((px[0], px[1], px[2]), (10, 200, 40), "rect color should read back");
        let bg = img.get_pixel(40, 40);
        assert_eq!((bg[0], bg[1], bg[2]), (0, 0, 0), "background should be the clear color");
    }
}
