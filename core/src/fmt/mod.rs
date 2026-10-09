//! `petal fmt` — the canonical layout of Petal source.
//!
//! Like `gofmt` there are no options: one layout, so a formatted file looks
//! the same whoever formatted it. Unlike a pretty-printer, nothing is
//! reprinted from the AST — every pass edits the whitespace *between* tokens
//! and copies the tokens (and comments) through verbatim, which is how a
//! comment inside an expression survives. The passes, in order:
//!
//! 0. **Keywords** ([`normalize_keywords`]) — the one place a token's text
//!    changes: the deprecated modifier `export` is written `pub`. The two
//!    spell the same declaration, so this is layout in the sense that matters
//!    here — the program is untouched.
//! 1. **Spacing** ([`spacing`]) — the gaps inside a line: one space around
//!    binary operators and after `,`/`:`, none inside brackets, and so on.
//! 2. **Indentation** ([`reindent`]) — 2 spaces per open construct, computed
//!    from the token stream; trailing whitespace trimmed.
//! 3. **Trailing comments** ([`comments::align_trailing_comments`]) — keep
//!    their column, at least one space from the code; on consecutive lines at
//!    one indent they share the rightmost column.
//!
//! **Alignment is kept.** A run of spaces that lines a token up with a token
//! on the line above or below — a table of records, a column of `=` — is left
//! as written; any other run collapses to one space. This is a deliberate
//! difference from gofmt and deno fmt, which flatten such tables: Petal code
//! is full of hand-aligned data (level layouts, vertex lists), and a
//! formatter that destroyed it would be switched off. The passes run to a
//! fixed point, so the result is still stable.
//! 4. **Blank lines** ([`comments::collapse_blank_lines`]) — at most one in a
//!    row, none at the start or end of the file, and a single final newline.
//!
//! What it deliberately does not do: wrap long lines (that needs the
//! reprinter this design avoids), reorder anything, or touch the inside of a
//! string, a raw string, or JSX text. Choices about *which* code to write —
//! `var` versus `let`, an `if` chain versus a `match` — are `petal lint`'s.
//!
//! **The gate.** Petal is newline-significant but not whitespace-significant,
//! except where the lexer looks at spacing (`a < b` versus the tag `a <b`).
//! [`format_source`] therefore re-lexes its output and refuses it unless the
//! token stream is the one it started from (blank-line runs aside, and
//! `export` counted as the `pub` it was rewritten to), so a
//! spacing rule that fused two tokens can cost a file its formatting, never
//! its meaning. The corpus tests additionally hold every repo `.ptl` to
//! identical compiled IR.
//!
//! **Opting out.** `// petal-fmt-ignore` on its own line keeps the next line
//! exactly as written; `// petal-fmt-off` … `// petal-fmt-on` keeps every line
//! between them; `// petal-fmt-ignore-file` anywhere leaves the whole file
//! alone. Use them for hand-aligned tables, where the columns carry meaning.

mod comments;
pub mod reindent;
mod spacing;

pub(crate) use comments::trailing_comments;
pub use reindent::reindent;

use crate::lexer::{Lexer, Token};

pub const IGNORE_NEXT: &str = "petal-fmt-ignore";
pub const IGNORE_FILE: &str = "petal-fmt-ignore-file";
pub const OFF: &str = "petal-fmt-off";
pub const ON: &str = "petal-fmt-on";

/// Format `source`. Errors when it does not parse (nothing is ever written for
/// such a file) and — a formatter bug, not a user error — when the result
/// would not lex to the same tokens.
pub fn format_source(source: &str) -> Result<String, String> {
    format_with(source, true)
}

/// [`format_source`] without the keyword pass: whitespace only. `petal lint`
/// formats its fixed text with this, because rewriting `export` is a lint
/// rule of its own there (`prefer-pub`) and a rule the user silenced must
/// stay silenced.
pub(crate) fn format_layout(source: &str) -> Result<String, String> {
    format_with(source, false)
}

fn format_with(source: &str, keywords: bool) -> Result<String, String> {
    let (_tree, stmts) = crate::rewrite::parse_ast(source)?;
    let directives = Directives::scan(source)?;
    if directives.ignore_file {
        return Ok(source.to_string());
    }
    let protected = &directives.protected;
    // The keyword pass replaces a word within its line, so the line numbers
    // `protected` is indexed by still hold for the passes below.
    let normalized;
    let source = if keywords {
        normalized = normalize_keywords(source, &stmts, protected)?;
        normalized.as_str()
    } else {
        source
    };

    // Alignment is judged against the neighbouring lines as they currently
    // stand, and respacing one line can knock its neighbour out of line, so
    // run the passes to a fixed point. It always arrives: a pass only ever
    // keeps or removes an alignment run, never adds one.
    let mut text = source.to_string();
    for _ in 0..8 {
        let spaced = spacing::respace(&text, protected)?;
        let indented = reindent::reindent_protected(&spaced, protected)?;
        let aligned = comments::align_trailing_comments(&indented, protected)?;
        if aligned == text {
            break;
        }
        text = aligned;
    }
    let output = comments::collapse_blank_lines(&text, protected)?;

    let before = token_signature(source)?;
    let after = token_signature(&output)
        .map_err(|e| format!("fmt bug: the formatted text no longer lexes ({e})"))?;
    if before != after {
        let at = before
            .iter()
            .zip(&after)
            .position(|(a, b)| a != b)
            .unwrap_or(before.len().min(after.len()));
        return Err(format!(
            "fmt bug: formatting changed the token stream (first difference at token {at}: \
             {:?} became {:?}) — refusing to produce output",
            before.get(at),
            after.get(at)
        ));
    }
    Ok(output)
}

