//! Comment stripping for the Petal sources embedded in a wasm build.
//!
//! Shared by `core/build.rs` (which applies it to every embedded `.ptl` when
//! the target is wasm32) and by a test in the crate (`src/strip_comments_check.rs`),
//! which lexes every embedded file before and after stripping and requires the
//! same tokens at the same line and column. Keep this file free of anything
//! but `std` so both can include it with `#[path]`.
//!
//! The rule is deliberately conservative:
//!
//! - a line whose first non-blank characters are `# ` (or a lone `#`, or the
//!   deprecated `//`) becomes an empty line;
//! - every other line loses its trailing whitespace and is otherwise kept;
//! - except that a line inside, opening or closing a `"""` raw string is kept
//!   verbatim (a `@doc` example's `# …` lines are string content).
//!
//! Lines are never removed, so every token keeps its line number, and nothing
//! before a token on its line changes, so it keeps its column too: error
//! positions inside an embedded module read the same as in the repository
//! file. A comment after code on the same line is left alone (telling it
//! apart from a `#` inside a string needs the real lexer). A `#` directly
//! followed by text is left alone too: that is what a `#define` line of a
//! GLSL raw string looks like. Raw strings are found by counting `"""` on
//! each line, which a `"""` inside an ordinary string or a trailing comment
//! can fool, and a line inside a multi-line `"…"` string that starts with
//! `# ` is still blanked — which is why the check test lexes the result
//! rather than trusting this rule.

/// `source` with its whole-line `# ` (and `//`) comments blanked and trailing whitespace
/// trimmed. Line count and every token's line and column are unchanged.
///
/// A line that starts inside a `"""` raw string, or that opens or closes
/// one, is kept verbatim: a `@doc` example is a raw string whose lines are
/// often the example's own `# …` comments, and its trailing whitespace is
/// part of the string.
pub fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len() / 2);
    let mut lines = source.split('\n').peekable();
    let mut in_raw_string = false;
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let hash = trimmed
            .strip_prefix('#')
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
        let triple_quotes = line.matches("\"\"\"").count();
        if in_raw_string || triple_quotes > 0 {
            out.push_str(line);
            in_raw_string ^= triple_quotes % 2 == 1;
        } else if !(hash || trimmed.starts_with("//")) {
            out.push_str(line.trim_end());
        }
        if lines.peek().is_some() {
            out.push('\n');
        }
    }
    out
}
