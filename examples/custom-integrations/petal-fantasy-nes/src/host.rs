//! `NesHost` — the fantasy console's delta on top of the `petal-sdl` game loop.
//!
//! The window, event pump, frame timing, hot reload, and the
//! agent/headless/screenshot/record modes all come from `petal-sdl`. This host
//! supplies only what makes the machine a console:
//!
//!   - the **native set**: the `nes`/`nes_sound` prelude modules over the
//!     video/audio/system natives, plus `petal-ui`'s input natives (so the pads
//!     and `dt`/`frame_count` come from the same normalized stream every other
//!     Petal host uses);
//!   - the **console state**: a [`Ppu`] and an [`Apu`] that the cart rewrites
//!     every frame, driven by the buffered command protocol in
//!     [`crate::natives`];
//!   - the **presentation**: one streaming texture carrying the 256x240 frame,
//!     integer-scaled and letterboxed into whatever the window happens to be,
//!     with an optional scanline filter.
//!
//! Frame order, as the loop defines it:
//!
//! ```text
//! prepare_frame (PPU begin_frame) → cart runs → end_frame (drain video+audio,
//!   pump the device) → after_frame (cart switch) → present (rasterize + blit)
//! ```
//!
//! A **speculative** frame — `--screenshot`'s final capture, the agent's
//! `screenshot` command — never reaches `end_frame`, by design. Its video
//! commands are applied to a *clone* of the PPU inside `render_image`, so
//! taking a picture cannot perturb a running game, and its audio commands are
//! dropped with the fork, so it cannot make noise.

use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, Texture};
use sdl2::video::Window;

use petal::env::Env;
use petal::stack::StackKey;
use petal_sdl::{EscapeAction, Host, ScriptSwitch};

use crate::apu::Apu;
use crate::audio::AudioEngine;
use crate::natives::system::{self, CartEntry};
use crate::natives::{audio as audio_natives, video as video_natives};
use crate::ppu::{FRAME_BYTES, Ppu, SCREEN_H, SCREEN_W};

/// The Petal-source prelude modules, both implicit imports for every cart.
pub const NES_MODULE: &str = "nes";
pub const NES_SOUND_MODULE: &str = "nes_sound";
const NES_PRELUDE: &str = include_str!("../prelude/nes.ptl");
const NES_SOUND_PRELUDE: &str = include_str!("../prelude/nes_sound.ptl");

/// The cart the console boots into when the CLI names no file.
pub const LAUNCHER_CART: &str = "carts/launcher.ptl";

