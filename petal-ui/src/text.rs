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
}

impl Default for FontMetrics {
    /// A typical monospace face: every glyph advances 0.6× the size.
    fn default() -> Self {
        FontMetrics {
            advance: DEFAULT_TEXT_ADVANCE,
            advances: Vec::new(),
        }
    }
}

impl FontMetrics {
    /// A proportional font described by a codepoint-indexed advance table,
    /// with `advance` covering codepoints past the table's end.
    pub fn proportional(advances: Vec<f64>, advance: f64) -> Self {
        FontMetrics { advance, advances }
    }

    /// A monospace font: one advance ratio for every glyph.
    pub fn monospace(advance: f64) -> Self {
        FontMetrics {
            advance,
            advances: Vec::new(),
        }
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
    FontMetrics {
        advance: uniform,
        advances: advances.unwrap_or_default(),
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
            return Some(FontMetrics {
                advance,
                advances: advances.unwrap_or_default(),
            });
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
    let style = match state.get_value(2)? {
        Value::Map(_) => TextStyle::from_value(state, &state.get_value(2)?)?,
        _ => TextStyle {
            size: state.get_int(2)?,
            font: match state.arg_count() >= 3 {
                true => Some(state.get_string(3)?),
                false => None,
            },
            ..TextStyle::default()
        },
    };

    // A style with no face still has a weight and a slant, and the host draws
    // those in its default face — so resolve that face's variants by the name
    // the host published (see `bind_default_font_name`) rather than measuring
    // regular metrics for bold text.
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
    let metrics = match &spec {
        Some(spec) => named_font_metrics(state, spec, style.weight, style.italic)
            .unwrap_or_else(|| default_font_metrics(state)),
        None => default_font_metrics(state),
    };

    let width =
        metrics.width_of(&text, style.size as f64) + style.spacing * text.chars().count() as f64;
    state.push_int(width.round() as i64);
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
