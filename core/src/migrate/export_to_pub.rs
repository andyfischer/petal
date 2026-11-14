//! Migration `export-to-pub` — spell the export modifier `pub`.
//!
//! ```text
//! export fn area(r) r.w * r.h end      =>   pub fn area(r) r.w * r.h end
//! export import bloom/button: *        =>   pub import bloom/button: *
//! ```
//!
//! `export` is the deprecated spelling of `pub`: the parser accepts both and
//! builds the same declaration, so the rewrite is one keyword splice (and, at
//! most, some padding in front of a trailing comment) and the compiled
//! program does not change. It fires on every form the modifier
//! takes — `fn`, `let`, `var`, `config let`, `state`, `enum`, `class` and
//! `import`.
//!
//! The two words lex to different tokens ([`Token::Pub`], [`Token::Export`])
//! and parse to the same declaration, so nothing downstream of the parser can
//! tell them apart: the AST carries one `exported` flag and the IR is
//! identical. A site is found from the AST rather than the token stream
//! because `export` stays an ordinary field and record-key name (`r.export`,
//! `{export: 1}`), which lexes to the same token and must be left alone. An
//! exported statement's span starts at its modifier, so the word at that
//! offset says which spelling was used.
//!
//! The compiler says nothing about the old spelling, so `petal lint` reports
//! it (report only, as `old-syntax`) and points here.
//!
//! [`Token::Pub`]: crate::lexer::Token::Pub
//! [`Token::Export`]: crate::lexer::Token::Export

use crate::ast::{ExprVisitor, Stmt, walk_stmt};
use crate::lint::Fix;
use crate::rewrite::Splice;
use crate::source_map::SourceSpan;

/// Plan every `export` → `pub` respelling in `stmts`, in source order.
/// `chars` is the text they were parsed from.
pub(super) fn plan_export_fixes(stmts: &[Stmt], chars: &[char]) -> Result<Vec<Fix>, String> {
    let sites = deprecated_export_spans(stmts, chars);
    let source: String = chars.iter().collect();
    // The keyword splice, plus padding where the shorter word would pull a
    // trailing comment out of a column it shares (see `plan_pub_rewrite`).
    let edits = plan_pub_rewrite(&source, &sites)?;
    Ok(sites
        .iter()
        .zip(edits)
        .map(|(site, edits)| Fix {
            anchor: site.start.offset as usize,
            message: format!("`{DEPRECATED}` is deprecated; write `{REPLACEMENT}`"),
            splices: edits,
        })
        .collect())
}

/// The deprecated modifier and the one that replaces it.
const DEPRECATED: &str = "export";
const REPLACEMENT: &str = "pub";

/// The span of every `export` modifier in `stmts`, in source order. `chars`
/// is the source the statements were parsed from; each span covers just the
/// keyword.
pub(super) fn deprecated_export_spans(stmts: &[Stmt], chars: &[char]) -> Vec<SourceSpan> {
    struct Finder<'a> {
        chars: &'a [char],
        out: Vec<SourceSpan>,
    }
    impl ExprVisitor for Finder<'_> {
        fn visit_stmt(&mut self, s: &Stmt) {
            // Past any `@` annotations (`@tunable export let X = 1`).
            let at = s.span.start.offset as usize;
            let kw = crate::ast::skip_annotations(self.chars, at);
            if s.exported && starts_with_export(self.chars, kw) {
                let mut start = s.span.start;
                for c in &self.chars[at..kw] {
                    start.offset += 1;
                    if *c == '\n' {
                        start.line += 1;
                        start.column = 1;
                    } else {
                        start.column += 1;
                    }
                }
                let len = DEPRECATED.len() as u32;
                let mut end = start;
                end.column += len;
                end.offset += len;
                self.out.push(SourceSpan {
                    start,
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
fn plan_pub_rewrite(source: &str, sites: &[SourceSpan]) -> Result<Vec<Vec<Splice>>, String> {
    let comments = crate::fmt::trailing_comments(source)?;
    let comment_col = |line: usize| comments.get(line).copied().flatten().map(|(_, col)| col);
    let rewritten: std::collections::HashSet<usize> =
        sites.iter().map(|s| s.start.line as usize - 1).collect();
    let shrink = DEPRECATED.len() - REPLACEMENT.len();

    let mut out = Vec::with_capacity(sites.len());
    for site in sites {
        let start = site.start.offset as usize;
        let mut edits = vec![Splice {
            start,
            end: start + DEPRECATED.len(),
            text: REPLACEMENT.to_string(),
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
                edits.push(Splice {
                    start: at,
                    end: at,
                    text: " ".repeat(shrink),
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
    fn an_annotated_declaration_reports_its_modifier() {
        assert_eq!(spans("@tunable export let a = 1\n"), vec![(1, 10, 16)]);
        assert_eq!(spans("@tunable(0, 9)\n// c\nexport let a = 1\n"), vec![(3, 1, 7)]);
        assert_eq!(spans("@tunable /* c */ export let a = 1\n"), vec![(1, 18, 24)]);
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
