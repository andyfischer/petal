//! Fonts: a per-size ladder per face, and a book of the faces this host can
//! offer a script — its roles plus any family installed on the machine.
//!
//! SDL_ttf bakes the pixel size into a `Font` at load time, so a single font
//! can't honor `DrawCommand::Text`'s `size`. Each face is therefore a ladder of
//! fonts, one per pixel size, and a text command renders with the rung baked
//! at **exactly** its size: a size the ladder hasn't seen is loaded from the
//! face's file the first time it is asked for and kept. Drawing at the exact
//! size is what keeps `text_width` (advance ratios × the requested size) in
//! step with the pixels — snapping 13 to a 12px rung drew runs ~8% narrower
//! than they measured. Only past [`MAX_RUNG_SIZE`] or [`MAX_RUNGS`] does a
//! request fall back to the nearest loaded rung. For the same agreement every
//! rung has kerning off: measurement sums one advance per glyph, so a kerned
//! pair would draw narrower than `text_width` says.
//!
//! Weight and italic are SDL_ttf's *synthetic* styles (emboldening and
//! shearing the regular outlines), not separately loaded faces — so bold here
//! is a real width change the metrics account for, but not a designer's bold.

use std::collections::{HashMap, HashSet};

use sdl2::ttf::{Font, FontStyle, Sdl2TtfContext};

/// Sizes preloaded for the built-in roles. Any other size is baked on demand,
/// so this is only a warm start for the sizes UI text uses most.
pub const DEFAULT_LADDER: &[u16] = &[10, 12, 14, 16, 18, 20, 24, 28, 32, 40, 48, 64];

/// The rung every face's size-independent measurement tables (advance and
/// vertical ratios) are taken from. Always loaded.
pub const MEASURE_SIZE: u16 = 32;

/// The largest size baked on demand. Bigger requests draw with the nearest
/// rung loaded (a 4000px "size" is a script bug, not a glyph cache to build).
pub const MAX_RUNG_SIZE: u16 = 1024;

/// How many distinct sizes one face keeps. A script that animates its text
/// size bakes each integer size once; past this the ladder stops growing and
/// new sizes snap to the nearest rung, so memory stays bounded.
pub const MAX_RUNGS: usize = 128;

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

/// One face (a file, and a face index inside it for `.ttc` collections) baked
/// at the sizes asked of it so far, sorted ascending by size.
pub struct FontLadder<'ttf> {
    ttf: &'ttf Sdl2TtfContext,
    path: String,
    index: u32,
    rungs: Vec<(u16, Font<'ttf, 'static>)>,
}

impl<'ttf> FontLadder<'ttf> {
    /// Load the ladder from the first available sans-serif system font,
    /// preloading `sizes`. Errs only if no font file loads at all.
    pub fn load_system(ttf: &'ttf Sdl2TtfContext, sizes: &[u16]) -> Result<Self, String> {
        Self::load_from(ttf, SANS_PATHS, sizes)
    }

    /// Load the ladder from the first path in `paths` that loads.
    fn load_from(ttf: &'ttf Sdl2TtfContext, paths: &[&str], sizes: &[u16]) -> Result<Self, String> {
        let path = resolve_font_path(ttf, paths)
            .ok_or_else(|| "No system font found. Install a TTF font.".to_string())?;
        Self::load_face(ttf, &path, 0, sizes)
    }

    /// Load face `index` of the font file at `path`, preloading `sizes` and
    /// [`MEASURE_SIZE`]. Errs if the measuring rung will not load.
    pub fn load_face(
        ttf: &'ttf Sdl2TtfContext,
        path: &str,
        index: u32,
        sizes: &[u16],
    ) -> Result<Self, String> {
        let mut ladder = FontLadder {
            ttf,
            path: path.to_string(),
            index,
            rungs: Vec::new(),
        };
        if ladder.bake(MEASURE_SIZE).is_none() {
            return Err(format!("Could not load a font from {path}."));
        }
        for &size in sizes {
            ladder.bake(size);
        }
        Ok(ladder)
    }

    /// The sizes baked so far, ascending.
    pub fn sizes(&self) -> Vec<u16> {
        self.rungs.iter().map(|(size, _)| *size).collect()
    }

