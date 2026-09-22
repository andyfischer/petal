//! Font metrics and text measurement.
//!
//! The [`draw`](crate::draw) vocabulary can *name* a face, a weight, a slant
//! and a letter-spacing on a `text` command; this module is what lets a script
//! know how wide the result will be. Hosts publish measurement data for the
//! faces they can actually render ([`bind_text_metrics`],
//! [`bind_text_advance_table`], [`bind_font_metrics`],
//! [`bind_font_variant_metrics`], [`bind_default_font_name`]); the `text_width`
//! native resolves a [`TextStyle`] against that registry, degrading to the
//! default font when the host lacks a face, so a script measures the same
//! metrics that will be rasterized.

use std::cell::RefCell;
use std::collections::HashMap;

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

use crate::draw::{num_as_f64, num_as_i64};

/// Uniform read by the default `text_width`: monospace advance as a fraction
/// of the font size. See [`bind_text_metrics`].
pub const SYM_TEXT_ADVANCE: &str = "text_advance";

/// Fallback advance ratio when the host hasn't bound one — a typical
/// monospace glyph advances ~0.6× the font size.
pub const DEFAULT_TEXT_ADVANCE: f64 = 0.6;

/// Per-glyph advance table read by `text_width` for proportional fonts:
/// a list of advance-÷-size ratios indexed by Unicode codepoint. When bound,
/// `text_width` sums per-glyph advances instead of `chars × size × ratio`. A
/// codepoint beyond the table's length falls back to [`SYM_TEXT_ADVANCE`].
pub const SYM_TEXT_ADVANCES: &str = "text_advances";

/// The face name the host's *default* font is also registered under, so a
/// style that names no `font` can still find the default face's bold or italic
/// variant: `text_width(s, {weight: 700})` resolves `"<default>@700"`. Hosts
/// set it with [`bind_default_font_name`]; without it, a font-less style
/// measures the plain default metrics whatever its weight.
pub const SYM_TEXT_DEFAULT_FONT: &str = "text_default_font";

/// Per-font metrics read by `text_width(s, size, font)`: a record keyed by
/// font name, each value a record `{advance: float, advances: [float]}` with
/// the same meaning as the default-font [`SYM_TEXT_ADVANCE`] /
/// [`SYM_TEXT_ADVANCES`] bindings. See [`bind_font_metrics`].
pub const SYM_TEXT_FONTS: &str = "text_fonts";

/// Per-font *vertical* metrics for the host's default face:
/// `{baseline, descent, line_height, cap_height, x_height}`, each a fraction
/// of the font size. See [`bind_text_vertical_metrics`] and
/// [`VerticalMetrics`].
pub const SYM_TEXT_VERTICAL: &str = "text_vertical";

/// CSS regular weight — the weight every pre-typography `text` command means.
pub const REGULAR_WEIGHT: u16 = 400;

/// Font size a style record without a `size` field draws at. Styles normally
/// name their size; this only keeps `{color: FG}` from being an error.
pub const DEFAULT_TEXT_SIZE: i64 = 14;

/// Bind the monospace text metric read by the default `text_width` native:
/// the glyph advance as a fraction of the font size (a typical monospace at
/// size 14 advances 8.4 px → ratio 0.6). Hosts with real text shaping can
/// instead register their own `text_width` native before
/// [`register_draw`](crate::draw::register_draw).
pub fn bind_text_metrics(env: &mut Env, advance_ratio: f64) {
    let s = env.intern_symbol(SYM_TEXT_ADVANCE);
    env.set_binding(s, Value::Float(advance_ratio));
}

/// Bind the per-glyph advance table read by the proportional `text_width`:
/// `ratios[codepoint]` is that glyph's advance as a fraction of the font size,
/// measured by the host from its actual font. Codepoints past the table's end
/// fall back to the uniform [`bind_text_metrics`] ratio. Binding this is what
/// lets a script measure a proportional glyph run correctly (centered /
/// right-aligned layout), instead of assuming monospace.
pub fn bind_text_advance_table(env: &mut Env, ratios: &[f64]) {
    let list: Vec<Value> = ratios.iter().map(|r| Value::Float(*r)).collect();
    let id = env.heap_mut().alloc_list(list);
    let s = env.intern_symbol(SYM_TEXT_ADVANCES);
    env.set_binding(s, Value::List(id));
}

/// Measurement data for one font, as ratios of the font size (so one table
/// serves every size — glyph advance scales linearly with size).
#[derive(Clone, Debug, PartialEq)]
pub struct FontMetrics {
    /// Advance ratio used for codepoints the table doesn't cover.
    pub advance: f64,
    /// `advances[codepoint]` = that glyph's advance ÷ font size. May be empty
    /// (a monospace font is fully described by `advance` alone).
    pub advances: Vec<f64>,
    /// Where this face's ink sits relative to a run's `y`. See
    /// [`VerticalMetrics`]; a host that publishes nothing gets the default
    /// UI-sans proportions.
    pub vertical: VerticalMetrics,
}

/// Where the ink of a text run sits relative to the `y` a script hands
/// `draw_text` — the half of measurement that has been missing, and the reason
/// every vertically centred label in every Petal UI lands a pixel or two high.
///
/// Every field is a **fraction of the font size**, so one record serves every
/// size, and every field is measured from the run's own `y` or from its
/// baseline — never from an em box the host may not use:
///
/// ```text
///   y ──────────────────────────── the point draw_text was given
///     │  ▲ baseline
///     │  │            ┌───┐   ▲ cap_height
///     │  │      ┌──┐  │   │   │        ▲ x_height
///     │  ▼      │  │  │   │   │        │
///   baseline ───┴──┴──┴───┴───▼────────▼
///     │  ▲ descent      │
///     │  ▼              g
///   ───────────────────────────── y + line_height (the next line's y)
/// ```
///
/// `baseline` is the one a host must get right: it is the *rendered* distance
/// from `y` down to the baseline, whatever the host's own convention is (SDL
/// blits a surface whose top is the ascent line; a canvas with
/// `textBaseline = "top"` uses its font bounding box; Garden lays a run out in
/// a line box with leading above it). A script never has to know which — it
/// asks for `baseline` and gets the truth about this host.
///
/// `cap_height` is what UI centring actually wants: a label like "Save" has no
/// descender, so centring its *line box* leaves it looking high. Centring the
/// cap height is what a designer means by "centred in the button".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalMetrics {
    /// `y` → baseline, as this host renders a run. ÷ font size.
    pub baseline: f64,
    /// Baseline → the bottom of the line box (positive, going down). ÷ size.
    pub descent: f64,
    /// One line's `y` → the next line's `y` for comfortable setting,
    /// including any line gap the face asks for. ÷ size.
    pub line_height: f64,
    /// Baseline → the top of a flat capital ("H"). ÷ size.
    pub cap_height: f64,
    /// Baseline → the top of a lowercase "x". ÷ size.
    pub x_height: f64,
}

impl Default for VerticalMetrics {
    /// A typical UI sans at its usual proportions. These are the numbers a
    /// script measures on a host that has published nothing — close enough
    /// that centring with them beats the "the run is `size` px tall"
    /// assumption they replace, and wrong enough that a host should publish
    /// its own.
    fn default() -> Self {
        VerticalMetrics {
            baseline: 0.8,
            descent: 0.2,
            line_height: 1.2,
            cap_height: 0.7,
            x_height: 0.52,
        }
    }
}

impl VerticalMetrics {
    /// The metrics of a face described the way a font file describes itself:
    /// ascent and descent above and below the baseline, plus the line gap
    /// between one line's descent and the next line's ascent. Cap and x
    /// heights are estimated from the ascent when the host cannot measure
    /// them; a host that can should set them.
    pub fn from_ascent_descent(ascent: f64, descent: f64, line_gap: f64) -> Self {
        VerticalMetrics {
            baseline: ascent,
            descent,
            line_height: ascent + descent + line_gap,
            cap_height: ascent * 0.92,
            x_height: ascent * 0.68,
        }
    }

    /// The same with a measured cap and x height — what a host that can ask
    /// its rasterizer for the extents of "H" and "x" should publish.
    pub fn with_heights(self, cap_height: f64, x_height: f64) -> Self {
        VerticalMetrics {
            cap_height,
            x_height,
            ..self
        }
    }
}

impl Default for FontMetrics {
    /// A typical monospace face: every glyph advances 0.6× the size.
    fn default() -> Self {
        FontMetrics {
            advance: DEFAULT_TEXT_ADVANCE,
            advances: Vec::new(),
            vertical: VerticalMetrics::default(),
        }
    }
}

impl FontMetrics {
    /// A proportional font described by a codepoint-indexed advance table,
    /// with `advance` covering codepoints past the table's end.
    pub fn proportional(advances: Vec<f64>, advance: f64) -> Self {
        FontMetrics {
            advance,
            advances,
            vertical: VerticalMetrics::default(),
        }
    }

    /// A monospace font: one advance ratio for every glyph.
    pub fn monospace(advance: f64) -> Self {
        FontMetrics {
            advance,
            advances: Vec::new(),
            vertical: VerticalMetrics::default(),
        }
    }

    /// The same metrics with this face's real vertical proportions attached.
    /// Every constructor here starts from [`VerticalMetrics::default`], so a
    /// host that measures its face calls this and a host that cannot is
    /// unchanged.
    pub fn with_vertical(self, vertical: VerticalMetrics) -> Self {
        FontMetrics { vertical, ..self }
    }