/// Shown when `carts/launcher.ptl` cannot be found — a console with no boot ROM
/// should say so on screen rather than exit with a path error.
const MISSING_LAUNCHER: &str = "\
set_backdrop(1)
log(\"fantasy-nes: carts/launcher.ptl not found; pass a cart path instead\")
";

/// How much darker a scanline row is drawn under `--crt`. Cheap, and the only
/// CRT affectation worth the pixels: no phosphor mask, no barrel distortion.
const SCANLINE_ALPHA: u8 = 60;

pub struct NesHost {
    /// Integer pixel scale requested for the window. A cart's `set_scale` can
    /// change it at runtime, which resizes the window on the next present.
    scale: u32,
    crt: bool,
    ppu: Ppu,
    apu: Apu,
    audio: AudioEngine,
    /// Reusable 256x240 RGB frame; rasterized into every present.
    frame: Vec<u8>,
    /// Streaming texture, built lazily from the loop's canvas.
    texture: Option<Texture<'static>>,
    /// Directory scanned for the launcher's cart list.
    carts_dir: PathBuf,
    /// Whether the loaded program is the launcher (Escape quits from the
    /// launcher, but returns to it from a cart).
    in_launcher: bool,
}

impl NesHost {
    pub fn new(scale: u32, crt: bool) -> Self {
        Self {
            scale: scale.max(1),
            crt,
            ppu: Ppu::new(),
            apu: Apu::new(),
            audio: AudioEngine::new(),
            frame: vec![0u8; FRAME_BYTES],
            texture: None,
            carts_dir: resolve_path("carts").unwrap_or_else(|| PathBuf::from("carts")),
            in_launcher: false,
        }
    }

    /// The launcher cart, read from disk so it hot-reloads like any other.
    fn launcher_switch(&self) -> ScriptSwitch {
        match resolve_path(LAUNCHER_CART) {
            Some(p) => match std::fs::read_to_string(&p) {
                Ok(source) => ScriptSwitch {
                    source,
                    path: Some(p.to_string_lossy().into_owned()),
                },
                Err(_) => embedded_launcher(),
            },
            None => embedded_launcher(),
        }
    }
}

fn embedded_launcher() -> ScriptSwitch {
    ScriptSwitch {
        source: MISSING_LAUNCHER.to_string(),
        path: None,
    }
}

/// Resolve a repo-relative path against the working directory first, then the
/// crate directory. Running `cargo run` from the crate root and running the
/// installed binary from elsewhere should both find the carts.
fn resolve_path(rel: &str) -> Option<PathBuf> {
    let cwd = Path::new(rel);
    if cwd.exists() {
        return Some(cwd.to_path_buf());
    }
    let from_crate = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    from_crate.exists().then_some(from_crate)
}

/// Name of the entry script inside a multi-file cart directory.
const CART_ENTRY: &str = "game.ptl";

/// The carts in `dir`, sorted, with the launcher itself excluded — it is the
/// menu, not an entry on it.
///
/// A cart is either a single `.ptl` file or a directory holding a `game.ptl`
/// entry script plus the modules it imports; the showcase game is the second
/// shape, and a launcher that only saw loose files could not boot it. Only one
/// level deep, and a directory is named for itself rather than for `game`.
fn scan_carts(dir: &Path) -> Vec<CartEntry> {
    let mut carts = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return carts;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.is_dir() {
            let entry_script = path.join(CART_ENTRY);
            if entry_script.is_file() {
                carts.push(CartEntry {
                    name: stem.to_string(),
                    path: entry_script.to_string_lossy().into_owned(),
                });
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("ptl") || stem == "launcher" {
            continue;
        }
        carts.push(CartEntry {
            name: stem.to_string(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    carts.sort_by(|a, b| a.name.cmp(&b.name));
    carts
}

impl Host for NesHost {
    fn register(&mut self, env: &mut Env) {
        // Input/timing/dimensions come from petal-ui, so pads, `dt`, and
        // `frame_count` behave identically to every other Petal host. The
        // draw vocabulary does not: a cart talks to the PPU, not to a canvas.
        petal_ui::input::register_input(env);
        crate::natives::register_all(env);

        // Both prelude modules are implicit, so a cart writes `note("c4")` and
        // `tile_row(...)` bare. `set_implicit_imports` replaces the whole list,
        // which is why this is composed here rather than calling each module's
        // own registration helper.
        env.register_module(NES_MODULE, NES_PRELUDE);
        env.register_module(NES_SOUND_MODULE, NES_SOUND_PRELUDE);
        env.set_implicit_imports(&[NES_MODULE, NES_SOUND_MODULE]);
    }

    fn on_sdl_init(&mut self, sdl: &sdl2::Sdl) {
        self.audio.open(sdl);
    }

    fn default_source(&mut self) -> Option<ScriptSwitch> {
        Some(self.launcher_switch())
    }

    fn on_program_loaded(&mut self, env: &mut Env, path: Option<&str>) {
        let carts = scan_carts(&self.carts_dir);
        system::bind_carts(env, &carts);
        self.in_launcher = match path {
            None => true,
            Some(p) => Path::new(p).file_stem().and_then(|s| s.to_str()) == Some("launcher"),
        };
        // A new cart inherits no sound. Chip channels are sticky by design, so
        // without this a note held when the old cart was replaced would drone
        // on under the new one.
        self.apu.mute();
        self.audio.reset();
    }

    fn prepare_frame(&mut self, _env: &mut Env) {
        self.ppu.begin_frame();
    }

    fn end_frame(&mut self, env: &mut Env) {
        video_natives::apply(env, &mut self.ppu);
        audio_natives::apply(env, &mut self.apu, &mut self.audio);

        let presentation = system::take_presentation(env);
        if let Some(scale) = presentation.scale {
            self.scale = scale.max(1);
        }
        if let Some(crt) = presentation.crt {
            self.crt = crt;
        }

        // Runs in every mode, windowed or not: with no device open this still
        // advances the chip, so a headless run and a windowed run agree.
        self.audio.pump(&mut self.apu);
    }

    fn present(&mut self, canvas: &mut Canvas<Window>, _env: &mut Env) -> Result<(), String> {
        self.ppu.render(&mut self.frame);

        if self.texture.is_none() {
            self.texture = Some(create_frame_texture(canvas)?);
        }
        let texture = self.texture.as_mut().unwrap();
        texture
            .update(None, &self.frame, SCREEN_W * 3)
            .map_err(|e| e.to_string())?;

        // A cart's `set_scale` retargets the window; the loop owns the canvas,
        // so this is the one place with a handle to resize it.
        let want = (SCREEN_W as u32 * self.scale, SCREEN_H as u32 * self.scale);
        if canvas.window().size() != want {
            let _ = canvas.window_mut().set_size(want.0, want.1);
        }

        let (w, h) = canvas.output_size()?;
        let dest = fit_rect(w, h);

        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();
        canvas
            .copy(texture, None, dest)
            .map_err(|e| e.to_string())?;
        if self.crt {
            draw_scanlines(canvas, dest)?;
        }
        canvas.present();
        Ok(())
    }

    fn render_image(
        &mut self,
        env: &mut Env,
        stack: StackKey,
        width: u32,
        height: u32,
    ) -> Result<RgbImage, String> {
        // A speculative frame gets its own console: apply its video writes to a
        // clone so the live game keeps its map, palettes and sprites.
        let mut ppu = self.ppu.clone();
        ppu.begin_frame();
        video_natives::apply_for(env, stack, &mut ppu);

        let mut frame = vec![0u8; FRAME_BYTES];
        ppu.render(&mut frame);

        // Same integer-scale-and-letterbox mapping as the window, so an agent's
        // screenshot is what a human would have seen.
        let dest = fit_rect(width, height);
        let mut img = RgbImage::from_pixel(width.max(1), height.max(1), Rgb([0, 0, 0]));
        let s = (dest.width() / SCREEN_W as u32).max(1);
        for y in 0..dest.height() {
            let sy = (y / s) as usize;
            for x in 0..dest.width() {
                let sx = (x / s) as usize;
                let i = (sy * SCREEN_W + sx) * 3;
                let px = Rgb([frame[i], frame[i + 1], frame[i + 2]]);
                img.put_pixel(dest.x() as u32 + x, dest.y() as u32 + y, px);
            }
        }
        Ok(img)
    }

    fn on_escape(&mut self, _env: &mut Env) -> EscapeAction {
        if self.in_launcher {
            EscapeAction::Quit
        } else {
            EscapeAction::Switch(self.launcher_switch())
        }
    }

    fn after_frame(&mut self, env: &mut Env) -> Option<ScriptSwitch> {
        let path = system::take_pending_launch(env)?;
        match std::fs::read_to_string(&path) {
            Ok(source) => Some(ScriptSwitch {
                source,
                path: Some(path),
            }),
            Err(e) => {
                eprintln!("[fantasy-nes] failed to read cart {}: {}", path, e);
                None
            }
        }
    }
}

/// Build the one streaming texture the frame is uploaded through.
///
/// The texture is created from the canvas's texture creator; we leak the
/// creator and pin the texture to `'static` (the standard sdl2-rust
/// workaround) so it can live in the host across frames while the loop keeps
/// the canvas alive.
fn create_frame_texture(canvas: &Canvas<Window>) -> Result<Texture<'static>, String> {
    let creator = canvas.texture_creator();
    // SAFETY: the texture is backed by `canvas`'s renderer, which the game loop
    // keeps alive for the whole run — longer than this host. Forgetting the
    // creator stops its lifetime from bounding the texture.
    let texture = unsafe {
        std::mem::transmute::<Texture<'_>, Texture<'static>>(
            creator
                .create_texture_streaming(PixelFormatEnum::RGB24, SCREEN_W as u32, SCREEN_H as u32)
                .map_err(|e| e.to_string())?,
        )
    };
    std::mem::forget(creator);
    Ok(texture)
}

/// The largest centered integer multiple of 256x240 that fits in `w`x`h`.
/// Integer-only: a fractional scale would turn even pixel art into uneven pixel
/// art, which is the one thing this console must not do.
fn fit_rect(w: u32, h: u32) -> Rect {
    let scale = (w / SCREEN_W as u32).min(h / SCREEN_H as u32).max(1);
    let (dw, dh) = (SCREEN_W as u32 * scale, SCREEN_H as u32 * scale);
    let x = (w.saturating_sub(dw) / 2) as i32;
    let y = (h.saturating_sub(dh) / 2) as i32;
    Rect::new(x, y, dw, dh)
}

/// Darken the last output row of each source scanline. At scale 1 there is no
/// row to spare, so the filter is skipped rather than blacking out the screen.
fn draw_scanlines(canvas: &mut Canvas<Window>, dest: Rect) -> Result<(), String> {
    let scale = (dest.height() / SCREEN_H as u32).max(1);
    if scale < 2 {
        return Ok(());
    }
    let previous = canvas.blend_mode();
    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(0, 0, 0, SCANLINE_ALPHA));
    let mut rows = Vec::with_capacity(SCREEN_H);
    for line in 0..SCREEN_H as u32 {
        let y = dest.y() + (line * scale + scale - 1) as i32;
        rows.push(Rect::new(dest.x(), y, dest.width(), 1));
    }
    canvas.fill_rects(&rows).map_err(|e| e.to_string())?;
    canvas.set_blend_mode(previous);
    Ok(())
}
