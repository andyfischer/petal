//! The default `petal-sdl` host: an SDL-canvas renderer over the standard
//! `petal-ui` draw vocabulary, plus the example browser, sandboxed file I/O and
//! `sfx`/`synth` sound effects ([`crate::sound`]).
//!
//! This is the [`Host`] the shipped `petal-sdl` binary runs. Sample apps that
//! only need 2D drawing + the `ui` prelude use this host unchanged (Shape A);
//! apps that need a different renderer or native set supply their own `Host`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use image::{Rgb, RgbImage};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::render::Canvas;
use sdl2::surface::Surface;
use sdl2::ttf::Sdl2TtfContext;
use sdl2::video::Window;

use petal::env::Env;
use petal::stack::StackKey;

use petal_ui::draw::{DrawCommand, take_draw_commands, take_draw_commands_for};

use crate::font::{self, FontBook, SdlFontSource};
use crate::game_loop::{EscapeAction, Host, ScriptSwitch};
use crate::hidpi::{self, Scale};
use crate::native_fns::{self, ExampleEntry, bind_examples, take_pending_launch};
use crate::renderer;
use crate::sound::SoundBoard;

const BROWSER_SCRIPT: &str = include_str!("../examples/browser.ptl");

/// The (weight, italic) variants measured and bound for every face. SDL_ttf
/// synthesizes exactly these two axes, so there is nothing to gain from a
/// finer weight ladder: 700 is what `>= 600` embolden to.
const VARIANTS: &[(u16, bool)] = &[(400, false), (700, false), (400, true), (700, true)];

