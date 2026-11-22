//! Turning Petal names into JavaScript ones, and writing JS literals.

use std::collections::{HashMap, HashSet};

/// JavaScript reserved words and globals a binding must not shadow.
const RESERVED: &[&str] = &[
    "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do",
    "else", "enum", "export", "extends", "false", "finally", "for", "function", "if", "import",
    "in", "instanceof", "new", "null", "return", "super", "switch", "this", "throw", "true",
    "try", "typeof", "var", "void", "while", "with", "yield", "let", "static", "implements",
    "interface", "package", "private", "protected", "public", "await", "async", "arguments",
    "eval", "undefined", "NaN", "Infinity", "Object", "Array", "String", "Number", "Math",
    "JSON", "Map", "Set", "Symbol", "Error", "Date", "Promise", "React", "console", "window",
    "document",
];

pub fn is_reserved(s: &str) -> bool {
    RESERVED.contains(&s)
}

/// `file_open` → `fileOpen`. A SCREAMING_CASE constant (`RAIL_W`) and a
/// name with no underscore are kept as written. Leading underscores stay.
pub fn camel(s: &str) -> String {
    if !s.contains('_') || is_screaming(s) {
        return s.to_string();
    }
    let lead = s.len() - s.trim_start_matches('_').len();
    let mut out = String::from(&s[..lead]);
    let mut upper = false;
    for (i, ch) in s[lead..].chars().enumerate() {
        if ch == '_' {
            upper = i > 0;
            continue;
        }
        if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// `workout_card` → `WorkoutCard`, `today_ios` → `TodayIos`.
pub fn pascal(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch == '.' || ch == ' ' || ch == '/' {
            upper = true;
            continue;
        }
        if !(ch.is_ascii_alphanumeric() || ch == '$') {
            upper = true;
            continue;
        }
        if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'X');
    }
    out
}

fn is_screaming(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_uppercase()) && !s.chars().any(|c| c.is_ascii_lowercase())
}

/// Whether `s` can be written as a bare JS identifier (or object key).
pub fn is_js_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// An object key: bare when it can be, quoted otherwise.
pub fn js_key(s: &str) -> String {
    if is_js_ident(s) { s.to_string() } else { js_string(s) }
}

/// A double-quoted JS string literal.
pub fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Text for the inside of a template literal.
pub fn template_text(s: &str) -> String {
    s.replace('\\', "\\\\").replace('`', "\\`").replace("${", "\\${")
}

/// The JS names of one module's bindings. A Petal name always maps to the
/// same JS name in a module, so scoping questions stay Petal's; a name that
/// would collide with another one's camel case, or with a name the output
/// imports, keeps an underscore suffix instead.
#[derive(Default)]
pub struct Names {
    map: HashMap<String, String>,
    taken: HashSet<String>,
}

impl Names {
    /// Reserve a name the output defines itself (an import, a component).
    pub fn reserve(&mut self, js: &str) {
        self.taken.insert(js.to_string());
    }

    pub fn is_taken(&self, js: &str) -> bool {
        self.taken.contains(js)
    }

    /// Pin a Petal name to a chosen JS name.
    pub fn pin(&mut self, petal: &str, js: &str) {
        self.map.insert(petal.to_string(), js.to_string());
        self.taken.insert(js.to_string());
    }

    pub fn get(&self, petal: &str) -> Option<&String> {
        self.map.get(petal)
    }

    /// The JS name of a binding.
    pub fn local(&mut self, petal: &str) -> String {
        if let Some(js) = self.map.get(petal) {
            return js.clone();
        }
        let mut js = camel(petal);
        if is_reserved(&js) || !is_js_ident(&js) {
            js = format!("{}_", js.replace(|c: char| !(c.is_ascii_alphanumeric() || c == '_'), "_"));
        }
        while self.taken.contains(&js) {
            js.push('_');
        }
        self.pin(petal, &js);
        js
    }

    /// A fresh JS name not used anywhere in the module.
    pub fn fresh(&mut self, base: &str) -> String {
        let mut js = base.to_string();
        let mut n = 1;
        while self.taken.contains(&js) || is_reserved(&js) {
            n += 1;
            js = format!("{base}{n}");
        }
        self.taken.insert(js.clone());
        js
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases() {
        assert_eq!(camel("file_open"), "fileOpen");
        assert_eq!(camel("RAIL_W"), "RAIL_W");
        assert_eq!(camel("_private_x"), "_privateX");
        assert_eq!(camel("x"), "x");
        assert_eq!(pascal("workout_card"), "WorkoutCard");
        assert_eq!(pascal("feed_ios"), "FeedIos");
        assert_eq!(pascal("3d"), "X3d");
        let mut n = Names::default();
        assert_eq!(n.local("new"), "new_");
        assert_eq!(n.local("file_open"), "fileOpen");
        assert_eq!(n.local("fileOpen"), "fileOpen_");
    }
}
