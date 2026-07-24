//! Fonts: a multi-size ladder per face, and a small book of the faces this
//! host can offer a script.
//!
//! SDL_ttf bakes the pixel size into a `Font` at load time, so a single font
//! can't honor `DrawCommand::Text`'s `size`. Instead we preload a ladder of
//! fonts at a spread of sizes and, per text command, render with the ladder
//! rung nearest the requested size. That preserves typographic hierarchy (a
//! 34px title really is larger than a 14px caption) while keeping glyphs crisp
//! — nearest-rung avoids the blur of scaling a single baked bitmap.
//!
//! Weight and italic are SDL_ttf's *synthetic* styles (emboldening and
//! shearing the regular outlines), not separately loaded faces — so bold here
//! is a real width change the metrics account for, but not a designer's bold.

use sdl2::ttf::{Font, FontStyle, Sdl2TtfContext};

/// The default spread of rungs. Dense in the UI range (captions → body →
/// headings) so nearest-match is never far off; sparser toward display sizes.
pub const DEFAULT_LADDER: &[u16] = &[10, 12, 14, 16, 18, 20, 24, 28, 32, 40, 48, 64];

/// The role a script's default (`font`-less) text renders in, and the book's
/// fallback for any face it doesn't have.
pub const DEFAULT_ROLE: &str = "ui";

/// Sans-serif system font search paths, tried in order. The first that both
/// exists and loads wins, and every rung is loaded from that same file.
const SANS_PATHS: &[&str] = &[
    // macOS
    "/System/Library/Fonts/Helvetica.ttc",
    "/Library/Fonts/Arial.ttf",
    // Linux
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    // Windows
    "C:\\Windows\\Fonts\\arial.ttf",
];

/// Fixed-pitch system font search paths — the `mono` role. A machine with none
/// of these simply doesn't offer the role, and `mono` degrades to the default
/// face rather than failing.
const MONO_PATHS: &[&str] = &[
    // macOS
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Monaco.ttf",
    // Linux
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    // Windows
    "C:\\Windows\\Fonts\\consola.ttf",
];

/// A set of fonts loaded from one file at several pixel sizes, sorted
/// ascending by size, with nearest-size lookup.
pub struct FontLadder<'ttf> {
    rungs: Vec<(u16, Font<'ttf, 'static>)>,
}

impl<'ttf> FontLadder<'ttf> {
    /// Load the ladder from the first available sans-serif system font at each
    /// of `sizes`. Sizes are deduplicated and sorted; a size that fails to load
    /// is skipped. Errs only if no font file loads at all.
    pub fn load_system(ttf: &'ttf Sdl2TtfContext, sizes: &[u16]) -> Result<Self, String> {
        Self::load_from(ttf, SANS_PATHS, sizes)
    }

