//! `petal fmt` — the canonical layout of Petal source.
//!
//! Like `gofmt` there are no options: one layout, so a formatted file looks
//! the same whoever formatted it. Unlike a pretty-printer, nothing is
//! reprinted from the AST — every pass edits the whitespace *between* tokens
//! and copies the tokens (and comments) through verbatim, which is how a
//! comment inside an expression survives. No token's text changes: an old
//! spelling (the `export` modifier) is `petal migrate-old-code`'s to rewrite
//! ([`crate::migrate`]), and fmt lays it out as written. The passes, in order:
//!
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
//! **The gate.** Indentation delimits blocks, and the lexer looks at spacing
//! too (`a < b` versus the tag `a <b`). [`format_source`] therefore re-reads
//! its output and refuses it unless it is the program it started from: the
//! same token stream (blank-line runs aside) and the same AST (spans aside),
//! so a rule that fused two tokens or moved a line out of its block can cost
//! a file its formatting, never its meaning. The corpus tests additionally
//! hold every repo `.ptl` to identical compiled IR.
//!
//! **Blocks.** A block may be closed by an `end` or by its indentation alone,
//! and a file may mix the two. The indentation pass takes the block
//! structure from the parse ([`Blocks`]) and only normalises how far each
//! level is indented; it adds and removes no `end`. Removing the `end`s that
//! indentation makes redundant is a separate transform,
//! [`remove_block_ends`], which `petal fmt` does not run.
//!
//! **Opting out.** `// petal-fmt-ignore` on its own line keeps the next line
//! exactly as written; `// petal-fmt-off` … `// petal-fmt-on` keeps every line
//! between them; `// petal-fmt-ignore-file` anywhere leaves the whole file
//! alone. Use them for hand-aligned tables, where the columns carry meaning.

pub mod blocks;
mod comments;
mod ends;
pub mod reindent;
mod spacing;

pub(crate) use comments::trailing_comments;
pub use blocks::Blocks;
pub use ends::{EndRemoval, remove_block_ends};
pub use reindent::reindent;

use crate::lexer::{Lexer, Token};

pub const IGNORE_NEXT: &str = "petal-fmt-ignore";
pub const IGNORE_FILE: &str = "petal-fmt-ignore-file";
pub const OFF: &str = "petal-fmt-off";
pub const ON: &str = "petal-fmt-on";

/// Format `source`. Errors when it does not parse (nothing is ever written for
/// such a file) and — a formatter bug, not a user error — when the result
/// would not be the same program.
pub fn format_source(source: &str) -> Result<String, String> {
    // Parsed first: nothing is written for a file that does not parse, and
    // the parse is where the blocks are: one closed by its indentation alone
    // shows in no token.
    let (tree, stmts, reading) = read_layout(source)?;
    let directives = Directives::scan(source)?;
    if directives.ignore_file {
        return Ok(source.to_string());
    }
    let protected = &directives.protected;
    let blocks = Blocks::of(&tree);

    // Alignment is judged against the neighbouring lines as they currently
    // stand, and respacing one line can knock its neighbour out of line, so
    // run the passes to a fixed point. It always arrives: a pass only ever
    // keeps or removes an alignment run, never adds one.
    let mut text = source.to_string();
    for _ in 0..8 {
        let spaced = spacing::respace(&text, protected)?;
        let indented = reindent::reindent_protected(&spaced, protected, &blocks)?;
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
    // The same tokens are not yet the same program: indentation delimits
    // blocks, so moving a line can move a statement out of its block. The
    // output must parse to the AST the input did. (Skipped when nothing
    // changed.)
    if reading != Reading::Layout || output != source {
        same_program(&stmts, &output).map_err(|e| match reading {
            Reading::ByEnd => format!(
                "the indentation of this file disagrees with its `end`s, and indenting it by \
                 its `end`s did not give the same program ({e})"
            ),
            Reading::Tabs => format!(
                "this file is indented with tabs, and replacing them did not give the same \
                 program ({e})"
            ),
            Reading::Layout => {
                format!("fmt bug: {e} — refusing to produce output")
            }
        })?;
    }
    Ok(output)
}

/// Check that `text` parses, by the language's own rules, to the program
/// `stmts` is: the same AST with the spans dropped. The gate of every
/// rewrite that may only change layout (`petal fmt`, [`remove_block_ends`]).
fn same_program(stmts: &[crate::ast::Stmt], text: &str) -> Result<(), String> {
    let (_tree, after) = crate::rewrite::parse_ast(text)
        .map_err(|e| format!("the rewritten text no longer parses: {e}"))?;
    if crate::rewrite::ast_shape(stmts)? != crate::rewrite::ast_shape(&after)? {
        return Err("the rewritten text parses to a different program".to_string());
    }
    Ok(())
}

/// Which reading of a file's layout `petal fmt` formats it by.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Reading {
    /// The language's own.
    Layout,
    /// By its `end`s alone ([`crate::cst::parse_source_by_end`]): a file from
    /// before indentation delimited blocks, indented in a way that disagrees
    /// with them.
    ByEnd,
    /// The language's own, were tabs allowed in indentation.
    Tabs,
}

