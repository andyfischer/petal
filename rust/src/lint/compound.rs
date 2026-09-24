//! Rule — fold `x = x + e` into `x += e`.
//!
//! ```text
//! fx = fx + chip(fx, fy, "P")      =>   fx += chip(fx, fy, "P")
//! set total = total * 2            =>   set total *= 2
//! ```
//!
//! The parser desugars `x op= e` to `x = x op e` (and `set x op= e` to
//! `set x = get x op e`), so the two spellings are the same program; the
//! compound one says the name once. The rule covers every operator that has
//! a compound form: `+ - * / % ++ ??`.
//!
//! It only fires when the target is a bare name, the left operand is that same
//! name (or `get` of it), and the text between the operand and `e` is nothing
//! but the operator — no parentheses, no comment, no line break — so replacing
//! everything before `e` with `x op= ` leaves `e` exactly as written.

use crate::ast::{AssignTarget, BinOp, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_stmt};

use super::Fix;
use super::to_match::Splice;

/// Plan every compound fold in `stmts`, one [`Fix`] per statement, in source
/// order.
pub(super) fn plan_compound_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let mut finder = Finder {
        chars,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

/// [`plan_compound_fixes`], flattened to splices.
#[cfg(test)]
pub(super) fn plan_compound_edits(stmts: &[Stmt], chars: &[char]) -> Vec<Splice> {
    super::flatten(plan_compound_fixes(stmts, chars))
}

struct Finder<'a> {
    chars: &'a [char],
    fixes: Vec<Fix>,
}

impl ExprVisitor for Finder<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        let planned = match &s.kind {
            StmtKind::Assign {
                target: AssignTarget::Name(name),
                value,
            } => plan_one(s, name, value, false, self.chars),
            StmtKind::Set {
                target: AssignTarget::Name(name),
                value,
            } => plan_one(s, name, value, true, self.chars),
            _ => None,
        };
        if let Some((splice, name, op)) = planned {
            self.fixes.push(Fix {
                anchor: s.span.start.offset as usize,
                message: format!("`{name} = {name} {op} …` can be written `{name} {op}= …`"),
                splices: vec![splice],
            });
        }
        // A fold only rewrites the text in front of `e`, so a fold inside `e`
        // (a lambda body, say) never overlaps it.
        walk_stmt(self, s);
    }
}

fn op_text(op: BinOp) -> Option<&'static str> {
    Some(match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Concat => "++",
        BinOp::Coalesce => "??",
        _ => return None,
    })
}

fn text(chars: &[char], start: usize, end: usize) -> Option<String> {
    (start <= end && end <= chars.len()).then(|| chars[start..end].iter().collect())
}

fn plan_one<'n>(
    stmt: &Stmt,
    name: &'n str,
    value: &Expr,
    is_set: bool,
    chars: &[char],
) -> Option<(Splice, &'n str, &'static str)> {
    let ExprKind::BinaryOp { op, left, right } = &value.kind else {
        return None;
    };
    let op = op_text(*op)?;
    let stmt_start = stmt.span.start.offset as usize;
    // An already-compound statement: its desugared value spans the whole
    // statement, so it starts where the statement does.
    if value.span.start.offset as usize == stmt_start {
        return None;
    }
    // The left operand is the target itself, spelled `x` or `get x`.
    let left_text = text(
        chars,
        left.span.start.offset as usize,
        left.span.end.offset as usize,
    )?;
    let same = match &left.kind {
        ExprKind::Ident(n) => n == name && left_text == name,
        ExprKind::CellGet(n) => {
            n == name && left_text.split_whitespace().collect::<Vec<_>>() == ["get", name]
        }
        _ => false,
    };
    if !same {
        return None;
    }
    // `x = ` (or `set x = `) in front of the operand, and only the operator
    // between the operand and `e`.
    let head = text(chars, stmt_start, left.span.start.offset as usize)?;
    let mut words = head.split_whitespace();
    if is_set && words.next() != Some("set") {
        return None;
    }
    if words.next() != Some(name) || words.next() != Some("=") || words.next().is_some() {
        return None;
    }
    let right_start = right.span.start.offset as usize;
    let glue = text(chars, left.span.end.offset as usize, right_start)?;
    if glue.contains('\n') || glue.trim() != op {
        return None;
    }
    let prefix = if is_set { "set " } else { "" };
    let splice = Splice {
        start: stmt_start,
        end: right_start,
        text: format!("{prefix}{name} {op}= "),
    };
    Some((splice, name, op))
}

#[cfg(test)]
mod tests {
    use super::super::to_match::apply_match_edits;
    use super::*;

    fn rewrite(src: &str) -> String {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        apply_match_edits(&chars, &plan_compound_edits(&stmts, &chars))
    }

    #[test]
    fn folds_every_compound_operator() {
        let src = "let x = 1\nx = x + 2\nx = x - 1\nx = x * 3\nx = x / 2\nx = x % 5\n\
                   let s = \"a\"\ns = s ++ \"b\"\nlet m = nil\nm = m ?? 4\n";
        assert_eq!(
            rewrite(src),
            "let x = 1\nx += 2\nx -= 1\nx *= 3\nx /= 2\nx %= 5\n\
             let s = \"a\"\ns ++= \"b\"\nlet m = nil\nm ??= 4\n"
        );
    }

    #[test]
    fn folds_set_and_get() {
        let src = "var n = 0\nfn bump() set n = get n + 1 end\nset n = n * 2\n";
        assert_eq!(
            rewrite(src),
            "var n = 0\nfn bump() set n += 1 end\nset n *= 2\n"
        );
    }

    #[test]
    fn keeps_what_would_change_meaning_or_lose_text() {
        for src in [
            // Left-associative: `x - a - b` is `(x - a) - b`.
            "let x = 1\nx = x - 2 - 3\n",
            "let x = 1\nx = x * 2 + 3\n",
            // The right side is parenthesized; the parens are glue.
            "let x = 1\nx = x * (2 + 3)\n",
            // The operand is some other name, or on the right.
            "let x = 1\nlet y = 2\nx = y + x\n",
            // Non-name targets are left alone.
            "let r = {a: 1}\nr.a = r.a + 1\n",
            // Already compound.
            "let x = 1\nx += 2\n",
            // Not a compound operator.
            "let x = 1\nx = x == 1\n",
        ] {
            assert_eq!(rewrite(src), src);
        }
    }
}
