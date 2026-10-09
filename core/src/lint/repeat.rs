//! Rule — a loop that appends the same string every pass is `repeat`.
//!
//! ```text
//! let out = ""
//! for i in range(0, n) do out = out ++ s end   =>   out ++= repeat(s, n)
//! ```
//!
//! `x ++ s` builds a new string, so the loop copies everything written so far
//! on every pass: `n` appends cost O(n²) bytes. `repeat(s, n)` allocates the
//! result once. The loop is the workaround people reach for when they assume
//! there is no `repeat` builtin, which is why it is worth a rule.
//!
//! The rewrite is an identity only under conditions the rule checks, and it
//! stays silent whenever one of them is not visible in the text:
//!
//! - The loop is a `for` *statement* over `range(n)` or `range(0, n)`, its
//!   body is the single statement `x = x ++ s` (or `x ++= s`, the same thing
//!   after parsing), and it is not the last statement of its block — in tail
//!   position a `for` is a value (a list) and so is the assignment replacing
//!   it (a string).
//! - `s` is a string literal, or a parameter of the enclosing function
//!   annotated `string` that the function never rebinds. `++` stringifies a
//!   number and `repeat` rejects one, so the piece has to be known to be text.
//!   (An annotation is trusted exactly as `no-redundant-cast` trusts it.)
//! - `x` is known to hold a string when the loop starts: the nearest earlier
//!   statement of the same block that writes `x` is `let x = "…"`, looking
//!   past plain appends (`x ++= e`, and loops of one). When the loop runs
//!   zero times `x` must come out untouched, and `x ++ ""` only leaves a
//!   string alone.
//! - Nothing in the file declares its own `range` or `repeat`, and no
//!   `import m: *` could be supplying one.
//! - The loop holds no comment, which the one-line replacement would drop.

use std::collections::HashSet;

use crate::ast::{
    AssignTarget, BinOp, ElseBranch, Expr, ExprKind, ExprVisitor, Literal, Param, Stmt, StmtKind,
    walk_expr, walk_stmt,
};
use crate::types::Type;

use super::Fix;
use super::to_match::Splice;

/// Plan every loop-to-`repeat` rewrite in `stmts`, one [`Fix`] per loop, in
/// source order. `chars` is the text they were parsed from.
pub(super) fn plan_repeat_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let mut names = Declared::default();
    for s in stmts {
        names.visit_stmt(s);
    }
    if names.shadows_builtin {
        return Vec::new();
    }
    let mut finder = Finder {
        chars,
        fixes: Vec::new(),
        string_params: vec![HashSet::new()],
    };
    finder.scan_block(stmts);
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

/// Does the file bind `range` or `repeat` itself, anywhere?
#[derive(Default)]
struct Declared {
    shadows_builtin: bool,
}

impl Declared {
    fn name(&mut self, n: &str) {
        if n == "range" || n == "repeat" {
            self.shadows_builtin = true;
        }
    }
}

impl ExprVisitor for Declared {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, .. } | StmtKind::State { name, .. } => self.name(name),
            StmtKind::For { var, .. } => self.name(var),
            StmtKind::FnDecl { name, params, .. } => {
                self.name(name);
                for p in params {
                    self.name(&p.name);
                }
            }
            StmtKind::Import(decl) => {
                if decl.star {
                    self.shadows_builtin = true;
                }
                for n in decl.names.iter().flatten() {
                    self.name(n);
                }
            }
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lambda { params, .. } => {
                for p in params {
                    self.name(&p.name);
                }
            }
            ExprKind::For { var, .. } => self.name(var),
            _ => {}
        }
        walk_expr(self, e);
    }
}

/// Does anything in a statement (at any depth) declare or assign `name`?
struct Writes<'a> {
    name: &'a str,
    found: bool,
}

impl ExprVisitor for Writes<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        let hit = match &s.kind {
            StmtKind::Let { name, .. } | StmtKind::State { name, .. } => name == self.name,
            StmtKind::Assign {
                target: AssignTarget::Name(name),
                ..
            }
            | StmtKind::Set {
                target: AssignTarget::Name(name),
                ..
            } => name == self.name,
            StmtKind::For { var, .. } => var == self.name,
            StmtKind::FnDecl { name, params, .. } => {
                name == self.name || params.iter().any(|p| p.name == self.name)
            }
            _ => false,
        };
        self.found |= hit;
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        let hit = match &e.kind {
            ExprKind::Lambda { params, .. } => params.iter().any(|p| p.name == self.name),
            ExprKind::For { var, .. } => var == self.name,
            _ => false,
        };
        self.found |= hit;
        walk_expr(self, e);
    }
}

fn writes(s: &Stmt, name: &str) -> bool {
    let mut w = Writes { name, found: false };
    w.visit_stmt(s);
    w.found
}

struct Finder<'a> {
    chars: &'a [char],
    fixes: Vec<Fix>,
    /// Per enclosing function: its `string`-annotated parameters that the
    /// body never rebinds. The bottom entry is the top level (none).
    string_params: Vec<HashSet<String>>,
}

