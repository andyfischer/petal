//! Advice: suggestions that are a *comment*, not a rewrite.
//!
//! The other kinds each end in text to splice, behind a proof that
//! splicing it is safe. Some things worth saying have neither: "this loop is a
//! sort" is a judgement about what the code *means*, the replacement depends
//! on what the author intended, and nothing can prove the two alike. Advice is
//! the channel for those. A piece of advice names a place and says something
//! about it; it carries no edit, `--apply` never acts on it, and `--verify`
//! does not count it.
//!
//! Detection is heuristic by design — the report says "looks like", and a
//! false positive costs a line of output, never a changed file. That is the
//! bar: a rule belongs here when it is right often enough to be worth reading
//! and cannot be made exact enough to be a `petal lint` fix.
//!
//! # The rules
//!
//! **`hand-written-sort`** — an insertion sort written out by hand, the shape
//! authors reach for when they do not know `sort` takes a comparator:
//!
//! ```petal
//! for r in rows do
//!   var placed = false
//!   var res = []
//!   for x in out do
//!     if !placed && less(r, x) then
//!       set res = append(res, r)
//!       set placed = true
//!     end
//!     set res = append(res, x)
//!   end
//!   if !placed then set res = append(res, r) end
//!   set out = res
//! end
//! ```
//!
//! What is matched is the part that makes it one: a loop over `r` containing a
//! loop over `x`, whose body appends `r` under a test that reads both `r` and
//! `x`, and appends `x` as well. That rebuilds the list once per element —
//! quadratic copying where `sort(list, compare)` or `sort_by(list, key)` is
//! one call.

use crate::ast::{Expr, ExprKind, ExprVisitor, Stmt, StmtKind, for_each_expr, walk_expr, walk_stmt};
use crate::source_map::SourceSpan;

pub const HAND_WRITTEN_SORT: &str = "hand-written-sort";

/// One piece of advice: a place, and a comment about it. No edit.
#[derive(Debug, Clone)]
pub struct Advice {
    /// The rule that produced it, kebab-case.
    pub rule: &'static str,
    /// 1-based position of the code it is about.
    pub line: usize,
    pub column: usize,
    /// Byte offset of that code, to order advice among the other kinds.
    pub at: usize,
    /// What it is about, for the report's heading: `fn sort_rows`, or
    /// `for loop` at the top level.
    pub subject: String,
    /// The comment itself.
    pub message: String,
}

/// Every piece of advice for a parsed file, in source order.
pub fn find(stmts: &[Stmt]) -> Vec<Advice> {
    let mut finder = Finder {
        fns: Vec::new(),
        out: Vec::new(),
    };
    for stmt in stmts {
        finder.visit_stmt(stmt);
    }
    finder.out.sort_by_key(|a| a.at);
    finder.out
}

struct Finder {
    /// The enclosing named functions, innermost last.
    fns: Vec<String>,
    out: Vec<Advice>,
}

impl Finder {
    /// A `for` over `var` with this `body`, at `span`.
    fn check_loop(&mut self, var: &str, body: &[Stmt], span: SourceSpan) {
        let Some(sort) = insertion_sort(var, body) else {
            return;
        };
        let mut message = String::from(
            "this looks like a hand-written insertion sort, which rebuilds the list once per \
             element. `sort(list, compare)` takes a comparator (true, or a negative number, \
             means the first argument goes first) and `sort_by(list, key)` sorts by a key; \
             both are stable.",
        );
        if let Some(less) = &sort.comparator {
            message.push_str(&format!(
                " The test here is `{less}({var}, {})`, so `sort(list, {less})` is likely the \
                 whole loop.",
                sort.inner_var
            ));
        }
        self.out.push(Advice {
            rule: HAND_WRITTEN_SORT,
            line: span.start.line as usize,
            column: span.start.column as usize,
            at: span.start.offset as usize,
            subject: match self.fns.last() {
                Some(name) => format!("fn {name}"),
                None => "for loop".to_string(),
            },
            message,
        });
    }
}

