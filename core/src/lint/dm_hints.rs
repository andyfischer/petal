//! Rules about the direct-manipulation hints `tunable` and `rigid`
//! (docs/proposals/dm-v2-hints.md). All three only report: what to write
//! instead is the author's call.
//!
//! **`dm-hint-not-literal`**: `tunable(e)` where `e` is computed
//! (`tunable(a + b)`). The hint marks the literal its argument is, or the
//! literal a name is bound to, so a computed argument marks nothing. (A name
//! bound to a computed value is caught where the hints are read,
//! `crate::dm_v2::hints`, which sees bindings.)
//!
//! **`dm-hint-bounds`**: a `tunable` bound (`min`, `max`, `step`) that is not
//! a number literal, so no tool can read it without running the program, or
//! a `min` above the `max`.
//!
//! **`dm-hint-state-init`**: `state x = tunable(…)`. A running program keeps
//! its state across a hot reload, so a drag cannot move a state's initial
//! value; the start value belongs in a `config let` the state reads.
//!
//! A file that binds the name `tunable` itself (a function, a parameter) is
//! not using the hint, and is left alone.

use crate::ast::{Expr, ExprKind, ExprVisitor, Literal, Stmt, StmtKind, UnaryOp, walk_expr, walk_stmt};

use super::Fix;
use super::util::bound_names;

/// The number a literal expression is (`4`, `-2.5`), if it is one.
fn number(e: &Expr) -> Option<f64> {
    match &e.kind {
        ExprKind::Literal(Literal::Int(n)) => Some(*n as f64),
        ExprKind::Literal(Literal::Float(f)) => Some(*f),
        ExprKind::UnaryOp { op: UnaryOp::Neg, operand } => number(operand).map(|v| -v),
        _ => None,
    }
}

/// `tunable(…)`'s arguments, with each one's name (`None` = positional).
fn tunable_call(e: &Expr) -> Option<Vec<(Option<&str>, &Expr)>> {
    let ExprKind::Call { function, args, arg_names } = &e.kind else { return None };
    let ExprKind::Ident(f) = &function.kind else { return None };
    if f != "tunable" {
        return None;
    }
    Some(
        args.iter()
            .enumerate()
            .map(|(i, a)| (arg_names.get(i).and_then(|n| n.as_deref()), a))
            .collect(),
    )
}

/// The three rules' findings, by rule name.
pub(super) struct HintFindings {
    pub not_literal: Vec<Fix>,
    pub bounds: Vec<Fix>,
    pub state_init: Vec<Fix>,
}

pub(super) fn plan_hint_reports(stmts: &[Stmt]) -> HintFindings {
    let mut v = Finder {
        out: HintFindings { not_literal: Vec::new(), bounds: Vec::new(), state_init: Vec::new() },
    };
    if bound_names(stmts).contains("tunable") {
        return v.out;
    }
    for s in stmts {
        v.visit_stmt(s);
    }
    v.out.not_literal.sort_by_key(|f| f.anchor);
    v.out.bounds.sort_by_key(|f| f.anchor);
    v.out.state_init.sort_by_key(|f| f.anchor);
    v.out
}

struct Finder {
    out: HintFindings,
}

fn report(anchor: &Expr, message: String) -> Fix {
    Fix { anchor: anchor.span.start.offset as usize, message, splices: Vec::new() }
}

impl ExprVisitor for Finder {
    fn visit_stmt(&mut self, s: &Stmt) {
        if let StmtKind::State { init, .. } = &s.kind
            && tunable_call(init).is_some()
        {
            self.out.state_init.push(report(
                init,
                "a drag cannot move a running `state`'s initial value (hot reload keeps state); \
                 declare the start value with `config let` and initialise the state from it"
                    .to_string(),
            ));
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        if let Some(args) = tunable_call(e) {
            let mut positional = 0usize;
            let mut bounds: [Option<f64>; 3] = [None; 3];
            for (name, a) in &args {
                let slot = match name {
                    Some(n) => ["value", "min", "max", "step"].iter().position(|x| x == n),
                    None => {
                        positional += 1;
                        Some(positional - 1)
                    }
                };
                match slot {
                    Some(0) => {
                        let marks = number(a).is_some()
                            || matches!(a.kind, ExprKind::Ident(_) | ExprKind::CellGet(_));
                        if !marks {
                            self.out.not_literal.push(report(
                                a,
                                "`tunable` marks a literal or a name bound to one; this argument \
                                 is computed, so nothing is marked. Wrap the literal itself, as in \
                                 `tunable(a) + b`"
                                    .to_string(),
                            ));
                        }
                    }
                    Some(k @ 1..=3) => match number(a) {
                        Some(v) => bounds[k - 1] = Some(v),
                        None if matches!(a.kind, ExprKind::Literal(Literal::Nil)) => {}
                        None => self.out.bounds.push(report(
                            a,
                            "`tunable` bounds must be literals so tools can read them without \
                             running the program"
                                .to_string(),
                        )),
                    },
                    _ => {}
                }
            }
            if let [Some(lo), Some(hi), _] = bounds
                && lo > hi
            {
                self.out.bounds.push(report(e, format!("`tunable` range is empty: min {lo} is above max {hi}")));
            }
        }
        walk_expr(self, e);
    }
}
