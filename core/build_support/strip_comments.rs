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
//! - a line whose first non-blank characters are `//` becomes an empty line;
//! - every other line loses its trailing whitespace and is otherwise kept.
//!
//! Lines are never removed, so every token keeps its line number, and nothing
//! before a token on its line changes, so it keeps its column too: error
//! positions inside an embedded module read the same as in the repository
//! file. A `//` after code on the same line is left alone (telling it apart
//! from `//` inside a string needs the real lexer), and so is a line inside a
//! multi-line string that happens to start with `//` — which is why the check
//! test lexes the result rather than trusting this rule.

/// `source` with its whole-line `//` comments blanked and trailing whitespace
/// trimmed. Line count and every token's line and column are unchanged.
pub fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len() / 2);
    let mut lines = source.split('\n').peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//") {
            out.push_str(line.trim_end());
        }
        if lines.peek().is_some() {
            out.push('\n');
        }
    }
    out
}
