//! The natives of Petal's optional `regex` package (`core-stdlib/regex`).
//!
//! The package's Petal half, `src/regex.ptl`, declares six natives with
//! `native fn` (`regex_find`, `regex_all`, `regex_test`, `regex_replace`,
//! `regex_split`, `regex_escape`); this crate implements them over Rust's
//! [`regex`] crate and registers them together with the package source:
//!
//! ```
//! let mut env = petal::env::Env::new();
//! petal_regex::register(&mut env);
//! let pid = env.load_program("import regex\nprint(regex_test(\"a1\", \"\\\\d\"))").unwrap();
//! # let _ = pid;
//! ```
//!
//! It is opt-in on purpose: neither core nor the `petal` command links it,
//! so a host that has no use for regular expressions (a game, a wasm build)
//! carries no regex engine. A host that wants it links this crate and calls
//! [`register`] before loading a program.
//!
//! Every native is a constant function of its arguments
//! ([`NativeEffects::CONST`]). Compiled patterns are kept in a small
//! per-thread cache ([`CACHE_CAPACITY`] patterns, least recently used out
//! first), which is invisible to the script.

use std::cell::RefCell;
use std::collections::HashMap;

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeFn, NativeResult, PetalCxt};
use petal::value::Value;
use regex::Regex;

/// The package name: scripts write `import regex`.
pub const PACKAGE_NAME: &str = "regex";

/// The package's manifest, `core-stdlib/regex/petal.toml`.
pub const MANIFEST: &str = include_str!("../../petal.toml");

/// The package's one module, `core-stdlib/regex/src/regex.ptl`: the
/// `native fn` declarations and their docs.
pub const SOURCE: &str = include_str!("../../src/regex.ptl");

/// How many compiled patterns the per-thread cache keeps.
pub const CACHE_CAPACITY: usize = 128;

/// The natives this crate registers, each [`NativeEffects::CONST`].
pub const NATIVES: &[(&str, NativeFn)] = &[
    ("regex_find", n_find),
    ("regex_all", n_all),
    ("regex_test", n_test),
    ("regex_replace", n_replace),
    ("regex_split", n_split),
    ("regex_escape", n_escape),
];

/// The package's files as [`Env::register_package`] takes them.
pub fn package_modules() -> [(&'static str, &'static str); 2] {
    [
        (PACKAGE_NAME, SOURCE),
        (petal::package::MANIFEST_FILE, MANIFEST),
    ]
}

/// Register the natives only, for a host that registers the package source
/// some other way (from disk with `env.add_package`, say).
pub fn register_natives(env: &mut Env) {
    for (name, func) in NATIVES {
        env.register_native(name, *func, NativeEffects::CONST);
    }
}

/// Register everything `import regex` needs: the natives, then the package
/// (so the natives take its declared parameter names), and announce that
/// this `Env` provides `regex` at the manifest's version, so a package whose
/// `petal.toml` says `[requires] regex = ">=0.1"` loads here. Call it before
/// `load_program`.
pub fn register(env: &mut Env) {
    register_natives(env);
    env.register_package(PACKAGE_NAME, package_modules())
        .expect("the regex package registers");
    env.provides_package(PACKAGE_NAME)
        .expect("the regex package has a version");
}

// ── Pattern cache ────────────────────────────────────────────────────────

struct Cache {
    patterns: HashMap<String, (Regex, u64)>,
    tick: u64,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache { patterns: HashMap::new(), tick: 0 });
}

/// `pattern` compiled, from the cache when it is there. A bad pattern is an
/// error naming the native, the pattern and the problem, on one line.
fn compile(native: &str, pattern: &str) -> Result<Regex, String> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.tick += 1;
        let tick = cache.tick;
        if let Some((re, used)) = cache.patterns.get_mut(pattern) {
            *used = tick;
            return Ok(re.clone());
        }
        let re = Regex::new(pattern)
            .map_err(|e| format!("{native}: bad pattern {pattern:?}: {}", describe(&e)))?;
        if cache.patterns.len() >= CACHE_CAPACITY {
            let oldest = cache
                .patterns
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                cache.patterns.remove(&k);
            }
        }
        cache
            .patterns
            .insert(pattern.to_string(), (re.clone(), tick));
        Ok(re)
    })
}

