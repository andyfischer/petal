//! Rule — `len(filter(xs, p)) > 0` is `any(xs, p)`.
//!
//! ```text
//! if len(filter(fights, fn(f) -> f.id == on)) > 0 then   =>   if any(fights, fn(f) -> f.id == on) then
//! if len(filter(offs, fn(id) -> id == 3)) == 0 then      =>   if !any(offs, fn(id) -> id == 3) then
//! ```
//!
//! `filter` builds a list of every match to count it; `any` stops at the
//! first. That is also the one way they differ: `any` does not call the
//! predicate on the elements after the first match. So the fix is applied
//! only when the predicate is a lambda written in place whose body is a
//! single expression that cannot have an effect ([`super::util::is_pure`]).
//! Any other predicate is reported, for the author to decide. A pure body can
//! still raise (a field of a value that has none), and `any` does not reach
//! the later elements, so a fixed program can finish where the old one
//! raised; it never raises where the old one did not.
//!
//! Nothing is reported in a file that binds `len`, `filter` or `any` itself.

use std::collections::HashSet;

use crate::ast::{BinOp, Expr, ExprKind, ExprVisitor, Literal, Stmt, StmtKind, walk_expr};

use super::Fix;
use super::to_match::Splice;
use super::util::{bound_names, call_of, is_pure, squash, text};

pub(super) fn plan_any_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let bound = bound_names(stmts);
    if ["len", "filter", "any"].iter().any(|n| bound.contains(*n)) {
        return Vec::new();
    }
    let mut finder = Finder {
        chars,
        bound,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

struct Finder<'a> {
    chars: &'a [char],
    bound: HashSet<String>,
    fixes: Vec<Fix>,
}

impl ExprVisitor for Finder<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        if let Some(fix) = self.plan(e) {
            self.fixes.push(fix);
        }
        walk_expr(self, e);
    }
}

impl Finder<'_> {
    fn plan(&self, e: &Expr) -> Option<Fix> {
        let ExprKind::BinaryOp { op, left, right } = &e.kind else {
            return None;
        };
        let ExprKind::Literal(Literal::Int(n)) = right.kind else {
            return None;
        };
        // Is the comparison "there is a match" or "there is none"?
        let (some, op_text) = match (op, n) {
            (BinOp::Gt, 0) => (true, ">0"),
            (BinOp::Ne, 0) => (true, "!=0"),
            (BinOp::Ge, 1) => (true, ">=1"),
            (BinOp::Eq, 0) => (false, "==0"),
            (BinOp::Lt, 1) => (false, "<1"),
            (BinOp::Le, 0) => (false, "<=0"),
            _ => return None,
        };
        let [inner] = call_of(left, "len")? else {
            return None;
        };
        let [list, pred] = call_of(inner, "filter")? else {
            return None;
        };
        let start = e.span.start.offset as usize;
        let end = e.span.end.offset as usize;
        let list_start = list.span.start.offset as usize;
        let filter_end = inner.span.end.offset as usize;
        // The text around the two arguments is exactly the calls and the
        // comparison, with no comment in it.
        if squash(&text(self.chars, start, list_start)?) != "len(filter("
            || squash(&text(self.chars, filter_end, end)?) != format!("){op_text}")
        {
            return None;
        }
        let call = if some { "any(" } else { "!any(" };
        let safe = match &pred.kind {
            ExprKind::Lambda { params, body } => {
                params.iter().all(|p| p.default.is_none())
                    && matches!(body.as_slice(),
                        [Stmt { kind: StmtKind::Expr(x), .. }] if is_pure(x, &self.bound))
            }
            _ => false,
        };
        let spelled = if some { "any(xs, p)" } else { "!any(xs, p)" };
        let (message, splices) = if safe {
            (
                format!("`len(filter(xs, p)) {}` can be written `{spelled}`", spaced(op_text)),
                vec![
                    Splice {
                        start,
                        end: list_start,
                        text: call.to_string(),
                    },
                    Splice {
                        start: filter_end,
                        end,
                        text: String::new(),
                    },
                ],
            )
        } else {
            (
                format!(
                    "`len(filter(xs, p)) {}` is `{spelled}` if the predicate has no side \
                     effects: `any` stops at the first match",
                    spaced(op_text)
                ),
                Vec::new(),
            )
        };
        Some(Fix {
            anchor: start,
            message,
            splices,
        })
    }
}

/// `>0` → `> 0`.
fn spaced(op: &str) -> String {
    let split = op.len() - 1;
    format!("{} {}", &op[..split], &op[split..])
}

#[cfg(test)]
mod tests {
    use super::super::to_match::apply_match_edits;
    use super::*;

    fn rewrite(src: &str) -> String {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        apply_match_edits(&chars, &super::super::flatten(plan_any_fixes(&stmts, &chars)))
    }

    fn count(src: &str) -> usize {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        plan_any_fixes(&stmts, &chars).len()
    }

    #[test]
    fn rewrites_each_comparison() {
        let src = "let xs = [1, 2]\n\
                   let a = len(filter(xs, fn(x) -> x > 1)) > 0\n\
                   let b = len(filter(xs, fn(x) -> x > 1)) == 0\n\
                   let c = len( filter(xs, fn(x) -> x.k == \"a\") ) != 0 && a\n\
                   let d = len(filter(xs, fn(x) -> x > 1)) >= 1\n\
                   let e = len(filter(xs, fn(x) -> x > 1)) < 1\n";
        assert_eq!(
            rewrite(src),
            "let xs = [1, 2]\n\
             let a = any(xs, fn(x) -> x > 1)\n\
             let b = !any(xs, fn(x) -> x > 1)\n\
             let c = any(xs, fn(x) -> x.k == \"a\") && a\n\
             let d = any(xs, fn(x) -> x > 1)\n\
             let e = !any(xs, fn(x) -> x > 1)\n"
        );
    }

    #[test]
    fn reports_without_fixing_a_predicate_that_may_have_effects() {
        for src in [
            // A user function may write state.
            "fn seen(x) true end\nlet xs = [1]\nlet a = len(filter(xs, fn(x) -> seen(x))) > 0\n",
            // A predicate passed by name is not read.
            "fn seen(x) true end\nlet xs = [1]\nlet a = len(filter(xs, seen)) > 0\n",
            // A block body.
            "let xs = [1]\nlet a = len(filter(xs, fn(x)\n  let y = x\n  y > 0\nend)) > 0\n",
        ] {
            assert_eq!(rewrite(src), src);
            assert_eq!(count(src), 1, "{src}");
        }
    }

    #[test]
    fn leaves_other_counts_and_shadowed_names_alone() {
        for src in [
            "let xs = [1]\nlet a = len(filter(xs, fn(x) -> x > 1)) > 1\n",
            "let xs = [1]\nlet a = len(filter(xs, fn(x) -> x > 1))\n",
            "let xs = [1]\nlet a = len(xs) > 0\n",
            // The file has its own `any`.
            "fn any(xs, p) false end\nlet xs = [1]\nlet a = len(filter(xs, fn(x) -> x > 1)) > 0\n",
            "fn f(filter) len(filter([1], fn(x) -> x > 1)) > 0 end\n",
            // A comment inside the call would be lost.
            "let xs = [1]\nlet a = len(filter(xs, fn(x) -> x > 1) // n\n) > 0\n",
        ] {
            assert_eq!(count(src), 0, "{src}");
        }
    }
}
