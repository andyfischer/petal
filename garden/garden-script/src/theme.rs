//! Plain color-theme representation captured from a Petal `color_theme(...)`
//! call.
//!
//! This crate must NOT depend on `garden-render`, so theme colors are stored
//! as bare rgba (`[f32; 4]`, each component normalized to `0.0..=1.0`). The
//! application layer (`garden-app`) maps these onto its own `Color` /
//! `theme::Theme` — see `docs/architecture.md` for the crate boundary.

use std::collections::HashMap;

/// A set of script-provided theme colors keyed by field name (e.g.
/// `"window_bg"`, `"syntax_keyword"`). Only the keys the script actually set
/// are present; every unset key keeps the application's built-in default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Theme {
    colors: HashMap<String, [f32; 4]>,
}

impl Theme {
    /// Look up the rgba for a theme key, or `None` if the script did not set it.
    pub fn get(&self, key: &str) -> Option<[f32; 4]> {
        self.colors.get(key).copied()
    }

    /// True when the script set no theme colors at all.
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// Insert/overwrite a color (used by the native fn while capturing).
    pub(crate) fn insert(&mut self, key: String, rgba: [f32; 4]) {
        self.colors.insert(key, rgba);
    }
}