/// Parse `source` for formatting. The language's own reading comes first.
/// A file it rejects is read by its `end`s, which is what its author's
/// compiler did; the indentation pass then derives the indentation from
/// them. A file that is rejected only for the tabs it is indented with is
/// read with a tab as one column, provided the width of a tab does not
/// decide what the file means. The error, when no reading parses, is the
/// language's own.
fn read_layout(
    source: &str,
) -> Result<(std::rc::Rc<crate::cst::GreenNode>, Vec<crate::ast::Stmt>, Reading), String> {
    let (tree, stmts, by_end) = match crate::rewrite::parse_ast_or_by_end(source) {
        Ok(parsed) => parsed,
        Err(e) => {
            let Ok((tree, stmts)) = crate::cst::parse_source_tabs_as_columns(source) else {
                return Err(e);
            };
            // The same program whether a tab is one column or eight?
            let wide = expand_tab_indentation(source, 8)?;
            if same_program(&stmts, &wide).is_err() {
                return Err(e);
            }
            return Ok((tree, stmts, Reading::Tabs));
        }
    };
    Ok((tree, stmts, if by_end { Reading::ByEnd } else { Reading::Layout }))
}

/// `source` with each tab in the indentation of a line of code written as
/// `width` spaces. Only the whitespace in front of the first token of a line
/// is indentation; the lines of a raw string are left alone.
fn expand_tab_indentation(source: &str, width: usize) -> Result<String, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut copied = 0usize;
    for (k, span) in lexer.token_spans.iter().enumerate() {
        if !lexer.token_tab_indented.get(k).copied().unwrap_or(false) {
            continue;
        }
        let at = span.start.offset as usize;
        let mut start = at;
        while start > 0 && matches!(chars[start - 1], ' ' | '\t') {
            start -= 1;
        }
        out.extend(&chars[copied..start]);
        for &c in &chars[start..at] {
            if c == '\t' {
                out.push_str(&" ".repeat(width));
            } else {
                out.push(c);
            }
        }
        copied = at;
    }
    out.extend(&chars[copied..]);
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
        // The lines a `/* … */` comment runs onto are its text, not layout,
        // so they keep their indentation and their blank lines, as the inside
        // of a raw string does.
        for line in block_comment_lines(source)? {
            if let Some(p) = protected.get_mut(line) {
                *p = true;
            }
        }
        Ok(Directives {
            ignore_file,
            protected,
        })
    }
}

/// The first word of a line comment's text (`# …`, or the deprecated
/// `// …`), when it names a `petal-` directive.
pub(crate) fn directive_word(comment: &str) -> Option<&str> {
    let body = crate::trivia::comment_body(comment)?.trim_start();
    let word = body.split(|c: char| c.is_whitespace() || c == ':').next()?;
    word.starts_with("petal-").then_some(word)
}

/// The 0-based lines that begin inside a multi-line `/* … */` comment: every
/// line of one but its first.
fn block_comment_lines(source: &str) -> Result<Vec<usize>, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let mut line_starts = vec![0usize];
    for (i, c) in source.chars().enumerate() {
        if c == '\n' {
            line_starts.push(i + 1);
        }
    }
    let line_of = |offset: usize| line_starts.partition_point(|&s| s <= offset) - 1;
    let mut out = Vec::new();
    for t in lexer.token_leading_trivia.iter().flatten() {
        if t.kind == crate::trivia::TriviaKind::BlockComment {
            let first = line_of(t.start as usize);
            let breaks = t.text.matches('\n').count();
            out.extend(first + 1..=first + breaks);
        }
    }
    Ok(out)
}

/// Every line comment in `source` (`#`, or the deprecated `//`) as (0-based
/// line, text, is it alone on its line). Comments are found in the gaps
/// between tokens, so text that merely looks like one inside a string is
/// never picked up.
pub(crate) fn line_comments(source: &str) -> Result<Vec<(usize, String, bool)>, String> {
    line_comments_with(source, false)
}

/// [`line_comments`], with `legacy` to read source that may still hold `#f80`
/// colors ([`Lexer::legacy`]): for `petal migrate-old-code` only.
pub(crate) fn line_comments_with(source: &str, legacy: bool) -> Result<Vec<(usize, String, bool)>, String> {
    let mut lexer = if legacy { Lexer::new(source).legacy() } else { Lexer::new(source) };
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
