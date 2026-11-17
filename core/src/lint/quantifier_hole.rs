//! Rule `quantifier-hole`: an interpolation hole that holds only a whole
//! number, written tight against its braces — `"\\d{4}"`, `"{2}"`.
//!
//! ```text
//! let year = "\\d{4}"      // reported: the string is `\d4`, not a regex
//! let s = "{n} items"      // not reported: the hole is an expression
//! ```
//!
//! `{1,2}` and `{2,}` do not parse as a hole, and the parser says why
//! (`Parser::interp_brace_error`). `{4}` parses — it interpolates the number 4
//! — which no one writes on purpose: it is a regex quantifier that lost its
//! braces. Report only: the fix is a raw string, `"""\d{4}"""`, and whether the
//! author meant that is theirs to say.

use crate::ast::{Expr, ExprKind, ExprVisitor, Literal, Stmt, walk_expr};

use super::Fix;

pub(super) fn plan_quantifier_hole_reports(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    struct Finder<'a> {
        chars: &'a [char],
        fixes: Vec<Fix>,
    }
    impl ExprVisitor for Finder<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            if let ExprKind::StringInterp { parts, exprs } = &e.kind {
                self.one(parts, exprs);
            }
            walk_expr(self, e);
        }
    }
    impl Finder<'_> {
        /// The number in a hole written exactly `{N}`, and where its `{` is.
        fn tight_int(&self, e: &Expr) -> Option<(i64, usize)> {
            let ExprKind::Literal(Literal::Int(n)) = e.kind else {
                return None;
            };
            let start = e.span.start.offset as usize;
            let end = e.span.end.offset as usize;
            let brace = start.checked_sub(1)?;
            (self.chars.get(brace) == Some(&'{') && self.chars.get(end) == Some(&'}') && n >= 0)
                .then_some((n, brace))
        }

        fn one(&mut self, parts: &[String], exprs: &[Expr]) {
            let holes: Vec<Option<(i64, usize)>> =
                exprs.iter().map(|e| self.tight_int(e)).collect();
            // The whole string as a raw one, when every hole is a quantifier.
            let raw = holes.iter().all(Option::is_some).then(|| {
                let mut out = String::new();
                for (i, part) in parts.iter().enumerate() {
                    out.push_str(part);
                    if let Some(Some((n, _))) = holes.get(i) {
                        out.push_str(&format!("{{{n}}}"));
                    }
                }
                out
            });
            let raw = raw
                .filter(|r| !r.contains("\"\"\"") && !r.contains('\n') && !r.ends_with('"'))
                .map_or_else(|| "\"\"\"…\"\"\"".to_string(), |r| format!("\"\"\"{r}\"\"\""));
            for (n, brace) in holes.into_iter().flatten() {
                self.fixes.push(Fix {
                    anchor: brace,
                    message: format!(
                        "`{{{n}}}` in a double-quoted string is an interpolation hole: it \
                         inserts the number {n}, and the braces are gone. If it is a regex \
                         quantifier, write the pattern as a raw string, {raw}, where braces \
                         and backslashes are literal (or write `\\{{{n}}}`); if the number is \
                         meant, write it without the braces"
                    ),
                    splices: Vec::new(),
                });
            }
        }
    }
    let mut finder = Finder {
        chars,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes
}