pub struct DefaultHost {
    /// The faces this host can render, each a ladder of baked sizes; `None` if
    /// none could load (headless on a machine with no TTF font — screenshots
    /// then fail informatively). Shared with the [`SdlFontSource`] the text
    /// natives measure installed families through, so measuring and drawing
    /// use the same ladders.
    fonts: Option<Rc<RefCell<FontBook<'static>>>>,
    /// When set, `petal-sdl` was launched with no script: run the example
    /// browser over the `.ptl` files in this directory.
    examples_dir: Option<PathBuf>,
    /// Persistent software framebuffer (windowed present). Pixels accumulate
    /// across frames unless a `Clear` command wipes them; lazily sized to the
    /// window's drawable area (in pixels, see [`crate::hidpi`]) on present.
    framebuffer: Option<Surface<'static>>,
    /// Whether the currently-loaded program is the browser (Escape quits from
    /// the browser, but returns to it from a launched example).
    in_browser: bool,
    /// `sfx`/`synth` playback: silent (but still draining requests) until
    /// `on_sdl_init` opens a device.
    sound: SoundBoard,
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
                FontBook::load_system(ttf, font::DEFAULT_LADDER)
                    .ok()
                    .map(|book| Rc::new(RefCell::new(book)))
            }
            Err(_) => None,
        };
        Self::with_fonts(fonts, examples_dir)
    }

    /// [`new`](Self::new) with the font book already loaded (or absent).
    fn with_fonts(
        fonts: Option<Rc<RefCell<FontBook<'static>>>>,
        examples_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            fonts,
            examples_dir,
            framebuffer: None,
            in_browser: false,
            sound: SoundBoard::new(),
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
            // Installed families (`font: "Georgia"`) are measured on demand
            // through the font source, from the same book the renderer draws
            // with. It lives in petal-ui's thread-local channel for the life of
            // the process; the loop runs every frame on this thread.
            petal_ui::draw::swap_font_provider(Some(Box::new(SdlFontSource::new(fonts.clone()))));
            let mut fonts = fonts.borrow_mut();
            let ratios = fonts.default_face().ascii_advance_ratios();
            petal_ui::draw::bind_text_advance_table(env, &ratios);
            // …and the other axis. This host blits a run's rendered surface
            // with its top-left at the command's `(x, y)`, so "where is the
            // baseline relative to y" has an exact answer here — publish it,
            // and a script can centre a label on its cap height instead of
            // assuming the run is `size` px tall.
            let vertical = fonts.default_face().vertical_ratios();
            petal_ui::draw::bind_text_vertical_metrics(env, &vertical);
            // The default font *is* the `ui` role here, so a style that names
            // no face still resolves that face's bold/italic variants.
            petal_ui::draw::bind_default_font_name(env, font::DEFAULT_ROLE);
            // One entry per face × synthetic variant this host can actually
            // draw, so a style's weight/italic measures the widths it will be
            // rasterized at (emboldening widens glyphs). Roles the machine
            // hasn't got (`serif`, and `mono` on a machine with no fixed-pitch
            // face) stay unregistered and degrade to the default font.
            for (role, face) in fonts.roles() {
                for (weight, italic) in VARIANTS {
                    let ratios = face.ascii_advance_ratios_styled(*weight, *italic);
                    let vertical = face.vertical_ratios_styled(*weight, *italic);
                    let metrics = petal_ui::draw::FontMetrics::proportional(
                        ratios,
                        petal_ui::draw::DEFAULT_TEXT_ADVANCE,
                    )
                    .with_vertical(vertical);
                    petal_ui::draw::bind_font_variant_metrics(
                        env, role, *weight, *italic, &metrics,
                    );
                }
            }
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
        self.present_commands(canvas, commands)
    }

    fn supports_timeline(&self) -> bool {
        true
    }

    fn high_dpi(&self) -> bool {
        true
    }

    fn present_commands(
        &mut self,
        canvas: &mut Canvas<Window>,
        commands: Vec<DrawCommand>,
    ) -> Result<(), String> {
        // The script drew in window points; paint in drawable pixels. The
        // persistent framebuffer is made lazily at that size, and again when
        // the size changes (the window moved to a screen of another scale).
        let scale = Scale::of(canvas);
        let (w, h) = scale.pixels();
        let surface = match self.framebuffer.take() {
            Some(s) if s.size() == (w, h) => s,
            _ => new_framebuffer(w, h)?,
        };
        let commands = hidpi::scale_commands(commands, scale.factor);

        // Render this frame's commands into the persistent surface, then blit
        // the whole surface to the window. Because the surface is software-
        // backed, pixels persist unless a `Clear` command wipes them.
        let mut sc = surface.into_canvas().map_err(|e| e.to_string())?;
        renderer::render(
            &mut sc,
            commands,
            &mut self
                .fonts
                .as_ref()
                .expect("fonts for windowed present")
                .borrow_mut(),
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
        Ok(render_commands(
            &commands,
            width,
            height,
            &mut fonts.borrow_mut(),
        ))
    }

    fn render_commands_image(
        &mut self,
        commands: &[DrawCommand],
        width: u32,
        height: u32,
    ) -> Result<RgbImage, String> {
        let fonts = self
            .fonts
            .as_ref()
            .ok_or_else(|| "screenshot unavailable: no system font could be loaded".to_string())?;
        Ok(render_commands(
            commands,
            width,
            height,
            &mut fonts.borrow_mut(),
        ))
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

    fn on_sdl_init(&mut self, sdl: &sdl2::Sdl) {
        self.sound.open(sdl);
    }

    fn end_frame(&mut self, env: &mut Env) {
        self.sound.end_frame(env);
    }

    /// The device queue is only topped up from `end_frame`, which a gated
    /// frame never reaches — so run every frame while a sound is playing,
    /// and gate again once the last one has been fed.
    fn frame_gating(&self) -> bool {
        !self.sound.is_playing()
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
    fonts: &mut FontBook,
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

    fn test_fonts(ttf: &sdl2::ttf::Sdl2TtfContext) -> Option<FontBook<'_>> {
        FontBook::load_system(ttf, DEFAULT_LADDER).ok()
    }

    /// A screenshot of text must contain real glyphs — lit strokes with black
    /// gaps — not a solid filled rectangle (the old block approximation).
    #[test]
    fn text_screenshot_has_glyphs_not_a_block() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = test_fonts(&ttf).expect("a system font for tests");
        let commands = vec![
            DrawCommand::Clear { r: 0, g: 0, b: 0 },
            DrawCommand::plain_text("Hello", 10, 10, 40, 255, 255, 255, 255),
        ];
        let img = render_commands(&commands, 200, 80, &mut fonts);
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
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let mut fonts = test_fonts(&ttf).expect("a system font for tests");
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
        let img = render_commands(&commands, 64, 64, &mut fonts);
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

    /// End to end through the script: `text_width` in an installed family
    /// (named in any case) measures that family, `font(name)` answers with its
    /// canonical spelling, and the renderer draws the run as wide as it
    /// measured. Skipped on a machine with none of the candidate families.
    #[test]
    fn an_installed_family_measures_and_draws_by_name() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        // `DefaultHost::new` leaks its TTF context for the process, which
        // would stop every other test from initializing SDL_ttf. Own one
        // here instead and shut it down after everything borrowing it.
        let ttf = Box::new(sdl2::ttf::init().unwrap());
        // SAFETY: every borrower — the book, the host and the font source in
        // petal-ui's channel — is dropped below before `ttf` is.
        let ttf_ref: &'static Sdl2TtfContext = unsafe { &*(ttf.as_ref() as *const _) };
        let Ok(book) = FontBook::load_system(ttf_ref, DEFAULT_LADDER) else {
            return;
        };
        let book = Rc::new(RefCell::new(book));
        let mut host = DefaultHost::with_fonts(Some(book.clone()), None);
        let families = book.borrow_mut().families();
        let Some(family) = [
            "Menlo",
            "Courier New",
            "Courier",
            "DejaVu Sans Mono",
            "Liberation Mono",
        ]
        .into_iter()
        .find(|f| families.iter().any(|g| g == f)) else {
            return;
        };
        let mut env = Env::new();
        native_fns::register_all(&mut env);
        petal_ui::draw::clear_font_cache();
        host.on_program_loaded(&mut env, None);
        let lower = family.to_lowercase();
        let mut run = |src: String| {
            env.run_source(&format!("import ui\n{src}"))
                .expect("script runs")
        };
        let int = |v: petal::value::Value| match v {
            petal::value::Value::Int(n) => n as f64,
            other => panic!("expected an int, got {other:?}"),
        };
        let default_w = int(run("text_width(\"illlli\", {size: 20})".to_string()));
        let family_w = int(run(format!(
            "text_width(\"illlli\", {{size: 20, font: \"{lower}\"}})"
        )));
        assert!(
            family_w > default_w,
            "{family} should measure wider: {family_w} vs {default_w}"
        );
        let canonical = run(format!("font(\"{lower}\").font == \"{family}\""));
        assert_eq!(
            canonical,
            petal::value::Value::Bool(true),
            "font() canonicalizes {lower}"
        );
        let listed = run("len(fonts()) > 0".to_string());
        assert_eq!(
            listed,
            petal::value::Value::Bool(true),
            "fonts() lists the installed families"
        );

        let run = DrawCommand::Text {
            text: "illlli".to_string(),
            x: 4,
            y: 4,
            size: 20,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            font: Some(lower.clone()),
            weight: 400,
            italic: false,
            spacing: 0.0,
        };
        let img = render_commands(&[run], 300, 40, &mut book.borrow_mut());
        let lit: Vec<u32> = (0..img.width())
            .filter(|&x| (0..img.height()).any(|y| img.get_pixel(x, y)[0] > 64))
            .collect();
        let drawn = (lit.last().unwrap() - lit.first().unwrap() + 1) as f64;
        assert!(
            (drawn - family_w).abs() <= family_w * 0.15 + 2.0,
            "{family} drew {drawn}px but measured {family_w}px"
        );
        drop(petal_ui::draw::swap_font_provider(None));
        petal_ui::draw::clear_font_cache();
        drop(env);
        drop(host);
        drop(book);
        drop(ttf);
    }
}
