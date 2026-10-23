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
//! It fires when the left operand is the target itself (or `get` of it), and
//! the text between the operand and `e` is nothing but the operator — no
//! parentheses, no comment, no line break — so replacing everything before
//! `e` with `x op= ` leaves `e` exactly as written. The target is a bare
//! name, or a field or element whose place is stable ([`plan_place`]):
//!
//! ```text
//! set pop[i] = get pop[i] - d      =>   set pop[i] -= d
//! r.n = r.n * 2                    =>   r.n *= 2
//! ```
//!
//! A statement whose operation the type pass proves raises (`segs = segs +
//! [s]` on two lists, [`TypeNote::FailingArithmetic`]), or that adds an
//! unknown to a list (`segs = segs + branch(n)`, [`TypeNote::ListPlusUnknown`],
//! most likely a concatenation spelled `+`), is left as it is: the compound
//! form means the same thing, and folding it reads as approval.

use crate::ast::{AssignTarget, BinOp, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_stmt};
use crate::source_map::SourceSpan;
use crate::typecheck::TypeNote;

use super::Fix;
use super::to_match::Splice;

/// Plan every compound fold in `stmts`, one [`Fix`] per statement, in source
/// order. `notes` are the type pass's over the same text.
pub(super) fn plan_compound_fixes(stmts: &[Stmt], chars: &[char], notes: &[TypeNote]) -> Vec<Fix> {
    let skip = notes
        .iter()
        .filter_map(|n| match *n {
            TypeNote::FailingArithmetic { expr } | TypeNote::ListPlusUnknown { expr } => {
                Some(expr)
            }
            _ => None,
        })
        .collect();
    let mut finder = Finder {
        chars,
        skip,
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
    let classes = crate::classes::ClassTable::new();
    let signatures = crate::compiler::collect_fn_signatures(stmts, &classes);
    let (_, notes) = crate::typecheck::find_typed_sites(stmts, &signatures, &classes);
    super::flatten(plan_compound_fixes(stmts, chars, &notes))
}

struct Finder<'a> {
    chars: &'a [char],
    /// Operations the type pass proves raise, or suspects are a list
    /// concatenation spelled `+`.
    skip: Vec<SourceSpan>,
    fixes: Vec<Fix>,
}

impl ExprVisitor for Finder<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        let planned = match &s.kind {
            StmtKind::Assign { value, .. } | StmtKind::Set { value, .. }
                if self.skip.contains(&value.span) =>
            {
                None
            }
            StmtKind::Assign {
                target: AssignTarget::Name(name),
                value,
            } => plan_one(s, name, value, false, self.chars),
            StmtKind::Set {
                target: AssignTarget::Name(name),
                value,
            } => plan_one(s, name, value, true, self.chars),
            StmtKind::Assign { value, .. } => plan_place(s, value, false, self.chars),
            StmtKind::Set { value, .. } => plan_place(s, value, true, self.chars),
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

/// A place read that names the same place every time it is evaluated, and
/// does nothing else: names, literals, operators, and field and index reads
/// of those. `xs[next()]` is not one.
fn is_stable_place(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::CellGet(_) => true,
        ExprKind::BinaryOp { left, right, .. } => is_stable_place(left) && is_stable_place(right),
        ExprKind::UnaryOp { operand, .. } => is_stable_place(operand),
        ExprKind::FieldAccess { object, .. } => is_stable_place(object),
        ExprKind::IndexAccess { object, index } => is_stable_place(object) && is_stable_place(index),
        _ => false,
    }
}

