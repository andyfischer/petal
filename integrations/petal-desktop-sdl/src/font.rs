//! A multi-size font ladder.
//!
//! SDL_ttf bakes the pixel size into a `Font` at load time, so a single font
//! can't honor `DrawCommand::Text`'s `size`. Instead we preload a ladder of
//! fonts at a spread of sizes and, per text command, render with the ladder
//! rung nearest the requested size. That preserves typographic hierarchy (a
//! 34px title really is larger than a 14px caption) while keeping glyphs crisp
//! — nearest-rung avoids the blur of scaling a single baked bitmap.

use sdl2::ttf::{Font, Sdl2TtfContext};

/// The default spread of rungs. Dense in the UI range (captions → body →
/// headings) so nearest-match is never far off; sparser toward display sizes.
pub const DEFAULT_LADDER: &[u16] = &[10, 12, 14, 16, 18, 20, 24, 28, 32, 40, 48, 64];

/// System font search paths, tried in order. The first that both exists and
/// loads wins, and every rung is loaded from that same file.
const FONT_PATHS: &[&str] = &[
    // macOS
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
    "/Library/Fonts/Arial.ttf",
    // Linux
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    // Windows
    "C:\\Windows\\Fonts\\arial.ttf",
];

/// A set of fonts loaded from one file at several pixel sizes, sorted
/// ascending by size, with nearest-size lookup.
pub struct FontLadder<'ttf> {
    rungs: Vec<(u16, Font<'ttf, 'static>)>,
}

impl<'ttf> FontLadder<'ttf> {
    /// Load the ladder from the first available system font at each of `sizes`.
    /// Sizes are deduplicated and sorted; a size that fails to load is skipped.
    /// Errs only if no font file loads at all.
    pub fn load_system(ttf: &'ttf Sdl2TtfContext, sizes: &[u16]) -> Result<Self, String> {
        let path = resolve_font_path(ttf)
            .ok_or_else(|| "No system font found. Install a TTF font.".to_string())?;

        let mut sizes: Vec<u16> = sizes.iter().copied().filter(|&s| s > 0).collect();
        sizes.sort_unstable();
        sizes.dedup();

        let mut rungs = Vec::with_capacity(sizes.len());
        for size in sizes {
            if let Ok(font) = ttf.load_font(&path, size) {
                rungs.push((size, font));
            }
        }

        if rungs.is_empty() {
            return Err(format!("Loaded no font sizes from {path}."));
        }
        Ok(FontLadder { rungs })
    }

    /// The rung whose baked size is closest to `size`. On a tie the smaller
    /// rung wins (rungs are sorted ascending, so the first minimum is kept).
    pub fn nearest(&self, size: u16) -> &Font<'ttf, 'static> {
        &self.rung_nearest(size).1
    }

    fn rung_nearest(&self, size: u16) -> &(u16, Font<'ttf, 'static>) {
        self.rungs
            .iter()
            .min_by_key(|(rung, _)| (*rung as i32 - size as i32).abs())
            .expect("FontLadder is never empty")
    }

    /// Per-codepoint advance ratios (glyph advance ÷ font size) for ASCII 0–127,
    /// measured from a representative rung — the table `text_width` sums for
    /// proportional layout. Control codes and glyphs the font lacks get 0.
    /// Measuring at a mid-size rung and normalizing keeps the ratios size-
    /// independent (glyph advance scales linearly with point size).
    pub fn ascii_advance_ratios(&self) -> Vec<f64> {
        let (size, font) = self.rung_nearest(32);
        let size = *size as f64;
        (0u32..128)
            .map(|cp| match char::from_u32(cp) {
                Some(c) if !c.is_control() => font
                    .find_glyph_metrics(c)
                    .map(|m| m.advance as f64 / size)
                    .unwrap_or(0.0),
                _ => 0.0,
            })
            .collect()
    }
}

/// The first system font path that exists and loads at a probe size.
fn resolve_font_path(ttf: &Sdl2TtfContext) -> Option<String> {
    for path in FONT_PATHS {
        if std::path::Path::new(path).exists() && ttf.load_font(path, 16).is_ok() {
            return Some((*path).to_string());
        }
    }
    None
}
