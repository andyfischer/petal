//! The default `petal-sdl` host: an SDL-canvas renderer over the standard
//! `petal-ui` draw vocabulary, plus the example browser and sandboxed file I/O.
//!
//! This is the [`Host`] the shipped `petal-sdl` binary runs. Sample apps that
//! only need 2D drawing + the `ui` prelude use this host unchanged (Shape A);
//! apps that need a different renderer or native set supply their own `Host`.

use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::render::Canvas;
use sdl2::surface::Surface;
use sdl2::ttf::Sdl2TtfContext;
use sdl2::video::Window;

use petal::env::Env;
use petal::stack::StackKey;

use crate::commands::{DrawCommand, take_draw_commands, take_draw_commands_for};
use crate::font::{self, FontLadder};
use crate::game_loop::{EscapeAction, Host, ScriptSwitch};
use crate::native_fns::{self, ExampleEntry, bind_examples, take_pending_launch};
use crate::renderer;

const BROWSER_SCRIPT: &str = include_str!("../examples/browser.ptl");

pub struct DefaultHost {
    /// System fonts at a ladder of sizes; `None` if none could load (headless
    /// on a machine with no TTF font — screenshots then fail informatively).
    fonts: Option<FontLadder<'static>>,
    /// When set, `petal-sdl` was launched with no script: run the example
    /// browser over the `.ptl` files in this directory.
    examples_dir: Option<PathBuf>,
    /// Persistent software framebuffer (windowed present). Pixels accumulate
    /// across frames unless a `Clear` command wipes them; lazily sized to the
    /// window's drawable area on first present.
    framebuffer: Option<Surface<'static>>,
    /// Whether the currently-loaded program is the browser (Escape quits from
    /// the browser, but returns to it from a launched example).
    in_browser: bool,
}

impl DefaultHost {
    /// Build the host. Loads a system font ladder up front (best-effort: a
    /// machine with no TTF font still runs non-screenshot modes). `examples_dir`
    /// enables browser mode when the CLI is given no script.
    pub fn new(examples_dir: Option<PathBuf>) -> Self {
        // Leak the TTF context so the font ladder can borrow it for the whole
        // process without a self-referential struct. It's a one-time init.
        let fonts = match sdl2::ttf::init() {
            Ok(ttf) => {
                let ttf: &'static Sdl2TtfContext = Box::leak(Box::new(ttf));
                FontLadder::load_system(ttf, font::DEFAULT_LADDER).ok()
            }
            Err(_) => None,
        };
        Self {
            fonts,
            examples_dir,
            framebuffer: None,
            in_browser: false,
        }
    }
}

impl Host for DefaultHost {
    fn register(&mut self, env: &mut Env) {
        native_fns::register_all(env);
    }

    fn default_source(&mut self) -> Option<ScriptSwitch> {
        self.examples_dir.as_ref().map(|_| ScriptSwitch {
            source: BROWSER_SCRIPT.to_string(),
            path: None,
        })
    }

    fn on_program_loaded(&mut self, env: &mut Env, path: Option<&str>) {
        // Bind proportional text metrics so `text_width()` matches rendered
        // glyphs (correct centering / right-alignment).
        if let Some(fonts) = &self.fonts {
            petal_ui::draw::bind_text_advance_table(env, &fonts.ascii_advance_ratios());
        }
        // (Re)bind the browser example list on every load — cheap and keeps the
        // browser's list correct after returning to it.
        if let Some(dir) = &self.examples_dir {
            let examples = load_examples(dir);
            bind_examples(env, &examples);
        }
        // The browser is the embedded, path-less program (only in browser mode).
        self.in_browser = path.is_none() && self.examples_dir.is_some();
    }

    fn prepare_frame(&mut self, env: &mut Env) {
        petal_ui::draw::reset_canvas_ids(env);
    }

    fn present(&mut self, canvas: &mut Canvas<Window>, env: &mut Env) -> Result<(), String> {
        let commands = take_draw_commands(env);

        // Lazily create the persistent framebuffer at the drawable size.
        let surface = match self.framebuffer.take() {
            Some(s) => s,
            None => {
                let (w, h) = canvas.output_size()?;
                new_framebuffer(w, h)?
            }
        };

        // Render this frame's commands into the persistent surface, then blit
        // the whole surface to the window. Because the surface is software-
        // backed, pixels persist unless a `Clear` command wipes them.
        let mut sc = surface.into_canvas().map_err(|e| e.to_string())?;
        renderer::render(
            &mut sc,
            commands,
            self.fonts.as_ref().expect("fonts for windowed present"),
        );
        let surface = sc.into_surface();

        let tc = canvas.texture_creator();
        let tex = tc
            .create_texture_from_surface(&surface)
            .map_err(|e| e.to_string())?;
        canvas.copy(&tex, None, None)?;
        canvas.present();

        self.framebuffer = Some(surface);
        Ok(())
    }

