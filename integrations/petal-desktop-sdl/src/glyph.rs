//! Text without SDL_ttf: font files read with [skrifa], glyph outlines filled
//! into a coverage mask with [tiny_skia]. All pure Rust, so a host built on
//! this crate links no FreeType and no SDL_ttf.
//!
//! A [`Font`] is one face of one file at one pixel size, as an SDL_ttf `Font`
//! was, and it keeps the contract the rest of the crate was built on:
//!
//! - **Advances are linear** (the font's own advance widths scaled to the
//!   size, never rounded), so a run is exactly as wide as the sum of its
//!   glyphs' advances: what `text_width` measures. No kerning, for the same
//!   reason.
//! - **Hinting is light**: FreeType's `FT_LOAD_TARGET_LIGHT` through skrifa's
//!   automatic hinter, which snaps outlines vertically only and leaves the
//!   horizontal metrics alone.
//! - **A rendered run is `height()` tall with its baseline `ascent()` down**
//!   from the top, so a text command's `(x, y)` is the run's top-left.
//! - **Synthetic styles** cover what a cut doesn't draw itself: bold widens
//!   each outline (and its advance) by a size-relative stroke, italic shears
//!   it by SDL_ttf's 0.207.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::{BitOr, BitOrAssign};
use std::sync::Arc;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{
    DrawSettings, Engine, GlyphStyles, HintingInstance, HintingOptions, OutlinePen, SmoothMode,
    Target,
};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{FillRule, Mask, PathBuilder, Stroke, Transform};

/// Synthetic styles drawn on top of a cut: SDL_ttf's `TTF_STYLE_BOLD` and
/// `TTF_STYLE_ITALIC`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FontStyle(u8);

impl FontStyle {
    pub const NORMAL: FontStyle = FontStyle(0);
    pub const BOLD: FontStyle = FontStyle(1);
    pub const ITALIC: FontStyle = FontStyle(2);

    pub fn contains(self, other: FontStyle) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for FontStyle {
    type Output = FontStyle;
    fn bitor(self, rhs: FontStyle) -> FontStyle {
        FontStyle(self.0 | rhs.0)
    }
}

impl BitOrAssign for FontStyle {
    fn bitor_assign(&mut self, rhs: FontStyle) {
        self.0 |= rhs.0;
    }
}

/// How far synthetic italic shears: x moves right by this much per pixel
/// above the baseline (tan 11.7°, SDL_ttf's and FreeType's value).
const ITALIC_SHEAR: f32 = 0.207;

/// Synthetic bold's stroke, as a fraction of the size: each outline grows by
/// half of it on every side, and each advance by all of it.
const BOLD_STRENGTH: f32 = 1.0 / 24.0;

/// Rendered runs a [`Font`] keeps before it starts over. A UI redraws the same
/// labels every frame; a script that animates its text bakes new runs, and
/// this bounds what they cost.
const MAX_CACHED_RUNS: usize = 2048;

/// One face of a font file, read once and shared by every size of it.
pub struct FontData {
    bytes: Arc<[u8]>,
    index: u32,
    /// The automatic hinter's per-font analysis, done once rather than per size.
    styles: GlyphStyles,
}

impl FontData {
    /// Face `index` of the file at `path` (a `.ttc` collection holds several),
    /// or `None` when the file can't be read or isn't a font skrifa can draw.
    pub fn load(path: &str, index: u32) -> Option<Arc<FontData>> {
        let bytes: Arc<[u8]> = std::fs::read(path).ok()?.into();
        let font = FontRef::from_index(&bytes, index).ok()?;
        let outlines = font.outline_glyphs();
        // A file with no outlines (a bitmap-only emoji font) draws nothing.
        outlines.get(font.charmap().map('a').or(font.charmap().map('A'))?)?;
        let styles = GlyphStyles::new(&outlines);
        Some(Arc::new(FontData {
            bytes,
            index,
            styles,
        }))
    }

    fn font(&self) -> FontRef<'_> {
        FontRef::from_index(&self.bytes, self.index).expect("checked when loaded")
    }
}

/// One run rendered as coverage: `width × height` bytes, row by row. Its
/// left edge sits `left` pixels from the pen's start (negative when the first
/// glyph's ink reaches back past it, as a 'j' or an italic does).
pub struct Run {
    pub left: i32,
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}

/// One glyph's extents at a font's size, in whole pixels: what SDL_ttf's
/// `GlyphMetrics` reported.
#[derive(Clone, Copy, Debug)]
pub struct GlyphMetrics {
    /// The advance, rounded. [`Font::advance`] is the exact one.
    pub advance: i32,
    /// How far the ink reaches above the baseline.
    pub maxy: i32,
}

/// A face at one pixel size, with the synthetic style set on it.
pub struct Font {
    data: Arc<FontData>,
    size: u16,
    style: FontStyle,
    hinting: Option<HintingInstance>,
    ascent: i32,
    descent: i32,
    height: i32,
    line_skip: i32,
    runs: RefCell<HashMap<(FontStyle, String), Arc<Run>>>,
}

