//! Text rendering via glyphon (cosmic-text shaping + glyph atlas).
//!
//! Each [`crate::Primitive::Text`] run becomes one glyphon `TextArea`. Text is
//! re-shaped every frame (acceptable for v1 — glyphon caches rasterized
//! glyphs in its atlas), but the glyphon `Buffer`s themselves are pooled and
//! reused across frames to limit allocation churn.

use glyphon::{
    Attrs, Cache, Family, FontSystem, Metrics, Resolution, Shaping, Style, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};

use crate::{Color, Rect, TextStyle, REGULAR_WEIGHT};

/// One text run staged for drawing, in logical pixels.
pub(crate) struct TextRun<'a> {
    pub pos: (f32, f32),
    pub text: &'a str,
    pub color: Color,
    pub clip: Rect,
    /// Font size in logical pixels for this run.
    pub size: f32,
    /// Weight / slant for this run. Letter-spacing is not handled here — the
    /// caller has already split a spaced run into per-glyph runs, since
    /// cosmic-text has no letter-spacing of its own.
    pub style: TextStyle,
}

/// JetBrains Mono Regular, embedded so there is no font discovery at startup.
/// Licensed under the SIL Open Font License 1.1 (see `assets/OFL.txt`).
const FONT_BYTES: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.ttf");

/// Default font size in logical pixels — what editor and chrome text render
/// at. A [`crate::Primitive::Text`] carries its own size, so a panel script
/// can ask for anything; this is what everything else uses.
pub const FONT_SIZE: f32 = 14.0;
/// Line height as a multiple of the font size.
pub const LINE_HEIGHT_RATIO: f32 = 1.4;
/// Line height of default-size text, in logical pixels.
const LINE_HEIGHT: f32 = FONT_SIZE * LINE_HEIGHT_RATIO;

/// System font files consulted when the primary monospace font lacks a glyph —
/// CJK ideographs, kana, hangul, broad Unicode symbols, and color emoji.
///
/// The embedded JetBrains Mono has no coverage outside Latin/Greek/Cyrillic, so
/// without these every CJK/emoji codepoint rasterizes as the notdef "tofu" box.
/// cosmic-text's advanced shaper (`Shaping::Advanced`) performs per-cluster font
/// fallback: when the default face is missing a glyph it walks script-specific,
/// then common, then *every remaining* face in the database (see
/// `FontFallbackIter::next_item` in cosmic-text), so simply having a covering
/// face loaded is enough — no per-run wiring, no dependency on the exact family
/// name. Paths that don't exist are silently skipped, so a machine lacking them
/// behaves exactly as before (tofu, no panic, no regression on Latin text).
///
/// Files are memory-mapped by fontdb (the `memmap` feature is on via
/// cosmic-text's `std`), so even the ~190 MB emoji collection is not copied into
/// RAM — only touched pages are paged in during shaping/rasterization.
///
/// macOS locations first (Garden's primary platform), then common Linux paths so
/// the crate degrades gracefully elsewhere.
const FALLBACK_FONT_CANDIDATES: &[&str] = &[
    // macOS — CJK (Han ideographs + kana; PingFang is the modern default,
    // Hiragino / STHeiti cover systems/versions where PingFang is absent).
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    // macOS — Hangul.
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    // macOS — broad BMP symbol coverage.
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    // macOS — color emoji (large; memory-mapped, not copied).
    "/System/Library/Fonts/Apple Color Emoji.ttc",
    // Linux — Noto CJK + color emoji, best-effort across distro layouts.
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
    "/usr/share/fonts/google-noto-emoji/NotoColorEmoji.ttf",
];

/// Load every fallback font in [`FALLBACK_FONT_CANDIDATES`] that exists on disk
/// into `db`, returning how many faces' files were loaded. Missing or
/// unparseable files are skipped; a return of `0` means fallback is unavailable
/// and missing glyphs will render as tofu (exactly the pre-fallback behavior).
fn load_fallback_fonts(db: &mut glyphon::fontdb::Database) -> usize {
    let mut loaded = 0;
    for path in FALLBACK_FONT_CANDIDATES {
        if std::path::Path::new(path).exists() && db.load_font_file(path).is_ok() {
            loaded += 1;
        }
    }
    loaded
}