    /// The rung baked at `size`, loading it now if this is the first time the
    /// size is asked for. `None` when the size is out of range, the ladder is
    /// full, or the file refuses to load at that size.
    fn bake(&mut self, size: u16) -> Option<usize> {
        match self.rungs.binary_search_by_key(&size, |(rung, _)| *rung) {
            Ok(at) => Some(at),
            Err(at) => {
                if size == 0 || size > MAX_RUNG_SIZE || self.rungs.len() >= MAX_RUNGS {
                    return None;
                }
                let mut font = self
                    .ttf
                    .load_font_at_index(&self.path, self.index, size)
                    .ok()?;
                // Light hinting snaps outlines vertically only, which lets
                // SDL_ttf lay a run out at subpixel advances. Those scale
                // linearly with size, so one ratio table measures every size
                // to within a pixel or two per line; full hinting rounds each
                // glyph's advance at the drawn size and drifts ~3% either way.
                font.set_hinting(sdl2::ttf::Hinting::Light);
                // Kerning off, for the same reason: `text_width` (and every
                // ellipsize, wrap and flow decision built on it) sums one
                // advance per glyph, with no notion of pairs. A kerned run
                // drew "AVAVA" in Helvetica ~10% narrower than it measured;
                // unkerned, the pixels are exactly what the table predicts.
                font.set_kerning(false);
                self.rungs.insert(at, (size, font));
                Some(at)
            }
        }
    }

    /// The rung for `size`: baked at exactly that size when it can be, else the
    /// nearest one loaded.
    fn index_for(&mut self, size: u16) -> usize {
        self.bake(size).unwrap_or_else(|| self.index_nearest(size))
    }

    /// The font baked at `size` (see [`index_for`](Self::index_for)), with
    /// SDL_ttf's synthetic bold/italic applied for this weight and slant. Takes
    /// `&mut self` because a size may be baked on the way, and because the
    /// style is *font* state, not a per-call argument: the rung is restyled on
    /// the way out and stays that way until the next caller sets it, which is
    /// why every text command goes through here.
    pub fn sized_styled(&mut self, size: u16, weight: u16, italic: bool) -> &Font<'ttf, 'static> {
        let style = synthetic_style(weight, italic);
        let index = self.index_for(size);
        let font = &mut self.rungs[index].1;
        if font.get_style() != style {
            font.set_style(style);
        }
        font
    }

    /// The loaded rung closest to `size`. On a tie the smaller rung wins
    /// (rungs are sorted ascending, so the first minimum is kept).
    fn index_nearest(&self, size: u16) -> usize {
        self.rungs
            .iter()
            .enumerate()
            .min_by_key(|(_, (rung, _))| (*rung as i32 - size as i32).abs())
            .expect("FontLadder is never empty")
            .0
    }

    /// Per-codepoint advance ratios (glyph advance ÷ font size) for ASCII 0–127,
    /// measured from the [`MEASURE_SIZE`] rung — the table `text_width` sums for
    /// proportional layout. Control codes and glyphs the font lacks get 0.
    /// Measuring at a mid-size rung and normalizing keeps the ratios size-
    /// independent (glyph advance scales linearly with point size).
    pub fn ascii_advance_ratios(&mut self) -> Vec<f64> {
        self.ascii_advance_ratios_styled(petal_ui::draw::REGULAR_WEIGHT, false)
    }

    /// Where this face's ink sits relative to the `y` a text command names —
    /// the vertical half of what a script needs to place a run, and the reason
    /// centred labels used to land high.
    ///
    /// This host blits SDL_ttf's rendered surface with its **top-left** at
    /// `(x, y)`, and that surface is `Font::height()` tall with the baseline
    /// `Font::ascent()` down from its top. So `y → baseline` is the ascent,
    /// full stop — no guessing about em boxes. Cap and x heights come from the
    /// rasterizer's own extents for "H" and "x", which is what a UI label
    /// wants to be centred on.
    ///
    /// Measured at the [`MEASURE_SIZE`] rung and normalized by its size,
    /// exactly as the advance table is, so one record serves every size.
    pub fn vertical_ratios(&mut self) -> petal_ui::draw::VerticalMetrics {
        self.vertical_ratios_styled(petal_ui::draw::REGULAR_WEIGHT, false)
    }

