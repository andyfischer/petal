//! Showing a software framebuffer in the window through one texture kept
//! between frames, so a frame uploads only the parts it changed.
//!
//! A host that paints into a persistent `Surface` (the default host, the
//! petal-creative editors) and repaints only the damage
//! [`petal_ui::damage::DamageTracker`] reports would still pay for uploading
//! the whole surface every frame (some 20 MB on a Retina window). A
//! [`ScreenTexture`] uploads the damage rectangles, or nothing.

use petal_ui::damage::{Damage, Rect as DamageRect};
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::render::{Canvas, Texture, TextureCreator};
use sdl2::surface::SurfaceRef;
use sdl2::video::{Window, WindowContext};

/// The window's texture of the framebuffer. See the [module docs](self).
#[derive(Default)]
pub struct ScreenTexture {
    /// The texture, the window it belongs to and its size. Its creator is
    /// leaked (one per window), so the texture can live in the host beside
    /// the canvas it is drawn on.
    texture: Option<(Texture<'static>, u32, (u32, u32))>,
}

impl ScreenTexture {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upload what `damage` covers of `fb` (an RGB888 surface) and present it
    /// in the window. The whole surface is uploaded the first time, after a
    /// resize, and for [`Damage::Full`]; nothing for [`Damage::None`].
    pub fn show(&mut self, canvas: &mut Canvas<Window>, fb: &SurfaceRef, damage: &Damage) -> Result<(), String> {
        let size = (fb.width(), fb.height());
        let window = canvas.window().id();
        let mut full = matches!(damage, Damage::Full);
        if self.texture.as_ref().is_none_or(|(_, w, s)| *w != window || *s != size) {
            let creator: &'static TextureCreator<WindowContext> = Box::leak(Box::new(canvas.texture_creator()));
            let texture = creator
                .create_texture_streaming(PixelFormatEnum::RGB888, size.0.max(1), size.1.max(1))
                .map_err(|e| e.to_string())?;
            self.texture = Some((texture, window, size));
            full = true;
        }
        let texture = &mut self.texture.as_mut().expect("made above").0;
        let pitch = fb.pitch() as usize;
        let screen = DamageRect::new(0, 0, size.0 as i32, size.1 as i32);
        fb.with_lock(|px| -> Result<(), String> {
            if full {
                return texture.update(None, px, pitch).map_err(|e| e.to_string());
            }
            if let Damage::Rects(rects) = damage {
                for r in rects {
                    let r = r.intersect(&screen);
                    if r.is_empty() {
                        continue;
                    }
                    let at = r.y0 as usize * pitch + r.x0 as usize * 4;
                    let to = Rect::new(r.x0, r.y0, r.w() as u32, r.h() as u32);
                    texture.update(Some(to), &px[at..], pitch).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        })?;
        canvas.copy(texture, None, None)?;
        canvas.present();
        Ok(())
    }
}

/// A damage rectangle as SDL's.
pub fn sdl_rect(r: &DamageRect) -> Rect {
    Rect::new(r.x0, r.y0, r.w().max(1) as u32, r.h().max(1) as u32)
}