impl Font {
    /// `data` at `size` pixels per em, or `None` for a size of 0.
    pub fn new(data: Arc<FontData>, size: u16) -> Option<Font> {
        if size == 0 {
            return None;
        }
        let font = data.font();
        let px = Size::new(size as f32);
        let metrics = font.metrics(px, LocationRef::default());
        // As SDL_ttf (and FreeType's FT_CEIL) round them: the ascent up, the
        // (negative) descent up toward the baseline, the height of the two
        // together up.
        let ascent = metrics.ascent.ceil() as i32;
        let descent = metrics.descent.ceil() as i32;
        let height = (metrics.ascent - metrics.descent).ceil() as i32;
        let line_skip = (metrics.ascent - metrics.descent + metrics.leading).ceil() as i32;
        let options = HintingOptions {
            engine: Engine::Auto(Some(data.styles.clone())),
            target: Target::Smooth {
                mode: SmoothMode::Light,
                symmetric_rendering: true,
                preserve_linear_metrics: true,
            },
        };
        let hinting =
            HintingInstance::new(&font.outline_glyphs(), px, LocationRef::default(), options).ok();
        Some(Font {
            data,
            size,
            style: FontStyle::NORMAL,
            hinting,
            ascent,
            descent,
            height,
            line_skip,
            runs: RefCell::new(HashMap::new()),
        })
    }

    pub fn size(&self) -> u16 {
        self.size
    }

    /// Pixels from the top of a rendered run to its baseline.
    pub fn ascent(&self) -> i32 {
        self.ascent
    }

    /// Pixels from the baseline down to the bottom of the line, negative (as
    /// SDL_ttf's was).
    pub fn descent(&self) -> i32 {
        self.descent
    }

    /// A rendered run's height.
    pub fn height(&self) -> i32 {
        self.height
    }

    /// The font's recommended distance from one baseline to the next.
    pub fn recommended_line_spacing(&self) -> i32 {
        self.line_skip
    }

    pub fn get_style(&self) -> FontStyle {
        self.style
    }

    pub fn set_style(&mut self, style: FontStyle) {
        self.style = style;
    }

    fn glyph(&self, c: char) -> Option<GlyphId> {
        self.data
            .font()
            .charmap()
            .map(c)
            .filter(|g| g.to_u32() != 0)
    }

    /// Whether the face has a glyph for `c`.
    pub fn has_glyph(&self, c: char) -> bool {
        self.glyph(c).is_some()
    }

    /// The width synthetic bold adds to every glyph, in pixels.
    fn bold_extra(&self) -> f32 {
        if self.style.contains(FontStyle::BOLD) {
            self.size as f32 * BOLD_STRENGTH
        } else {
            0.0
        }
    }

    /// `c`'s exact (unrounded) advance in pixels, or `None` when the face has
    /// no glyph for it.
    pub fn advance(&self, c: char) -> Option<f32> {
        let glyph = self.glyph(c)?;
        let font = self.data.font();
        let advance = font
            .glyph_metrics(Size::new(self.size as f32), LocationRef::default())
            .advance_width(glyph)?;
        Some(advance + self.bold_extra())
    }

    /// `c`'s rounded advance and ink top, or `None` when the face lacks it.
    pub fn find_glyph_metrics(&self, c: char) -> Option<GlyphMetrics> {
        let glyph = self.glyph(c)?;
        let font = self.data.font();
        let bounds = font
            .glyph_metrics(Size::new(self.size as f32), LocationRef::default())
            .bounds(glyph);
        let top = bounds.map_or(0.0, |b| b.y_max) + self.bold_extra() / 2.0;
        Some(GlyphMetrics {
            advance: self.advance(c)?.round() as i32,
            maxy: top.ceil() as i32,
        })
    }

    /// The width a run of `text` advances the pen by: the sum of its glyphs'
    /// advances (a glyph the face lacks advances by nothing).
    pub fn run_advance(&self, text: &str) -> f32 {
        text.chars().filter_map(|c| self.advance(c)).sum()
    }

    /// `text` rendered as coverage, with this font's current style: cached,
    /// since a UI draws the same labels frame after frame. `None` when
    /// nothing in it has ink (spaces, or glyphs the face lacks).
    pub fn render(&self, text: &str) -> Option<Arc<Run>> {
        let key = (self.style, text.to_string());
        if let Some(run) = self.runs.borrow().get(&key) {
            return Some(run.clone());
        }
        let run = Arc::new(self.rasterize(text)?);
        let mut runs = self.runs.borrow_mut();
        if runs.len() >= MAX_CACHED_RUNS {
            runs.clear();
        }
        runs.insert(key, run.clone());
        Some(run)
    }