    /// [`vertical_ratios`](Self::vertical_ratios) for one synthetic variant.
    /// Emboldening does not move the baseline, but it does change the extents
    /// slightly, and italic shears them — measuring the variant keeps a bold
    /// label centred on the same line as its regular neighbour.
    pub fn vertical_ratios_styled(
        &mut self,
        weight: u16,
        italic: bool,
    ) -> petal_ui::draw::VerticalMetrics {
        let font = self.sized_styled(MEASURE_SIZE, weight, italic);
        measure_vertical(font, MEASURE_SIZE)
    }

    /// [`ascii_advance_ratios`](Self::ascii_advance_ratios) for one synthetic
    /// variant. Emboldening widens glyphs, so bold text measured with the
    /// regular table would come out short — this is the table a script's
    /// `text_width(s, {weight: 700, ...})` needs to agree with the pixels.
    ///
    /// The rung is restyled to measure and left that way (the next draw sets
    /// what it needs), which is why this takes `&mut self`.
    pub fn ascii_advance_ratios_styled(&mut self, weight: u16, italic: bool) -> Vec<f64> {
        let index = self.index_for(MEASURE_SIZE);
        let font = &mut self.rungs[index].1;
        font.set_style(synthetic_style(weight, italic));
        measure_ascii(font, MEASURE_SIZE)
    }