pub(crate) struct TextStack {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    /// One renderer per *text batch* in the current frame, all sharing `atlas`
    /// and `viewport`. A scene interleaves shapes and text to preserve
    /// painter's order, so each contiguous stretch of text runs is staged into
    /// its own renderer and drawn at its own point in the pass. The pool grows
    /// to the high-water mark of batches and is reused across frames.
    renderers: Vec<TextRenderer>,
    /// Batches staged by the current frame — `renderers[..batches]` are live.
    batches: usize,
    /// Multisample state the pool's renderers are built with.
    samples: u32,
    /// Pool of shaping buffers, one per text run, reused across frames.
    /// Only the first `texts.len()` entries are shaped and drawn each frame.
    buffers: Vec<glyphon::Buffer>,
    /// Family name of the embedded font; `None` falls back to the system
    /// monospace family (only if the embedded font failed to parse).
    family_name: Option<String>,
    /// (advance_width, line_height) in logical pixels.
    cell_size: (f32, f32),
    /// Per-batch: did this batch's `prepare` fail because the atlas was full?
    /// A failed batch holds whatever vertices it was left with by an earlier
    /// frame, so it must not be drawn — see [`render_batch`](Self::render_batch).
    failed: Vec<bool>,
    /// Running counters of atlas pressure, surfaced through
    /// [`crate::Renderer::text_atlas_stats`].
    stats: AtlasStats,
}

/// What the glyph atlas is being asked to hold, and whether it ever gave up.
///
/// The atlas has a hard ceiling: it doubles until it hits the device's
/// `max_texture_dimension_2d` and then starts evicting, and once every glyph it
/// holds is in use by the frame being staged there is nowhere left to put the
/// next one. That is `overflows > 0`, and it is the state in which text
/// silently disappears — so it is counted, logged, and reported rather than
/// left to be discovered by squinting at a screenshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AtlasStats {
    /// Text runs staged in the last prepared frame.
    pub runs: usize,
    /// Distinct font sizes (rounded to 1/4 px, the shaping granularity) in the
    /// last prepared frame. The failure mode this whole area is about scales
    /// with this number, so it is the useful pressure reading.
    pub distinct_sizes: usize,
    /// Text batches the last frame refused to draw because the atlas was full.
    pub dropped_batches: usize,
    /// Total atlas-full events since startup. Nonzero means text has been
    /// missing from at least one frame.
    pub overflows: u64,
}

/// Load the embedded font into a fresh `FontSystem`. Returns the font system
/// and the embedded family name (`None` falls back to system monospace).
///
/// When `with_fallbacks` is set, CJK/emoji/Unicode fallback faces
/// ([`load_fallback_fonts`]) are also loaded so the shaper can resolve glyphs
/// the primary monospace font lacks. Pure-measurement callers pass `false`:
/// the cell advance is measured from the primary Latin face alone, so the
/// (potentially large) fallback files never need to be opened just to compute
/// layout metrics.
fn build_font_system(with_fallbacks: bool) -> (FontSystem, Option<String>) {
    let mut db = glyphon::fontdb::Database::new();
    db.load_font_data(FONT_BYTES.to_vec());
    let family_name = db
        .faces()
        .next()
        .and_then(|face| face.families.first().map(|(name, _)| name.clone()));
    if family_name.is_none() {
        // The embedded font failed to parse; fall back to system fonts
        // and let cosmic-text pick a monospace family.
        db.load_system_fonts();
    }
    if with_fallbacks {
        // Add CJK/kana/hangul/emoji/Unicode faces. The primary monospace face
        // stays the default (`family_name` is unchanged), so Latin text and the
        // cell advance are unaffected; cosmic-text only consults these for
        // clusters the primary face can't cover.
        load_fallback_fonts(&mut db);
    }
    let font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
    (font_system, family_name)
}

/// Measure the monospace cell with a throwaway `FontSystem` — no GPU
/// involved. Backs [`crate::cell_metrics`] so windowless frontends share the
/// windowed renderer's layout math. No fallback fonts are loaded: the advance
/// comes from the primary face alone.
pub(crate) fn measure_cell_standalone() -> (f32, f32) {
    let (mut font_system, family_name) = build_font_system(false);
    measure_cell(&mut font_system, family_name.as_deref())
}