impl Finder<'_> {
    /// Walk one block: plan each loop against its earlier siblings, then
    /// descend into the nested blocks. Only blocks reached here are examined;
    /// one this walk does not know how to reach is simply left alone.
    fn scan_block(&mut self, block: &[Stmt]) {
        for (i, s) in block.iter().enumerate() {
            // Never the tail statement: see the module docs.
            if i + 1 < block.len()
                && let Some(fix) = self.plan(block, i)
            {
                self.fixes.push(fix);
                continue;
            }
            match &s.kind {
                StmtKind::FnDecl { params, body, .. } => self.scan_fn(params, body),
                StmtKind::For { body, .. } | StmtKind::While { body, .. } => self.scan_block(body),
                StmtKind::Expr(e)
                | StmtKind::Let { value: e, .. }
                | StmtKind::Assign { value: e, .. }
                | StmtKind::Return(Some(e)) => self.scan_expr(e),
                _ => {}
            }
        }
    }

    fn scan_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::If {
                then_body,
                else_body,
                ..
            } => {
                self.scan_block(then_body);
                match else_body {
                    Some(ElseBranch::Block(b)) => self.scan_block(b),
                    Some(ElseBranch::ElseIf(next)) => self.scan_expr(next),
                    None => {}
                }
            }
            ExprKind::Lambda { params, body } => self.scan_fn(params, body),
            _ => {}
        }
    }

    fn scan_fn(&mut self, params: &[Param], body: &[Stmt]) {
        let strings = params
            .iter()
            .filter(|p| p.ty.as_ref().and_then(|t| t.resolved.as_ref()) == Some(&Type::String))
            .filter(|p| !body.iter().any(|s| writes(s, &p.name)))
            .map(|p| p.name.clone())
            .collect();
        self.string_params.push(strings);
        self.scan_block(body);
        self.string_params.pop();
    }

    fn text(&self, start: usize, end: usize) -> Option<String> {
        (start <= end && end <= self.chars.len()).then(|| self.chars[start..end].iter().collect())
    }

    fn plan(&self, block: &[Stmt], i: usize) -> Option<Fix> {
        let stmt = &block[i];
        let StmtKind::For { var, iter, body } = &stmt.kind else {
            return None;
        };
        let [
            Stmt {
                kind:
                    StmtKind::Assign {
                        target: AssignTarget::Name(acc),
                        value,
                    },
                ..
            },
        ] = body.as_slice()
        else {
            return None;
        };
        let ExprKind::BinaryOp {
            op: BinOp::Concat,
            left,
            right,
        } = &value.kind
        else {
            return None;
        };
        if !matches!(&left.kind, ExprKind::Ident(n) if n == acc) || acc == var {
            return None;
        }

        // The piece: a literal, or a parameter known to be a string.
        let piece = self.text(
            right.span.start.offset as usize,
            right.span.end.offset as usize,
        )?;
        match &right.kind {
            ExprKind::Literal(Literal::String(_)) => {
                if !(piece.len() >= 2 && piece.starts_with('"') && piece.ends_with('"')) {
                    return None;
                }
            }
            ExprKind::Ident(p) => {
                let known = self.string_params.last().is_some_and(|s| s.contains(p));
                if !known || p == acc || p == var || &piece != p {
                    return None;
                }
            }
            _ => return None,
        }

        // The count: the text of `n` in `range(n)` / `range(0, n)`, taken from
        // the call's own parentheses so that it survives whatever `n` is.
        let ExprKind::Call {
            function,
            args,
            arg_names,
        } = &iter.kind
        else {
            return None;
        };
        if !matches!(&function.kind, ExprKind::Ident(n) if n == "range") || !arg_names.is_empty() {
            return None;
        }
        let call_end = iter.span.end.offset as usize;
        if call_end == 0 || self.chars.get(call_end - 1) != Some(&')') {
            return None;
        }
        let after = match args.as_slice() {
            [_] => function.span.end.offset as usize,
            [zero, _] if matches!(zero.kind, ExprKind::Literal(Literal::Int(0))) => {
                zero.span.end.offset as usize
            }
            _ => return None,
        };
        let lead = self.text(after, call_end - 1)?;
        let opener = if args.len() == 1 { '(' } else { ',' };
        let count = lead.trim_start().strip_prefix(opener)?.trim().to_string();
        if count.is_empty() {
            return None;
        }

        // The accumulator holds a string when the loop starts.
        // Appends in between (`x ++= e`, or a loop of one) keep it a string.
        let seeded = block[..i]
            .iter()
            .rev()
            .find(|s| writes(s, acc) && !is_append(s, acc))?;
        let StmtKind::Let {
            name,
            value: seed,
            is_var: false,
            ..
        } = &seeded.kind
        else {
            return None;
        };
        let is_text = matches!(
            seed.kind,
            ExprKind::Literal(Literal::String(_)) | ExprKind::StringInterp { .. }
        );
        if name != acc || !is_text || writes_in_expr(seed, acc) {
            return None;
        }

        let start = stmt.span.start.offset as usize;
        let end = stmt.span.end.offset as usize;
        let whole = self.text(start, end)?;
        if !whole.starts_with("for") || !whole.ends_with("end") || whole.contains("//") {
            return None;
        }
        Some(Fix {
            anchor: start,
            message: format!(
                "this loop copies `{acc}` on every pass to append `{piece}`; \
                 write `{acc} ++= repeat({piece}, {count})`"
            ),
            splices: vec![Splice {
                start,
                end,
                text: format!("{acc} ++= repeat({piece}, {count})"),
            }],
        })
    }
}