/// A regex error on one line. A syntax error's display is several lines
/// (the pattern, a caret under the spot, `error: what`); the last says what
/// is wrong, and the caret's column says where.
fn describe(e: &regex::Error) -> String {
    match e {
        regex::Error::Syntax(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let what = lines
                .iter()
                .rev()
                .find_map(|l| l.strip_prefix("error: "))
                .unwrap_or_else(|| text.trim());
            match lines.iter().find(|l| l.trim_start().starts_with('^')) {
                Some(caret) => {
                    // The caret line is indented like the pattern line above it.
                    let indent = lines
                        .iter()
                        .find(|l| l.starts_with("    "))
                        .map_or(0, |_| 4);
                    let col = caret.find('^').unwrap_or(0).saturating_sub(indent);
                    format!("{what} (at character {col})")
                }
                None => what.to_string(),
            }
        }
        regex::Error::CompiledTooBig(limit) => {
            format!("it compiles to more than the {limit}-byte limit; make it smaller")
        }
        other => other.to_string(),
    }
}

// ── Arguments and results ────────────────────────────────────────────────

/// Argument `index` as text, `None` for `nil`.
fn text_arg(
    cx: &PetalCxt,
    native: &str,
    index: usize,
    name: &str,
) -> Result<Option<String>, String> {
    match cx.get_value(index)? {
        Value::Nil => Ok(None),
        Value::String(id) => Ok(Some(cx.heap().get_string(id).to_string())),
        other => Err(format!(
            "{native}: `{name}` should be a string or nil, got {}",
            other.type_name()
        )),
    }
}

/// Argument `index`, which must be a string.
fn string_arg(cx: &PetalCxt, native: &str, index: usize, name: &str) -> Result<String, String> {
    match cx.get_value(index)? {
        Value::String(id) => Ok(cx.heap().get_string(id).to_string()),
        other => Err(format!(
            "{native}: `{name}` should be a string, got {}",
            other.type_name()
        )),
    }
}

fn string_value(cx: &mut PetalCxt, s: &str) -> Value {
    Value::String(cx.heap_mut().alloc_string(s.to_string()))
}

/// Character positions from byte offsets that only move forward, so a scan
/// over every match counts each character once.
struct CharPos<'a> {
    text: &'a str,
    byte: usize,
    chars: usize,
}

impl<'a> CharPos<'a> {
    fn new(text: &'a str) -> Self {
        CharPos {
            text,
            byte: 0,
            chars: 0,
        }
    }

    fn at(&mut self, byte: usize) -> usize {
        if byte < self.byte {
            self.byte = 0;
            self.chars = 0;
        }
        self.chars += self.text[self.byte..byte].chars().count();
        self.byte = byte;
        self.chars
    }
}

/// The record for one match: `{match, start, end, groups}`, plus `named`
/// when the pattern names a group.
fn match_record(cx: &mut PetalCxt, re: &Regex, caps: &regex::Captures, pos: &mut CharPos) -> Value {
    let whole = caps.get(0).expect("group 0 is the whole match");
    let start = pos.at(whole.start());
    let end = pos.at(whole.end());
    let mut groups = Vec::with_capacity(caps.len().saturating_sub(1));
    for g in caps.iter().skip(1) {
        groups.push(match g {
            Some(g) => string_value(cx, g.as_str()),
            None => Value::Nil,
        });
    }
    let mut record = petal::heap::record_map_with_capacity(5);
    let text = string_value(cx, whole.as_str());
    record.insert("match".to_string(), text);
    record.insert("start".to_string(), Value::Int(start as i64));
    record.insert("end".to_string(), Value::Int(end as i64));
    let groups = Value::List(cx.heap_mut().alloc_list(groups));
    record.insert("groups".to_string(), groups);
    if re.capture_names().any(|n| n.is_some()) {
        let mut named = petal::heap::record_map_with_capacity(caps.len());
        for name in re.capture_names().flatten() {
            let v = match caps.name(name) {
                Some(g) => string_value(cx, g.as_str()),
                None => Value::Nil,
            };
            named.insert(name.to_string(), v);
        }
        let named = Value::Map(cx.heap_mut().alloc_map(named));
        record.insert("named".to_string(), named);
    }
    Value::Map(cx.heap_mut().alloc_map(record))
}