    fn width_of(&self, text: &str, size: f64) -> f64 {
        text.chars()
            .map(|c| {
                self.advances
                    .get(c as usize)
                    .copied()
                    .unwrap_or(self.advance)
                    * size
            })
            .sum()
    }
}

/// Bind measurement data for a *named* font, so a script can measure text in a
/// face other than the host's default: `text_width(s, size, "mono")`. Hosts
/// register one entry per face they can render, under the role names scripts
/// select by (`ui`, `mono`, `serif`) and/or concrete family names. The
/// unnamed default font stays with [`bind_text_metrics`] /
/// [`bind_text_advance_table`]; a name the host never bound falls back to it,
/// so a script asking for a face this host lacks degrades instead of breaking.
pub fn bind_font_metrics(env: &mut Env, font: &str, metrics: &FontMetrics) {
    let advances: Vec<Value> = metrics.advances.iter().map(|r| Value::Float(*r)).collect();
    let advances_id = env.heap_mut().alloc_list(advances);
    let mut entry = indexmap::IndexMap::new();
    entry.insert("advance".to_string(), Value::Float(metrics.advance));
    entry.insert("advances".to_string(), Value::List(advances_id));
    for (key, value) in vertical_fields(&metrics.vertical) {
        entry.insert(key.to_string(), Value::Float(value));
    }
    let entry_id = env.heap_mut().alloc_map(entry);

    let sym = env.intern_symbol(SYM_TEXT_FONTS);
    let mut fonts = match env.binding(sym) {
        Some(Value::Map(id)) => env.heap().get_map(id).clone(),
        _ => indexmap::IndexMap::new(),
    };
    fonts.insert(font.to_string(), Value::Map(entry_id));
    let fonts_id = env.heap_mut().alloc_map(fonts);
    env.set_binding(sym, Value::Map(fonts_id));
}

/// Bind measurement data for one *variant* of a face — the bold, the italic,
/// the bold-italic — so a style's `weight`/`italic` measures the metrics that
/// will actually be rasterized (bold is wider than regular in most faces).
/// Sugar over [`bind_font_metrics`] with the canonical variant key.
///
/// A host binds only the variants it really has. Measurement then degrades the
/// way rendering does: a style asking for bold on a host with one weight
/// measures — and draws — the regular face, rather than erroring or silently
/// using another family's bold. See [`font_variant_key`] for the match order.
pub fn bind_font_variant_metrics(
    env: &mut Env,
    font: &str,
    weight: u16,
    italic: bool,
    metrics: &FontMetrics,
) {
    bind_font_metrics(env, &font_variant_key(font, weight, italic), metrics);
}

/// The key one face variant is registered under: `"ui"`, `"ui@700"`, `"ui@i"`,
/// `"ui@700i"`. Regular upright is the bare name, so a host that binds one
/// face per family writes exactly what it wrote before typography existed.
///
/// Lookup walks a family's variants most-specific first — `ui@700i`, `ui@700`,
/// `ui@i`, `ui` — before moving to the next family in a fallback list, which
/// is CSS's family-then-variant order.
pub fn font_variant_key(font: &str, weight: u16, italic: bool) -> String {
    match (weight == REGULAR_WEIGHT, italic) {
        (true, false) => font.to_string(),
        (true, true) => format!("{font}@i"),
        (false, false) => format!("{font}@{weight}"),
        (false, true) => format!("{font}@{weight}i"),
    }
}

/// Name the role the host's default font *is*, so a style with no `font` can
/// still resolve that face's variants. A host whose default font is its `ui`
/// role calls `bind_default_font_name(env, "ui")`; then
/// `text_width(s, {weight: 700})` — no face named — measures `ui@700` if the
/// host bound one, instead of quietly measuring regular.
///
/// Drawing already behaves this way (a font-less bold command renders in the
/// default face, bold), so without this the two sides disagree exactly where
/// it is least visible: a bold label with no explicit face.
/// Bind the *vertical* metrics of the host's default face, so a script can
/// place a run rather than guessing that its ink is `size` px tall starting at
/// `y`. The counterpart to [`bind_text_metrics`] on the other axis, and read
/// by the `text_metrics` native.
///
/// A host that never calls this publishes nothing and scripts measure
/// [`VerticalMetrics::default`] — typical UI-sans proportions, which are much
/// closer than the assumption they replace but are still a guess about someone
/// else's font. Any host that can ask its rasterizer where the baseline lands
/// should call this.
pub fn bind_text_vertical_metrics(env: &mut Env, vertical: &VerticalMetrics) {
    let mut fields = indexmap::IndexMap::new();
    for (key, value) in vertical_fields(vertical) {
        fields.insert(key.to_string(), Value::Float(value));
    }
    let id = env.heap_mut().alloc_map(fields);
    let sym = env.intern_symbol(SYM_TEXT_VERTICAL);
    env.set_binding(sym, Value::Map(id));
}

/// One vertical record as name/value pairs — the single spelling of these
/// field names, shared by the default-face binding and the per-face registry
/// so the two can never drift apart.
fn vertical_fields(v: &VerticalMetrics) -> [(&'static str, f64); 5] {
    [
        ("baseline", v.baseline),
        ("descent", v.descent),
        ("line_height", v.line_height),
        ("cap_height", v.cap_height),
        ("x_height", v.x_height),
    ]
}

/// Read a vertical record back out of a script-side map, filling any field the
/// host left out from `fallback`. A host that publishes only a baseline gets
/// sensible proportions around it rather than zeros.
fn vertical_from_map(
    map: &indexmap::IndexMap<String, Value>,
    fallback: VerticalMetrics,
) -> VerticalMetrics {
    let get = |key: &str, default: f64| map.get(key).map_or(default, |v| num_or(v, default));
    VerticalMetrics {
        baseline: get("baseline", fallback.baseline),
        descent: get("descent", fallback.descent),
        line_height: get("line_height", fallback.line_height),
        cap_height: get("cap_height", fallback.cap_height),
        x_height: get("x_height", fallback.x_height),
    }
}

pub fn bind_default_font_name(env: &mut Env, font: &str) {
    let sym = env.intern_symbol(SYM_TEXT_DEFAULT_FONT);
    let id = env.heap_mut().alloc_string(font.to_string());
    env.set_binding(sym, Value::String(id));
}

/// The keys to try, in order, for one family at a given weight/style: the
/// asked-for variant, then the fallbacks that drop first the slant, then the
/// weight. Variants that collapse onto one already listed (regular weight,
/// upright, or both) are dropped, so the list is 1–4 distinct keys.
fn font_variant_candidates(font: &str, weight: u16, italic: bool) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (w, i) in [
        (weight, italic),
        (weight, false),
        (REGULAR_WEIGHT, italic),
        (REGULAR_WEIGHT, false),
    ] {
        let key = font_variant_key(font, w, i);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// Read a numeric `Value` as f64, or `default` if it isn't a number.
fn num_or(v: &Value, default: f64) -> f64 {
    match v {
        Value::Float(f) => *f,
        Value::Int(n) => *n as f64,
        _ => default,
    }
}

/// Decode a list `Value` of advance ratios into a table.
fn advance_list(state: &mut PetalCxt, v: &Value, uniform: f64) -> Option<Vec<f64>> {
    match v {
        Value::List(id) => Some(
            state
                .heap()
                .get_list(*id)
                .iter()
                .map(|r| num_or(r, uniform))
                .collect(),
        ),
        _ => None,
    }
}

/// The host's default-font metrics — the [`bind_text_metrics`] /
/// [`bind_text_advance_table`] bindings every host has always used.
fn default_font_metrics(state: &mut PetalCxt) -> FontMetrics {
    let uniform = num_or(&state.binding_named(SYM_TEXT_ADVANCE), DEFAULT_TEXT_ADVANCE);
    let table = state.binding_named(SYM_TEXT_ADVANCES);
    let advances = advance_list(state, &table, uniform);
    let vertical = match state.binding_named(SYM_TEXT_VERTICAL) {
        Value::Map(id) => {
            let map = state.heap().get_map(id).clone();
            vertical_from_map(&map, VerticalMetrics::default())
        }
        _ => VerticalMetrics::default(),
    };
    FontMetrics {
        advance: uniform,
        advances: advances.unwrap_or_default(),
        vertical,
    }
}

/// The default face's vertical metrics, or the built-in proportions when the
/// host published none.
fn default_vertical(state: &mut PetalCxt) -> VerticalMetrics {
    match state.binding_named(SYM_TEXT_VERTICAL) {
        Value::Map(id) => {
            let map = state.heap().get_map(id).clone();
            vertical_from_map(&map, VerticalMetrics::default())
        }
        _ => VerticalMetrics::default(),
    }
}

/// Resolve a font spec — a name or a CSS-style fallback list (`"Inter, ui"`) —
/// against the [`bind_font_metrics`] registry. The first name the host bound
/// wins; if none did, the caller falls back to the default font.
fn named_font_metrics(
    state: &mut PetalCxt,
    spec: &str,
    weight: u16,
    italic: bool,
) -> Option<FontMetrics> {
    let fonts = match state.binding_named(SYM_TEXT_FONTS) {
        Value::Map(id) => state.heap().get_map(id).clone(),
        _ => return None,
    };
    for name in spec.split(',') {
        // Family first, then variant within it: a host that has this family's
        // bold measures the bold; one that only has its regular measures that
        // rather than jumping to the next family.
        for key in font_variant_candidates(name.trim(), weight, italic) {
            let Some(Value::Map(entry_id)) = fonts.get(&key) else {
                continue;
            };
            let entry = state.heap().get_map(*entry_id).clone();
            let advance = entry
                .get("advance")
                .map_or(DEFAULT_TEXT_ADVANCE, |v| num_or(v, DEFAULT_TEXT_ADVANCE));
            let advances = entry
                .get("advances")
                .cloned()
                .and_then(|v| advance_list(state, &v, advance));
            // A face registered before vertical metrics existed carries none,
            // so its entry falls back field by field to the default face's —
            // the same direction every other lookup here degrades in.
            let vertical = vertical_from_map(&entry, default_vertical(state));
            return Some(FontMetrics {
                advance,
                advances: advances.unwrap_or_default(),
                vertical,
            });
        }
    }
    None
}

/// What a host can tell a script about the faces it can draw.
///
/// The [`bind_font_metrics`] registry answers for the handful of faces a host
/// publishes up front — its roles, its embedded faces. That is the wrong shape
/// for "any font installed on this machine": there can be hundreds, measuring
/// one means shaping every glyph in it, and a script typically wants two. So a
/// host that can reach a real font database attaches a `FontSource` instead
/// and is asked, lazily, only about the faces a script actually names.
///
/// The pre-existing registry still wins when it has an answer, so a host can
/// keep publishing its default roles eagerly and let the source cover the rest.
/// A host with no source at all is unchanged: `font(name)` hands back the name
/// it was given, and measuring it falls through to the default font.
pub trait FontSource {
    /// The canonical family name for `name` — the spelling this host will
    /// shape with — or `None` if it cannot draw that family. `name` may be a
    /// CSS-style fallback list (`"Helvetica, ui"`).
    ///
    /// Returning the *canonical* name is what keeps measuring and drawing in
    /// step: it is the name that goes into the style record, so the same
    /// string reaches [`metrics`](Self::metrics) here and the rasterizer
    /// later.
    fn resolve(&mut self, name: &str) -> Option<String>;

    /// ASCII advance ratios for one cut of a resolved family. `None` when the
    /// host cannot measure it, which degrades to the default font's metrics.
    fn metrics(&mut self, family: &str, weight: u16, italic: bool) -> Option<FontMetrics>;

    /// Every family a script could name here, for a font picker or a
    /// diagnostic. May be empty.
    fn families(&mut self) -> Vec<String>;
}

/// A host's attached [`FontSource`], owned between frames and swapped into the
/// thread-local channel for the duration of `env.run` — the same shape as
/// [`crate::host_data::swap_data_provider`], and for the same reason: natives
/// are plain fn pointers with no place to hang host state.
pub type FontProvider = Box<dyn FontSource>;

thread_local! {
    static FONT_PROVIDER: RefCell<Option<FontProvider>> = const { RefCell::new(None) };
    /// Measurements already taken from the provider this process, keyed by
    /// `(family, weight, italic)`. Measuring a face is expensive and its
    /// answer cannot change while the process runs, so it is remembered
    /// across frames — a `text_width` in a 60fps draw loop must not re-measure
    /// a font every frame. `None` records "asked, and the host had no answer",
    /// so a miss is not re-asked either.
    static FONT_METRICS_CACHE: RefCell<HashMap<(String, u16, bool), Option<FontMetrics>>> =
        RefCell::new(HashMap::new());
    /// Names already resolved through the provider, for the same reason.
    static FONT_NAME_CACHE: RefCell<HashMap<String, Option<String>>> =
        RefCell::new(HashMap::new());
}

/// Install `provider` as the font source the `font` / `fonts` / `text_width`
/// natives consult, returning whatever was there. Hosts swap theirs in around
/// `env.run` and take it back afterwards, so a panic in the script cannot
/// strand it.
pub fn swap_font_provider(provider: Option<FontProvider>) -> Option<FontProvider> {
    FONT_PROVIDER.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), provider))
}