    /// Load the ladder from the first path in `paths` that loads.
    fn load_from(
        ttf: &'ttf Sdl2TtfContext,
        paths: &[&str],
        sizes: &[u16],
    ) -> Result<Self, String> {
        let path = resolve_font_path(ttf, paths)
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

    /// The nearest rung, with SDL_ttf's synthetic bold/italic applied for this
    /// weight and slant. Takes `&mut self` because the style is *font* state,
    /// not a per-call argument: the rung is restyled on the way out and stays
    /// that way until the next caller sets it, which is why every text command
    /// goes through here rather than [`nearest`](Self::nearest).
    pub fn nearest_styled(
        &mut self,
        size: u16,
        weight: u16,
        italic: bool,
    ) -> &Font<'ttf, 'static> {
        let style = synthetic_style(weight, italic);
        let index = self.index_nearest(size);
        let font = &mut self.rungs[index].1;
        if font.get_style() != style {
            font.set_style(style);
        }
        font
    }

    fn rung_nearest(&self, size: u16) -> &(u16, Font<'ttf, 'static>) {
        &self.rungs[self.index_nearest(size)]
    }

    fn index_nearest(&self, size: u16) -> usize {
        self.rungs
            .iter()
            .enumerate()
            .min_by_key(|(_, (rung, _))| (*rung as i32 - size as i32).abs())
            .expect("FontLadder is never empty")
            .0
    }

    /// Per-codepoint advance ratios (glyph advance ÷ font size) for ASCII 0–127,
    /// measured from a representative rung — the table `text_width` sums for
    /// proportional layout. Control codes and glyphs the font lacks get 0.
    /// Measuring at a mid-size rung and normalizing keeps the ratios size-
    /// independent (glyph advance scales linearly with point size).
    pub fn ascii_advance_ratios(&self) -> Vec<f64> {
        let (size, font) = self.rung_nearest(32);
        measure_ascii(font, *size)
    }

    /// [`ascii_advance_ratios`](Self::ascii_advance_ratios) for one synthetic
    /// variant. Emboldening widens glyphs, so bold text measured with the
    /// regular table would come out short — this is the table a script's
    /// `text_width(s, {weight: 700, ...})` needs to agree with the pixels.
    ///
    /// The rung is restyled to measure and left that way (the next draw sets
    /// what it needs), which is why this takes `&mut self`.
    pub fn ascii_advance_ratios_styled(&mut self, weight: u16, italic: bool) -> Vec<f64> {
        let index = self.index_nearest(32);
        let (size, font) = &mut self.rungs[index];
        let size = *size;
        font.set_style(synthetic_style(weight, italic));
        measure_ascii(font, size)
    }
}

/// SDL_ttf's synthetic style for a CSS weight and slant. Anything at or above
/// semibold embolden; lighter-than-regular has no synthetic equivalent, so it
/// renders regular.
fn synthetic_style(weight: u16, italic: bool) -> FontStyle {
    let mut style = FontStyle::NORMAL;
    if weight >= 600 {
        style |= FontStyle::BOLD;
    }
    if italic {
        style |= FontStyle::ITALIC;
    }
    style
}

/// Per-codepoint advance ratios for ASCII 0–127 of `font`, baked at `size`.
fn measure_ascii(font: &Font, size: u16) -> Vec<f64> {
    let size = size as f64;
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

/// The faces this host offers a script, by role name. `ui` always exists (it is
/// the default font); `mono` exists when the machine has a fixed-pitch face.
/// A role the book lacks resolves to the default, so a script that asks for
/// something this machine hasn't got degrades instead of failing.
pub struct FontBook<'ttf> {
    faces: Vec<(&'static str, FontLadder<'ttf>)>,
}

impl<'ttf> FontBook<'ttf> {
    /// Load every role this machine can supply. Errs only when even the
    /// default sans face is missing — the state where no text can render.
    pub fn load_system(ttf: &'ttf Sdl2TtfContext, sizes: &[u16]) -> Result<Self, String> {
        let mut faces = vec![(DEFAULT_ROLE, FontLadder::load_from(ttf, SANS_PATHS, sizes)?)];
        if let Ok(mono) = FontLadder::load_from(ttf, MONO_PATHS, sizes) {
            faces.push(("mono", mono));
        }
        Ok(FontBook { faces })
    }

    /// The face a `font` spec selects: the first role in a CSS-style fallback
    /// list (`"Inter, mono"`) this book has, or the default face.
    pub fn resolve(&mut self, spec: Option<&str>) -> &mut FontLadder<'ttf> {
        let index = spec
            .and_then(|spec| {
                spec.split(',')
                    .find_map(|name| self.faces.iter().position(|(role, _)| *role == name.trim()))
            })
            .unwrap_or(0);
        &mut self.faces[index].1
    }

    /// Whether this machine supplied a given role. Only `ui` is guaranteed;
    /// `mono` depends on the fixed-pitch fonts installed.
    pub fn has_role(&self, role: &str) -> bool {
        self.faces.iter().any(|(name, _)| *name == role)
    }

    /// The default face — what `font`-less text renders in, and what the
    /// default-font metric bindings describe.
    pub fn default_face(&self) -> &FontLadder<'ttf> {
        &self.faces[0].1
    }

    /// Every role this book offers, for binding measurement data.
    pub fn roles(&mut self) -> impl Iterator<Item = (&'static str, &mut FontLadder<'ttf>)> {
        self.faces.iter_mut().map(|(role, ladder)| (*role, ladder))
    }
}

/// The first font path in `paths` that exists and loads at a probe size.
fn resolve_font_path(ttf: &Sdl2TtfContext, paths: &[&str]) -> Option<String> {
    for path in paths {
        if std::path::Path::new(path).exists() && ttf.load_font(path, 16).is_ok() {
            return Some((*path).to_string());
        }
    }
    None
}
