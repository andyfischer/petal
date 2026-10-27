//! Surface effects behind the offscreen-canvas commands: compositing a canvas
//! onto a target, snapshotting a target's pixels, and blurring a surface.

use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, RenderTarget};
use sdl2::surface::Surface;

/// Shared offscreen-canvas blit body. Uploads `src` as an alpha-blended texture
/// and copies it onto `canvas` at (`x`, `y`). Generic over the canvas target and
/// the texture-creator context so it serves both the window and software paths.
#[allow(clippy::too_many_arguments)]
pub(super) fn blit_surface_impl<T, C>(
    canvas: &mut Canvas<T>,
    texture_creator: &sdl2::render::TextureCreator<C>,
    src: &Surface,
    x: i32,
    y: i32,
    a: u8,
    w: u32,
    h: u32,
) where
    T: RenderTarget,
{
    let mut texture = match texture_creator.create_texture_from_surface(src) {
        Ok(t) => t,
        Err(_) => return,
    };
    // Composite: transparent regions of the offscreen canvas let the destination
    // show through, only the painted pixels land. The alpha mod scales the
    // whole layer's opacity at once — a group of overlapping shapes reads as
    // one object at `a`, which is what `draw_canvas(c, x, y, a)` promises.
    texture.set_blend_mode(BlendMode::Blend);
    texture.set_alpha_mod(a);
    let (sw, sh) = src.size();
    let w = if w == 0 { sw } else { w };
    let h = if h == 0 { sh } else { h };
    let target = Rect::new(x, y, w.max(1), h.max(1));
    let _ = canvas.copy(&texture, None, Some(target));
}

/// Copy the pixels of `canvas` under the `w`×`h` rect at (`x`, `y`) into a
/// fresh ARGB surface of that size — the `snapshot` primitive. Pixels outside
/// the target are left transparent.
pub(super) fn snapshot_surface<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) -> Option<Surface<'static>> {
    let (tw, th) = canvas.output_size().ok()?;
    let mut out = Surface::new(w.max(1), h.max(1), PixelFormatEnum::ARGB8888).ok()?;
    let _ = out.fill_rect(None, Color::RGBA(0, 0, 0, 0));
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(tw as i32);
    let y1 = (y + h as i32).min(th as i32);
    if x1 <= x0 || y1 <= y0 {
        return Some(out);
    }
    let region = Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32);
    let mut pixels = canvas
        .read_pixels(Some(region), PixelFormatEnum::ARGB8888)
        .ok()?;
    let pitch = region.width() * 4;
    let src = Surface::from_data(
        &mut pixels,
        region.width(),
        region.height(),
        pitch,
        PixelFormatEnum::ARGB8888,
    )
    .ok()?;
    // A straight copy (no blending): the snapshot *is* the backdrop, opaque
    // where the target was.
    let dest = Rect::new(x0 - x, y0 - y, region.width(), region.height());
    let mut src = src;
    src.set_blend_mode(BlendMode::None).ok()?;
    src.blit(None, &mut out, Some(dest)).ok()?;
    Some(out)
}

/// Gaussian-blur an ARGB surface in place with standard deviation `sigma`
/// px, approximated by three box blurs (the classic approximation, within a
/// few percent of the true kernel). Runs in premultiplied space so translucent
/// edges do not darken, and clamps at the borders so a backdrop at the edge of
/// the window smears its edge pixels outward rather than fading to nothing.
pub(super) fn blur_surface(surface: &mut Surface, sigma: f32) {
    if sigma < 0.25 {
        return;
    }
    let (w, h) = surface.size();
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 {
        return;
    }
    // Box widths for three passes approximating a Gaussian of `sigma`
    // (Kovesi's method, the common integer form).
    let ideal = ((12.0 * sigma * sigma / 3.0 + 1.0).sqrt()).floor() as usize;
    let radius = (ideal.max(1) - 1) / 2;
    if radius == 0 {
        return;
    }
    let pitch = surface.pitch() as usize;
    surface.with_lock_mut(|bytes| {
        // Premultiply into a float working buffer.
        let mut buf: Vec<[f32; 4]> = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let i = y * pitch + x * 4;
                let (b, g, r, a) = (bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]);
                let a = a as f32 / 255.0;
                buf.push([r as f32 * a, g as f32 * a, b as f32 * a, a * 255.0]);
            }
        }
        let mut tmp = vec![[0.0f32; 4]; w * h];
        for _ in 0..3 {
            // Rows, then columns.
            box_blur_1d(&buf, &mut tmp, h, w, radius, |y, x| y * w + x);
            box_blur_1d(&tmp, &mut buf, w, h, radius, |x, y| y * w + x);
        }
        // Un-premultiply back into the surface.
        for y in 0..h {
            for x in 0..w {
                let p = buf[y * w + x];
                let a = p[3] / 255.0;
                let i = y * pitch + x * 4;
                let un = |v: f32| {
                    if a > 0.0001 {
                        (v / a).round().clamp(0.0, 255.0) as u8
                    } else {
                        0
                    }
                };
                bytes[i] = un(p[2]);
                bytes[i + 1] = un(p[1]);
                bytes[i + 2] = un(p[0]);
                bytes[i + 3] = p[3].round().clamp(0.0, 255.0) as u8;
            }
        }
    });
}

/// One box-blur pass along one axis (`radius` each side, edges clamped) from
/// `src` into `dst`. The buffer is `lines` runs of `len` pixels each, and
/// `index(line, i)` is where pixel `i` of `line` lives — rows for the
/// horizontal pass, columns for the vertical one.
fn box_blur_1d(
    src: &[[f32; 4]],
    dst: &mut [[f32; 4]],
    lines: usize,
    len: usize,
    radius: usize,
    index: impl Fn(usize, usize) -> usize,
) {
    let norm = 1.0 / (2 * radius + 1) as f32;
    for line in 0..lines {
        let at = |i: isize| src[index(line, i.clamp(0, len as isize - 1) as usize)];
        let mut acc = [0.0f32; 4];
        for i in -(radius as isize)..=(radius as isize) {
            let p = at(i);
            for c in 0..4 {
                acc[c] += p[c];
            }
        }
        for i in 0..len {
            let out = &mut dst[index(line, i)];
            for c in 0..4 {
                out[c] = acc[c] * norm;
            }
            let leaving = at(i as isize - radius as isize);
            let entering = at(i as isize + radius as isize + 1);
            for c in 0..4 {
                acc[c] += entering[c] - leaving[c];
            }
        }
    }
}
