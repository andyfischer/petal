//! Color palettes for Garden's built-in themes.
//!
//! [`Theme`] holds one [`Color`] per UI element. A built-in [`ThemeScheme`]
//! provides the base palette; a Petal script can override any subset via
//! `color_theme({ ... })` (see [`Theme::with_script_overrides`]). garden-script
//! captures script colors as plain rgba (it must not depend on garden-render);
//! this layer maps each one onto a [`Color`].

use garden_render::Color;

pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    rgba(r, g, b, 1.0)
}

/// Translucent green/red diff tints, shared so the built-in editor diff view
/// ([`crate::editor_view`]'s `bg_color`) and the panel-theme projection
/// ([`Theme::to_panel_theme`]) paint added/removed rows the *same* color — the
/// editor composites the tint live, the panel bakes it opaque over the pane bg.
pub const DIFF_ADDED_TINT: Color = rgba(0x3f, 0xb9, 0x50, 0.15);
pub const DIFF_REMOVED_TINT: Color = rgba(0xe0, 0x6c, 0x75, 0.15);

/// Quantize a sRGB [`Color`] to `[r, g, b, a]` bytes (0..=255) — the units the
/// panel draw natives consume. sRGB is passed straight through (no linearize);
/// only the alpha of the source color is carried.
fn srgb_u8(c: Color) -> [u8; 4] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(c.r), q(c.g), q(c.b), q(c.a)]
}

/// `c` with its alpha replaced — used to tint a base hue before compositing it
/// over the pane background into an opaque diff/hunk fill.
fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// Linear interpolation between two opaque sRGB colors at `t` ∈ 0..=1 — the
/// blend a drawer used to do by hand to synthesize the extra text tiers Garden's
/// two-tier `text`/`text_dim` doesn't provide. Kept in sRGB (matching the old
/// script-side `mix`) so the tiers land exactly where the drawers expect.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