/// Rewrite every `export` modifier to `pub`, outside protected lines. The
/// sites come from the AST ([`crate::export_keyword`]), so `export` used as a
/// field or record key is left alone.
///
/// This pass changes tokens, which the gate in [`format_with`] cannot vouch
/// for (it compares against this pass's output), so it carries its own: the
/// result must lex to the original stream with exactly the planned `Export`
/// tokens turned into `Pub`.
fn normalize_keywords(
    source: &str,
    stmts: &[crate::ast::Stmt],
    protected: &[bool],
) -> Result<String, String> {
    use crate::export_keyword::{deprecated_export_spans, plan_pub_rewrite};
    let chars: Vec<char> = source.chars().collect();
    let sites: Vec<_> = deprecated_export_spans(stmts, &chars)
        .into_iter()
        .filter(|span| {
            !protected
                .get(span.start.line as usize - 1)
                .copied()
                .unwrap_or(false)
        })
        .collect();
    if sites.is_empty() {
        return Ok(source.to_string());
    }
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    for edit in plan_pub_rewrite(source, &sites)?.into_iter().flatten() {
        out.extend(&chars[at..edit.start]);
        out.push_str(edit.text);
        at = edit.end;
    }
    out.extend(&chars[at..]);

    let before = token_signature(source)?;
    let after = token_signature(&out)
        .map_err(|e| format!("fmt bug: rewriting `export` broke the lexer ({e})"))?;
    let rewritten = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| matches!((a, b), (Token::Export, Token::Pub)))
        .count();
    let same = before.len() == after.len()
        && before
            .iter()
            .zip(&after)
            .all(|(a, b)| a == b || matches!((a, b), (Token::Export, Token::Pub)));
    if !same || rewritten != sites.len() {
        return Err(
            "fmt bug: rewriting `export` to `pub` changed more than the keyword — refusing to \
             produce output"
                .to_string(),
        );
    }
    Ok(out)
}

/// The tokens of `source`, with every run of `Newline`s collapsed to one and
/// none at either end: blank lines are layout, a line break is not.
fn token_signature(source: &str) -> Result<Vec<Token>, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let mut out: Vec<Token> = Vec::with_capacity(lexer.tokens.len());
    for t in lexer.tokens.drain(..) {
        if matches!(t, Token::Newline) && matches!(out.last(), None | Some(Token::Newline)) {
            continue;
        }
        if matches!(t, Token::Eof) && matches!(out.last(), Some(Token::Newline)) {
            out.pop();
        }
        out.push(t);
    }
    Ok(out)
}

/// The `// petal-fmt-*` comments in a file.
struct Directives {
    ignore_file: bool,
    /// Per 0-based line: keep this line exactly as written.
    protected: Vec<bool>,
}

impl Directives {
    fn scan(source: &str) -> Result<Self, String> {
        let line_count = source.split('\n').count();
        let mut protected = vec![false; line_count];
        let mut ignore_file = false;
        let mut off_since: Option<usize> = None;
        for (line, text, whole_line) in line_comments(source)? {
            let word = directive_word(&text);
            match word {
                Some(w) if w == IGNORE_FILE => ignore_file = true,
                Some(w) if w == IGNORE_NEXT && whole_line => {
                    if let Some(p) = protected.get_mut(line + 1) {
                        *p = true;
                    }
                }
                Some(w) if w == OFF && whole_line => {
                    off_since.get_or_insert(line);
                }
                Some(w) if w == ON && whole_line => {
                    if let Some(start) = off_since.take() {
                        for p in &mut protected[start + 1..line] {
                            *p = true;
                        }
                    }
                }
                _ => {}
            }
        }
        // An `off` with no `on` runs to the end of the file.
        if let Some(start) = off_since {
            for p in &mut protected[start + 1..] {
                *p = true;
            }
        }
        Ok(Directives {
            ignore_file,
            protected,
        })
    }
}

/// The first word of a `//` comment's text, when it names a `petal-` directive.
pub(crate) fn directive_word(comment: &str) -> Option<&str> {
    let body = comment.strip_prefix("//")?.trim_start();
    let word = body.split(|c: char| c.is_whitespace() || c == ':').next()?;
    word.starts_with("petal-").then_some(word)
}

/// Every `//` comment in `source` as (0-based line, text, is it alone on its
/// line). Comments are found in the gaps between tokens, so text that merely
/// looks like one inside a string is never picked up.
pub(crate) fn line_comments(source: &str) -> Result<Vec<(usize, String, bool)>, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    // Char offset of the start of each line, for offset -> line lookups.
    let mut line_starts = vec![0usize];
    for (i, c) in source.chars().enumerate() {
        if c == '\n' {
            line_starts.push(i + 1);
        }
    }
    let line_of = |offset: usize| line_starts.partition_point(|&s| s <= offset) - 1;
    let mut out = Vec::new();
    for (k, trivia) in lexer.token_leading_trivia.iter().enumerate() {
        for t in trivia {
            if t.kind != crate::trivia::TriviaKind::LineComment {
                continue;
            }
            let line = line_of(t.start as usize);
            // Alone on its line when no token before it ends on this line.
            let whole_line = k == 0
                || matches!(lexer.tokens[k - 1], Token::Newline)
                || line_of(lexer.token_spans[k - 1].end.offset as usize) < line;
            out.push((line, t.text.clone(), whole_line));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