/// [`measure_ascii_advances`] with a throwaway `FontSystem` — backs
/// [`crate::ascii_advance_ratios`].
pub(crate) fn measure_ascii_advances_standalone() -> Vec<f64> {
    let (mut font_system, family_name) = build_font_system(false);
    measure_ascii_advances(&mut font_system, family_name.as_deref())
}

impl TextStack {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let (mut font_system, family_name) = build_font_system(true);

        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let atlas = TextAtlas::new(device, queue, &cache, surface_format);

        let cell_size = measure_cell(&mut font_system, family_name.as_deref());

        Self {
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderers: Vec::new(),
            batches: 0,
            samples,
            buffers: Vec::new(),
            family_name,
            cell_size,
            failed: Vec::new(),
            stats: AtlasStats::default(),
        }
    }

    /// Atlas pressure as of the last prepared frame.
    pub fn atlas_stats(&self) -> AtlasStats {
        self.stats
    }

    /// Grow the renderer pool to `n` entries. Renderers share the atlas, so a
    /// new one costs a vertex buffer, not a new glyph cache.
    fn ensure_renderers(&mut self, device: &wgpu::Device, n: usize) {
        while self.renderers.len() < n {
            // Glyph coverage is already antialiased in the atlas, so MSAA buys
            // nothing on its own — but text shares the scene pass with the
            // shapes, and every pipeline in a pass must agree on the sample
            // count.
            self.renderers.push(TextRenderer::new(
                &mut self.atlas,
                device,
                wgpu::MultisampleState {
                    count: self.samples,
                    ..Default::default()
                },
                None,
            ));
        }
    }

    /// (advance_width, line_height) of one monospace cell, in logical pixels.
    pub fn cell_size(&self) -> (f32, f32) {
        self.cell_size
    }

    fn attrs(family_name: Option<&str>) -> Attrs<'_> {
        Self::styled_attrs(family_name, TextStyle::default())
    }

    /// [`attrs`](Self::attrs) carrying a run's weight and slant. cosmic-text
    /// resolves them against the faces it has loaded: with only the embedded
    /// regular, a bold request comes back regular rather than synthesized.
    fn styled_attrs(family_name: Option<&str>, style: TextStyle) -> Attrs<'_> {
        let family = match family_name {
            Some(name) => Family::Name(name),
            None => Family::Monospace,
        };
        let slant = match style.italic {
            true => Style::Italic,
            false => Style::Normal,
        };
        Attrs::new()
            .family(family)
            .weight(Weight(style.weight))
            .style(slant)
    }

    /// Shape this frame's text runs and upload glyphs to the atlas.
    ///
    /// `scale_factor` converts the runs' logical pixels to the physical
    /// pixels glyphon renders in.
    ///
    /// `batches` partitions `texts` into the contiguous stretches that are
    /// drawn at distinct points in the pass (see [`render_batch`]). It must
    /// cover `texts` in order; passing a single full-width range reproduces
    /// the old "all text last" behavior.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        physical_size: (u32, u32),
        scale_factor: f32,
        texts: &[TextRun<'_>],
        batches: &[std::ops::Range<usize>],
    ) {
        self.viewport.update(
            queue,
            Resolution {
                width: physical_size.0,
                height: physical_size.1,
            },
        );

        let metrics = Metrics::new(FONT_SIZE, LINE_HEIGHT);
        while self.buffers.len() < texts.len() {
            self.buffers
                .push(glyphon::Buffer::new(&mut self.font_system, metrics));
        }
        for (buffer, run) in self.buffers.iter_mut().zip(texts) {
            let attrs = Self::styled_attrs(self.family_name.as_deref(), run.style);
            // Buffers are pooled across frames and a run carries its own size,
            // so re-state the metrics every frame rather than at creation.
            buffer.set_metrics(
                &mut self.font_system,
                Metrics::new(run.size, run.size * LINE_HEIGHT_RATIO),
            );
            // No wrapping: a Text primitive is a single pre-laid-out run.
            buffer.set_size(&mut self.font_system, None, None);
            buffer.set_text(
                &mut self.font_system,
                run.text,
                &attrs,
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(&mut self.font_system, false);
        }

        self.ensure_renderers(device, batches.len());
        self.batches = batches.len();
        self.failed.clear();
        self.failed.resize(batches.len(), false);

        let mut sizes: Vec<u32> = texts.iter().map(|r| (r.size * 4.0) as u32).collect();
        sizes.sort_unstable();
        sizes.dedup();
        self.stats.runs = texts.len();
        self.stats.distinct_sizes = sizes.len();
        self.stats.dropped_batches = 0;

        // Each batch is staged into its own renderer. They share one atlas, so
        // a glyph rasterized for an earlier batch is reused by a later one; and
        // because `TextAtlas::grow` preserves existing glyph coordinates, the
        // vertices an earlier `prepare` already emitted stay valid even if a
        // later batch grows the atlas.
        for (index, (renderer, batch)) in self.renderers.iter_mut().zip(batches).enumerate() {
            let end = batch.end.min(texts.len());
            let start = batch.start.min(end);
            let mut areas: Vec<TextArea<'_>> = Vec::with_capacity(end - start);
            for (buffer, run) in self.buffers[start..end].iter().zip(&texts[start..end]) {
                let area = TextArea {
                    buffer,
                    left: run.pos.0 * scale_factor,
                    top: run.pos.1 * scale_factor,
                    scale: scale_factor,
                    bounds: TextBounds {
                        left: (run.clip.x * scale_factor).floor() as i32,
                        top: (run.clip.y * scale_factor).floor() as i32,
                        right: ((run.clip.x + run.clip.w) * scale_factor).ceil() as i32,
                        bottom: ((run.clip.y + run.clip.h) * scale_factor).ceil() as i32,
                    },
                    default_color: to_glyphon_color(run.color),
                    custom_glyphs: &[],
                };
                // Faux bold: the embedded family ships Regular only, so a
                // bold request resolves back to Regular and `weight` would
                // otherwise be a no-op the protocol accepts and ignores.
                // Smearing the same run horizontally thickens the stems
                // without touching layout — advances, and therefore
                // `text_width` and every column the caller computed from it,
                // are unchanged.
                if let Some(offset) = embolden_offset(run.style.weight, run.size) {
                    areas.push(TextArea {
                        left: area.left + offset * scale_factor,
                        ..area
                    });
                }
                areas.push(area);
            }

            let outcome = renderer.prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            );

            // The atlas is full: every glyph it holds is already in use by
            // this frame and it cannot grow past the device's texture limit.
            // glyphon leaves the renderer holding whatever vertices it had, so
            // drawing it would composite a *previous* frame's text into this
            // one. Skip the batch, count it, and say so — a missing line of
            // text with a log line beside it is debuggable; silently stale
            // text is what cost days of screenshot-squinting.
            if let Err(glyphon::PrepareError::AtlasFull) = outcome {
                self.failed[index] = true;
                self.stats.dropped_batches += 1;
                self.stats.overflows += 1;
                if self.stats.dropped_batches == 1 {
                    eprintln!(
                        "garden-render: GLYPH ATLAS FULL — dropped a text batch \
                         ({} runs, {} distinct sizes this frame, {} overflows total). \
                         Text in that batch is NOT on screen. Reduce the number of \
                         distinct font sizes drawn in one frame.",
                        self.stats.runs, self.stats.distinct_sizes, self.stats.overflows
                    );
                }
            }
        }
    }

    /// Draw text batch `index` (as partitioned by [`prepare`]) into the pass.
    pub fn render_batch(&self, index: usize, pass: &mut wgpu::RenderPass<'_>) {
        // A batch whose `prepare` hit a full atlas still holds an older
        // frame's vertices; drawing it would show stale text.
        if self.failed.get(index).copied().unwrap_or(false) {
            return;
        }
        let Some(renderer) = self.renderers.get(index).filter(|_| index < self.batches) else {
            return;
        };
        renderer
            .render(&self.atlas, &self.viewport, pass)
            .expect("glyphon text render failed");
    }

    /// Per-frame atlas maintenance; call after the frame is submitted.
    pub fn end_frame(&mut self) {
        self.atlas.trim();
    }
}

