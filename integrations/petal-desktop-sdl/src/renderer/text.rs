//! TTF text: blitting a rendered run, and the gamma-correcting coverage LUTs
//! that make its antialiased edges match native text weight.

use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, RenderTarget};
use sdl2::surface::Surface;
use sdl2::ttf::Font;

/// Shared text-rendering body. Generic over the `Canvas` target `T` and the
/// `TextureCreator`'s context `C`; the caller supplies a creator matching the
/// canvas so the produced texture is compatible with `canvas.copy`.
pub(super) fn render_text_impl<T, C>(
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
    // Render the glyphs fully opaque, then scale coverage by the requested
    // alpha via the texture's alpha-mod so translucent text composites.
    let opaque = Color::RGB(color.r, color.g, color.b);
    let mut surface = match font.render(text).blended(opaque) {
        Ok(s) => s,
        Err(_) => return,
    };
    correct_coverage(&mut surface, opaque);
    let mut texture = match texture_creator.create_texture_from_surface(&surface) {
        Ok(t) => t,
        Err(_) => return,
    };
    if color.a < 255 {
        texture.set_blend_mode(BlendMode::Blend);
        texture.set_alpha_mod(color.a);
    }
    let query = texture.query();
    let target = Rect::new(x, y, query.width, query.height);
    let _ = canvas.copy(&texture, None, Some(target));
}

/// Number of text-luminance buckets the coverage LUTs are built for. Text
/// color only picks a curve, so a coarse quantization is invisible.
const COVERAGE_BUCKETS: usize = 16;

/// Coverage-correction exponent for the brightest text. FreeType coverage is
/// linear, but SDL blends it in sRGB-encoded space, so light-on-dark edges
/// come out thin and stair-stepped. Raising coverage to `1/GAMMA` restores the
/// weight a linear-light blend (and CoreText's correction) would give. 2.0 was
/// picked by comparing snapshots against CoreText renders of the same strings:
/// it lands close to native ink weight without reading as bold.
const LIGHT_TEXT_GAMMA: f32 = 2.0;

/// Exponent for the darkest text. Dark-on-light already reads close to full
/// weight under the sRGB blend; CoreText is still a touch heavier (its stem
/// darkening), so it gets only a slight lift.
const DARK_TEXT_GAMMA: f32 = 1.2;

/// The 256-entry alpha remap for text whose relative luminance falls in
/// `bucket`. Built once for all buckets and cached.
fn coverage_lut(bucket: usize) -> &'static [u8; 256] {
    static LUTS: std::sync::OnceLock<Vec<[u8; 256]>> = std::sync::OnceLock::new();
    let luts = LUTS.get_or_init(|| {
        (0..COVERAGE_BUCKETS)
            .map(|b| {
                let t = b as f32 / (COVERAGE_BUCKETS - 1) as f32;
                build_coverage_lut(DARK_TEXT_GAMMA + (LIGHT_TEXT_GAMMA - DARK_TEXT_GAMMA) * t)
            })
            .collect()
    });
    &luts[bucket.min(COVERAGE_BUCKETS - 1)]
}

/// `a' = a^(1/gamma)`, rounded, with 0 and 255 fixed so glyph interiors and
/// empty space are untouched.
fn build_coverage_lut(gamma: f32) -> [u8; 256] {
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = (c.powf(1.0 / gamma) * 255.0).round() as u8;
    }
    lut
}