    fn rasterize(&self, text: &str) -> Option<Run> {
        let font = self.data.font();
        let outlines = font.outline_glyphs();
        let px = Size::new(self.size as f32);
        let metrics = font.glyph_metrics(px, LocationRef::default());
        let charmap = font.charmap();
        let shear = if self.style.contains(FontStyle::ITALIC) {
            ITALIC_SHEAR
        } else {
            0.0
        };
        let bold = self.bold_extra();

        // Every glyph's outline, in pixels, with the baseline at y = 0 and
        // y growing downward (the mask's way up), each placed at its pen x.
        let mut pen = GlyphPen {
            path: PathBuilder::new(),
            x: 0.0,
            shear,
        };
        for c in text.chars() {
            let Some(glyph) = charmap.map(c).filter(|g| g.to_u32() != 0) else {
                continue;
            };
            if let Some(outline) = outlines.get(glyph) {
                let drawn = match &self.hinting {
                    Some(hinting) => outline.draw(DrawSettings::hinted(hinting, false), &mut pen),
                    None => outline.draw(DrawSettings::unhinted(px, LocationRef::default()), &mut pen),
                };
                if drawn.is_err() {
                    continue;
                }
            }
            pen.x += metrics.advance_width(glyph).unwrap_or(0.0) + bold;
        }
        let end = pen.x;
        let path = pen.path.finish()?;
        let outset = if bold > 0.0 {
            Some(path.stroke(
                &Stroke {
                    width: bold,
                    ..Stroke::default()
                },
                1.0,
            )?)
        } else {
            None
        };

        let bounds = path.bounds();
        let grow = bold / 2.0;
        let left = ((bounds.left() - grow).floor() as i32).min(0);
        let right = ((bounds.right() + grow).ceil() as i32).max(end.ceil() as i32);
        let width = (right - left).max(1) as u32;
        let height = self.height.max(1) as u32;
        let mut mask = Mask::new(width, height)?;
        let place = Transform::from_translate(-left as f32, self.ascent as f32);
        mask.fill_path(&path, FillRule::Winding, true, place);
        if let Some(outset) = &outset {
            mask.fill_path(outset, FillRule::Winding, true, place);
        }
        Some(Run {
            left,
            width,
            height,
            coverage: mask.take(),
        })
    }
}

/// Collects glyph outlines (font units scaled to pixels, y up) into one
/// tiny-skia path in run coordinates (y down), offset by the pen and sheared
/// for synthetic italic.
struct GlyphPen {
    path: PathBuilder,
    x: f32,
    shear: f32,
}

impl GlyphPen {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x + y * self.shear, -y)
    }
}

impl OutlinePen for GlyphPen {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.path.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.path.line_to(x, y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (cx0, cy0) = self.at(cx0, cy0);
        let (x, y) = self.at(x, y);
        self.path.quad_to(cx0, cy0, x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (cx0, cy0) = self.at(cx0, cy0);
        let (cx1, cy1) = self.at(cx1, cy1);
        let (x, y) = self.at(x, y);
        self.path.cubic_to(cx0, cy0, cx1, cy1, x, y);
    }

    fn close(&mut self) {
        self.path.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_font() -> Option<Arc<FontData>> {
        [
            "/System/Library/Fonts/Helvetica.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ]
        .iter()
        .find_map(|p| FontData::load(p, 0))
    }

    #[test]
    fn a_run_is_as_wide_as_its_advances_and_as_tall_as_the_font() {
        let Some(data) = system_font() else { return };
        let font = Font::new(data, 24).unwrap();
        let run = font.render("Hello").expect("ink");
        assert_eq!(run.height as i32, font.height());
        let advance = font.run_advance("Hello");
        assert!((run.width as f32 - advance).abs() <= 2.0, "{} vs {advance}", run.width);
        assert!(run.coverage.iter().any(|&c| c == 255), "solid stems");
        assert!(font.render("   ").is_none(), "spaces have no ink");
    }

    #[test]
    fn synthetic_bold_widens_and_italic_leans() {
        let Some(data) = system_font() else { return };
        let mut font = Font::new(data, 24).unwrap();
        let regular = font.advance('n').unwrap();
        let ink = |f: &Font| f.render("l").map_or(0, |r| r.coverage.iter().map(|&c| c as u32).sum::<u32>());
        let regular_ink = ink(&font);
        font.set_style(FontStyle::BOLD);
        assert!(font.advance('n').unwrap() > regular);
        assert!(ink(&font) > regular_ink, "bold has more ink");
        font.set_style(FontStyle::ITALIC);
        assert_eq!(font.advance('n').unwrap(), regular, "italic doesn't change advances");
        assert!(font.render("l").unwrap().width > Font::new(system_font().unwrap(), 24).unwrap().render("l").unwrap().width);
    }

    #[test]
    fn a_missing_glyph_is_reported() {
        let Some(data) = system_font() else { return };
        let font = Font::new(data, 16).unwrap();
        assert!(font.has_glyph('A'));
        assert!(!font.has_glyph('\u{10FFFD}'));
        assert!(font.advance('\u{10FFFD}').is_none());
    }
}
