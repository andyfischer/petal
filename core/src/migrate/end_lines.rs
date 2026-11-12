//! `end-lines`: the `end` on a line of its own goes; indentation closes the
//! block (docs/syntax/indentation.md, docs/tasks/pythonic-syntax.md §7.2).
//!
//! Unlike the other migrations this one rewrites no old *form*: an `end` is
//! still the language, and stays the way to close a one-line body. It
//! converts a file from the spelling every block had before indentation
//! delimited blocks to the one written since. The steps:
//!
//! 1. **`petal fmt` first** ([`crate::fmt::format_source`]). Old source may
//!    be indented in a way that disagrees with its `end`s; the formatter
//!    indents it by them, so that afterwards the indentation alone says what
//!    the `end`s said. A file with no `end` at the start of a line is not
//!    formatted: there is nothing to migrate in it.
//! 2. **The `end`s go** ([`crate::fmt::remove_block_ends`]): an `end` alone
//!    on its line, and the `end` of `end)` / `end]` / `end}` (the bracket
//!    keeps the column). A comment after a removed `end` stays, on a line of
//!    its own at the block's column.
//! 3. **What stays:** an `end` on the line of the block it closes
//!    (`if c then a else b end`, `map(xs, fn(x) x * 2 end)`); `end,` (the
//!    next argument or element follows, and without the `end` the line
//!    would start with a comma: two block lambdas in one call are the usual
//!    case); `end |> f` and `end.len()`; the `end` of a block with no
//!    statement; one whose next line starts with an operator, which would
//!    then continue the block's last statement; and any other `end` whose
//!    removal changes the parse (`if c then a; b` / `end`).
//! 4. **The gate:** the migrated text parses, by the language's own rules,
//!    to the AST the original had (spans aside), where the original is read
//!    by its `end`s when its indentation disagrees with them. A file that
//!    fails is left as the earlier migrations wrote it and is reported.
//! 5. **Idempotent:** the output is a fixed point: a second run removes
//!    nothing and reformats nothing.
//!
//! `# petal-lint-ignore-file end-lines` keeps a file's `end`s.

use crate::lexer::{Lexer, Token};

/// What the migration did to one file.
pub(super) struct EndLines {
    pub(super) text: String,
    /// How many `end`s were removed.
    pub(super) removed: usize,
    /// How many `end`s at the start of a line were kept.
    pub(super) kept: usize,
    /// The 1-based line, in the text the migration was given, of the first
    /// `end` at the start of a line (where the finding is anchored).
    pub(super) first_line: usize,
}

/// Run the migration over `source`. `Ok(None)` when it has nothing to do.
/// An error is a file the migration could not convert: the formatter could
/// not lay it out, or the gate failed.
pub(super) fn migrate(source: &str) -> Result<Option<EndLines>, String> {
    let Some(first_line) = first_end_line(source) else {
        return Ok(None);
    };
    let (_, original, by_end) = crate::rewrite::parse_ast_or_by_end(source)?;
    let shape = crate::rewrite::ast_shape(&original)?;

    // To a fixed point: the formatter may settle a comment that outlived its
    // `end`, and what it settles must not open another removal on the next
    // run. One round does the work on every file of the corpus; the second
    // confirms it.
    let mut text = source.to_string();
    let mut removed = 0usize;
    let mut kept = 0usize;
    for round in 0..4 {
        let formatted = crate::fmt::format_source(&text)?;
        let step = crate::fmt::remove_block_ends(&formatted)?;
        kept = step.kept.len();
        if step.removed == 0 {
            // Nothing (more) to remove. The formatting of a round that
            // removed nothing is kept only where the file needed it to be
            // read at all, or where it settles what an earlier round wrote.
            if round > 0 || by_end {
                text = formatted;
            }
            break;
        }
        removed += step.removed;
        text = step.text;
    }
    if removed == 0 && !by_end {
        return Ok(None);
    }

    let (_, after) = crate::rewrite::parse_ast(&text).map_err(|e| {
        format!("migrate bug: without its `end` lines the file no longer parses ({e}) — refusing to produce output")
    })?;
    if crate::rewrite::ast_shape(&after)? != shape {
        return Err(
            "migrate bug: without its `end` lines the file parses to a different program — \
             refusing to produce output"
                .to_string(),
        );
    }
    Ok(Some(EndLines { text, removed, kept, first_line }))
}

/// The 1-based line of the first `end` token that starts a line, if any.
/// (It may turn out to spell a name, `end: 3`; the transform decides.)
fn first_end_line(source: &str) -> Option<usize> {
    if !source.contains("end") {
        return None;
    }
    let mut lexer = Lexer::new(source);
    lexer.tokenize().ok()?;
    (0..lexer.tokens.len())
        .find(|&i| matches!(lexer.tokens[i], Token::End) && lexer.token_first_on_line[i])
        .map(|i| lexer.token_spans[i].start.line as usize)
}
