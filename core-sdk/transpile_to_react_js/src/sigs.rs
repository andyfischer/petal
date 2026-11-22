//! What a runtime file exports, read from its source.
//!
//! The runtime files are the single list of what the transpiler can convert:
//! a Petal call of `name` is converted when a runtime exports `name`, and its
//! named arguments are placed by the JS parameter list. A trailing
//! destructured parameter (`{ group, props } = {}`) is an options object: the
//! Petal parameters it names are passed in it by name.

use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct JsSig {
    /// The positional parameters, in order.
    pub positional: Vec<String>,
    /// The keys of a trailing options object, in Petal parameter order.
    pub options: Vec<String>,
    /// `...rest`: takes any number of further arguments.
    pub rest: bool,
    /// Exported as a function (`export function`); `false` for a const or a
    /// class, whose call is passed through as written.
    pub is_fn: bool,
}

impl JsSig {
    /// Every Petal parameter name, in the order a positional call fills them.
    pub fn params(&self) -> impl Iterator<Item = &String> {
        self.positional.iter().chain(self.options.iter())
    }
}

/// The exports of one runtime file.
pub fn exports(src: &str) -> HashMap<String, JsSig> {
    let mut out = HashMap::new();
    let mut rest = src;
    while let Some(i) = rest.find("export ") {
        let at_line_start = i == 0 || rest[..i].ends_with('\n');
        rest = &rest[i + 7..];
        if !at_line_start {
            continue;
        }
        if let Some(r) = rest.strip_prefix("function ").or_else(|| rest.strip_prefix("async function ")) {
            let name: String = r.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
            let after = &r[name.len()..];
            if let Some(params) = after.strip_prefix('(').and_then(|p| balanced(p, '(', ')')) {
                out.insert(name, parse_params(params));
            }
        } else if let Some(r) = rest.strip_prefix("const ").or_else(|| rest.strip_prefix("class ")).or_else(|| rest.strip_prefix("let ")) {
            let name: String = r.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
            out.insert(name, JsSig::default());
        } else if let Some(r) = rest.strip_prefix('{') {
            // `export { a, b as c }`
            if let Some(end) = r.find('}') {
                for part in r[..end].split(',') {
                    let name = part.split(" as ").last().unwrap_or("").trim();
                    if !name.is_empty() {
                        out.insert(name.to_string(), JsSig::default());
                    }
                }
            }
        }
    }
    out
}

/// The text up to the bracket that closes the one already opened.
fn balanced(s: &str, open: char, close: char) -> Option<&str> {
    let mut depth = 1;
    for (i, c) in s.char_indices() {
        if c == open || (open == '(' && (c == '{' || c == '[')) {
            depth += 1;
        } else if c == close || (open == '(' && (c == '}' || c == ']')) {
            depth -= 1;
            if depth == 0 {
                return Some(&s[..i]);
            }
        }
    }
    None
}

fn parse_params(s: &str) -> JsSig {
    let mut sig = JsSig { is_fn: true, ..Default::default() };
    for part in split_top(s) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(r) = part.strip_prefix("...") {
            let _ = r;
            sig.rest = true;
        } else if let Some(inner) = part.strip_prefix('{') {
            let inner = inner.split('}').next().unwrap_or("");
            for key in split_top(inner) {
                let key = key.split(['=', ':']).next().unwrap_or("").trim();
                if !key.is_empty() {
                    sig.options.push(key.to_string());
                }
            }
        } else {
            let name = part.split('=').next().unwrap_or("").trim();
            sig.positional.push(name.to_string());
        }
    }
    sig
}

/// Split on commas outside brackets.
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    let mut in_str: Option<char> = None;
    for (i, c) in s.char_indices() {
        if let Some(q) = in_str {
            if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => in_str = Some(c),
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_signatures() {
        let e = exports(
            "export function a(x, y = 2) {}\nexport function b(name, render, { group, props = {} } = {}) {}\nexport function c(...xs) {}\nexport const pi = 3;\n  export function nested() {}\n",
        );
        assert_eq!(e["a"].positional, ["x", "y"]);
        assert_eq!(e["b"].options, ["group", "props"]);
        assert!(e["c"].rest);
        assert!(!e["pi"].is_fn);
        assert!(!e.contains_key("nested"));
    }
}