    /// One glyph's advance ÷ size in this face and variant, or `None` when the
    /// face has no glyph for it. What `text_width` asks for a non-ASCII
    /// character the advance table doesn't cover.
    pub fn glyph_advance_ratio(&mut self, weight: u16, italic: bool, ch: char) -> Option<f64> {
        let index = self.index_for(MEASURE_SIZE);
        let font = &mut self.rungs[index].1;
        font.set_style(synthetic_style(weight, italic));
        advance_ratio(font, MEASURE_SIZE, ch)
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
fn measure_ascii(font: &mut Font, size: u16) -> Vec<f64> {
    (0u32..128)
        .map(|cp| match char::from_u32(cp) {
            Some(c) if !c.is_control() => advance_ratio(font, size, c).unwrap_or(0.0),
            _ => 0.0,
        })
        .collect()
}

/// How many copies of a glyph [`advance_ratio`] lays out to measure it.
const ADVANCE_SAMPLE: usize = 16;

/// One glyph's advance ÷ size as SDL_ttf actually lays a run out, or `None`
/// when `font` has no glyph for it. `GlyphMetrics::advance` is a whole pixel
/// (rounded at this size), so it is off by up to half a pixel per glyph —
/// which, multiplied back up by a script's size, is the drift between a
/// measured and a drawn line. Laying out a run of the glyph and dividing
/// recovers the fractional advance the renderer really uses. Every rung has
/// kerning off (see [`FontLadder::bake`]), so the sample is unkerned too.
fn advance_ratio(font: &mut Font, size: u16, c: char) -> Option<f64> {
    font.find_glyph(c)?;
    let run: String = std::iter::repeat_n(c, ADVANCE_SAMPLE).collect();
    let (width, _) = font.size_of(&run).ok()?;
    Some(width as f64 / (ADVANCE_SAMPLE as f64 * size as f64))
}

/// The vertical metrics of `font`, baked at `size`, as ratios of the size.
///
/// `descent()` is negative in SDL_ttf (it measures downward from the baseline
/// as a signed offset), so it is negated here into the positive "how far below
/// the baseline" every other part of this contract means.
fn measure_vertical(font: &Font, size: u16) -> petal_ui::draw::VerticalMetrics {
    let size = size as f64;
    // The extent of a flat capital and of a lowercase x above the baseline.
    // A face missing either glyph falls back to a proportion of the ascent
    // rather than to zero, which would collapse every centred label onto the
    // baseline.
    let ascent = font.ascent() as f64 / size;
    let extent = |c: char, fallback: f64| {
        font.find_glyph_metrics(c)
            .map(|m| m.maxy as f64 / size)
            .filter(|h| *h > 0.0)
            .unwrap_or(ascent * fallback)
    };
    petal_ui::draw::VerticalMetrics {
        baseline: ascent,
        descent: -(font.descent() as f64) / size,
        line_height: font.recommended_line_spacing() as f64 / size,
        cap_height: extent('H', 0.92),
        x_height: extent('x', 0.68),
    }
}

/// The faces this host offers a script. The roles come first: `ui` always
/// exists (it is the default font); `mono` exists when the machine has a
/// fixed-pitch face. After them, any **installed family** a script names
/// (`font: "Georgia"`) is found in the system font directories the first time
/// it is asked for, loaded as a ladder like the roles, and kept. A name the
/// book can't supply resolves to the default face, so a script that asks for
/// something this machine hasn't got degrades instead of failing.
pub struct FontBook<'ttf> {
    ttf: &'ttf Sdl2TtfContext,
    /// Loaded faces by name: the roles, then families in the order first asked
    /// for. Index 0 is the default face.
    faces: Vec<(String, FontLadder<'ttf>)>,
    /// How many of `faces` are roles (the rest are families loaded on demand).
    role_count: usize,
    /// The installed families, scanned on the first name the book doesn't
    /// already know. `None` until then.
    system: Option<SystemFamilies>,
    /// Lower-cased names already looked up and not found (or whose file
    /// wouldn't load), so a miss in a draw loop is not re-scanned every frame.
    misses: HashSet<String>,
    /// Faces to scan instead of the system's, for tests.
    #[cfg(test)]
    scan_override: Option<SystemFamilies>,
}

impl<'ttf> FontBook<'ttf> {
    /// Load every role this machine can supply. Errs only when even the
    /// default sans face is missing — the state where no text can render.
    /// Installed families are not scanned until a script names one.
    pub fn load_system(ttf: &'ttf Sdl2TtfContext, sizes: &[u16]) -> Result<Self, String> {
        let mut faces = vec![(
            DEFAULT_ROLE.to_string(),
            FontLadder::load_from(ttf, SANS_PATHS, sizes)?,
        )];
        if let Ok(mono) = FontLadder::load_from(ttf, MONO_PATHS, sizes) {
            faces.push(("mono".to_string(), mono));
        }
        let role_count = faces.len();
        Ok(FontBook {
            ttf,
            faces,
            role_count,
            system: None,
            misses: HashSet::new(),
            #[cfg(test)]
            scan_override: None,
        })
    }

    /// The face a `font` spec selects: the first name in a CSS-style fallback
    /// list (`"Inter, mono"`) that is a role or an installed family, or the
    /// default face.
    pub fn resolve(&mut self, spec: Option<&str>) -> &mut FontLadder<'ttf> {
        let index = spec
            .and_then(|spec| {
                spec.split(',')
                    .find_map(|name| self.face_index(name.trim()))
            })
            .unwrap_or(0);
        &mut self.faces[index].1
    }

    /// The canonical name of the first face in `spec` this book can draw — a
    /// role as written, or a family as the font file spells it — or `None`
    /// when nothing in the list resolves (it would draw in the default face).
    pub fn canonical_name(&mut self, spec: &str) -> Option<String> {
        spec.split(',')
            .find_map(|name| self.face_index(name.trim()))
            .map(|index| self.faces[index].0.clone())
    }

    /// The loaded face for one name, loading an installed family on first use.
    /// Roles match exactly (`mono`); families match case-insensitively, by
    /// family or PostScript name.
    fn face_index(&mut self, name: &str) -> Option<usize> {
        if name.is_empty() {
            return None;
        }
        if let Some(at) = self.faces[..self.role_count]
            .iter()
            .position(|(role, _)| role == name)
        {
            return Some(at);
        }
        let key = name.to_lowercase();
        if let Some(at) = self.faces[self.role_count..]
            .iter()
            .position(|(family, _)| family.to_lowercase() == key)
        {
            return Some(self.role_count + at);
        }
        if self.misses.contains(&key) {
            return None;
        }
        let ttf = self.ttf;
        let loaded = self.system_families().lookup(&key).and_then(|entry| {
            FontLadder::load_face(ttf, &entry.path, entry.index, &[])
                .ok()
                .map(|ladder| (entry.family.clone(), ladder))
        });
        match loaded {
            Some((family, ladder)) => {
                // Two spellings (family and PostScript name) can reach one
                // face; keep a single ladder for it.
                if let Some(at) = self.faces.iter().position(|(f, _)| *f == family) {
                    return Some(at);
                }
                self.faces.push((family, ladder));
                Some(self.faces.len() - 1)
            }
            None => {
                self.misses.insert(key);
                None
            }
        }
    }

    /// The installed-family index, scanned now if it hasn't been.
    fn system_families(&mut self) -> &SystemFamilies {
        if self.system.is_none() {
            #[cfg(test)]
            if let Some(scan) = self.scan_override.take() {
                self.system = Some(scan);
            }
            if self.system.is_none() {
                self.system = Some(SystemFamilies::scan());
            }
        }
        self.system.as_ref().expect("just scanned")
    }

    /// Every family name a script could pass as `font` here, sorted — the
    /// answer to `fonts()`. Scans the system fonts on first call.
    pub fn families(&mut self) -> Vec<String> {
        self.system_families().names.clone()
    }

    /// Whether this machine supplied a given role. Only `ui` is guaranteed;
    /// `mono` depends on the fixed-pitch fonts installed.
    pub fn has_role(&self, role: &str) -> bool {
        self.faces[..self.role_count]
            .iter()
            .any(|(name, _)| name == role)
    }

    /// The default face — what `font`-less text renders in, and what the
    /// default-font metric bindings describe.
    pub fn default_face(&mut self) -> &mut FontLadder<'ttf> {
        &mut self.faces[0].1
    }

