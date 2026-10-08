//! `export`, the deprecated spelling of the `pub` modifier.
//!
//! The two words lex to different tokens ([`Token::Pub`], [`Token::Export`])
//! and parse to the same declaration, so nothing downstream of the parser can
//! tell them apart: the AST carries one `exported` flag and the IR is
//! identical. This module is the one place that still can. It finds every
//! `export` written as a modifier, and three callers act on that:
//!
//! - the module loader, which turns each site into a deprecation warning on
//!   the compiled program (`petal check`, the LSP);
//! - `petal lint`'s `prefer-pub` rule, which rewrites it under `--fix`;
//! - `petal fmt`, which normalizes it.
//!
//! A site is found from the AST rather than the token stream because
//! `export` stays an ordinary field and record-key name (`r.export`,
//! `{export: 1}`), which lexes to the same token and must be left alone. An
//! exported statement's span starts at its modifier, so the word at that
//! offset says which spelling was used.
//!
//! [`Token::Pub`]: crate::lexer::Token::Pub
//! [`Token::Export`]: crate::lexer::Token::Export

use crate::ast::{ExprVisitor, Stmt, walk_stmt};
use crate::source_map::SourceSpan;

/// The deprecated modifier and the one that replaces it.
pub const DEPRECATED: &str = "export";
pub const REPLACEMENT: &str = "pub";

/// The message `petal check` and the LSP attach to each deprecated site.
pub const DEPRECATION_MESSAGE: &str =
    "`export` is deprecated: write `pub` instead (`petal lint --fix` rewrites it)";

/// The span of every `export` modifier in `stmts`, in source order. `chars`
/// is the source the statements were parsed from; each span covers just the
/// keyword.
pub fn deprecated_export_spans(stmts: &[Stmt], chars: &[char]) -> Vec<SourceSpan> {
    struct Finder<'a> {
        chars: &'a [char],
        out: Vec<SourceSpan>,
    }
    impl ExprVisitor for Finder<'_> {
        fn visit_stmt(&mut self, s: &Stmt) {
            if s.exported && starts_with_export(self.chars, s.span.start.offset as usize) {
                let len = DEPRECATED.len() as u32;
                let mut end = s.span.start;
                end.column += len;
                end.offset += len;
                self.out.push(SourceSpan {
                    start: s.span.start,
                    end,
                    file: s.span.file,
                });
            }
            // `export` is only meaningful at the top level, but the parser
            // accepts it on any statement, so a nested one is found too.
            walk_stmt(self, s);
        }
    }
    let mut finder = Finder {
        chars,
        out: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.out.sort_by_key(|span| span.start.offset);
    finder.out
}

/// One replacement of the chars `start..end` (char offsets) with `text`.
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: &'static str,
}

/// The edits that respell each of `sites` (spans from
/// [`deprecated_export_spans`] over `source`) as `pub`: per site, in source
/// order, the keyword replacement and — sometimes — a second edit behind it.
///
/// `pub` is three columns shorter than `export`, so everything after it on
/// the line moves left, a trailing comment included. That is right when the
/// comment stands alone or its whole aligned group moves with it (a table of
/// `export let`s). When the group also holds a line that is *not* being
/// rewritten, the column is shared with something that stays put, so the
/// second edit pads in front of the comment to keep it where it was.
pub fn plan_pub_rewrite(source: &str, sites: &[SourceSpan]) -> Result<Vec<Vec<Edit>>, String> {
    let comments = crate::fmt::trailing_comments(source)?;
    let comment_col = |line: usize| comments.get(line).copied().flatten().map(|(_, col)| col);
    let rewritten: std::collections::HashSet<usize> =
        sites.iter().map(|s| s.start.line as usize - 1).collect();
    let shrink = DEPRECATED.len() - REPLACEMENT.len();

    let mut out = Vec::with_capacity(sites.len());
    for site in sites {
        let start = site.start.offset as usize;
        let mut edits = vec![Edit {
            start,
            end: start + DEPRECATED.len(),
            text: REPLACEMENT,
        }];
        let line = site.start.line as usize - 1;
        if let Some(col) = comment_col(line) {
            // The aligned group: the run of adjacent lines whose trailing
            // comments sit in this same column.
            let mut first = line;
            while first > 0 && comment_col(first - 1) == Some(col) {
                first -= 1;
            }
            let mut last = line;
            while comment_col(last + 1) == Some(col) {
                last += 1;
            }
            if (first..=last).any(|l| !rewritten.contains(&l)) {
                let at = start - (site.start.column as usize - 1) + col;
                edits.push(Edit {
                    start: at,
                    end: at,
                    text: &"      "[..shrink],
                });
            }
        }
        out.push(edits);
    }
    Ok(out)
}

/// Is the word at char offset `at` exactly `export`?
fn starts_with_export(chars: &[char], at: usize) -> bool {
    let end = at + DEPRECATED.len();
    end <= chars.len()
        && chars[at..end].iter().copied().eq(DEPRECATED.chars())
        && chars
            .get(end)
            .is_none_or(|c| !(c.is_alphanumeric() || *c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(src: &str) -> Vec<(u32, u32, u32)> {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        deprecated_export_spans(&stmts, &chars)
            .iter()
            .map(|s| (s.start.line, s.start.column, s.end.column))
            .collect()
    }

    #[test]
    fn finds_every_modifier_form() {
        let src = "export import m: *\n\
                   export fn f() 1 end\n\
                   export let a = 1\n\
                   export var b = 2\n\
                   export config let c = 3\n\
                   export state d = 4\n\
                   export enum E A, B end\n\
                   export class P x: int end\n";
        let found = spans(src);
        assert_eq!(found.len(), 8, "{found:?}");
        for (i, (line, col, end)) in found.iter().enumerate() {
            assert_eq!((*line, *col, *end), (i as u32 + 1, 1, 7));
        }
    }

    #[test]
    fn pub_is_not_reported() {
        assert!(spans("pub import m\npub fn f() 1 end\npub let a = 1\n").is_empty());
    }

    #[test]
    fn export_as_a_field_or_key_is_not_a_modifier() {
        // `export` lexes to the keyword token here too; only the AST knows
        // these are names.
        assert!(spans("let r = {export: 1}\nprint(r.export)\n").is_empty());
        // ...including on a statement that *is* exported.
        assert_eq!(spans("pub let r = {export: 1}\n"), vec![]);
        assert_eq!(spans("export let r = {export: 1}\n"), vec![(1, 1, 7)]);
    }
}
