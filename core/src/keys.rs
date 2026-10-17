//! The canonical key names: the one vocabulary every host maps its keyboard
//! onto and every script's `key_down` / `key_pressed` / `key_released` reads.
//!
//! The list lives here, in core, rather than in petal-ui (which re-exports it
//! as `petal_ui::input::KEY_NAMES`), so that the compiler can check a key
//! name written as a literal: `key_pressed("enter")` is always false — the key
//! is `"return"` — and the checker says so instead of leaving a dead shortcut.

/// Canonical key names. Hosts map their platform's key events onto these
/// spellings so scripts are portable across embedders. Keypad keys have no
/// names of their own: a host maps keypad Enter to `"return"` and the keypad
/// digits and operators to the main-row keys they print.
pub const KEY_NAMES: &[&str] = &[
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "return",
    "escape",
    "backspace",
    "tab",
    "space",
    "up",
    "down",
    "left",
    "right",
    "pageup",
    "pagedown",
    "home",
    "end",
    "delete",
    "insert",
    "shift",
    "ctrl",
    "alt",
    "cmd",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "minus",
    "equals",
    "comma",
    "period",
    "slash",
    "backslash",
    "semicolon",
    "quote",
    "backquote",
    "leftbracket",
    "rightbracket",
];

/// The natives whose first argument is a key name.
pub const KEY_QUERY_NATIVES: &[&str] = &["key_down", "key_pressed", "key_released"];

pub fn is_canonical_key(name: &str) -> bool {
    KEY_NAMES.contains(&name)
}

/// Other names people write for a canonical key — other toolkits' spellings
/// and the characters a key prints — that edit distance would not find.
const ALIASES: &[(&str, &str)] = &[
    ("enter", "return"),
    ("kp_enter", "return"),
    ("ret", "return"),
    ("esc", "escape"),
    ("bksp", "backspace"),
    ("back", "backspace"),
    ("del", "delete"),
    ("ins", "insert"),
    (" ", "space"),
    ("spacebar", "space"),
    ("control", "ctrl"),
    ("option", "alt"),
    ("opt", "alt"),
    ("meta", "cmd"),
    ("command", "cmd"),
    ("super", "cmd"),
    ("gui", "cmd"),
    ("win", "cmd"),
    ("arrowup", "up"),
    ("arrowdown", "down"),
    ("arrowleft", "left"),
    ("arrowright", "right"),
    ("pgup", "pageup"),
    ("pgdn", "pagedown"),
    ("pgdown", "pagedown"),
    ("-", "minus"),
    ("=", "equals"),
    ("equal", "equals"),
    (",", "comma"),
    (".", "period"),
    ("/", "slash"),
    ("\\", "backslash"),
    (";", "semicolon"),
    ("'", "quote"),
    ("apostrophe", "quote"),
    ("`", "backquote"),
    ("grave", "backquote"),
    ("backtick", "backquote"),
    ("[", "leftbracket"),
    ("]", "rightbracket"),
];

/// The canonical key a non-canonical `name` most likely meant: a known alias
/// (`"enter"` → `"return"`), the same name in lower case or without
/// separators (`"Page_Up"` → `"pageup"`), or the nearest name within two
/// edits. `None` when nothing is close.
pub fn suggest_key(name: &str) -> Option<&'static str> {
    if is_canonical_key(name) {
        return None;
    }
    let lower = name.to_lowercase();
    let squashed: String = lower
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .collect();
    for candidate in [lower.as_str(), squashed.as_str()] {
        if let Some(&k) = KEY_NAMES.iter().find(|k| **k == candidate) {
            return Some(k);
        }
        if let Some(&(_, k)) = ALIASES.iter().find(|(alias, _)| *alias == candidate) {
            return Some(k);
        }
    }
    // A single character is too short to measure: every one-letter key is one
    // edit from every other.
    if squashed.chars().count() < 3 {
        return None;
    }
    crate::diagnostic::closest(&squashed, KEY_NAMES.iter().copied(), 2)
}

/// Why `name` is not a key name, for a diagnostic: names the likely key when
/// [`suggest_key`] finds one.
pub fn unknown_key_message(name: &str) -> String {
    match suggest_key(name) {
        Some(k) => format!("`{name:?}` is not a key name; did you mean `{k:?}`?"),
        None => format!(
            "`{name:?}` is not a key name, so this is always false (key names: {})",
            KEY_NAMES.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions() {
        assert_eq!(suggest_key("enter"), Some("return"));
        assert_eq!(suggest_key("Enter"), Some("return"));
        assert_eq!(suggest_key("ESC"), Some("escape"));
        assert_eq!(suggest_key("A"), Some("a"));
        assert_eq!(suggest_key("page_up"), Some("pageup"));
        assert_eq!(suggest_key("escpae"), Some("escape"));
        assert_eq!(suggest_key("="), Some("equals"));
        assert_eq!(suggest_key("return"), None);
        assert_eq!(suggest_key("xyzzy"), None);
        assert_eq!(suggest_key("!"), None);
    }

    #[test]
    fn every_alias_names_a_canonical_key() {
        for (alias, key) in ALIASES {
            assert!(is_canonical_key(key), "{alias} -> {key}");
            assert!(!is_canonical_key(alias), "{alias} is itself canonical");
        }
    }
}