    /// Every role this book offers, for binding measurement data. Families
    /// loaded on demand are measured through [`SdlFontSource`] instead.
    pub fn roles(&mut self) -> impl Iterator<Item = (&str, &mut FontLadder<'ttf>)> {
        self.faces[..self.role_count]
            .iter_mut()
            .map(|(role, ladder)| (role.as_str(), ladder))
    }

    /// Scan `families` instead of the machine's fonts when a name first misses.
    #[cfg(test)]
    fn with_families(mut self, families: SystemFamilies) -> Self {
        self.scan_override = Some(families);
        self
    }
}

/// Where one installed family's regular face lives.
#[derive(Clone, Debug)]
struct FamilyEntry {
    /// The family as its font file names it — the canonical spelling.
    family: String,
    path: String,
    index: u32,
}

/// The installed font families, by lower-cased family and PostScript name,
/// each pointing at the face closest to upright regular.
#[derive(Default)]
struct SystemFamilies {
    by_name: HashMap<String, FamilyEntry>,
    /// Every family name, sorted and deduplicated.
    names: Vec<String>,
}

impl SystemFamilies {
    /// Read the name tables of every font in the system font directories
    /// (each file is memory-mapped only long enough to parse it).
    fn scan() -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let faces = db.faces().filter_map(|face| {
            let path = match &face.source {
                fontdb::Source::File(path) => path,
                fontdb::Source::SharedFile(path, _) => path,
                fontdb::Source::Binary(_) => return None,
            };
            let family = face.families.first()?.0.clone();
            Some(ScannedFace {
                family,
                post_script_name: face.post_script_name.clone(),
                path: path.to_string_lossy().into_owned(),
                index: face.index,
                regularity: regularity(face.style, face.weight.0, face.stretch),
            })
        });
        Self::from_faces(faces)
    }

    /// Index scanned faces, keeping per family the one closest to regular.
    fn from_faces(faces: impl IntoIterator<Item = ScannedFace>) -> Self {
        let mut best: HashMap<String, ScannedFace> = HashMap::new();
        let mut post_script: HashMap<String, ScannedFace> = HashMap::new();
        for face in faces {
            let key = face.family.to_lowercase();
            if best
                .get(&key)
                .is_none_or(|kept| face.regularity < kept.regularity)
            {
                best.insert(key, face.clone());
            }
            if !face.post_script_name.is_empty() {
                post_script.insert(face.post_script_name.to_lowercase(), face);
            }
        }
        let mut names: Vec<String> = best.values().map(|f| f.family.clone()).collect();
        names.sort_unstable_by_key(|n| n.to_lowercase());
        names.dedup();
        let entry = |f: &ScannedFace| FamilyEntry {
            family: f.family.clone(),
            path: f.path.clone(),
            index: f.index,
        };
        let mut by_name: HashMap<String, FamilyEntry> = post_script
            .iter()
            .map(|(k, f)| (k.clone(), entry(f)))
            .collect();
        // A family name wins over a PostScript name that happens to match it.
        for (k, f) in &best {
            by_name.insert(k.clone(), entry(f));
        }
        SystemFamilies { by_name, names }
    }

    fn lookup(&self, lower_name: &str) -> Option<&FamilyEntry> {
        self.by_name.get(lower_name)
    }
}