    fn render_image(
        &mut self,
        env: &mut Env,
        stack: StackKey,
        width: u32,
        height: u32,
    ) -> Result<RgbImage, String> {
        let fonts = self
            .fonts
            .as_ref()
            .ok_or_else(|| "screenshot unavailable: no system font could be loaded".to_string())?;
        let commands = take_draw_commands_for(env, stack);
        Ok(render_commands(&commands, width, height, fonts))
    }

    fn draw_commands_json(&mut self, env: &mut Env, stack: StackKey) -> serde_json::Value {
        let commands = take_draw_commands_for(env, stack);
        serde_json::to_value(&commands).unwrap_or(serde_json::Value::Null)
    }

    fn on_escape(&mut self, _env: &mut Env) -> EscapeAction {
        if self.in_browser || self.examples_dir.is_none() {
            EscapeAction::Quit
        } else {
            // Return to the browser (a fresh embedded load).
            EscapeAction::Switch(ScriptSwitch {
                source: BROWSER_SCRIPT.to_string(),
                path: None,
            })
        }
    }

    fn after_frame(&mut self, env: &mut Env) -> Option<ScriptSwitch> {
        let path = take_pending_launch(env)?;
        match std::fs::read_to_string(&path) {
            Ok(source) => Some(ScriptSwitch {
                source,
                path: Some(path),
            }),
            Err(e) => {
                eprintln!("[browser] failed to read {}: {}", path, e);
                None
            }
        }
    }
}

/// A black, persistent software framebuffer. Only a `Clear` command wipes it,
/// which is what makes accumulative generative art work.
fn new_framebuffer(width: u32, height: u32) -> Result<Surface<'static>, String> {
    let mut surface = Surface::new(width.max(1), height.max(1), PixelFormatEnum::RGB888)
        .map_err(|e| e.to_string())?;
    surface
        .fill_rect(None, Color::RGB(0, 0, 0))
        .map_err(|e| e.to_string())?;
    Ok(surface)
}

/// Rasterize a command stream through the real renderer into an RGB image —
/// the exact path the live window uses, so screenshots match the screen (real
/// glyphs, honored clips, correct offscreen-canvas compositing).
fn render_commands(
    commands: &[DrawCommand],
    width: u32,
    height: u32,
    fonts: &FontLadder,
) -> RgbImage {
    let mut surface = Surface::new(width.max(1), height.max(1), PixelFormatEnum::RGB888)
        .expect("create screenshot surface");
    let _ = surface.fill_rect(None, Color::RGB(0, 0, 0));

    let mut sc = surface
        .into_canvas()
        .expect("screenshot surface into canvas");
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

/// Scan a directory for example `.ptl` scripts (excluding the browser itself),
/// returning display-name/path entries sorted by path.
fn load_examples(examples_dir: &Path) -> Vec<ExampleEntry> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Ok(dir_entries) = std::fs::read_dir(examples_dir) {
        for entry in dir_entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "ptl") {
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name != "browser" {
                    paths.push(path);
                }
            }
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            // Capitalize the first letter for display.
            let display_name = {
                let mut c = name.chars();
                match c.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                }
            };
            ExampleEntry {
                name: display_name,
                path: path.to_string_lossy().to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::DEFAULT_LADDER;

    fn test_ladder(ttf: &sdl2::ttf::Sdl2TtfContext) -> Option<FontLadder<'_>> {
        FontLadder::load_system(ttf, DEFAULT_LADDER).ok()
    }

    /// A screenshot of text must contain real glyphs — lit strokes with black
    /// gaps — not a solid filled rectangle (the old block approximation).
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
            "a real glyph run leaves black gaps between letters"
        );
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
            DrawCommand::Rect {
                x: 5,
                y: 5,
                w: 20,
                h: 20,
                r: 10,
                g: 200,
                b: 40,
                a: 255,
                radius: 0,
            },
        ];
        let img = render_commands(&commands, 64, 64, &fonts);
        let px = img.get_pixel(15, 15);
        assert_eq!(
            (px[0], px[1], px[2]),
            (10, 200, 40),
            "rect color should read back"
        );
        let bg = img.get_pixel(40, 40);
        assert_eq!(
            (bg[0], bg[1], bg[2]),
            (0, 0, 0),
            "background should be the clear color"
        );
    }
}
