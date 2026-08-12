//! Reject a closure that captures a module binding which is rebound after it.
//!
//! A closure captures by value at its own textual position: `MakeClosure` takes
//! the term carrying the name's value on the line the `fn` is written, and a
//! later rebinding produces a *different* term the closure never sees. That is
//! the correct consequence of `let` being an immutable dataflow edge — but it
//! means the read inside the body silently answers a different question from
//! the same read written one scope out, and in a script that re-runs each frame
//! the gap is exactly one frame, every frame.
//!
//! So the capture is an error, and the fix is to take the value as a parameter.
//! The same move as §2a's cross-function `=` (docs/dev/var-next-steps.md): the
//! honest half of the behaviour was the half that failed.
//!
//! Together with `get` this closes the hole from both sides. `get` says which
//! *timing* a cell read wants; this says a lexical read can only capture a name
//! whose value is settled. After both, a bare name inside a function body is
//! always a value that cannot change under you.
//!
//! # Where the rule deliberately stops
//!
//! Two shapes keep today's behaviour, both because flagging them would cost
//! more than it buys:
//!
//! - **Inline lambdas.** `map(xs, fn(a) … end)` runs and discards its callback
//!   inside the statement that created it, so it cannot outlive a later
//!   rebinding. It is also unfixable if flagged — the author does not control a
//!   `map` callback's parameter list. A lambda that really is stored and called
//!   later is missed as a consequence.
//! - **An enclosing function's locals.** The same staleness is possible one
//!   scope in (`fn f()` declares `x`, a nested lambda captures it, `f` then
//!   rebinds `x`), but the rebindings scanned here are module-level only.
//!
//! Both are under-approximations: they let a real hazard through rather than
//! reject working code, which is the right way round for a rule that is a hard
//! error.

use std::collections::HashMap;

use crate::ast::{AssignTarget, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_expr, walk_stmt};
use crate::source_map::SourceSpan;

/// Every top-level rebinding in one module, keyed by the name it rebinds.
///
/// "Top level" means module scope — including inside a top-level `if`, `for`,
/// `while` or `match` body, which still run as part of the module. Function and
/// lambda bodies are deliberately not walked: an assignment to a module binding
/// from inside a function is already a compile error, so every rebinding that
/// can reach a module name is out here.
#[derive(Default)]
pub(super) struct ModuleRebinds {
    spans: HashMap<String, Vec<SourceSpan>>,
}

impl ModuleRebinds {
    pub(super) fn collect(stmts: &[Stmt]) -> Self {
        let mut out = ModuleRebinds {
            spans: HashMap::new(),
        };
        for stmt in stmts {
            out.visit_stmt(stmt);
        }
        for v in out.spans.values_mut() {
            v.sort_by_key(|s| s.start.offset);
        }
        out
    }

    /// The first rebinding of `name` at or after `offset`, if any.
    pub(super) fn first_after(&self, name: &str, offset: u32) -> Option<SourceSpan> {
        self.spans
            .get(name)?
            .iter()
            .find(|s| s.start.offset >= offset)
            .copied()
    }
}

impl ExprVisitor for ModuleRebinds {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Assign { target, .. } => {
                if let Some(root) = assign_root(target) {
                    self.spans.entry(root.to_string()).or_default().push(s.span);
                }
                walk_stmt(self, s);
            }
            // Not module scope — a function body's own statements run when it
            // is called, and cannot assign a module binding anyway.
            StmtKind::FnDecl { .. } => {}
            // A `set` writes a cell. Cells are exempt: a cell read is live by
            // construction, and `get` is what makes that visible at the read.
            StmtKind::Set { .. } => {}
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lambda { .. } => {}
            _ => walk_expr(self, e),
        }
    }
}

fn assign_root(target: &AssignTarget) -> Option<&str> {
    match target {
        AssignTarget::Name(name) => Some(name.as_str()),
        AssignTarget::Field(object, _) | AssignTarget::Index(object, _) => expr_root(object),
    }
}

fn expr_root(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(name.as_str()),
        ExprKind::FieldAccess { object, .. } | ExprKind::IndexAccess { object, .. } => {
            expr_root(object)
        }
        _ => None,
    }
}