/// One face found by a scan.
#[derive(Clone, Debug)]
struct ScannedFace {
    family: String,
    post_script_name: String,
    path: String,
    index: u32,
    /// Distance from upright regular normal-width; lower is closer.
    regularity: u32,
}

/// How far a face is from the upright, regular, normal-width cut a family is
/// drawn in when a script names it (bold and italic are synthesized from it).
fn regularity(style: fontdb::Style, weight: u16, stretch: fontdb::Stretch) -> u32 {
    let slant = match style {
        fontdb::Style::Normal => 0,
        _ => 10_000,
    };
    let width = (stretch.to_number() as i32 - fontdb::Stretch::Normal.to_number() as i32)
        .unsigned_abs()
        * 1_000;
    slant + width + (weight as i32 - 400).unsigned_abs()
}

/// The [`petal_ui::draw::FontSource`] over a shared [`FontBook`], so
/// `font(name)`, `fonts()` and `text_width` see the same installed families
/// the renderer draws — measured from the very ladder that will rasterize
/// them. The roles are also published eagerly as bindings (which win where
/// both answer); this covers every other name.
pub struct SdlFontSource {
    book: std::rc::Rc<std::cell::RefCell<FontBook<'static>>>,
}

impl SdlFontSource {
    pub fn new(book: std::rc::Rc<std::cell::RefCell<FontBook<'static>>>) -> Self {
        SdlFontSource { book }
    }
}

impl petal_ui::draw::FontSource for SdlFontSource {
    fn resolve(&mut self, name: &str) -> Option<String> {
        self.book.borrow_mut().canonical_name(name)
    }

    fn metrics(
        &mut self,
        family: &str,
        weight: u16,
        italic: bool,
    ) -> Option<petal_ui::draw::FontMetrics> {
        let mut book = self.book.borrow_mut();
        book.canonical_name(family)?;
        let face = book.resolve(Some(family));
        let ratios = face.ascii_advance_ratios_styled(weight, italic);
        let vertical = face.vertical_ratios_styled(weight, italic);
        Some(
            petal_ui::draw::FontMetrics::proportional(ratios, petal_ui::draw::DEFAULT_TEXT_ADVANCE)
                .with_vertical(vertical),
        )
    }

    fn families(&mut self) -> Vec<String> {
        self.book.borrow_mut().families()
    }