impl ExprVisitor for Finder {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::FnDecl { name, .. } => {
                self.fns.push(name.clone());
                walk_stmt(self, s);
                self.fns.pop();
            }
            StmtKind::For { var, body, .. } => {
                self.check_loop(var, body, s.span);
                walk_stmt(self, s);
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        if let ExprKind::For { var, body, .. } = &e.kind {
            self.check_loop(var, body, e.span);
        }
        walk_expr(self, e);
    }
}

struct InsertionSort {
    inner_var: String,
    /// The function the insertion test calls as `f(outer, inner)`, when it is
    /// exactly that — which is already the comparator `sort` wants.
    comparator: Option<String>,
}

/// Whether the body of a loop over `outer` is an insertion step: see the
/// module docs for the shape.
fn insertion_sort(outer: &str, body: &[Stmt]) -> Option<InsertionSort> {
    let mut found = None;
    let mut look = |var: &str, inner: &[Stmt]| {
        // Shadowing the element makes the inner loop about something else.
        if found.is_some() || var == outer {
            return;
        }
        if let Some(comparator) = insertion_step(outer, var, inner) {
            found = Some(InsertionSort {
                inner_var: var.to_string(),
                comparator,
            });
        }
    };
    for stmt in body {
        if let StmtKind::For { var, body, .. } = &stmt.kind {
            look(var, body);
        }
        crate::ast::for_each_expr_in_stmt(stmt, &mut |e| {
            if let ExprKind::For { var, body, .. } = &e.kind {
                look(var, body);
            }
        });
    }
    found
}

/// Whether `inner_body` (a loop over `inner`) both appends `inner` and appends
/// `outer` under a test that reads the two of them. `Some(comparator)` when it
/// does; the comparator is itself optional (see [`InsertionSort`]).
fn insertion_step(outer: &str, inner: &str, inner_body: &[Stmt]) -> Option<Option<String>> {
    let mut keeps_inner = false;
    let mut test: Option<Option<String>> = None;
    for stmt in inner_body {
        crate::ast::for_each_expr_in_stmt(stmt, &mut |e| {
            if appends(e, inner) {
                keeps_inner = true;
            }
            if let ExprKind::If {
                condition,
                then_body,
                ..
            } = &e.kind
                && test.is_none()
                && mentions(condition, outer)
                && mentions(condition, inner)
                && then_body.iter().any(|s| stmt_appends(s, outer))
            {
                test = Some(comparator_call(condition, outer, inner));
            }
        });
    }
    if keeps_inner { test } else { None }
}

/// `f(outer, inner)` somewhere in `condition`, with `f` a bare name.
fn comparator_call(condition: &Expr, outer: &str, inner: &str) -> Option<String> {
    let mut name = None;
    for_each_expr(condition, &mut |e| {
        if let ExprKind::Call { function, args, .. } = &e.kind
            && let ExprKind::Ident(f) = &function.kind
            && let [a, b] = args.as_slice()
            && is_ident(a, outer)
            && is_ident(b, inner)
            && name.is_none()
        {
            name = Some(f.clone());
        }
    });
    name
}

fn stmt_appends(s: &Stmt, var: &str) -> bool {
    let mut hit = false;
    crate::ast::for_each_expr_in_stmt(s, &mut |e| hit |= appends(e, var));
    hit
}

/// `append(xs, var)` and its relatives, with `var` handed over directly or
/// inside a list literal (`concat(xs, [var])`).
fn appends(e: &Expr, var: &str) -> bool {
    let ExprKind::Call { function, args, .. } = &e.kind else {
        return false;
    };
    let ExprKind::Ident(f) = &function.kind else {
        return false;
    };
    matches!(f.as_str(), "append" | "push" | "concat" | "prepend")
        && args.iter().any(|a| match &a.kind {
            ExprKind::List(items) => items.iter().any(|i| is_ident(i, var)),
            _ => is_ident(a, var),
        })
}

fn is_ident(e: &Expr, name: &str) -> bool {
    matches!(&e.kind, ExprKind::Ident(n) if n == name)
}

fn mentions(e: &Expr, name: &str) -> bool {
    let mut hit = false;
    for_each_expr(e, &mut |x| hit |= is_ident(x, name));
    hit
}
