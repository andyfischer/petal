//! Bridges the software framebuffer to the SDL2 window via a streaming texture.
//!
//! We rasterize everything into our own `Vec<u8>` (see `framebuffer.rs`) and
//! upload it once per frame. Doing it this way lets headless/screenshot modes
//! use the exact same rasterizer as the windowed mode — agent-visible pixels
//! === human-visible pixels.
//!
//! The generic game loop owns the `Canvas<Window>`; this presenter owns only
//! the streaming texture and is handed the canvas each frame.

use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{Texture, WindowCanvas};

use crate::framebuffer::Framebuffer;

pub struct Presenter {
    texture: Texture<'static>,
    width: u32,
}

impl Presenter {
    /// Build a streaming texture sized to the framebuffer. The texture is
    /// created from `canvas`'s texture creator; we leak the creator and pin the
    /// texture to `'static` (the standard sdl2-rust workaround) so it can live
    /// in the host across frames while the loop keeps the canvas alive.
    pub fn new(canvas: &WindowCanvas, width: u32, height: u32) -> Result<Self, String> {
        let texture_creator = canvas.texture_creator();
        // SAFETY: the texture is backed by `canvas`'s renderer, which the game
        // loop keeps alive for the whole run — longer than this presenter. We
        // forget the creator so its lifetime doesn't bound the texture.
        let texture = unsafe {
            std::mem::transmute::<Texture<'_>, Texture<'static>>(
                texture_creator
                    .create_texture_streaming(PixelFormatEnum::RGB24, width, height)
                    .map_err(|e| e.to_string())?,
            )
        };
        std::mem::forget(texture_creator);
        Ok(Self { texture, width })
    }

    pub fn present(&mut self, canvas: &mut WindowCanvas, fb: &Framebuffer) -> Result<(), String> {
        self.texture
            .update(None, &fb.color, (self.width * 3) as usize)
            .map_err(|e| e.to_string())?;
        canvas.clear();
        canvas
            .copy(&self.texture, None, None)
            .map_err(|e| e.to_string())?;
        canvas.present();
        Ok(())
    }
}