/// Composite translucent `fg` over opaque `bg` (straight source-over in sRGB),
/// returning an opaque color. Used to bake a drawer's selection/hover fills into
/// one flat color that reads over the pane background in every scheme.
fn over(fg: Color, bg: Color) -> Color {
    let a = fg.a.clamp(0.0, 1.0);
    Color {
        r: fg.r * a + bg.r * (1.0 - a),
        g: fg.g * a + bg.g * (1.0 - a),
        b: fg.b * a + bg.b * (1.0 - a),
        a: 1.0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeScheme {
    Dark,
    Light,
    Brown,
    Amiga,
}

impl ThemeScheme {
    pub const ALL: [ThemeScheme; 4] = [
        ThemeScheme::Dark,
        ThemeScheme::Light,
        ThemeScheme::Brown,
        ThemeScheme::Amiga,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeScheme::Dark => "Midnight",
            ThemeScheme::Light => "Paper",
            ThemeScheme::Brown => "Cocoa",
            ThemeScheme::Amiga => "Amiga 70s",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ThemeScheme::Dark => "dark",
            ThemeScheme::Light => "light",
            ThemeScheme::Brown => "brown",
            ThemeScheme::Amiga => "amiga",
        }
    }

    /// The scheme for a persisted [`key`](Self::key), or `None` for an
    /// unrecognized name (a hand-edited `color_scheme("...")` typo, or a scheme
    /// from a newer version) — the caller keeps its default in that case.
    pub fn from_key(key: &str) -> Option<ThemeScheme> {
        ThemeScheme::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// The full set of themed colors. Field names are the keys a Petal script uses
/// in `color_theme({ ... })` (snake_case, one per field).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub window_bg: Color,
    pub pane_bg: Color,
    pub pane_bg_focused: Color,
    pub border: Color,
    pub border_focused: Color,
    pub text: Color,
    pub text_dim: Color,
    pub cursor: Color,
    pub cursor_block: Color,
    pub selection: Color,
    pub search_match: Color,
    pub cursor_line: Color,
    pub titlebar_bg: Color,
    pub titlebar_text: Color,
    pub status_bg: Color,
    pub status_text: Color,
    pub error_text: Color,
    pub scrollbar_track: Color,
    pub scrollbar_thumb: Color,

    // Syntax-token colors (consumed by a later highlighting chunk). Defaults
    // fit the existing dark palette.
    pub syntax_keyword: Color,
    pub syntax_type: Color,
    pub syntax_function: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_comment: Color,
    pub syntax_constant: Color,
    pub syntax_punctuation: Color,
    pub syntax_variable: Color,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::scheme(ThemeScheme::Dark)
    }
}

impl Theme {
    pub fn scheme(scheme: ThemeScheme) -> Theme {
        match scheme {
            ThemeScheme::Dark => Theme::dark(),
            ThemeScheme::Light => Theme::light(),
            ThemeScheme::Brown => Theme::brown(),
            ThemeScheme::Amiga => Theme::amiga(),
        }
    }

    fn dark() -> Theme {
        Theme {
            window_bg: rgb(0x09, 0x0a, 0x0d),
            pane_bg: rgb(0x0e, 0x10, 0x14),
            pane_bg_focused: rgb(0x12, 0x15, 0x1a),
            border: rgb(0x24, 0x29, 0x31),
            border_focused: rgb(0x4f, 0x8c, 0xc9),
            text: rgb(0xe4, 0xea, 0xf3),
            text_dim: rgb(0x6b, 0x77, 0x86),
            cursor: rgb(0x8f, 0xbc, 0xea),
            cursor_block: rgba(0x8f, 0xbc, 0xea, 0.45),
            selection: rgba(0x4f, 0x8c, 0xc9, 0.30),
            search_match: rgba(0xd7, 0x9a, 0x3e, 0.30),
            cursor_line: rgba(0xff, 0xff, 0xff, 0.05),
            titlebar_bg: rgb(0x0e, 0x10, 0x14),
            titlebar_text: rgb(0x9a, 0xa4, 0xb2),
            status_bg: rgb(0x0b, 0x0d, 0x10),
            status_text: rgb(0xa8, 0xb2, 0xbf),
            error_text: rgb(0xe0, 0x6c, 0x75),
            scrollbar_track: rgba(0xff, 0xff, 0xff, 0.07),
            scrollbar_thumb: rgba(0xff, 0xff, 0xff, 0.38),

            syntax_keyword: rgb(0xc6, 0x78, 0xdd),     // purple
            syntax_type: rgb(0xe5, 0xc0, 0x7b),        // yellow
            syntax_function: rgb(0x61, 0xaf, 0xef),    // blue
            syntax_string: rgb(0x98, 0xc3, 0x79),      // green
            syntax_number: rgb(0xd1, 0x9a, 0x66),      // orange
            syntax_comment: rgb(0x6b, 0x77, 0x86),     // dim (= text_dim)
            syntax_constant: rgb(0xd1, 0x9a, 0x66),    // orange
            syntax_punctuation: rgb(0x8a, 0x93, 0xa0), // between text and text_dim
            syntax_variable: rgb(0xe4, 0xea, 0xf3),    // = text
        }
    }

    fn light() -> Theme {
        Theme {
            window_bg: rgb(0xf1, 0xf0, 0xec),
            pane_bg: rgb(0xfb, 0xfa, 0xf6),
            pane_bg_focused: rgb(0xff, 0xfe, 0xfb),
            border: rgb(0xd3, 0xce, 0xc3),
            border_focused: rgb(0x2e, 0x74, 0xa6),
            text: rgb(0x23, 0x29, 0x2f),
            text_dim: rgb(0x76, 0x78, 0x72),
            cursor: rgb(0x1f, 0x6f, 0x99),
            cursor_block: rgba(0x1f, 0x6f, 0x99, 0.32),
            selection: rgba(0x52, 0x9a, 0xc8, 0.28),
            search_match: rgba(0xf2, 0xbf, 0x4c, 0.38),
            cursor_line: rgba(0x00, 0x00, 0x00, 0.04),
            titlebar_bg: rgb(0xea, 0xe7, 0xdf),
            titlebar_text: rgb(0x59, 0x5e, 0x61),
            status_bg: rgb(0xe2, 0xde, 0xd3),
            status_text: rgb(0x42, 0x46, 0x49),
            error_text: rgb(0xb8, 0x33, 0x3d),
            scrollbar_track: rgba(0x00, 0x00, 0x00, 0.10),
            scrollbar_thumb: rgba(0x00, 0x00, 0x00, 0.42),

            syntax_keyword: rgb(0x8b, 0x42, 0x94),
            syntax_type: rgb(0xa2, 0x63, 0x12),
            syntax_function: rgb(0x1d, 0x6d, 0x93),
            syntax_string: rgb(0x4c, 0x7f, 0x32),
            syntax_number: rgb(0xa7, 0x5d, 0x2a),
            syntax_comment: rgb(0x7a, 0x82, 0x79),
            syntax_constant: rgb(0xa7, 0x5d, 0x2a),
            syntax_punctuation: rgb(0x66, 0x69, 0x69),
            syntax_variable: rgb(0x23, 0x29, 0x2f),
        }
    }

    fn brown() -> Theme {
        Theme {
            window_bg: rgb(0x16, 0x10, 0x0d),
            pane_bg: rgb(0x20, 0x18, 0x13),
            pane_bg_focused: rgb(0x29, 0x20, 0x19),
            border: rgb(0x4b, 0x3a, 0x2e),
            border_focused: rgb(0xb4, 0x83, 0x55),
            text: rgb(0xef, 0xe0, 0xc8),
            text_dim: rgb(0x9c, 0x84, 0x6e),
            cursor: rgb(0xd9, 0xa4, 0x6a),
            cursor_block: rgba(0xd9, 0xa4, 0x6a, 0.38),
            selection: rgba(0xb4, 0x83, 0x55, 0.30),
            search_match: rgba(0xd6, 0xba, 0x6e, 0.28),
            cursor_line: rgba(0xff, 0xe1, 0xb2, 0.05),
            titlebar_bg: rgb(0x1b, 0x14, 0x10),
            titlebar_text: rgb(0xc7, 0xac, 0x8f),
            status_bg: rgb(0x13, 0x0f, 0x0c),
            status_text: rgb(0xc8, 0xb3, 0x9c),
            error_text: rgb(0xe1, 0x7f, 0x65),
            scrollbar_track: rgba(0xff, 0xd7, 0xa3, 0.10),
            scrollbar_thumb: rgba(0xff, 0xd7, 0xa3, 0.42),

            syntax_keyword: rgb(0xc8, 0x92, 0x74),
            syntax_type: rgb(0xd8, 0xbb, 0x75),
            syntax_function: rgb(0xd5, 0xa0, 0x60),
            syntax_string: rgb(0x9f, 0xb7, 0x77),
            syntax_number: rgb(0xd7, 0x8f, 0x5d),
            syntax_comment: rgb(0x91, 0x77, 0x62),
            syntax_constant: rgb(0xd7, 0x8f, 0x5d),
            syntax_punctuation: rgb(0xb7, 0x9b, 0x82),
            syntax_variable: rgb(0xef, 0xe0, 0xc8),
        }
    }

    fn amiga() -> Theme {
        Theme {
            window_bg: rgb(0x13, 0x12, 0x18),
            pane_bg: rgb(0x1e, 0x1b, 0x24),
            pane_bg_focused: rgb(0x27, 0x22, 0x30),
            border: rgb(0x52, 0x46, 0x62),
            border_focused: rgb(0xff, 0x9d, 0x3b),
            text: rgb(0xf8, 0xe9, 0xc7),
            text_dim: rgb(0xa7, 0x97, 0xb2),
            cursor: rgb(0x4e, 0xd3, 0xc2),
            cursor_block: rgba(0x4e, 0xd3, 0xc2, 0.42),
            selection: rgba(0xff, 0x9d, 0x3b, 0.30),
            search_match: rgba(0xf6, 0xd3, 0x4d, 0.34),
            cursor_line: rgba(0xff, 0xf0, 0xc9, 0.05),
            titlebar_bg: rgb(0x2e, 0x26, 0x3b),
            titlebar_text: rgb(0xff, 0xce, 0x7a),
            status_bg: rgb(0x0e, 0x2f, 0x42),
            status_text: rgb(0xd4, 0xf5, 0xf2),
            error_text: rgb(0xff, 0x65, 0x72),
            scrollbar_track: rgba(0xff, 0xce, 0x7a, 0.11),
            scrollbar_thumb: rgba(0x4e, 0xd3, 0xc2, 0.50),

            syntax_keyword: rgb(0xff, 0x78, 0xb4),
            syntax_type: rgb(0xff, 0xce, 0x7a),
            syntax_function: rgb(0x6d, 0xd6, 0xff),
            syntax_string: rgb(0x9f, 0xd9, 0x66),
            syntax_number: rgb(0xff, 0x9d, 0x3b),
            syntax_comment: rgb(0x9a, 0x8d, 0xaa),
            syntax_constant: rgb(0xff, 0x9d, 0x3b),
            syntax_punctuation: rgb(0xce, 0xc0, 0xdb),
            syntax_variable: rgb(0xf8, 0xe9, 0xc7),
        }
    }

    /// Project this theme onto the read-only [`PanelTheme`](garden_script::PanelTheme)
    /// injected into every `panel(...)` drawer each frame — the palette a drawer
    /// script reads through the `panel_theme()` native so it can paint in the
    /// app's colors instead of a hardcoded set (see `docs/writing-gpp-apps.md`).
    ///
    /// Each key maps to one sRGB `[r, g, b, a]` (0..=255) — the units the panel
    /// draw natives consume, so nothing is linearized here (CLAUDE.md: the app is
    /// sRGB everywhere). The mapping picks stable *semantic* names, blending a
    /// few convenience fills so a drawer needs no alpha math:
    ///
    /// | key               | source                              |
    /// |-------------------|-------------------------------------|
    /// | `window_bg`       | `window_bg`                         |
    /// | `panel`           | `pane_bg` (card / text_view backdrop)|
    /// | `panel_focused`   | `pane_bg_focused`                   |
    /// | `border`          | `border` (hairlines, bar tracks)    |
    /// | `border_focused`  | `border_focused`                    |
    /// | `text`            | `text`                              |
    /// | `text_mut`        | `text`→`text_dim` blend (2nd tier)  |
    /// | `text_dim`        | `text_dim`                          |
    /// | `text_faint`      | `text_dim`→`pane_bg` blend (4th tier)|
    /// | `cursor`          | `cursor`                            |
    /// | `accent`          | `border_focused` (titles/focus ring)|
    /// | `focus`           | `selection`→`accent` (focused row fill)|
    /// | `sel`             | `selection` composited over `pane_bg` (opaque row fill) |
    /// | `hover`           | `cursor_line` composited over `pane_bg` (opaque row fill)|
    /// | `green`           | `syntax_string` (positive/added)    |
    /// | `orange`          | `syntax_number` (warning/edit)      |
    /// | `red`             | `error_text` (error/removed)        |
    /// | `purple`          | `syntax_keyword` (special)          |
    /// | `blue`            | `syntax_function` (info)            |
    /// | `error`           | `error_text`                        |
    /// | `added_bg`        | green diff tint over `pane_bg` (opaque, matches the editor) |
    /// | `removed_bg`      | red diff tint over `pane_bg` (opaque, matches the editor)   |
    /// | `hunk`            | `syntax_function` (hunk header)     |
    /// | `hunk_bg`         | blue hunk-band tint over `pane_bg` (opaque)|
    /// | `hunk_bg_hover`   | stronger hunk-band tint (hovered)   |
    /// | `scrollbar_thumb` | `scrollbar_thumb`                   |
    /// | `scrollbar_track` | `scrollbar_track`                   |
    ///
    /// The diff/hunk fills reuse the exact translucent tints the built-in editor
    /// diff view composites ([`crate::editor_view`]'s `bg_color`), baked opaque
    /// over the pane background here so a panel drawer paints them flat and reads
    /// identically to the editor in every scheme.
    pub fn to_panel_theme(&self) -> garden_script::PanelTheme {
        let mut t = garden_script::PanelTheme::new();
        t.set("window_bg", srgb_u8(self.window_bg));
        t.set("panel", srgb_u8(self.pane_bg));
        t.set("panel_focused", srgb_u8(self.pane_bg_focused));
        t.set("border", srgb_u8(self.border));
        t.set("border_focused", srgb_u8(self.border_focused));
        t.set("text", srgb_u8(self.text));
        // Two extra text tiers Garden's two-tier text/text_dim doesn't provide,
        // blended so a drawer's three/four-level hierarchy is consistent in every
        // scheme: `text_mut` is a secondary (brighter than dim), `text_faint` the
        // faintest (dim faded toward the background).
        t.set("text_mut", srgb_u8(mix(self.text, self.text_dim, 0.42)));
        t.set("text_dim", srgb_u8(self.text_dim));
        t.set("text_faint", srgb_u8(mix(self.text_dim, self.pane_bg, 0.5)));
        t.set("cursor", srgb_u8(self.cursor));
        t.set("accent", srgb_u8(self.border_focused));
        // The fill for a focused/selected row: the selection tint pushed toward
        // the accent so it reads a step stronger than a plain `sel` row.
        t.set(
            "focus",
            srgb_u8(mix(
                over(self.selection, self.pane_bg),
                self.border_focused,
                0.35,
            )),
        );
        // Pre-composited opaque fills over the pane background, so a drawer can
        // paint a selected/hovered row with one flat color that reads in every
        // scheme (the translucent source would need alpha math script-side).
        t.set("sel", srgb_u8(over(self.selection, self.pane_bg)));
        t.set("hover", srgb_u8(over(self.cursor_line, self.pane_bg)));
        // Semantic accents drawn from the syntax hues, which are tuned to read
        // on each scheme's background (light *and* dark).
        t.set("green", srgb_u8(self.syntax_string));
        t.set("orange", srgb_u8(self.syntax_number));
        t.set("red", srgb_u8(self.error_text));
        t.set("purple", srgb_u8(self.syntax_keyword));
        t.set("blue", srgb_u8(self.syntax_function));
        t.set("error", srgb_u8(self.error_text));
        // Diff/hunk fills, baked opaque over the pane bg. The added/removed tints
        // are the *same* translucent colors the editor's diff view composites
        // (editor_view::bg_color), so a panel diff matches the editor exactly;
        // the hunk band is a faint blue wash keyed off the hunk-header hue.
        t.set("added_bg", srgb_u8(over(DIFF_ADDED_TINT, self.pane_bg)));
        t.set("removed_bg", srgb_u8(over(DIFF_REMOVED_TINT, self.pane_bg)));
        t.set("hunk", srgb_u8(self.syntax_function));
        t.set(
            "hunk_bg",
            srgb_u8(over(with_alpha(self.syntax_function, 0.12), self.pane_bg)),
        );
        t.set(
            "hunk_bg_hover",
            srgb_u8(over(with_alpha(self.syntax_function, 0.20), self.pane_bg)),
        );
        t.set("scrollbar_thumb", srgb_u8(self.scrollbar_thumb));
        t.set("scrollbar_track", srgb_u8(self.scrollbar_track));
        t
    }

    /// Overlay script-provided color overrides onto this theme. For each field,
    /// if the script set a value for the matching key (= the field name), that
    /// field is replaced; unset keys keep their current value.
    pub fn with_script_overrides(mut self, src: &garden_script::Theme) -> Theme {
        // Each entry maps a script key to the field it overrides.
        macro_rules! apply {
            ($($field:ident),* $(,)?) => {
                $(
                    if let Some([r, g, b, a]) = src.get(stringify!($field)) {
                        self.$field = Color { r, g, b, a };
                    }
                )*
            };
        }
        apply!(
            window_bg,
            pane_bg,
            pane_bg_focused,
            border,
            border_focused,
            text,
            text_dim,
            cursor,
            cursor_block,
            selection,
            search_match,
            cursor_line,
            titlebar_bg,
            titlebar_text,
            status_bg,
            status_text,
            error_text,
            scrollbar_track,
            scrollbar_thumb,
            syntax_keyword,
            syntax_type,
            syntax_function,
            syntax_string,
            syntax_number,
            syntax_comment,
            syntax_constant,
            syntax_punctuation,
            syntax_variable,
        );
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_legacy_palette() {
        let t = Theme::default();
        assert_eq!(t.window_bg, rgb(0x09, 0x0a, 0x0d));
        assert_eq!(t.selection, rgba(0x4f, 0x8c, 0xc9, 0.30));
    }

    #[test]
    fn named_schemes_include_light_and_dark_variants() {
        assert_eq!(ThemeScheme::ALL.len(), 4);
        assert!(Theme::scheme(ThemeScheme::Light).window_bg.r > 0.8);
        assert!(Theme::scheme(ThemeScheme::Brown).window_bg.r < 0.12);
        assert_ne!(
            Theme::scheme(ThemeScheme::Amiga).status_bg,
            Theme::default().status_bg
        );
    }

    #[test]
    fn panel_theme_projects_srgb_bytes_and_tracks_the_scheme() {
        // The `panel` key is the pane background verbatim (the card/text_view
        // backdrop), quantized to sRGB bytes — light in a light scheme, dark in
        // a dark one, so a drawer's card matches the embedded editor everywhere.
        let dark = Theme::scheme(ThemeScheme::Dark).to_panel_theme();
        let light = Theme::scheme(ThemeScheme::Light).to_panel_theme();
        assert!(!dark.is_empty());

        // pane_bg dark = #0e1014 → byte 0x0e; light = #fbfaf6 → byte 0xfb.
        assert_eq!(dark.get("panel"), Some([0x0e, 0x10, 0x14, 0xff]));
        assert_eq!(light.get("panel"), Some([0xfb, 0xfa, 0xf6, 0xff]));
        // text flips bright↔dark with the scheme.
        assert_eq!(dark.get("text"), Some([0xe4, 0xea, 0xf3, 0xff]));
        assert_eq!(light.get("text"), Some([0x23, 0x29, 0x2f, 0xff]));

        // The composited fills are opaque (alpha baked to 255) and every
        // documented key is present.
        for key in [
            "window_bg",
            "panel",
            "panel_focused",
            "border",
            "border_focused",
            "text",
            "text_dim",
            "cursor",
            "accent",
            "sel",
            "hover",
            "green",
            "orange",
            "red",
            "purple",
            "blue",
            "error",
            "scrollbar_thumb",
            "scrollbar_track",
        ] {
            assert!(dark.get(key).is_some(), "missing key {key}");
        }
        assert_eq!(dark.get("sel").unwrap()[3], 0xff, "sel is opaque");
        assert_eq!(dark.get("hover").unwrap()[3], 0xff, "hover is opaque");
    }

    #[test]
    fn panel_theme_includes_derived_text_tiers_and_diff_colors() {
        // The extra colors a drawer used to synthesize by hand (the third/fourth
        // text tiers and the diff/hunk fills) are now projected here, so every
        // GPP app reads them from one place and stays consistent per scheme.
        let dark = Theme::scheme(ThemeScheme::Dark).to_panel_theme();
        let light = Theme::scheme(ThemeScheme::Light).to_panel_theme();

        for key in [
            "text_mut",
            "text_faint",
            "added_bg",
            "removed_bg",
            "hunk",
            "hunk_bg",
            "hunk_bg_hover",
            "focus",
        ] {
            assert!(dark.get(key).is_some(), "dark missing key {key}");
            assert!(light.get(key).is_some(), "light missing key {key}");
        }

        // The diff/hunk fills are opaque (baked over the pane bg) so a drawer
        // paints them flat, and they track the scheme (light bg → light fill).
        for key in ["added_bg", "removed_bg", "hunk_bg", "hunk_bg_hover"] {
            assert_eq!(dark.get(key).unwrap()[3], 0xff, "{key} is opaque");
            assert_eq!(light.get(key).unwrap()[3], 0xff, "{key} is opaque");
            // A dark scheme's fill is dark; a light scheme's is light.
            assert!(dark.get(key).unwrap()[0] < 0x80, "dark {key} stays dark");
            assert!(light.get(key).unwrap()[0] > 0x80, "light {key} stays light");
        }

        // The middle text tier sits between the primary text and the dim tier;
        // the faint tier is dimmer still (fades toward the background).
        let luma = |c: [u8; 4]| c[0] as u32 + c[1] as u32 + c[2] as u32;
        let text = luma(dark.get("text").unwrap());
        let mut_ = luma(dark.get("text_mut").unwrap());
        let dim = luma(dark.get("text_dim").unwrap());
        let faint = luma(dark.get("text_faint").unwrap());
        assert!(
            dim < mut_ && mut_ < text,
            "text_mut sits between text and dim"
        );
        assert!(faint < dim, "text_faint is dimmer than text_dim");

        // The added/removed fills lean green/red respectively.
        let added = dark.get("added_bg").unwrap();
        let removed = dark.get("removed_bg").unwrap();
        assert!(
            added[1] > added[0] && added[1] > added[2],
            "added_bg leans green"
        );
        assert!(
            removed[0] > removed[1] && removed[0] > removed[2],
            "removed_bg leans red"
        );
    }

    #[test]
    fn script_overrides_replace_only_set_fields() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "garden-theme-override-{}-{}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("init.ptl");
        // Override only window_bg; leave every other field at its default.
        std::fs::write(
            &path,
            "color_theme({ window_bg: \"#112233\" })\nlayout(editor(\"a\"))\n",
        )
        .unwrap();

        let host = garden_script::ScriptHost::load(&path).expect("load");
        let theme = Theme::default().with_script_overrides(host.theme());

        // Overridden field changed...
        assert_eq!(theme.window_bg, rgb(0x11, 0x22, 0x33));
        assert_ne!(theme.window_bg, Theme::default().window_bg);
        // ...while an unset field kept its default.
        assert_eq!(theme.text, Theme::default().text);
    }
}
