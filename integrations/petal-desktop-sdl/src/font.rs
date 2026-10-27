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
//! Weight and italic draw from the face's own cuts where the font has them —
//! Helvetica.ttc holds Regular, Bold, Oblique, Light and more, each loaded as
//! its own ladder the first time a weight or slant picks it, and measured
//! from that same ladder. Only what no cut covers (bold of a family without
//! one, italic of an upright-only file) falls back to SDL_ttf's *synthetic*
//! styles, emboldening or shearing the nearest cut.

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

/// One cut of a face — a file, a face index inside it for `.ttc` collections,
/// and the weight and slant its designer drew it at.
#[derive(Clone, Debug, PartialEq)]
struct CutSpec {
    path: String,
    index: u32,
    weight: u16,
    italic: bool,
}

impl CutSpec {
    /// Face `index` of the file at `path`, as the upright regular cut.
    fn regular(path: String, index: u32) -> Self {
        CutSpec {
            path,
            index,
            weight: petal_ui::draw::REGULAR_WEIGHT,
            italic: false,
        }
    }
}

/// One cut baked at the sizes asked of it so far, sorted ascending by size.
struct Cut<'ttf> {
    spec: CutSpec,
    /// The style the face itself declares (a bold cut is flagged bold).
    /// SDL_ttf's `get_style` reports it on top of whatever was set, so it is
    /// part of what a correctly styled rung reads back.
    native: FontStyle,
    rungs: Vec<(u16, Font<'ttf, 'static>)>,
}

/// One face — every cut of it this host found (regular, bold, italic, light,
/// ...) — each baked at the sizes asked of it so far. Cut 0 is the regular
/// one: it is the one preloaded, and the one synthetic styles fall back to.
pub struct FontLadder<'ttf> {
    ttf: &'ttf Sdl2TtfContext,
    cuts: Vec<Cut<'ttf>>,
}

impl<'ttf> FontLadder<'ttf> {
    /// Load the ladder from the first available sans-serif system font,
    /// preloading `sizes`. Errs only if no font file loads at all.
    pub fn load_system(ttf: &'ttf Sdl2TtfContext, sizes: &[u16]) -> Result<Self, String> {
        Self::load_from(ttf, SANS_PATHS, sizes)
    }

    /// Load the ladder from the first path in `paths` that loads, with every
    /// cut of its family that the file (or collection) holds.
    fn load_from(ttf: &'ttf Sdl2TtfContext, paths: &[&str], sizes: &[u16]) -> Result<Self, String> {
        let path = resolve_font_path(ttf, paths)
            .ok_or_else(|| "No system font found. Install a TTF font.".to_string())?;
        let mut cuts = file_cuts(&path);
        if cuts.is_empty() {
            cuts.push(CutSpec::regular(path, 0));
        }
        Self::load_cuts(ttf, cuts, sizes)
    }

    /// Load face `index` of the font file at `path` as a single-cut ladder,
    /// preloading `sizes` and [`MEASURE_SIZE`]. Every weight and slant is
    /// synthesized from it. Errs if the measuring rung will not load.
    pub fn load_face(
        ttf: &'ttf Sdl2TtfContext,
        path: &str,
        index: u32,
        sizes: &[u16],
    ) -> Result<Self, String> {
        Self::load_cuts(ttf, vec![CutSpec::regular(path.to_string(), index)], sizes)
    }

    /// A ladder over `cuts`, the first of them the regular cut: that one is
    /// preloaded at `sizes` and [`MEASURE_SIZE`] (and errs if the measuring
    /// rung will not load); the rest are baked the first time a weight or
    /// slant picks them.
    fn load_cuts(
        ttf: &'ttf Sdl2TtfContext,
        cuts: Vec<CutSpec>,
        sizes: &[u16],
    ) -> Result<Self, String> {
        let mut ladder = FontLadder {
            ttf,
            cuts: cuts
                .into_iter()
                .map(|spec| Cut {
                    spec,
                    native: FontStyle::NORMAL,
                    rungs: Vec::new(),
                })
                .collect(),
        };
        if ladder.cuts.is_empty() || ladder.bake(0, MEASURE_SIZE).is_none() {
            let path = ladder.cuts.first().map_or("", |c| c.spec.path.as_str());
            return Err(format!("Could not load a font from {path}."));
        }
        for &size in sizes {
            ladder.bake(0, size);
        }
        Ok(ladder)
    }

