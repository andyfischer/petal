//! Bridges the software framebuffer to an SDL2 window via a streaming texture.
//!
//! We rasterize everything into our own Vec<u8> (see `framebuffer.rs`) and
//! upload once per frame. Doing it this way lets headless/screenshot modes use
//! the exact same rasterizer as the windowed mode — agent-visible pixels ===
//! human-visible pixels.

use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{Texture, WindowCanvas};

use crate::framebuffer::Framebuffer;

pub struct Renderer {
    pub canvas: WindowCanvas,
    texture: Texture<'static>,
    width: u32,
    height: u32,
}

impl Renderer {
    pub fn new(canvas: WindowCanvas, width: u32, height: u32) -> Result<Self, String> {
        let texture_creator = canvas.texture_creator();
        // SAFETY: The texture borrows from texture_creator, which lives only
        // as long as canvas. We use a little unsafe transmute to keep both
        // together in the same struct. This pattern is standard for SDL2-rust.
        let texture = unsafe {
            std::mem::transmute::<Texture<'_>, Texture<'static>>(
                texture_creator
                    .create_texture_streaming(PixelFormatEnum::RGB24, width, height)
                    .map_err(|e| e.to_string())?,
            )
        };
        // We must leak the texture_creator so its lifetime outlives our
        // texture. It's a small one-time allocation.
        std::mem::forget(texture_creator);
        Ok(Self { canvas, texture, width, height })
    }

    pub fn present(&mut self, fb: &Framebuffer) -> Result<(), String> {
        self.texture
            .update(None, &fb.color, (self.width * 3) as usize)
            .map_err(|e| e.to_string())?;
        self.canvas.clear();
        self.canvas
            .copy(&self.texture, None, None)
            .map_err(|e| e.to_string())?;
        self.canvas.present();
        Ok(())
    }
}