/// Run `f` with the provider borrowed out of the channel, or return `None` if
/// no host attached one. The provider is moved out for the call and put back
/// afterwards, so a provider that itself calls back into Petal cannot alias it.
fn with_font_provider<T>(f: impl FnOnce(&mut dyn FontSource) -> T) -> Option<T> {
    let mut provider = swap_font_provider(None)?;
    let out = f(provider.as_mut());
    swap_font_provider(Some(provider));
    Some(out)
}

/// The canonical family name for `spec`, via the host's font source. `None`
/// when there is no source, or the source cannot draw any name in `spec`.
fn resolve_font_name(spec: &str) -> Option<String> {
    if let Some(hit) = FONT_NAME_CACHE.with(|c| c.borrow().get(spec).cloned()) {
        return hit;
    }
    let resolved = with_font_provider(|p| p.resolve(spec)).flatten();
    FONT_NAME_CACHE.with(|c| {
        c.borrow_mut().insert(spec.to_string(), resolved.clone());
    });
    resolved
}

/// Metrics for one cut of `family` from the host's font source, memoized.
fn provider_font_metrics(family: &str, weight: u16, italic: bool) -> Option<FontMetrics> {
    let key = (family.to_string(), weight, italic);
    if let Some(hit) = FONT_METRICS_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let metrics = with_font_provider(|p| p.metrics(family, weight, italic)).flatten();
    FONT_METRICS_CACHE.with(|c| {
        c.borrow_mut().insert(key, metrics.clone());
    });
    metrics
}

/// Forget everything learned from a font source. Only useful in tests, where
/// one process attaches several different sources and must not see an earlier
/// one's answers.
pub fn clear_font_cache() {
    FONT_NAME_CACHE.with(|c| c.borrow_mut().clear());
    FONT_METRICS_CACHE.with(|c| c.borrow_mut().clear());
}

/// `font(name) -> style`: a **font object** — the style record `draw_text`,
/// `draw_text_center`, `text_width` and the widget `style` arguments all take,
/// carrying nothing but the face.
///
/// Naming a face by object rather than by string is what lets a size, a
/// weight, a slant or a letter-spacing travel with it: the prelude's
/// `font_size` / `font_bold` / `font_italic` / `font_spacing` / `font_color`
/// return the same record with one more field set, so one value describes the
/// whole appearance of a run and measuring it cannot drift from drawing it.
///
/// The recorded name is the host's *canonical* spelling when it recognizes the
/// family, so `font("helvetica")` and `font("Helvetica")` produce the same
/// object and the rasterizer gets a name it can match. A face this host cannot
/// draw keeps the name as written and measures with the default font — the
/// same direction rendering degrades in.
pub(crate) fn native_font(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let resolved = resolve_font_name(&name).unwrap_or(name);
    let id = state.heap_mut().alloc_string(resolved);
    let mut fields = indexmap::IndexMap::new();
    fields.insert("font".to_string(), Value::String(id));
    let map = state.heap_mut().alloc_map(fields);
    state.push_value(Value::Map(map));
    Ok(1)
}

/// `fonts() -> [string]`: the family names this host can draw, for a font
/// picker. Empty when the host has attached no [`FontSource`] — a script
/// should treat that as "this host offers only its own faces", not as an
/// error.
pub(crate) fn native_fonts(state: &mut PetalCxt) -> NativeResult {
    // The list comes from the host's font source, not the binding table.
    state.note_host_read();
    let names = with_font_provider(|p| p.families()).unwrap_or_default();
    let items: Vec<Value> = names
        .into_iter()
        .map(|n| Value::String(state.heap_mut().alloc_string(n)))
        .collect();
    state.push_list(items);
    Ok(1)
}

/// [`named_font_metrics`] against the host's [`FontSource`] instead of the
/// published registry: the first name in the fallback list the host can both
/// resolve and measure wins.
fn source_font_metrics(spec: &str, weight: u16, italic: bool) -> Option<FontMetrics> {
    for name in spec.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let Some(family) = resolve_font_name(name) else {
            continue;
        };
        if let Some(metrics) = provider_font_metrics(&family, weight, italic) {
            return Some(metrics);
        }
    }
    None
}

/// One text style, as a script writes it: a record of any subset of
/// `{size, color, font, weight, italic, spacing}`. Missing fields take the
/// defaults that describe every pre-typography `draw_text` — the host's own
/// font, upright, regular weight, no letter-spacing — so a partial style is a
/// diff against "plain text", not a half-specified command.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub size: i64,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub font: Option<String>,
    pub weight: u16,
    pub italic: bool,
    pub spacing: f64,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            size: DEFAULT_TEXT_SIZE,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            font: None,
            weight: REGULAR_WEIGHT,
            italic: false,
            spacing: 0.0,
        }
    }
}