    /// The file the regular cut is drawn from.
    pub fn path(&self) -> &str {
        &self.cuts[0].spec.path
    }

    /// The sizes of the regular cut baked so far, ascending.
    pub fn sizes(&self) -> Vec<u16> {
        self.cuts[0].rungs.iter().map(|(size, _)| *size).collect()
    }

    /// The rung of cut `cut` baked at `size`, loading it now if this is the
    /// first time the size is asked for. `None` when the size is out of
    /// range, the cut is full, or the file refuses to load at that size.
    fn bake(&mut self, cut: usize, size: u16) -> Option<usize> {
        let ttf = self.ttf;
        let cut = &mut self.cuts[cut];
        match cut.rungs.binary_search_by_key(&size, |(rung, _)| *rung) {
            Ok(at) => Some(at),
            Err(at) => {
                if size == 0 || size > MAX_RUNG_SIZE || cut.rungs.len() >= MAX_RUNGS {
                    return None;
                }
                let mut font = ttf
                    .load_font_at_index(&cut.spec.path, cut.spec.index, size)
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
                // Fresh, the font has no style set: what it reports is the
                // face's own.
                cut.native = font.get_style();
                cut.rungs.insert(at, (size, font));
                Some(at)
            }
        }
    }

    /// The rung of `cut` for `size`: baked at exactly that size when it can
    /// be, else the nearest one loaded.
    fn index_for(&mut self, cut: usize, size: u16) -> usize {
        self.bake(cut, size)
            .unwrap_or_else(|| self.index_nearest(cut, size))
    }

    /// The cut that draws `weight` and `italic`, and the synthetic style still
    /// needed on top of it (see [`pick_cut`]). A cut whose file will not load
    /// is dropped on the way, so it is tried once, not every frame.
    fn cut_for(&mut self, weight: u16, italic: bool) -> (usize, FontStyle) {
        loop {
            let specs: Vec<&CutSpec> = self.cuts.iter().map(|c| &c.spec).collect();
            let (cut, style) = pick_cut(&specs, weight, italic);
            if cut == 0 || self.bake(cut, MEASURE_SIZE).is_some() {
                return (cut, style);
            }
            self.cuts.remove(cut);
        }
    }

    /// The font baked at `size` (see [`index_for`](Self::index_for)) in the
    /// cut nearest this weight and slant, with SDL_ttf's synthetic bold or
    /// italic applied for whatever that cut doesn't draw itself. Takes
    /// `&mut self` because a size may be baked on the way, and because the
    /// style is *font* state, not a per-call argument: the rung is restyled on
    /// the way out and stays that way until the next caller sets it, which is
    /// why every text command goes through here.
    pub fn sized_styled(&mut self, size: u16, weight: u16, italic: bool) -> &Font<'ttf, 'static> {
        self.styled_rung(size, weight, italic)
    }

    /// [`sized_styled`](Self::sized_styled), mutable — what the measuring
    /// methods need, so they measure exactly the font a draw would use.
    fn styled_rung(&mut self, size: u16, weight: u16, italic: bool) -> &mut Font<'ttf, 'static> {
        let (cut, style) = self.cut_for(weight, italic);
        let index = self.index_for(cut, size);
        let cut = &mut self.cuts[cut];
        let font = &mut cut.rungs[index].1;
        if font.get_style() != style | cut.native {
            font.set_style(style);
        }
        font
    }