/// Is `s` exactly `x = x ++ e` (which leaves a string a string), or a `for`
/// whose whole body is that — with nothing else in it writing `x`?
fn is_append(s: &Stmt, acc: &str) -> bool {
    match &s.kind {
        StmtKind::Assign {
            target: AssignTarget::Name(name),
            value,
        } => {
            let ExprKind::BinaryOp {
                op: BinOp::Concat,
                left,
                right,
            } = &value.kind
            else {
                return false;
            };
            name == acc
                && matches!(&left.kind, ExprKind::Ident(n) if n == acc)
                && !writes_in_expr(right, acc)
        }
        StmtKind::For { var, iter, body } => {
            var != acc
                && !writes_in_expr(iter, acc)
                && matches!(body.as_slice(), [only] if is_append(only, acc))
        }
        _ => false,
    }
}

fn writes_in_expr(e: &Expr, name: &str) -> bool {
    let mut w = Writes { name, found: false };
    w.visit_expr(e);
    w.found
}

#[cfg(test)]
mod tests {
    use super::super::to_match::apply_match_edits;
    use super::*;

    fn rewrite(src: &str) -> String {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        let edits = super::super::flatten(plan_repeat_fixes(&stmts, &chars));
        apply_match_edits(&chars, &edits)
    }

    #[test]
    fn rewrites_the_hand_written_repeat() {
        let src = "fn rep(s: string, n: int) -> string\n  let out = \"\"\n  \
                   for i in range(0, n) do out = out ++ s end\n  out\nend\n";
        assert_eq!(
            rewrite(src),
            "fn rep(s: string, n: int) -> string\n  let out = \"\"\n  \
             out ++= repeat(s, n)\n  out\nend\n"
        );
    }

    #[test]
    fn takes_literals_compound_form_and_any_count_expression() {
        let src = "let w = 4\nlet line = \"+\"\nlet other = 1\n\
                   for i in range((w + 1) * 2) do\n  line ++= \"-\"\nend\nprint(line)\n";
        assert_eq!(
            rewrite(src),
            "let w = 4\nlet line = \"+\"\nlet other = 1\n\
             line ++= repeat(\"-\", (w + 1) * 2)\nprint(line)\n"
        );
    }

    /// Earlier appends to the accumulator do not hide the `let` that makes it
    /// a string, so two loops in a row are both rewritten in one pass.
    #[test]
    fn sees_through_earlier_appends() {
        let src = "let b = \"[\"\nfor i in range(3) do b ++= \"#\" end\nb ++= 7\n\
                   for i in range(0, 2) do b ++= \".\" end\nprint(b)\n";
        assert_eq!(
            rewrite(src),
            "let b = \"[\"\nb ++= repeat(\"#\", 3)\nb ++= 7\n\
             b ++= repeat(\".\", 2)\nprint(b)\n"
        );
    }

    #[test]
    fn leaves_loops_it_cannot_prove() {
        for src in [
            // Tail position: the loop is the block's value.
            "let out = \"\"\nfor i in range(0, 3) do out = out ++ \"x\" end\n",
            // The piece depends on the loop, or is not known to be a string.
            "let out = \"\"\nfor i in range(0, 3) do out = out ++ i end\nprint(out)\n",
            "fn f(s, n)\n  let out = \"\"\n  for i in range(n) do out ++= s end\n  out\nend\n",
            "fn f(s: string, n)\n  let out = \"\"\n  s = 5\n  \
             for i in range(n) do out ++= s end\n  out\nend\n",
            // The accumulator is not known to start as a string.
            "let out = []\nfor i in range(0, 3) do out = out ++ \"x\" end\nprint(out)\n",
            "let out = \"\"\nout = 5\nfor i in range(0, 3) do out = out ++ \"x\" end\nprint(out)\n",
            "fn f(out)\n  for i in range(0, 3) do out = out ++ \"x\" end\n  out\nend\n",
            // Not a zero-based range, more than one statement, or a comment.
            "let out = \"\"\nfor i in range(1, 3) do out = out ++ \"x\" end\nprint(out)\n",
            "let out = \"\"\nfor i in range(3) do\n  out ++= \"x\"\n  print(i)\nend\nprint(out)\n",
            "let out = \"\"\nfor i in range(3) do\n  out ++= \"x\" // pad\nend\nprint(out)\n",
            // Prepending is a different string when the piece is not uniform.
            "let out = \"\"\nfor i in range(3) do out = \"x\" ++ out end\nprint(out)\n",
            // The file has its own `repeat`.
            "fn repeat(a, b) a end\nlet out = \"\"\n\
             for i in range(3) do out ++= \"x\" end\nprint(out)\n",
        ] {
            assert_eq!(rewrite(src), src, "{src}");
        }
    }
}