/// Which coverage LUT suits text drawn in `color`: its relative luminance
/// (linearized sRGB, Rec. 709 weights) quantized into `COVERAGE_BUCKETS`.
fn coverage_bucket(color: Color) -> usize {
    fn lin(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    let y = 0.2126 * lin(color.r) + 0.7152 * lin(color.g) + 0.0722 * lin(color.b);
    // Luminance is linear, so take it back toward perceptual lightness (a
    // square root is close enough), then ramp from the dark curve to the full
    // light curve between 0.25 and 0.55. UI text is rarely mid-gray, and the
    // dim grays used for secondary text on dark panels (~#8a93a2, lightness
    // ~0.53) need nearly the full boost to keep up with the primary text.
    let t = ((y.sqrt() - 0.25) / 0.3).clamp(0.0, 1.0);
    (t * (COVERAGE_BUCKETS - 1) as f32).round() as usize
}

/// Remap the alpha channel of a `blended()` text surface through the coverage
/// LUT for `color`. Only the alpha byte changes; color channels are the flat
/// text color SDL_ttf filled in.
fn correct_coverage(surface: &mut Surface, color: Color) {
    let lut = coverage_lut(coverage_bucket(color));
    // blended() yields ARGB8888; anything else is left as-is rather than
    // guessing where alpha lives.
    if surface.pixel_format_enum() != PixelFormatEnum::ARGB8888 {
        return;
    }
    let (w, h) = surface.size();
    let pitch = surface.pitch() as usize;
    // ARGB8888 is a packed native-endian u32, so alpha is the high byte:
    // offset 3 on little-endian, 0 on big-endian.
    let a_off = if cfg!(target_endian = "little") { 3 } else { 0 };
    surface.with_lock_mut(|px| {
        for row in 0..h as usize {
            let start = row * pitch;
            let line = &mut px[start..start + w as usize * 4];
            for p in line.chunks_exact_mut(4) {
                p[a_off] = lut[p[a_off] as usize];
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_lut_keeps_endpoints_and_boosts_light_text() {
        let light = coverage_lut(coverage_bucket(Color::RGB(255, 255, 255)));
        let dark = coverage_lut(coverage_bucket(Color::RGB(0, 0, 0)));
        for lut in [light, dark] {
            assert_eq!(lut[0], 0);
            assert_eq!(lut[255], 255);
            assert!(lut.windows(2).all(|w| w[0] <= w[1]), "LUT must be monotonic");
        }
        // Partial coverage is lifted, and more for light text than dark.
        assert!(light[64] > dark[64]);
        assert!(dark[64] >= 64);
        assert_eq!(light[64], (0.25f32.powf(1.0 / LIGHT_TEXT_GAMMA) * 255.0).round() as u8);
    }

    #[test]
    fn coverage_bucket_ramps_with_text_luminance() {
        assert_eq!(coverage_bucket(Color::RGB(0, 0, 0)), 0);
        assert_eq!(coverage_bucket(Color::RGB(30, 30, 30)), 0);
        assert_eq!(coverage_bucket(Color::RGB(255, 255, 255)), COVERAGE_BUCKETS - 1);
        // The dim secondary-text gray on dark panels gets (nearly) full boost.
        assert!(coverage_bucket(Color::RGB(138, 147, 162)) >= COVERAGE_BUCKETS - 3);
    }

    #[test]
    fn correct_coverage_remaps_only_alpha() {
        let mut s = Surface::new(3, 2, PixelFormatEnum::ARGB8888).unwrap();
        let c = Color::RGB(255, 255, 255);
        s.with_lock_mut(|px| {
            for (i, p) in px.chunks_exact_mut(4).enumerate() {
                p.copy_from_slice(&u32::to_ne_bytes(((i as u32 * 50) << 24) | 0x00ff_ffff));
            }
        });
        correct_coverage(&mut s, c);
        let lut = coverage_lut(coverage_bucket(c));
        let pitch = s.pitch() as usize;
        s.with_lock(|px| {
            for row in 0..2 {
                for col in 0..3 {
                    let at = row * pitch + col * 4;
                    let v = u32::from_ne_bytes(px[at..at + 4].try_into().unwrap());
                    let i = (row * 3 + col) as u32;
                    assert_eq!(v & 0x00ff_ffff, 0x00ff_ffff);
                    assert_eq!(v >> 24, lut[(i * 50) as usize] as u32);
                }
            }
        });
    }
}