    /// The loaded rung of `cut` closest to `size`. On a tie the smaller rung
    /// wins (rungs are sorted ascending, so the first minimum is kept).
    fn index_nearest(&self, cut: usize, size: u16) -> usize {
        self.cuts[cut]
            .rungs
            .iter()
            .enumerate()
            .min_by_key(|(_, (rung, _))| (*rung as i32 - size as i32).abs())
            .expect("a cut in use always has its measuring rung")
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

    /// [`vertical_ratios`](Self::vertical_ratios) for one weight and slant,
    /// measured from the cut that draws it. A bold cut's extents differ
    /// slightly from the regular's, and italic shears them — measuring the
    /// variant keeps a bold label centred on the same line as its regular
    /// neighbour.
    pub fn vertical_ratios_styled(
        &mut self,
        weight: u16,
        italic: bool,
    ) -> petal_ui::draw::VerticalMetrics {
        let font = self.styled_rung(MEASURE_SIZE, weight, italic);
        measure_vertical(font, MEASURE_SIZE)
    }

    /// [`ascii_advance_ratios`](Self::ascii_advance_ratios) for one weight and
    /// slant, measured from the cut (and synthetic style) that draws it. Bold
    /// glyphs are wider, so bold text measured with the regular table would
    /// come out short — this is the table a script's
    /// `text_width(s, {weight: 700, ...})` needs to agree with the pixels.
    ///
    /// The rung is restyled to measure and left that way (the next draw sets
    /// what it needs), which is why this takes `&mut self`.
    pub fn ascii_advance_ratios_styled(&mut self, weight: u16, italic: bool) -> Vec<f64> {
        let font = self.styled_rung(MEASURE_SIZE, weight, italic);
        measure_ascii(font, MEASURE_SIZE)
    }

    /// Everything `text_width` and friends know about one weight and slant of
    /// this face: the advance table from
    /// [`ascii_advance_ratios_styled`](Self::ascii_advance_ratios_styled) with
    /// the vertical metrics from
    /// [`vertical_ratios_styled`](Self::vertical_ratios_styled).
    pub fn metrics_styled(&mut self, weight: u16, italic: bool) -> petal_ui::draw::FontMetrics {
        let ratios = self.ascii_advance_ratios_styled(weight, italic);
        let vertical = self.vertical_ratios_styled(weight, italic);
        petal_ui::draw::FontMetrics::proportional(ratios, petal_ui::draw::DEFAULT_TEXT_ADVANCE)
            .with_vertical(vertical)
    }

    /// One glyph's advance ÷ size in this face and variant, or `None` when the
    /// face has no glyph for it. What `text_width` asks for a non-ASCII
    /// character the advance table doesn't cover.
    pub fn glyph_advance_ratio(&mut self, weight: u16, italic: bool, ch: char) -> Option<f64> {
        let font = self.styled_rung(MEASURE_SIZE, weight, italic);
        advance_ratio(font, MEASURE_SIZE, ch)
    }
}

/// The cut among `cuts` that draws a CSS `weight` and slant, and the synthetic
/// style SDL_ttf adds on top for what that cut lacks.
///
/// Slant matters most: an italic request takes an italic cut of any weight
/// before an upright one (whose shear would be synthetic), and an upright
/// request never takes an italic cut while an upright one exists. Then the
/// nearest weight wins; on a tie, CSS's rule — lighter for a request at or
/// under 450, heavier above — so 500 draws regular rather than bold. The
/// synthetic part is what remains: bold when the request is semibold or
/// heavier but the cut is not; italic when the cut is upright. Lighter than
/// the lightest cut has no synthetic equivalent, so it draws that cut.
fn pick_cut(cuts: &[&CutSpec], weight: u16, italic: bool) -> (usize, FontStyle) {
    let heavier_wins = weight > 450;
    let at = cuts
        .iter()
        .enumerate()
        .min_by_key(|(_, cut)| {
            let distance = (cut.weight as i32 - weight as i32).unsigned_abs();
            let tie = (cut.weight > weight) != heavier_wins;
            (cut.italic != italic, distance, tie)
        })
        .map_or(0, |(at, _)| at);
    let mut style = FontStyle::NORMAL;
    if let Some(cut) = cuts.get(at) {
        if weight >= 600 && cut.weight < 600 {
            style |= FontStyle::BOLD;
        }
        if italic && !cut.italic {
            style |= FontStyle::ITALIC;
        }
    }
    (at, style)
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
        let index = spec.and_then(|spec| self.first_face(spec)).unwrap_or(0);
        &mut self.faces[index].1
    }