// ── The natives ──────────────────────────────────────────────────────────

fn n_find(cx: &mut PetalCxt) -> NativeResult {
    let text = text_arg(cx, "regex_find", 1, "text")?;
    let re = compile("regex_find", &string_arg(cx, "regex_find", 2, "pattern")?)?;
    let Some(text) = text else {
        cx.push_nil();
        return Ok(1);
    };
    match re.captures(&text) {
        Some(caps) => {
            let v = match_record(cx, &re, &caps, &mut CharPos::new(&text));
            cx.push_value(v);
        }
        None => cx.push_nil(),
    }
    Ok(1)
}

fn n_all(cx: &mut PetalCxt) -> NativeResult {
    let text = text_arg(cx, "regex_all", 1, "text")?;
    let re = compile("regex_all", &string_arg(cx, "regex_all", 2, "pattern")?)?;
    let mut out = Vec::new();
    if let Some(text) = &text {
        let mut pos = CharPos::new(text);
        for caps in re.captures_iter(text) {
            out.push(match_record(cx, &re, &caps, &mut pos));
        }
    }
    cx.push_list(out);
    Ok(1)
}

fn n_test(cx: &mut PetalCxt) -> NativeResult {
    let text = text_arg(cx, "regex_test", 1, "text")?;
    let re = compile("regex_test", &string_arg(cx, "regex_test", 2, "pattern")?)?;
    cx.push_bool(text.is_some_and(|t| re.is_match(&t)));
    Ok(1)
}

fn n_replace(cx: &mut PetalCxt) -> NativeResult {
    let text = text_arg(cx, "regex_replace", 1, "text")?;
    let re = compile(
        "regex_replace",
        &string_arg(cx, "regex_replace", 2, "pattern")?,
    )?;
    let with = string_arg(cx, "regex_replace", 3, "with")?;
    match text {
        Some(text) => cx.push_string(re.replace_all(&text, with.as_str()).into_owned()),
        None => cx.push_nil(),
    }
    Ok(1)
}

fn n_split(cx: &mut PetalCxt) -> NativeResult {
    let text = text_arg(cx, "regex_split", 1, "text")?;
    let re = compile("regex_split", &string_arg(cx, "regex_split", 2, "pattern")?)?;
    let mut out = Vec::new();
    if let Some(text) = &text {
        for piece in re.split(text) {
            out.push(string_value(cx, piece));
        }
    }
    cx.push_list(out);
    Ok(1)
}

fn n_escape(cx: &mut PetalCxt) -> NativeResult {
    let text = string_arg(cx, "regex_escape", 1, "text")?;
    cx.push_string(regex::escape(&text));
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_syntax_error_is_one_line_with_its_position() {
        let e = compile("regex_find", "a(b").unwrap_err();
        assert!(!e.contains('\n'), "{e}");
        assert!(e.starts_with("regex_find: bad pattern \"a(b\": "), "{e}");
        assert!(e.contains("unclosed group"), "{e}");
    }

    #[test]
    fn the_cache_is_bounded() {
        for i in 0..CACHE_CAPACITY + 10 {
            compile("regex_test", &format!("a{{{i}}}")).unwrap();
        }
        CACHE.with(|c| assert_eq!(c.borrow().patterns.len(), CACHE_CAPACITY));
        // The most recent stays; the first went first.
        CACHE.with(|c| {
            let c = c.borrow();
            assert!(
                c.patterns
                    .contains_key(&format!("a{{{}}}", CACHE_CAPACITY + 9))
            );
            assert!(!c.patterns.contains_key("a{0}"));
        });
    }

    #[test]
    fn char_positions_count_characters() {
        let text = "héllo wörld";
        let mut pos = CharPos::new(text);
        let w = text.find('w').unwrap();
        assert_eq!(pos.at(w), 6);
        assert_eq!(pos.at(text.len()), 11);
    }
}