    fn glyph_advance(&mut self, face: &str, weight: u16, italic: bool, ch: char) -> Option<f64> {
        self.book
            .borrow_mut()
            .resolve(Some(face))
            .glyph_advance_ratio(weight, italic, ch)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned(
        family: &str,
        ps: &str,
        path: &str,
        index: u32,
        weight: u16,
        italic: bool,
    ) -> ScannedFace {
        let style = match italic {
            true => fontdb::Style::Italic,
            false => fontdb::Style::Normal,
        };
        ScannedFace {
            family: family.to_string(),
            post_script_name: ps.to_string(),
            path: path.to_string(),
            index,
            regularity: regularity(style, weight, fontdb::Stretch::Normal),
        }
    }

    #[test]
    fn a_family_is_drawn_from_its_most_regular_face() {
        let families = SystemFamilies::from_faces([
            scanned("Acme Sans", "AcmeSans-Bold", "/f/acme.ttc", 1, 700, false),
            scanned("Acme Sans", "AcmeSans-Italic", "/f/acme.ttc", 2, 400, true),
            scanned(
                "Acme Sans",
                "AcmeSans-Regular",
                "/f/acme.ttc",
                0,
                400,
                false,
            ),
            scanned("Other", "Other-Light", "/f/other.ttf", 0, 300, false),
        ]);
        let acme = families
            .lookup("acme sans")
            .expect("family by lower-cased name");
        assert_eq!((acme.family.as_str(), acme.index), ("Acme Sans", 0));
        // A PostScript name reaches its own face; the family is still canonical.
        let bold = families
            .lookup("acmesans-bold")
            .expect("by PostScript name");
        assert_eq!((bold.family.as_str(), bold.index), ("Acme Sans", 1));
        assert_eq!(
            families.names,
            vec!["Acme Sans".to_string(), "Other".to_string()]
        );
        assert!(families.lookup("missing").is_none());
    }

    /// A book whose "installed" families are one fake family backed by the
    /// default face's own file, so the test needs no particular system font.
    fn book_with_fake_family(ttf: &Sdl2TtfContext) -> Option<FontBook<'_>> {
        let path = resolve_font_path(ttf, SANS_PATHS)?;
        let families = SystemFamilies::from_faces([scanned(
            "Petal Test Face",
            "PetalTestFace-Regular",
            &path,
            0,
            400,
            false,
        )]);
        Some(
            FontBook::load_system(ttf, &[])
                .ok()?
                .with_families(families),
        )
    }

    #[test]
    fn a_family_name_loads_once_and_resolves_case_insensitively() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let Some(mut book) = book_with_fake_family(&ttf) else {
            return;
        };
        let roles = book.faces.len();
        assert_eq!(
            book.canonical_name("petal test face").as_deref(),
            Some("Petal Test Face")
        );
        assert_eq!(
            book.faces.len(),
            roles + 1,
            "the family was loaded as a face"
        );
        // Fallback lists, other spellings and the PostScript name reuse it.
        assert_eq!(
            book.canonical_name("Nope, PETAL TEST FACE").as_deref(),
            Some("Petal Test Face")
        );
        assert_eq!(
            book.canonical_name("PetalTestFace-Regular").as_deref(),
            Some("Petal Test Face")
        );
        assert_eq!(book.faces.len(), roles + 1, "and only once");
        // The family has its own ladder, baked on demand like the roles.
        let face = book.resolve(Some("petal test face"));
        face.sized_styled(19, 400, false);
        assert!(face.sizes().contains(&19));
        assert_eq!(book.families(), vec!["Petal Test Face".to_string()]);
    }

    #[test]
    fn an_unknown_name_falls_back_to_the_default_and_is_remembered() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let Some(mut book) = book_with_fake_family(&ttf) else {
            return;
        };
        assert_eq!(book.canonical_name("No Such Face"), None);
        assert!(book.misses.contains("no such face"));
        // Roles match as written and are never looked up on disk.
        assert_eq!(book.canonical_name("ui").as_deref(), Some("ui"));
        let default = book.default_face().path.clone();
        assert_eq!(book.resolve(Some("No Such Face")).path, default);
        assert_eq!(book.resolve(None).path, default);
    }

    /// The real system scan, end to end: some fixed-pitch family that most
    /// machines have resolves by name and draws narrow glyphs wider than the
    /// proportional default. Skipped where none of them is installed.
    #[test]
    fn an_installed_family_resolves_from_the_system_fonts() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let Ok(mut book) = FontBook::load_system(&ttf, &[]) else {
            return;
        };
        let families = book.families();
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
        assert_eq!(
            book.canonical_name(&family.to_uppercase()).as_deref(),
            Some(family)
        );
        let i = 'i' as usize;
        let default_i = book.default_face().ascii_advance_ratios()[i];
        let family_i = book.resolve(Some(family)).ascii_advance_ratios()[i];
        assert!(
            family_i > default_i,
            "{family}'s 'i' ({family_i:.3}) should be wider than the default's ({default_i:.3})"
        );
    }
}