    /// The canonical name of the first face in `spec` this book can draw — a
    /// role as written, or a family as the font file spells it — or `None`
    /// when nothing in the list resolves (it would draw in the default face).
    pub fn canonical_name(&mut self, spec: &str) -> Option<String> {
        self.first_face(spec).map(|index| self.faces[index].0.clone())
    }

    /// The index of the first face in `spec`'s fallback list this book can
    /// draw.
    fn first_face(&mut self, spec: &str) -> Option<usize> {
        spec.split(',').find_map(|name| self.face_index(name.trim()))
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
            FontLadder::load_cuts(ttf, entry.cuts.clone(), &[])
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

/// Where one installed family's cuts live.
#[derive(Clone, Debug)]
struct FamilyEntry {
    /// The family as its font file names it — the canonical spelling.
    family: String,
    /// The face closest to upright regular first, then the family's other
    /// cuts at its width (see [`family_cuts`]).
    cuts: Vec<CutSpec>,
}

/// The installed font families, by lower-cased family and PostScript name.
/// A family name reaches all of the family's cuts; a PostScript name reaches
/// just its own face (every variant synthesized from it).
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
        Self::from_faces(db.faces().filter_map(ScannedFace::from_info))
    }

    /// Index scanned faces: each family by all its cuts, each PostScript name
    /// by its own face.
    fn from_faces(faces: impl IntoIterator<Item = ScannedFace>) -> Self {
        let mut families: HashMap<String, Vec<ScannedFace>> = HashMap::new();
        let mut post_script: HashMap<String, ScannedFace> = HashMap::new();
        for face in faces {
            if !face.post_script_name.is_empty() {
                post_script.insert(face.post_script_name.to_lowercase(), face.clone());
            }
            families
                .entry(face.family.to_lowercase())
                .or_default()
                .push(face);
        }
        let mut names: Vec<String> = families.values().map(|f| f[0].family.clone()).collect();
        names.sort_unstable_by_key(|n| n.to_lowercase());
        names.dedup();
        let mut by_name: HashMap<String, FamilyEntry> = post_script
            .iter()
            .map(|(k, f)| {
                let entry = FamilyEntry {
                    family: f.family.clone(),
                    cuts: vec![f.cut_spec(petal_ui::draw::REGULAR_WEIGHT, false)],
                };
                (k.clone(), entry)
            })
            .collect();
        // A family name wins over a PostScript name that happens to match it.
        for (k, faces) in &families {
            let cuts = family_cuts(faces);
            let family = faces[0].family.clone();
            by_name.insert(k.clone(), FamilyEntry { family, cuts });
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
    weight: u16,
    /// Italic or oblique.
    italic: bool,
    stretch: fontdb::Stretch,
}

impl ScannedFace {
    /// The face fontdb found, or `None` for one not backed by a file.
    fn from_info(face: &fontdb::FaceInfo) -> Option<Self> {
        let path = match &face.source {
            fontdb::Source::File(path) => path,
            fontdb::Source::SharedFile(path, _) => path,
            fontdb::Source::Binary(_) => return None,
        };
        Some(ScannedFace {
            family: face.families.first()?.0.clone(),
            post_script_name: face.post_script_name.clone(),
            path: path.to_string_lossy().into_owned(),
            index: face.index,
            weight: face.weight.0,
            italic: face.style != fontdb::Style::Normal,
            stretch: face.stretch,
        })
    }

    /// Distance from upright regular normal-width; lower is closer.
    fn regularity(&self) -> u32 {
        let slant = if self.italic { 10_000 } else { 0 };
        let width = (self.stretch.to_number() as i32
            - fontdb::Stretch::Normal.to_number() as i32)
            .unsigned_abs()
            * 1_000;
        slant + width + (self.weight as i32 - 400).unsigned_abs()
    }

    /// This face as a cut drawing `weight` and `italic`.
    fn cut_spec(&self, weight: u16, italic: bool) -> CutSpec {
        CutSpec {
            path: self.path.clone(),
            index: self.index,
            weight,
            italic,
        }
    }
}

/// The cuts a family draws from, out of all its scanned faces: the one
/// closest to upright regular normal-width first, then every other face of
/// that same width, one per weight and slant (the most regular file wins a
/// duplicate). Condensed or expanded faces are left out — a width change is
/// not what `weight` asks for. Empty only when `faces` is.
fn family_cuts(faces: &[ScannedFace]) -> Vec<CutSpec> {
    let Some(regular) = faces.iter().min_by_key(|f| f.regularity()) else {
        return Vec::new();
    };
    let mut rest: Vec<&ScannedFace> = faces
        .iter()
        .filter(|f| f.stretch == regular.stretch && !std::ptr::eq(*f, regular))
        .collect();
    rest.sort_by_key(|f| (f.weight, f.italic, f.path.clone(), f.index));
    let mut cuts = vec![regular.cut_spec(regular.weight, regular.italic)];
    for face in rest {
        if !cuts
            .iter()
            .any(|c| c.weight == face.weight && c.italic == face.italic)
        {
            cuts.push(face.cut_spec(face.weight, face.italic));
        }
    }
    cuts
}

/// The cuts of the family the font file at `path` is mostly about — every
/// face of the file's most regular face's family (see [`family_cuts`]). A
/// role's file is read this way, so `ui` on macOS draws Helvetica.ttc's own
/// Bold and Oblique. Empty when fontdb can't read the file.
fn file_cuts(path: &str) -> Vec<CutSpec> {
    let mut db = fontdb::Database::new();
    if db.load_font_file(path).is_err() {
        return Vec::new();
    }
    let faces: Vec<ScannedFace> = db.faces().filter_map(ScannedFace::from_info).collect();
    let Some(regular) = faces.iter().min_by_key(|f| f.regularity()) else {
        return Vec::new();
    };
    let family = regular.family.clone();
    let faces: Vec<ScannedFace> = faces.into_iter().filter(|f| f.family == family).collect();
    family_cuts(&faces)
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
        let index = book.first_face(family)?;
        Some(book.faces[index].1.metrics_styled(weight, italic))
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
        ScannedFace {
            family: family.to_string(),
            post_script_name: ps.to_string(),
            path: path.to_string(),
            index,
            weight,
            italic,
            stretch: fontdb::Stretch::Normal,
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
        assert_eq!((acme.family.as_str(), acme.cuts[0].index), ("Acme Sans", 0));
        // A PostScript name reaches its own face; the family is still canonical.
        let bold = families
            .lookup("acmesans-bold")
            .expect("by PostScript name");
        assert_eq!((bold.family.as_str(), bold.cuts[0].index), ("Acme Sans", 1));
        assert_eq!(bold.cuts.len(), 1, "a PostScript name is just its face");
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
        let default = book.default_face().path().to_string();
        assert_eq!(book.resolve(Some("No Such Face")).path(), default);
        assert_eq!(book.resolve(None).path(), default);
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

    fn cut(weight: u16, italic: bool) -> CutSpec {
        CutSpec {
            path: format!("/f/{weight}{italic}"),
            index: 0,
            weight,
            italic,
        }
    }

    #[test]
    fn a_weight_draws_from_the_nearest_cut_and_synthesizes_the_rest() {
        let helvetica = [
            cut(400, false),
            cut(300, false),
            cut(700, false),
            cut(400, true),
            cut(700, true),
        ];
        let cuts: Vec<&CutSpec> = helvetica.iter().collect();
        let pick = |weight, italic| {
            let (at, style) = pick_cut(&cuts, weight, italic);
            (cuts[at].weight, cuts[at].italic, style)
        };
        assert_eq!(pick(400, false), (400, false, FontStyle::NORMAL));
        assert_eq!(pick(700, false), (700, false, FontStyle::NORMAL));
        assert_eq!(pick(600, false), (700, false, FontStyle::NORMAL));
        assert_eq!(pick(900, true), (700, true, FontStyle::NORMAL));
        assert_eq!(pick(300, false), (300, false, FontStyle::NORMAL));
        assert_eq!(pick(100, false), (300, false, FontStyle::NORMAL));
        // 500 sits between regular and bold; CSS takes the lighter.
        assert_eq!(pick(500, false), (400, false, FontStyle::NORMAL));
        // An italic light has no italic light cut: the regular italic beats
        // shearing the upright light.
        assert_eq!(pick(300, true), (400, true, FontStyle::NORMAL));

        // A regular-only file synthesizes everything from it.
        let single = [cut(400, false)];
        let cuts: Vec<&CutSpec> = single.iter().collect();
        assert_eq!(
            pick_cut(&cuts, 700, true),
            (0, FontStyle::BOLD | FontStyle::ITALIC)
        );
        assert_eq!(pick_cut(&cuts, 500, false), (0, FontStyle::NORMAL));
        // A bold-only cut never needs synthetic bold on top.
        let bold_only = [cut(400, false), cut(700, false)];
        let cuts: Vec<&CutSpec> = bold_only.iter().collect();
        assert_eq!(pick_cut(&cuts, 700, true), (1, FontStyle::ITALIC));
    }

    #[test]
    fn a_family_keeps_every_cut_at_its_width() {
        let mut condensed = scanned("Acme", "Acme-CondBold", "/f/acme.ttc", 4, 700, false);
        condensed.stretch = fontdb::Stretch::Condensed;
        let families = SystemFamilies::from_faces([
            scanned("Acme", "Acme-Bold", "/f/acme.ttc", 1, 700, false),
            scanned("Acme", "Acme-Regular", "/f/acme.ttc", 0, 400, false),
            scanned("Acme", "Acme-Italic", "/f/acme.ttc", 2, 400, true),
            scanned("Acme", "Acme-Bold2", "/f/acme2.ttf", 0, 700, false),
            condensed,
        ]);
        let acme = families.lookup("acme").expect("family");
        let cuts: Vec<(u32, u16, bool)> = acme
            .cuts
            .iter()
            .map(|c| (c.index, c.weight, c.italic))
            .collect();
        // Regular first; one bold (no duplicate), no condensed cut.
        assert_eq!(cuts, vec![(0, 400, false), (2, 400, true), (1, 700, false)]);
    }

    /// The default role on a machine whose sans file is a collection with a
    /// real bold (macOS's Helvetica.ttc) draws bold from that cut — no
    /// synthetic emboldening — and measures it from the same cut.
    #[test]
    fn the_default_role_draws_bold_from_a_real_cut_when_the_file_has_one() {
        let _ttf_guard = crate::renderer::ttf_test_lock();
        let ttf = sdl2::ttf::init().unwrap();
        let Ok(mut book) = FontBook::load_system(&ttf, &[]) else {
            return;
        };
        let face = book.default_face();
        let (cut, style) = face.cut_for(700, false);
        if cut == 0 {
            return; // a regular-only sans file: bold is synthetic here
        }
        assert_eq!(style, FontStyle::NORMAL, "a real bold cut needs no synthetic bold");
        assert!(face.cuts[cut].spec.weight >= 600);
        // SDL_ttf reports the cut's own bold as its style; nothing is added.
        assert_eq!(face.sized_styled(17, 700, false).get_style(), FontStyle::BOLD);
        let regular = face.ascii_advance_ratios();
        let bold = face.ascii_advance_ratios_styled(700, false);
        let m = 'm' as usize;
        assert!(bold[m] > regular[m], "bold 'm' measures wider than regular");
        assert_eq!(face.sizes(), vec![MEASURE_SIZE], "other cuts don't grow the regular's");
    }
}