impl TextStyle {
    /// Decode a style record from a script value. `color` is a `{r, g, b, [a]}`
    /// record — the same shape the prelude's record draw overloads take — so a
    /// theme color drops straight into a style.
    pub(crate) fn from_value(state: &mut PetalCxt, v: &Value) -> Result<TextStyle, String> {
        let Value::Map(id) = v else {
            return Err(format!(
                "text style must be a record, got {}",
                v.type_name()
            ));
        };
        let fields = state.heap().get_map(*id).clone();
        let mut style = TextStyle::default();
        if let Some(size) = fields.get("size").and_then(num_as_i64) {
            style.size = size;
        }
        if let Some(Value::Map(color_id)) = fields.get("color") {
            let color = state.heap().get_map(*color_id).clone();
            let channel = |name: &str, default: u8| {
                color
                    .get(name)
                    .and_then(num_as_i64)
                    .map_or(default, |n| n as u8)
            };
            style.r = channel("r", 255);
            style.g = channel("g", 255);
            style.b = channel("b", 255);
            style.a = channel("a", 255);
        }
        if let Some(Value::String(font_id)) = fields.get("font") {
            style.font = Some(state.heap().get_string(*font_id).to_string());
        }
        if let Some(weight) = fields.get("weight").and_then(num_as_i64) {
            style.weight = weight as u16;
        }
        style.italic = matches!(fields.get("italic"), Some(Value::Bool(true)));
        if let Some(spacing) = fields.get("spacing").and_then(num_as_f64) {
            style.spacing = spacing;
        }
        Ok(style)
    }

    /// The emitted arg list for a `text` command in this style. The
    /// typography args are appended only when they differ from plain text, so
    /// an unstyled draw emits the byte-identical 8-arg command it always has.
    pub(crate) fn emit_args(
        &self,
        state: &mut PetalCxt,
        text: String,
        x: i64,
        y: i64,
    ) -> Vec<Value> {
        let mut args = vec![
            Value::String(state.heap_mut().alloc_string(text)),
            Value::Int(x),
            Value::Int(y),
            Value::Int(self.size),
            Value::Int(self.r as i64),
            Value::Int(self.g as i64),
            Value::Int(self.b as i64),
            Value::Int(self.a as i64),
        ];
        if self.font.is_none()
            && self.weight == REGULAR_WEIGHT
            && !self.italic
            && self.spacing == 0.0
        {
            return args;
        }
        args.push(match &self.font {
            Some(font) => {
                let id = state.heap_mut().alloc_string(font.clone());
                Value::String(id)
            }
            None => Value::Nil,
        });
        args.push(Value::Int(self.weight as i64));
        args.push(Value::Bool(self.italic));
        args.push(Value::Float(self.spacing));
        args
    }
}

/// `text_width(s, size, [font]) -> int`: width in logical px of `s` at font
/// `size`. If the host bound a per-glyph advance table
/// ([`bind_text_advance_table`]), the width is the sum of each glyph's advance
/// × `size` — correct for proportional fonts. Otherwise it falls back to the
/// monospace model `chars × size × ratio`, with the ratio from
/// [`bind_text_metrics`] (default 0.6).
///
/// The optional `font` selects a face registered with [`bind_font_metrics`],
/// by role name or CSS-style fallback list (`"Inter, ui"`). A face this host
/// doesn't offer measures with the default font.
///
/// `text_width(s, style)` measures a [`TextStyle`] record instead — the same
/// record `draw_text` takes, so what you measure is what you draw: the style's
/// face *and* weight/italic variant, plus its letter-spacing.
pub(crate) fn native_text_width(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let (style, metrics) = style_and_metrics(state, 2)?;
    state.push_int(run_width(&metrics, &style, &text).round() as i64);
    Ok(1)
}

/// `text_advance(s, style) -> float`: the same measurement as `text_width`,
/// unrounded. `text_width` rounds to a whole pixel because layouts built on it
/// rely on an integer; column math over a monospace face wants the true
/// advance instead — `text_width("m", {size: 13})` is 8 where the advance is
/// 7.8, which drifts a whole character by column 30.
pub(crate) fn native_text_advance(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let (style, metrics) = style_and_metrics(state, 2)?;
    state.push_float(run_width(&metrics, &style, &text));
    Ok(1)
}

/// The style argument at `index` (a style record, or a bare size optionally
/// followed by a face name at `index + 1`) together with the metrics it
/// resolves to. Every text native takes its style this way, so one call
/// measures, wraps and places against the same face.
fn style_and_metrics(
    state: &mut PetalCxt,
    index: usize,
) -> Result<(TextStyle, FontMetrics), String> {
    let style = match state.get_value(index)? {
        Value::Map(_) => TextStyle::from_value(state, &state.get_value(index)?)?,
        // A bare size, optionally followed by a face name. The face is
        // recognized by *being a string* rather than by position, because the
        // natives that take a width or an alignment after the style would
        // otherwise read one of those as a font name.
        _ => TextStyle {
            size: state.get_int(index)?,
            font: match state.arg_count() > index
                && matches!(state.get_value(index + 1)?, Value::String(_))
            {
                true => Some(state.get_string(index + 1)?),
                false => None,
            },
            ..TextStyle::default()
        },
    };
    let metrics = resolve_metrics(state, &style);
    Ok((style, metrics))
}

/// The metrics `style` will actually be drawn with on this host.
///
/// A style with no face still has a weight and a slant, and the host draws
/// those in its default face — so resolve that face's variants by the name the
/// host published (see [`bind_default_font_name`]) rather than measuring
/// regular metrics for bold text.
///
/// Registry first (what the host published up front), then its font source
/// (measured on demand), then the default font. A host that publishes a face
/// eagerly and also attaches a source gets the eager answer, so the two can
/// never disagree about the same name.
fn resolve_metrics(state: &mut PetalCxt, style: &TextStyle) -> FontMetrics {
    let spec = match &style.font {
        Some(spec) => Some(spec.clone()),
        None if style.weight != REGULAR_WEIGHT || style.italic => {
            match state.binding_named(SYM_TEXT_DEFAULT_FONT) {
                Value::String(id) => Some(state.heap().get_string(id).to_string()),
                _ => None,
            }
        }
        None => None,
    };
    match &spec {
        Some(spec) => named_font_metrics(state, spec, style.weight, style.italic)
            .or_else(|| source_font_metrics(spec, style.weight, style.italic))
            .unwrap_or_else(|| default_font_metrics(state)),
        None => default_font_metrics(state),
    }
}

/// The advance width of one run: the summed glyph advances plus the style's
/// letter-spacing after every glyph, exactly as the run will be drawn.
fn run_width(metrics: &FontMetrics, style: &TextStyle, text: &str) -> f64 {
    metrics.width_of(text, style.size as f64) + style.spacing * text.chars().count() as f64
}

/// `text_metrics(style) -> {size, baseline, descent, line_height, cap_height,
/// x_height}`: where the ink of a run in this style lands relative to the `y`
/// `draw_text` is given, in **pixels** at that style's size.
///
/// This is the measurement that makes vertical placement possible at all.
/// `text_width` has always answered "how wide", and every script has had to
/// guess the other axis — almost always as "the run is `size` px tall,
/// starting at `y`", which is wrong on every host: the run starts at its
/// ascent line, is taller than `size`, and its ink sits high inside that box.
/// A label centred on that guess lands a pixel or two high at 14 px and
/// visibly high at 32.
///
/// Centre a UI label on `cap_height` (a label with no descender looks high
/// when its *line box* is centred), set body text on `line_height`, and
/// convert between a baseline and a `draw_text` `y` with `baseline`. See
/// [`VerticalMetrics`] for the diagram.
///
/// `style` is the same argument `text_width` takes: a style record, or a bare
/// size with an optional face name.
pub(crate) fn native_text_metrics(state: &mut PetalCxt) -> NativeResult {
    let (style, metrics) = style_and_metrics(state, 1)?;
    let size = style.size as f64;
    let v = metrics.vertical;
    let mut fields = indexmap::IndexMap::new();
    fields.insert("size".to_string(), Value::Float(size));
    for (key, ratio) in vertical_fields(&v) {
        fields.insert(key.to_string(), Value::Float(ratio * size));
    }
    let id = state.heap_mut().alloc_map(fields);
    state.push_value(Value::Map(id));
    Ok(1)
}

// ── Fitting text into a box ───────────────────────────────────────────────
//
// Wrapping, ellipsizing and caret hit-testing are all the same loop: walk the
// glyphs, accumulate advances, stop at a width. A script *can* write that loop
// — the `ui` prelude's `ellipsize` does, one `text_width` call per character —
// but it pays a native call and a fresh string per step, which is the wrong
// shape for something a UI does on every label of every frame. These natives
// walk the advance table once and return the answer.

/// One glyph's advance in this style, letter-spacing included.
fn char_width(metrics: &FontMetrics, style: &TextStyle, c: char) -> f64 {
    metrics
        .advances
        .get(c as usize)
        .copied()
        .unwrap_or(metrics.advance)
        * style.size as f64
        + style.spacing
}