/// The fold for a field or index target: `set xs[i] = get xs[i] + d` →
/// `set xs[i] += d`, `r.n = r.n * 2` → `r.n *= 2`.
///
/// The target and the left operand are compared as text, whitespace aside
/// (and the operand's leading `get`, which a `set` reads through), so they
/// are the same place written twice. The place must be stable
/// ([`is_stable_place`]): the long form evaluates the index twice and the
/// compound form desugars to exactly that, but a reader of `xs[next()] += 1`
/// would not expect two calls, so those are left as they are.
fn plan_place(
    stmt: &Stmt,
    value: &Expr,
    is_set: bool,
    chars: &[char],
) -> Option<(Splice, String, &'static str)> {
    let ExprKind::BinaryOp { op, left, right } = &value.kind else {
        return None;
    };
    let op = op_text(*op)?;
    if !matches!(
        left.kind,
        ExprKind::FieldAccess { .. } | ExprKind::IndexAccess { .. }
    ) || !is_stable_place(left)
    {
        return None;
    }
    let stmt_start = stmt.span.start.offset as usize;
    let left_start = left.span.start.offset as usize;
    // An already-compound statement synthesizes its operand over the target.
    if value.span.start.offset as usize == stmt_start || left_start <= stmt_start {
        return None;
    }
    let head = text(chars, stmt_start, left_start)?;
    let head = head.trim_end().strip_suffix('=')?.trim_end();
    let place = if is_set {
        let rest = head.strip_prefix("set")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        rest.trim_start()
    } else {
        head
    };
    if place.contains('\n') || place.is_empty() {
        return None;
    }
    let left_text = text(chars, left_start, left.span.end.offset as usize)?;
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let read = squash(&left_text);
    let read = match (is_set, read.strip_prefix("get")) {
        // `get xs[i]`: the squashed text starts `getxs[`. A place whose own
        // name starts with `get` is told apart by comparing both ways.
        (true, Some(rest)) if rest == squash(place) => rest.to_string(),
        _ => read,
    };
    if read != squash(place) {
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
        text: format!("{prefix}{place} {op}= "),
    };
    Some((splice, place.to_string(), op))
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

fn plan_one(
    stmt: &Stmt,
    name: &str,
    value: &Expr,
    is_set: bool,
    chars: &[char],
) -> Option<(Splice, String, &'static str)> {
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
    Some((splice, name.to_string(), op))
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
    fn folds_a_list_plus_a_number() {
        // Adds to each element: proven well-typed, so it folds.
        assert_eq!(rewrite("let xs = [0]\nxs = xs + 1\n"), "let xs = [0]\nxs += 1\n");
        assert_eq!(rewrite("let xs = [0]\nxs = xs ++ [1]\n"), "let xs = [0]\nxs ++= [1]\n");
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
    fn folds_field_and_index_targets() {
        let src = "var xs = [1, 2]\nvar r = {n: {m: 1}}\nlet t = {a: 1, ys: [0]}\n\
                   fn f(i, d)\n  set xs[i] = get xs[i] + d\n  set xs[i + 1] = xs[i + 1] * 2\n  \
                   set r.n.m = get r.n.m - d\nend\n\
                   t.a = t.a + 1\nt.ys[0] = t.ys[0] ++ \"x\"\n";
        assert_eq!(
            rewrite(src),
            "var xs = [1, 2]\nvar r = {n: {m: 1}}\nlet t = {a: 1, ys: [0]}\n\
             fn f(i, d)\n  set xs[i] += d\n  set xs[i + 1] *= 2\n  \
             set r.n.m -= d\nend\n\
             t.a += 1\nt.ys[0] ++= \"x\"\n"
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
            // A different place on the two sides.
            "let r = {a: 1, b: 2}\nr.a = r.b + 1\n",
            "let xs = [1, 2]\nlet i = 0\nxs[i] = xs[i + 1] + 1\n",
            // An index that is a call is evaluated twice in the long form.
            "fn k() 0 end\nlet xs = [1, 2]\nxs[k()] = xs[k()] + 1\n",
            "let xs = [1, 2]\nxs[0] = xs[0] - 2 - 3\n",
            // Already compound.
            "let x = 1\nx += 2\n",
            // Not a compound operator.
            "let x = 1\nx = x == 1\n",
            // Two lists: raises either way; the fix is `++`, not `+=`.
            "let xs = [0]\nxs = xs + [1]\n",
            // A list plus something not proven a number: most likely the same.
            "fn branch(n) [n] end\nlet segs = [0]\nsegs = segs + branch(1)\n",
            "let s = \"a\"\ns = s + \"b\"\n",
        ] {
            assert_eq!(rewrite(src), src);
        }
    }
}