/// Weight at or above which a run is emboldened by smearing. 600 is CSS
/// semibold — below that a run is drawn once, as before.
const BOLD_THRESHOLD: u16 = 600;

/// How far to offset the doubled draw of a `weight >= 600` run, in logical
/// pixels, or `None` for a run that needs no emboldening.
///
/// The offset scales with the font size so the stem weight stays proportional,
/// and is floored at half a logical pixel so small text still thickens
/// visibly. It is deliberately well under a quarter of the advance: the smear
/// must not close the counters of small glyphs or bleed into the next cell.
fn embolden_offset(weight: u16, size: f32) -> Option<f32> {
    if weight < BOLD_THRESHOLD {
        return None;
    }
    // 400 -> 0, 700 -> 1, 900 -> ~1.67 of the base stroke increment.
    let extra = (weight - REGULAR_WEIGHT) as f32 / 300.0;
    Some((size * 0.035 * extra).max(0.5))
}

fn to_glyphon_color(c: Color) -> glyphon::Color {
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    glyphon::Color::rgba(to_u8(c.r), to_u8(c.g), to_u8(c.b), to_u8(c.a))
}

/// Shape `text` in the primary face and return the advance of its first glyph,
/// in logical pixels. Reuses the caller's `buffer` so a measurement loop shapes
/// into one allocation. `~0.6em` is a typical monospace advance, used only if
/// shaping produces no glyph.
fn first_glyph_advance(
    font_system: &mut FontSystem,
    buffer: &mut glyphon::Buffer,
    family_name: Option<&str>,
    text: &str,
) -> f32 {
    buffer.set_text(
        font_system,
        text,
        &TextStack::attrs(family_name),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    buffer
        .layout_runs()
        .next()
        .and_then(|run| run.glyphs.first().map(|glyph| glyph.w))
        .unwrap_or(FONT_SIZE * 0.6)
}

/// Measure the monospace cell by shaping a reference glyph once at startup.
fn measure_cell(font_system: &mut FontSystem, family_name: Option<&str>) -> (f32, f32) {
    let mut buffer = glyphon::Buffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
    let advance = first_glyph_advance(font_system, &mut buffer, family_name, "M");
    (advance, LINE_HEIGHT)
}

/// Shape each printable ASCII character once and return its advance as a
/// fraction of the font size, indexed by codepoint.
///
/// The primary face is monospace, so in practice these are all the same
/// number — but it's the font's real number rather than the 0.6 guess petal-ui
/// falls back to, and the table shape is what a proportional face would need.
/// Control codes measure 0 (they draw nothing and must add no width).
fn measure_ascii_advances(font_system: &mut FontSystem, family_name: Option<&str>) -> Vec<f64> {
    let mut buffer = glyphon::Buffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
    let mut ratios = vec![0.0f64; 128];
    for cp in 0x20u32..0x7f {
        let ch = char::from_u32(cp).expect("ASCII is valid UTF-32");
        let advance = first_glyph_advance(font_system, &mut buffer, family_name, &ch.to_string());
        ratios[cp as usize] = (advance / FONT_SIZE) as f64;
    }
    ratios
}

#[cfg(test)]
mod tests {
    use super::*;
    use glyphon::fontdb;

    #[test]
    fn ascii_advance_ratios_match_the_measured_cell() {
        let ratios = measure_ascii_advances_standalone();
        let (advance, _) = measure_cell_standalone();
        let expected = (advance / FONT_SIZE) as f64;
        // The primary face is monospace, so every printable glyph advances the
        // same amount — the cell width the editor lays out with.
        for cp in 0x20..0x7f {
            assert!(
                (ratios[cp] - expected).abs() < 1e-6,
                "codepoint {cp:#x} advance {} != cell {expected}",
                ratios[cp]
            );
        }
        // Control codes draw nothing and must add no width.
        assert_eq!(ratios[0x09], 0.0);
        // A real measurement, not the 0.6 fallback guess.
        assert!(
            expected > 0.4 && expected < 0.8,
            "implausible advance ratio {expected}"
        );
    }

    /// Rasterize the first glyph of `text` at `size` and return the pixel
    /// height of the resulting bitmap. This is the raster stage only — the
    /// same path `TextRenderer::prepare` takes to fill the glyph atlas.
    fn raster_height(
        fs: &mut FontSystem,
        swash: &mut SwashCache,
        family: Option<&str>,
        text: &str,
        size: f32,
    ) -> u32 {
        let mut buffer = glyphon::Buffer::new(fs, Metrics::new(size, size * LINE_HEIGHT_RATIO));
        buffer.set_text(fs, text, &TextStack::attrs(family), Shaping::Advanced, None);
        buffer.shape_until_scroll(fs, false);
        let key = buffer
            .layout_runs()
            .next()
            .expect("one layout run")
            .glyphs
            .first()
            .expect("at least one glyph")
            .physical((0.0, 0.0), 1.0)
            .cache_key;
        swash
            .get_image_uncached(fs, key)
            .expect("glyph rasterizes")
            .placement
            .height
    }

    /// A glyph must rasterize at the size it was asked for, no matter how many
    /// other sizes the process has already drawn.
    ///
    /// swash caches hinting instances in a fixed-size table
    /// (`MAX_CACHED_HINT_INSTANCES = 8`). Through swash 0.2.8, evicting an entry
    /// reconfigured the instance to the new size but left the entry's recorded
    /// `size` at the old value, so the *next* request for that old size matched
    /// the stale entry and rasterized at the wrong size — while layout, which
    /// never consults the hinting cache, kept reporting the right advances. The
    /// visible result was text drawn at a stale size on correct advances once a
    /// panel used more than eight distinct sizes, with `/scene` still reporting
    /// the requested size. Fixed in swash 0.2.10; this pins it.
    #[test]
    fn glyphs_rasterize_at_their_own_size_past_the_hinting_cache_limit() {
        // Comfortably more than MAX_CACHED_HINT_INSTANCES, so the table is
        // forced to evict; a panel with a real type scale hits this easily.
        const SIZES: [f32; 12] = [
            10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 40.0, 48.0, 56.0,
        ];

        let (mut fs, family) = build_font_system(false);
        let mut swash = SwashCache::new();

        // Cold pass: every size is seen for the first time.
        let cold: Vec<u32> = SIZES
            .iter()
            .map(|&s| raster_height(&mut fs, &mut swash, family.as_deref(), "H", s))
            .collect();

        // Warm pass: the hinting table is now full, so each of these lookups
        // goes through the eviction path that used to corrupt the entry.
        let warm: Vec<u32> = SIZES
            .iter()
            .map(|&s| raster_height(&mut fs, &mut swash, family.as_deref(), "H", s))
            .collect();

        assert_eq!(
            cold, warm,
            "a glyph rasterized to a different size on a warm hinting cache: \
             cold={cold:?} warm={warm:?}"
        );

        // And the raster must actually track the requested size, in both passes
        // — equal-but-wrong would satisfy the check above.
        for pass in [&cold, &warm] {
            for w in pass.windows(2) {
                assert!(
                    w[1] >= w[0],
                    "raster height must not shrink as size grows: {pass:?}"
                );
            }
            assert!(
                pass[pass.len() - 1] > pass[0] * 2,
                "56px must rasterize much taller than 10px: {pass:?}"
            );
        }
    }

    /// A brand-new glyph introduced into a warm cache must also come out at its
    /// own size — the exact shape of the Garden bug (a panel hot-reloads, new
    /// characters appear, and they rasterize at a stale size).
    #[test]
    fn a_new_glyph_in_a_warm_cache_rasterizes_at_its_own_size() {
        const SIZES: [f32; 10] = [10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 32.0, 44.0, 56.0];
        let (mut fs, family) = build_font_system(false);
        let mut swash = SwashCache::new();

        // Warm every size on one glyph, filling the hinting table.
        for &s in &SIZES {
            raster_height(&mut fs, &mut swash, family.as_deref(), "H", s);
        }

        // Now a different glyph at the same sizes, and a reference measurement
        // of that glyph taken on a completely cold cache.
        let warm: Vec<u32> = SIZES
            .iter()
            .map(|&s| raster_height(&mut fs, &mut swash, family.as_deref(), "B", s))
            .collect();

        let (mut cold_fs, cold_family) = build_font_system(false);
        let mut cold_swash = SwashCache::new();
        let cold: Vec<u32> = SIZES
            .iter()
            .map(|&s| {
                raster_height(
                    &mut cold_fs,
                    &mut cold_swash,
                    cold_family.as_deref(),
                    "B",
                    s,
                )
            })
            .collect();

        assert_eq!(
            cold, warm,
            "a new glyph rasterized differently in a warm process: \
             cold={cold:?} warm={warm:?}"
        );
    }

    /// Shape `text` and return the `(font_id, glyph_id)` of its first glyph.
    /// A `glyph_id` of 0 is the notdef ("tofu") box; a `font_id` other than the
    /// primary face's means cosmic-text fell back to a covering font.
    fn first_glyph(fs: &mut FontSystem, family: Option<&str>, text: &str) -> (fontdb::ID, u16) {
        let mut buffer = glyphon::Buffer::new(fs, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        buffer.set_text(fs, text, &TextStack::attrs(family), Shaping::Advanced, None);
        buffer.shape_until_scroll(fs, false);
        let run = buffer.layout_runs().next().expect("one layout run");
        let glyph = run.glyphs.first().expect("at least one glyph");
        (glyph.font_id, glyph.glyph_id)
    }

    /// True when at least one fallback font is installed in this environment;
    /// the CJK/emoji assertions are gated on this so CI without the fonts skips
    /// rather than fails.
    fn fallbacks_available() -> bool {
        FALLBACK_FONT_CANDIDATES
            .iter()
            .any(|p| std::path::Path::new(p).exists())
    }

    #[test]
    fn latin_uses_primary_face_and_fallbacks_do_not_change_advance() {
        let (mut fs, family) = build_font_system(true);
        let (_a_font, a_glyph) = first_glyph(&mut fs, family.as_deref(), "A");
        assert_ne!(a_glyph, 0, "'A' must resolve to a real glyph, not notdef");

        // The mono advance must be identical whether or not fallback fonts are
        // loaded — panel/editor column alignment depends on it staying stable.
        let (adv_with, lh) = measure_cell(&mut fs, family.as_deref());
        let (mut fs_bare, fam_bare) = build_font_system(false);
        let (adv_without, _) = measure_cell(&mut fs_bare, fam_bare.as_deref());
        assert!(adv_with > 0.0 && lh > 0.0);
        assert_eq!(
            adv_with, adv_without,
            "loading fallback fonts must not shift the monospace advance"
        );
    }

    #[test]
    fn cjk_falls_back_to_a_real_glyph() {
        if !fallbacks_available() {
            eprintln!("skipping cjk_falls_back_to_a_real_glyph: no fallback font on this system");
            return;
        }
        let (mut fs, family) = build_font_system(true);
        let (primary_font, _) = first_glyph(&mut fs, family.as_deref(), "A");

        // U+4E2D 中 — a Han ideograph absent from JetBrains Mono.
        let (cjk_font, cjk_glyph) = first_glyph(&mut fs, family.as_deref(), "中");
        assert_ne!(
            cjk_glyph, 0,
            "CJK must resolve to a real (non-notdef) glyph instead of tofu"
        );
        assert_ne!(
            cjk_font, primary_font,
            "CJK glyph must come from a fallback face, not the primary mono font"
        );
    }

    #[test]
    fn no_fallbacks_loaded_keeps_cjk_as_notdef_without_panicking() {
        // With fallbacks disabled the primary face has no CJK coverage, so the
        // codepoint must resolve to notdef (0) from the primary face — the exact
        // pre-fallback behavior, proving the guard degrades cleanly.
        let (mut fs, family) = build_font_system(false);
        let (primary_font, _) = first_glyph(&mut fs, family.as_deref(), "A");
        let (cjk_font, cjk_glyph) = first_glyph(&mut fs, family.as_deref(), "中");
        assert_eq!(cjk_glyph, 0, "no fallback loaded => CJK is notdef");
        assert_eq!(cjk_font, primary_font, "notdef comes from the primary face");
    }
}