/// Greedy word wrap: the lines `text` breaks into so that none is wider than
/// `max_width`.
///
/// Existing newlines are hard breaks and are always honoured, so a wrapped
/// paragraph keeps the shape its author gave it. Within a paragraph the break
/// goes at the last space that fits; the trailing spaces of a broken line are
/// not counted in its width (a line that ends at a space is not "too wide"
/// because of that space), and they are trimmed off the returned line.
///
/// A single word wider than the whole box is broken at a character boundary
/// rather than allowed to overflow — the choice a text field wants, and the
/// only one that terminates.
fn wrap_lines(metrics: &FontMetrics, style: &TextStyle, text: &str, max_width: f64) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let paragraph = paragraph.strip_suffix('\r').unwrap_or(paragraph);
        // A non-positive box cannot fit anything; wrapping into it would loop
        // forever breaking one character at a time, so the paragraph passes
        // through whole and the caller's clip deals with it.
        if max_width <= 0.0 {
            out.push(paragraph.to_string());
            continue;
        }
        let mut line = Line::new(max_width);
        // A word, and the run of spaces that preceded it. They travel together
        // because a break *at* the spaces discards them: a wrapped line does
        // not begin with the gap that pushed it over.
        let mut word = String::new();
        let mut word_w = 0.0f64;
        let mut gap = String::new();
        let mut gap_w = 0.0f64;
        for c in paragraph.chars() {
            if c == ' ' || c == '\t' {
                line.take(&mut word, &mut word_w, &mut gap, &mut gap_w, metrics, style, &mut out);
                gap.push(c);
                gap_w += char_width(metrics, style, c);
            } else {
                word.push(c);
                word_w += char_width(metrics, style, c);
            }
        }
        line.take(&mut word, &mut word_w, &mut gap, &mut gap_w, metrics, style, &mut out);
        out.push(line.finish());
    }
    out
}

/// The line being filled, and the width of it that counts.
///
/// Two widths, because trailing spaces are not "too wide": a line ending at a
/// space is as wide as its last non-space character, and those spaces are
/// trimmed off when the line is emitted. `trimmed` is that prefix.
struct Line {
    text: String,
    max_width: f64,
    /// Byte length and width of `text` up to its last non-space character.
    trimmed_len: usize,
    trimmed_w: f64,
    /// Width of the whole of `text`, trailing spaces included.
    width: f64,
}

impl Line {
    fn new(max_width: f64) -> Line {
        Line {
            text: String::new(),
            max_width,
            trimmed_len: 0,
            trimmed_w: 0.0,
            width: 0.0,
        }
    }

    /// Add `word` (preceded by `gap`) to this line, breaking to a new one first
    /// if it does not fit. Both are left empty.
    #[allow(clippy::too_many_arguments)]
    fn take(
        &mut self,
        word: &mut String,
        word_w: &mut f64,
        gap: &mut String,
        gap_w: &mut f64,
        metrics: &FontMetrics,
        style: &TextStyle,
        out: &mut Vec<String>,
    ) {
        if word.is_empty() {
            gap.clear();
            *gap_w = 0.0;
            return;
        }
        if !self.text.is_empty() && self.trimmed_w + *gap_w + *word_w > self.max_width {
            self.wrap(out);
        } else {
            self.text.push_str(gap);
            self.width += *gap_w;
        }
        if *word_w > self.max_width && self.text.is_empty() {
            // A word wider than the whole box: break it at a character
            // boundary rather than let it overflow. This is also the only
            // branch that makes the wrap terminate on unbroken text.
            let mut acc = 0.0f64;
            for c in word.chars() {
                let cw = char_width(metrics, style, c);
                if acc + cw > self.max_width && !self.text.is_empty() {
                    out.push(std::mem::take(&mut self.text));
                    acc = 0.0;
                }
                self.text.push(c);
                acc += cw;
            }
            self.width = acc;
        } else {
            self.text.push_str(word);
            self.width += *word_w;
        }
        self.trimmed_len = self.text.len();
        self.trimmed_w = self.width;
        word.clear();
        *word_w = 0.0;
        gap.clear();
        *gap_w = 0.0;
    }

    /// Emit what is on the line, trailing spaces trimmed, and start a new one.
    fn wrap(&mut self, out: &mut Vec<String>) {
        self.text.truncate(self.trimmed_len);
        out.push(std::mem::take(&mut self.text));
        self.width = 0.0;
        self.trimmed_w = 0.0;
        self.trimmed_len = 0;
    }

    /// The last line, trailing spaces trimmed.
    fn finish(mut self) -> String {
        self.text.truncate(self.trimmed_len);
        self.text
    }
}

/// `text_wrap(s, style, max_width) -> [string]`: the lines `s` breaks into to
/// fit `max_width` px in `style`.
///
/// Existing newlines are always honoured; a word wider than the box is broken
/// rather than allowed to overflow. Always returns at least one line, so a
/// caller can lay the result out without a special case for empty text.
///
/// `style` is the argument `text_width` takes — a style record, or a bare size
/// (a face name may follow it, before `max_width`). Measure and draw with the
/// same one and the wrap is the one you see.
pub(crate) fn native_text_wrap(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let (style, metrics) = style_and_metrics(state, 2)?;
    let max_width = state.get_float(state.arg_count())?;
    let lines = wrap_lines(&metrics, &style, &text, max_width);
    let items: Vec<Value> = lines
        .into_iter()
        .map(|line| Value::String(state.heap_mut().alloc_string(line)))
        .collect();
    state.push_list(items);
    Ok(1)
}

/// Which end of an over-long string an ellipsis eats.
#[derive(Clone, Copy, PartialEq)]
enum Elide {
    /// Keep the head, "a long lab…" — a label.
    Tail,
    /// Keep the tail, "…/src/app.ptl" — a path.
    Head,
    /// Keep both ends, "chapter…final" — an identifier.
    Middle,
}

/// The one-character ellipsis. Three bytes, one glyph: the reason the trimming
/// below counts characters and never bytes.
const ELLIPSIS: char = '…';

fn elide(
    metrics: &FontMetrics,
    style: &TextStyle,
    text: &str,
    max_width: f64,
    mode: Elide,
) -> String {
    if run_width(metrics, style, text) <= max_width {
        return text.to_string();
    }
    let ell = char_width(metrics, style, ELLIPSIS);
    let budget = max_width - ell;
    if budget <= 0.0 {
        // Not even the ellipsis fits. It is still what gets returned: the
        // marker is the one thing worth keeping when there is no room, because
        // an empty cell reads as "nothing here" and "…" reads as "something
        // here that did not fit". A caller with a box this small is going to
        // clip whatever it is given anyway.
        return String::from(ELLIPSIS);
    }
    let chars: Vec<char> = text.chars().collect();
    match mode {
        Elide::Tail => {
            let mut acc = 0.0;
            let mut end = 0;
            for (i, c) in chars.iter().enumerate() {
                let cw = char_width(metrics, style, *c);
                if acc + cw > budget {
                    break;
                }
                acc += cw;
                end = i + 1;
            }
            let mut out: String = chars[..end].iter().collect();
            out.push(ELLIPSIS);
            out
        }
        Elide::Head => {
            let mut acc = 0.0;
            let mut start = chars.len();
            for (i, c) in chars.iter().enumerate().rev() {
                let cw = char_width(metrics, style, *c);
                if acc + cw > budget {
                    break;
                }
                acc += cw;
                start = i;
            }
            let mut out = String::from(ELLIPSIS);
            out.extend(chars[start..].iter());
            out
        }
        Elide::Middle => {
            // The head gets the larger half of an odd budget, because the
            // start of a string is the part a reader identifies it by — so the
            // glyph that straddles the midpoint is kept rather than dropped.
            let head_budget = budget / 2.0;
            let mut head_w = 0.0;
            let mut head = 0;
            for (i, c) in chars.iter().enumerate() {
                let cw = char_width(metrics, style, *c);
                if head_w + cw / 2.0 > head_budget {
                    break;
                }
                head_w += cw;
                head = i + 1;
            }
            let mut tail_w = 0.0;
            let mut tail = chars.len();
            for (i, c) in chars.iter().enumerate().rev() {
                if i < head {
                    break;
                }
                let cw = char_width(metrics, style, *c);
                if head_w + tail_w + cw > budget {
                    break;
                }
                tail_w += cw;
                tail = i;
            }
            let mut out: String = chars[..head].iter().collect();
            out.push(ELLIPSIS);
            out.extend(chars[tail..].iter());
            out
        }
    }
}

/// `text_ellipsize(s, style, max_width, [where]) -> string`: `s` shortened to
/// fit `max_width` px, with a "…" marking what was cut. Returns `s` untouched
/// when it already fits.
///
/// `where` is `"tail"` (the default — keep the head, for a label), `"head"`
/// (keep the tail, for a path, where the directories are the part every row
/// shares) or `"middle"` (keep both ends). A box with no room even for the
/// ellipsis returns the bare "…": the marker is what says there is text here.
///
/// The prelude's `ellipsize` does this with a `text_width` call per character
/// and is careful about byte-vs-character trimming because it re-measures a
/// string that already carries its ellipsis. Here the ellipsis is never
/// measured back in: the budget is `max_width` minus its width, once, and the
/// walk is over characters.
pub(crate) fn native_text_ellipsize(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let (style, metrics) = style_and_metrics(state, 2)?;
    // The style may have eaten an optional face-name argument, so the width is
    // found by counting back from the end rather than by a fixed index.
    let has_mode = matches!(state.get_value(state.arg_count())?, Value::String(_))
        && state.arg_count() >= 4;
    let width_index = if has_mode {
        state.arg_count() - 1
    } else {
        state.arg_count()
    };
    let max_width = state.get_float(width_index)?;
    let mode = if has_mode {
        match state.get_string(state.arg_count())?.as_str() {
            "head" => Elide::Head,
            "middle" => Elide::Middle,
            "tail" => Elide::Tail,
            other => {
                return Err(format!(
                    "text_ellipsize: unknown end '{other}' — expected \"tail\", \"head\" or \"middle\""
                ));
            }
        }
    } else {
        Elide::Tail
    };
    let out = elide(&metrics, &style, &text, max_width, mode);
    state.push_string(out);
    Ok(1)
}

