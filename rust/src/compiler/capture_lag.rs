//! Warn about a closure that captures a *reactive* module binding which is
//! rebound after it.
//!
//! A closure captures by value at its own textual position: `MakeClosure` takes
//! the term carrying the name's value on the line the `fn` is written, and a
//! later rebinding produces a *different* term the closure never sees.
//!
//! # `let` is not a hazard
//!
//! For a `let` that is the *defined* semantics, not a mistake: a rebinding is a
//! new binding, and a function written above it is supposed to see the earlier
//! one — exactly as it would if the rebinding were spelled `let` a second time.
//! Nothing is stale, so nothing is reported. Capture-at-definition is the
//! documented answer and the rule stays quiet about it.
//!
//! # `state` is
//!
//! A `state` name is not a fresh binding per assignment: `x = e` on a `state`
//! lowers to a `StateWrite` into the persisted slot (see
//! `Compiler::rebind_name`), and the *next* run of the file initialises the name
//! from that slot. So a function above the write reads the value the slot held
//! when the file reached the `fn` — the value from the previous frame. The read
//! is a frame behind, every frame, which is the shape that reads as input lag
//! rather than as a mistake. That earns a warning, and the fix is to take the
//! value as a parameter.
//!
//! It is only a warning: the behaviour is well-defined and sometimes what the
//! author wanted, so a whole program must not fail to compile over it.
//!
//! Cells (`var`, `state var`) are exempt from this pass entirely. A bare read
//! of an outer cell is already rejected by `check_cell_read_says_get`, and the
//! `get` it demands is a live read that cannot lag — `get` says which timing a
//! cell read wants, and this pass covers the non-cell half.
//!
//! # Where the rule deliberately stops
//!
//! Two further shapes keep today's behaviour, both because flagging them would
//! cost more than it buys:
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
//! shout at working code, which is the right way round for a rule that fires
//! without being asked.

use std::collections::{HashMap, HashSet};

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
    /// Module-level `state` names (not `state var`, which is a cell). Only
    /// these are reported: a rebound `let` reads its pre-rebinding value by
    /// design, while a rebound `state` writes the slot the *next* run reads
    /// from, so the capture is a frame behind.
    reactive: HashSet<String>,
}

impl ModuleRebinds {
    pub(super) fn collect(stmts: &[Stmt]) -> Self {
        let mut out = ModuleRebinds::default();
        for stmt in stmts {
            out.visit_stmt(stmt);
        }
        for v in out.spans.values_mut() {
            v.sort_by_key(|s| s.start.offset);
        }
        out
    }

    /// The first rebinding of `name` at or after `offset` that leaves a capture
    /// taken at `offset` reading a stale value — `None` when there is none, or
    /// when `name` is not a reactive (`state`) binding.
    pub(super) fn lagging_rebind(&self, name: &str, offset: u32) -> Option<SourceSpan> {
        if !self.reactive.contains(name) {
            return None;
        }
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
            StmtKind::State {
                name,
                is_var: false,
                ..
            } => {
                self.reactive.insert(name.clone());
                walk_stmt(self, s);
            }
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