/// `text_index_at(s, style, dx) -> int`: the character index a click `dx` px
/// from the run's left edge falls on — the caret position, snapped to whichever
/// character boundary is nearer.
///
/// The caret hit-test every text field needs. Done in a script it is a
/// `text_width` call per character *per frame*, over a string the user is
/// typing into; here it is one walk of the advance table. The result is a
/// **character** index, which is what `char_slice` takes.
pub(crate) fn native_text_index_at(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let (style, metrics) = style_and_metrics(state, 2)?;
    let dx = state.get_float(state.arg_count())?;
    let mut acc = 0.0f64;
    let mut best = 0usize;
    let mut best_d = dx.abs();
    for (i, c) in text.chars().enumerate() {
        acc += char_width(&metrics, &style, c);
        let d = (dx - acc).abs();
        if d <= best_d {
            best_d = d;
            best = i + 1;
        } else {
            // Advances are non-negative, so once the boundaries start moving
            // away from `dx` they never come back.
            break;
        }
    }
    state.push_int(best as i64);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::{DrawCommand, register_draw, take_draw_commands};

    #[test]
    fn variant_candidates_walk_most_specific_first() {
        assert_eq!(font_variant_candidates("ui", 400, false), vec!["ui"]);
        assert_eq!(font_variant_candidates("ui", 400, true), vec!["ui@i", "ui"]);
        assert_eq!(
            font_variant_candidates("ui", 700, false),
            vec!["ui@700", "ui"]
        );
        assert_eq!(
            font_variant_candidates("ui", 700, true),
            vec!["ui@700i", "ui@700", "ui@i", "ui"]
        );
    }

    /// The string a run produced, for the fitting tests below.
    fn as_string(env: &Env, v: &Value) -> String {
        match v {
            Value::String(id) => env.heap().get_string(*id).to_string(),
            other => panic!("expected a string, got {other:?}"),
        }
    }

    /// A fixed-width face makes every expectation below arithmetic: at size
    /// 10 every glyph is 5 px, so "abc" is 15 and a 30 px box holds six.
    fn half_width_env() -> Env {
        let mut env = Env::new();
        register_draw(&mut env);
        bind_text_metrics(&mut env, 0.5);
        env
    }

    /// The float a metrics field holds, for comparisons that must tolerate the
    /// last bit (0.72 × 10 is not exactly 7.2 in binary).
    fn as_float(v: &Value) -> f64 {
        match v {
            Value::Float(f) => *f,
            Value::Int(n) => *n as f64,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn text_metrics_defaults_are_the_ui_sans_proportions() {
        let mut env = Env::new();
        register_draw(&mut env);
        // A host that published nothing still answers, with typical UI-sans
        // proportions rather than the "the run is `size` px tall" guess every
        // script used to make.
        for (field, want) in [
            ("baseline", 16.0),
            ("descent", 4.0),
            ("line_height", 24.0),
            ("cap_height", 14.0),
        ] {
            let v = env
                .run_source(&format!("text_metrics(20).{field}"))
                .expect("run");
            assert!(
                (as_float(&v) - want).abs() < 1e-9,
                "{field}: {v:?} != {want}"
            );
        }
    }

    #[test]
    fn text_metrics_reports_the_bound_face_in_pixels() {
        let mut env = Env::new();
        register_draw(&mut env);
        bind_text_vertical_metrics(
            &mut env,
            &VerticalMetrics {
                baseline: 0.9,
                descent: 0.25,
                line_height: 1.4,
                cap_height: 0.72,
                x_height: 0.5,
            },
        );
        let v = env.run_source("text_metrics(10).baseline").expect("run");
        assert_eq!(v, Value::Float(9.0));
        let v = env.run_source("text_metrics(10).line_height").expect("run");
        assert_eq!(v, Value::Float(14.0));
        // A style record measures the same as a bare size.
        let v = env
            .run_source("text_metrics({size: 10, color: {r: 1, g: 2, b: 3}}).cap_height")
            .expect("run");
        assert!((as_float(&v) - 7.2).abs() < 1e-9, "{v:?}");
    }

    #[test]
    fn a_named_face_inherits_vertical_metrics_it_did_not_publish() {
        let mut env = Env::new();
        register_draw(&mut env);
        bind_text_vertical_metrics(&mut env, &VerticalMetrics::default());
        // Registered with advances only, the way every host did before
        // vertical metrics existed.
        bind_font_metrics(&mut env, "mono", &FontMetrics::monospace(0.5));
        let v = env.run_source("text_metrics({size: 10, font: \"mono\"}).baseline").expect("run");
        assert_eq!(v, Value::Float(8.0));
        // And a face that publishes its own is measured with those.
        bind_font_metrics(
            &mut env,
            "tall",
            &FontMetrics::monospace(0.5).with_vertical(
                VerticalMetrics::default().with_heights(0.75, 0.55),
            ),
        );
        let v = env.run_source("text_metrics({size: 10, font: \"tall\"}).cap_height").expect("run");
        assert_eq!(v, Value::Float(7.5));
    }

    #[test]
    fn text_wrap_breaks_at_spaces_and_keeps_hard_newlines() {
        let mut env = half_width_env();
        // 30 px = six glyphs. "aaa bbb ccc" breaks after each 3-letter word
        // (3 + 1 + 3 = 7 glyphs would be 35 px).
        let v = env
            .run_source("text_wrap(\"aaa bbb ccc\", 10, 30.0) |> join(\"|\")")
            .expect("run");
        assert_eq!(as_string(&env, &v), "aaa|bbb|ccc");
        // 40 px = eight glyphs: "aaa bbb" is seven and fits.
        let v = env
            .run_source("text_wrap(\"aaa bbb ccc\", 10, 40.0) |> join(\"|\")")
            .expect("run");
        assert_eq!(as_string(&env, &v), "aaa bbb|ccc");
        // An author's own newline is a break wherever it falls.
        let v = env
            .run_source("text_wrap(\"a\\nb\", 10, 500.0) |> join(\"|\")")
            .expect("run");
        assert_eq!(as_string(&env, &v), "a|b");
    }

    #[test]
    fn text_wrap_breaks_a_word_wider_than_the_box() {
        let mut env = half_width_env();
        // No space to break at, so the word itself breaks rather than
        // overflowing — and the loop terminates, which the naive version
        // ("find a space; there is none") does not.
        let v = env
            .run_source("text_wrap(\"aaaaaaaaa\", 10, 20.0) |> join(\"|\")")
            .expect("run");
        assert_eq!(as_string(&env, &v), "aaaa|aaaa|a");
    }

    #[test]
    fn text_wrap_always_returns_a_line() {
        let mut env = half_width_env();
        let v = env.run_source("len(text_wrap(\"\", 10, 100.0))").expect("run");
        assert_eq!(v, Value::Int(1));
        // A zero-width box passes text through rather than looping forever.
        let v = env
            .run_source("text_wrap(\"abc\", 10, 0.0) |> join(\"|\")")
            .expect("run");
        assert_eq!(as_string(&env, &v), "abc");
    }

    #[test]
    fn text_ellipsize_fits_the_box_including_its_ellipsis() {
        let mut env = half_width_env();
        // Fits already: untouched.
        let v = env.run_source("text_ellipsize(\"abc\", 10, 100.0)").expect("run");
        assert_eq!(as_string(&env, &v), "abc");
        // 30 px = six glyphs, one of which the ellipsis takes.
        let v = env
            .run_source("text_ellipsize(\"abcdefghij\", 10, 30.0)")
            .expect("run");
        assert_eq!(as_string(&env, &v), "abcde…");
        // The result really fits: this is the property the prelude's
        // byte-trimming version had to be careful to keep.
        let v = env
            .run_source("text_width(text_ellipsize(\"abcdefghij\", 10, 30.0), 10) <= 30")
            .expect("run");
        assert_eq!(v, Value::Bool(true));
    }

    #[test]
    fn text_ellipsize_can_keep_the_tail_or_both_ends() {
        let mut env = half_width_env();
        let v = env
            .run_source("text_ellipsize(\"abcdefghij\", 10, 30.0, \"head\")")
            .expect("run");
        assert_eq!(as_string(&env, &v), "…fghij");
        let v = env
            .run_source("text_ellipsize(\"abcdefghij\", 10, 30.0, \"middle\")")
            .expect("run");
        assert_eq!(as_string(&env, &v), "abc…ij");
        // A box with no room even for the ellipsis keeps the marker: "…" says
        // there is text here, where "" would say there is not.
        let v = env.run_source("text_ellipsize(\"abcdef\", 10, 3.0)").expect("run");
        assert_eq!(as_string(&env, &v), "…");
        let v = env.run_source("text_ellipsize(\"abcdef\", 10, 0.0)").expect("run");
        assert_eq!(as_string(&env, &v), "…");
    }

    #[test]
    fn text_index_at_snaps_to_the_nearer_boundary() {
        let mut env = half_width_env();
        // Glyphs are 5 px: boundaries at 0, 5, 10, 15.
        assert_eq!(env.run_source("text_index_at(\"abc\", 10, 0.0)").expect("run"), Value::Int(0));
        assert_eq!(env.run_source("text_index_at(\"abc\", 10, 2.0)").expect("run"), Value::Int(0));
        assert_eq!(env.run_source("text_index_at(\"abc\", 10, 3.0)").expect("run"), Value::Int(1));
        assert_eq!(env.run_source("text_index_at(\"abc\", 10, 7.0)").expect("run"), Value::Int(1));
        // Past the end clamps to the end, not past it.
        assert_eq!(env.run_source("text_index_at(\"abc\", 10, 900.0)").expect("run"), Value::Int(3));
    }

    #[test]
    fn text_width_uses_bound_ratio() {
        let mut env = Env::new();
        register_draw(&mut env);
        // Default ratio 0.6: 5 chars at size 10 → 30.
        let v = env.run_source("text_width(\"hello\", 10)").expect("run");
        assert_eq!(v, Value::Int(30));
        // Typical monospace metric: ratio 0.6 at size 14 → 8.4 px/char.
        bind_text_metrics(&mut env, 0.6);
        let v = env.run_source("text_width(\"abc\", 14)").expect("run");
        assert_eq!(v, Value::Int(25)); // 3 × 14 × 0.6 = 25.2 → 25
    }

    #[test]
    fn text_advance_is_text_width_unrounded() {
        let mut env = Env::new();
        register_draw(&mut env);
        bind_text_metrics(&mut env, 0.6);
        // 13 × 0.6 = 7.8: text_width rounds to 8, which drifts a whole glyph
        // by column 30; text_advance keeps the fraction.
        assert_eq!(env.run_source("text_width(\"m\", {size: 13})").expect("run"), Value::Int(8));
        let v = env.run_source("text_advance(\"m\", {size: 13})").expect("run");
        assert!(matches!(v, Value::Float(_)), "a float, not a rounded int: {v:?}");
        assert!((as_float(&v) - 7.8).abs() < 1e-9, "{v:?}");
        // Same style argument forms as text_width, spacing included.
        let v = env.run_source("text_advance(\"abc\", 10)").expect("run");
        assert!((as_float(&v) - 18.0).abs() < 1e-9, "{v:?}");
        let v = env.run_source("text_advance(\"abc\", {size: 10, spacing: 0.5})").expect("run");
        assert!((as_float(&v) - 19.5).abs() < 1e-9, "{v:?}");
    }

    #[test]
    fn text_width_uses_advance_table_when_bound() {
        let mut env = Env::new();
        register_draw(&mut env);
        // A proportional table: 'i' is narrow, 'W' is wide; everything else 0.6.
        let mut ratios = vec![0.6f64; 128];
        ratios['i' as usize] = 0.2;
        ratios['W' as usize] = 0.9;
        bind_text_advance_table(&mut env, &ratios);

        // Per-glyph sum, not chars × uniform: 3 × 10 × 0.2 = 6, 3 × 10 × 0.9 = 27.
        let narrow = env.run_source("text_width(\"iii\", 10)").expect("run");
        let wide = env.run_source("text_width(\"WWW\", 10)").expect("run");
        assert_eq!(narrow, Value::Int(6));
        assert_eq!(wide, Value::Int(27));
        assert!(
            narrow != wide,
            "a proportional font must measure 'iii' and 'WWW' differently"
        );
    }

    #[test]
    fn text_width_measures_a_named_font() {
        let mut env = Env::new();
        register_draw(&mut env);
        // Default font: proportional, narrow 'i'. A second face, "mono", is
        // registered by name.
        let mut ratios = vec![0.6f64; 128];
        ratios['i' as usize] = 0.2;
        bind_text_advance_table(&mut env, &ratios);
        bind_font_metrics(&mut env, "mono", &FontMetrics::monospace(0.5));

        // Two args → default (proportional) font: 3 × 10 × 0.2 = 6.
        let v = env.run_source("text_width(\"iii\", 10)").expect("run");
        assert_eq!(v, Value::Int(6));
        // Three args → the named face: 3 × 10 × 0.5 = 15.
        let v = env
            .run_source("text_width(\"iii\", 10, \"mono\")")
            .expect("run");
        assert_eq!(v, Value::Int(15));
        // A fallback list picks the first registered name.
        let v = env
            .run_source("text_width(\"iii\", 10, \"Inter, mono\")")
            .expect("run");
        assert_eq!(v, Value::Int(15));
        // A face this host never bound degrades to the default font.
        let v = env
            .run_source("text_width(\"iii\", 10, \"serif\")")
            .expect("run");
        assert_eq!(v, Value::Int(6));
    }

    /// A stand-in for a host with a real font database: two families it can
    /// draw, one of which has a wider bold cut, and nothing else.
    struct TestFonts;

    impl FontSource for TestFonts {
        fn resolve(&mut self, name: &str) -> Option<String> {
            // Case-folded, and answers with the canonical spelling — the two
            // properties `font()` promises about the name it records.
            match name.trim().to_lowercase().as_str() {
                "helvetica" => Some("Helvetica".to_string()),
                "courier" => Some("Courier".to_string()),
                _ => None,
            }
        }

        fn metrics(&mut self, family: &str, weight: u16, _italic: bool) -> Option<FontMetrics> {
            match (family, weight) {
                ("Helvetica", 700) => Some(FontMetrics::monospace(0.8)),
                ("Helvetica", _) => Some(FontMetrics::monospace(0.5)),
                ("Courier", _) => Some(FontMetrics::monospace(0.6)),
                _ => None,
            }
        }

        fn families(&mut self) -> Vec<String> {
            vec!["Courier".to_string(), "Helvetica".to_string()]
        }
    }

    /// Run `source` with [`TestFonts`] attached, the way a host wraps its own
    /// `env.run`.
    fn with_test_fonts(env: &mut Env, source: &str) -> Value {
        clear_font_cache();
        let saved = swap_font_provider(Some(Box::new(TestFonts)));
        let out = env.run_source(source);
        swap_font_provider(saved);
        clear_font_cache();
        out.expect("run")
    }

    #[test]
    fn font_returns_a_style_record_naming_the_canonical_family() {
        let mut env = Env::new();
        register_draw(&mut env);
        crate::register_prelude(&mut env);
        // However it was spelled, the object carries the host's spelling — the
        // one the rasterizer will match.
        let v = with_test_fonts(&mut env, r#"font("helvetica").font"#);
        let Value::String(id) = v else {
            panic!("font() must record a family name, got {v:?}");
        };
        assert_eq!(env.heap().get_string(id), "Helvetica");
    }

    #[test]
    fn a_font_this_host_cannot_draw_keeps_its_name() {
        let mut env = Env::new();
        register_draw(&mut env);
        crate::register_prelude(&mut env);
        let v = with_test_fonts(&mut env, r#"font("Papyrus").font"#);
        let Value::String(id) = v else {
            panic!("font() must always produce a record with a name");
        };
        assert_eq!(env.heap().get_string(id), "Papyrus");
    }

    /// The reason `font()` returns an object instead of a string: the size and
    /// the decorations ride along, and one value both measures and draws.
    #[test]
    fn a_font_object_carries_size_and_decorations_into_measuring_and_drawing() {
        let mut env = Env::new();
        register_draw(&mut env);
        crate::register_prelude(&mut env);

        // Regular: 4 chars × 20 × 0.5 = 40. Bold is a real, wider cut in this
        // host, so measuring it must not report the regular's width.
        let regular = with_test_fonts(&mut env, r#"text_width("abcd", font("Helvetica", 20))"#);
        assert_eq!(regular, Value::Int(40));
        let bold = with_test_fonts(
            &mut env,
            r#"text_width("abcd", font_bold(font("Helvetica", 20)))"#,
        );
        assert_eq!(bold, Value::Int(64), "4 × 20 × 0.8");

        // …and the same object drawn emits exactly those axes.
        with_test_fonts(
            &mut env,
            r#"draw_text("abcd", {x: 3, y: 5},
                 font_spacing(font_italic(font_bold(font("Helvetica", 20))), 2))"#,
        );
        let cmds = take_draw_commands(&mut env);
        assert_eq!(
            cmds[0],
            DrawCommand::Text {
                text: "abcd".into(),
                x: 3,
                y: 5,
                size: 20,
                r: 255,
                g: 255,
                b: 255,
                a: 255,
                font: Some("Helvetica".into()),
                weight: 700,
                italic: true,
                spacing: 2.0,
            }
        );
    }

    #[test]
    fn the_decorations_do_not_mutate_the_font_they_are_given() {
        let mut env = Env::new();
        register_draw(&mut env);
        crate::register_prelude(&mut env);
        let v = with_test_fonts(
            &mut env,
            r#"
            let body = font("Helvetica", 20)
            let heading = font_size(font_bold(body), 40)
            [text_width("abcd", body), text_width("abcd", heading)]
            "#,
        );
        let Value::List(id) = v else {
            panic!("expected a list, got {v:?}");
        };
        // body is still 20px regular (40), heading is 40px bold (4 × 40 × 0.8).
        assert_eq!(
            env.heap().get_list(id),
            &[Value::Int(40), Value::Int(128)][..]
        );
    }

    #[test]
    fn text_width_falls_through_to_the_font_source_and_back_to_the_default() {
        let mut env = Env::new();
        register_draw(&mut env);
        bind_text_metrics(&mut env, 0.6);
        // Bound in the registry: the eager answer wins over the source's.
        bind_font_metrics(&mut env, "Helvetica", &FontMetrics::monospace(0.1));
        let published = with_test_fonts(&mut env, r#"text_width("abcd", 10, "Helvetica")"#);
        assert_eq!(published, Value::Int(4));

        // Not in the registry: measured through the source.
        let measured = with_test_fonts(&mut env, r#"text_width("abcd", 10, "Courier")"#);
        assert_eq!(measured, Value::Int(24));

        // Neither: the default font, rather than an error.
        let fallback = with_test_fonts(&mut env, r#"text_width("abcd", 10, "Papyrus")"#);
        assert_eq!(fallback, Value::Int(24));

        // A fallback list skips past the name this host lacks.
        let listed = with_test_fonts(&mut env, r#"text_width("abcd", 10, "Papyrus, Courier")"#);
        assert_eq!(listed, Value::Int(24));
    }

    #[test]
    fn fonts_lists_what_the_host_offers_and_is_empty_without_a_source() {
        let mut env = Env::new();
        register_draw(&mut env);
        let v = with_test_fonts(&mut env, "fonts()");
        let Value::List(id) = v else {
            panic!("fonts() must return a list, got {v:?}");
        };
        let names: Vec<String> = env
            .heap()
            .get_list(id)
            .iter()
            .map(|v| match v {
                Value::String(id) => env.heap().get_string(*id).to_string(),
                other => panic!("expected strings, got {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["Courier".to_string(), "Helvetica".to_string()]);

        // A host with no font source offers none — not an error.
        let v = env.run_source("fonts()").expect("run");
        let Value::List(id) = v else {
            panic!("fonts() must return a list even with no source");
        };
        assert!(env.heap().get_list(id).is_empty());
    }

    #[test]
    fn plain_text_commands_are_unchanged_by_typography() {
        // The whole backward-compatibility claim in one place: a flat
        // `draw_text` still emits exactly 8 args, decodes to the pre-typography
        // defaults, and serializes without a single new key.
        let mut env = Env::new();
        register_draw(&mut env);
        env.run_source("draw_text(\"hi\", 1, 2, 14, 10, 20, 30)")
            .expect("run");
        let cmds = take_draw_commands(&mut env);
        assert_eq!(
            cmds[0],
            DrawCommand::Text {
                text: "hi".into(),
                x: 1,
                y: 2,
                size: 14,
                r: 10,
                g: 20,
                b: 30,
                a: 255,
                font: None,
                weight: REGULAR_WEIGHT,
                italic: false,
                spacing: 0.0,
            }
        );
        assert_eq!(
            serde_json::to_string(&cmds[0]).unwrap(),
            r#"{"op":"text","text":"hi","x":1,"y":2,"size":14,"r":10,"g":20,"b":30}"#
        );
    }

    #[test]
    fn styled_text_carries_face_weight_italic_and_spacing() {
        let mut env = Env::new();
        register_draw(&mut env);
        env.run_source(
            "draw_text(\"bold\", 4, 8, {size: 20, color: {r: 1, g: 2, b: 3, a: 128}, \
             font: \"ui\", weight: 700, italic: true, spacing: 1.5})",
        )
        .expect("run");
        // A style that names no typography at all is plain text: same command,
        // same JSON, so styles are safe to use for ordinary labels too.
        env.run_source("draw_text(\"plain\", 4, 8, {size: 20, color: {r: 1, g: 2, b: 3}})")
            .expect("run");
        let cmds = take_draw_commands(&mut env);
        assert_eq!(
            cmds[0],
            DrawCommand::Text {
                text: "bold".into(),
                x: 4,
                y: 8,
                size: 20,
                r: 1,
                g: 2,
                b: 3,
                a: 128,
                font: Some("ui".into()),
                weight: 700,
                italic: true,
                spacing: 1.5,
            }
        );
        assert_eq!(
            serde_json::to_string(&cmds[1]).unwrap(),
            r#"{"op":"text","text":"plain","x":4,"y":8,"size":20,"r":1,"g":2,"b":3}"#
        );
    }

    #[test]
    fn text_width_measures_the_style_it_will_draw() {
        let mut env = Env::new();
        register_draw(&mut env);
        bind_font_metrics(&mut env, "ui", &FontMetrics::monospace(0.5));
        bind_font_variant_metrics(&mut env, "ui", 700, false, &FontMetrics::monospace(0.6));
        bind_font_variant_metrics(&mut env, "ui", 400, true, &FontMetrics::monospace(0.55));
        bind_font_variant_metrics(&mut env, "ui", 700, true, &FontMetrics::monospace(0.7));

        let mut width = |src: &str| env.run_source(src).expect("run");
        // Each variant measures its own metrics: 4 chars × 10 px × ratio.
        assert_eq!(
            width("text_width(\"abcd\", {size: 10, font: \"ui\"})"),
            Value::Int(20)
        );
        assert_eq!(
            width("text_width(\"abcd\", {size: 10, font: \"ui\", weight: 700})"),
            Value::Int(24)
        );
        assert_eq!(
            width("text_width(\"abcd\", {size: 10, font: \"ui\", italic: true})"),
            Value::Int(22)
        );
        assert_eq!(
            width("text_width(\"abcd\", {size: 10, font: \"ui\", weight: 700, italic: true})"),
            Value::Int(28)
        );
        // Letter-spacing counts once per glyph, as CSS does.
        assert_eq!(
            width("text_width(\"abcd\", {size: 10, font: \"ui\", spacing: 2})"),
            Value::Int(28)
        );
    }

    #[test]
    fn a_font_less_style_still_measures_its_weight() {
        // Drawing a font-less bold command renders the default face's bold, so
        // measuring it has to as well — otherwise the one style people write
        // most (bold, no face named) is the one that measures wrong.
        let mut env = Env::new();
        register_draw(&mut env);
        bind_text_metrics(&mut env, 0.5);
        bind_font_metrics(&mut env, "ui", &FontMetrics::monospace(0.5));
        bind_font_variant_metrics(&mut env, "ui", 700, false, &FontMetrics::monospace(0.8));

        const BOLD: &str = "text_width(\"ab\", {size: 10, weight: 700})";
        assert_eq!(
            env.run_source(BOLD).expect("run"),
            Value::Int(10),
            "until the host says which face is the default, bold measures regular"
        );
        bind_default_font_name(&mut env, "ui");
        assert_eq!(
            env.run_source(BOLD).expect("run"),
            Value::Int(16),
            "with the default face named, a font-less bold finds ui@700"
        );
        assert_eq!(
            env.run_source("text_width(\"ab\", {size: 10})")
                .expect("run"),
            Value::Int(10),
            "regular text still measures the plain default metrics"
        );
    }

    #[test]
    fn a_missing_variant_degrades_within_its_family() {
        // A host with one weight per family: bold must measure that family's
        // regular, not another family's bold — the same face it will be drawn
        // in. Only a family the host has never heard of falls through.
        let mut env = Env::new();
        register_draw(&mut env);
        bind_text_metrics(&mut env, 0.9);
        bind_font_metrics(&mut env, "ui", &FontMetrics::monospace(0.5));
        bind_font_variant_metrics(&mut env, "mono", 700, false, &FontMetrics::monospace(0.8));

        let mut width = |src: &str| env.run_source(src).expect("run");
        assert_eq!(
            width("text_width(\"ab\", {size: 10, font: \"ui\", weight: 700})"),
            Value::Int(10),
            "ui has no bold: measure ui regular"
        );
        assert_eq!(
            width("text_width(\"ab\", {size: 10, font: \"ui, mono\", weight: 700})"),
            Value::Int(10),
            "family before variant: ui regular beats mono bold"
        );
        assert_eq!(
            width("text_width(\"ab\", {size: 10, font: \"Papyrus\", weight: 700})"),
            Value::Int(18),
            "an unknown family falls back to the host's default font"
        );
    }

    #[test]
    fn named_fonts_accumulate_and_carry_tables() {
        let mut env = Env::new();
        register_draw(&mut env);
        let mut ui = vec![0.6f64; 128];
        ui['W' as usize] = 1.0;
        bind_font_metrics(&mut env, "ui", &FontMetrics::proportional(ui, 0.6));
        // A second registration must not drop the first.
        bind_font_metrics(&mut env, "mono", &FontMetrics::monospace(0.5));

        let v = env
            .run_source("text_width(\"WW\", 10, \"ui\")")
            .expect("run");
        assert_eq!(v, Value::Int(20));
        let v = env
            .run_source("text_width(\"WW\", 10, \"mono\")")
            .expect("run");
        assert_eq!(v, Value::Int(10));
    }

    #[test]
    fn text_width_advance_table_falls_back_for_untabled_chars() {
        let mut env = Env::new();
        register_draw(&mut env);
        // Table only covers a few ASCII slots; a char beyond its length uses the
        // uniform ratio (default 0.6).
        let ratios = vec![0.3f64; 65]; // covers up to 'A' - 1
        bind_text_advance_table(&mut env, &ratios);
        // 'Z' (0x5A) is past the table → uniform 0.6: 2 × 10 × 0.6 = 12.
        let v = env.run_source("text_width(\"ZZ\", 10)").expect("run");
        assert_eq!(v, Value::Int(12));
    }
}
