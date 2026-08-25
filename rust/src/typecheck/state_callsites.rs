//! `state-shared-callsites` lint: an in-function `state` whose enclosing
//! function is called from more than one place, or from inside a loop.
//!
//! Today a `state` slot is keyed by its declaration alone, so *every* call of
//! the declaring function reaches the same slot — a C `static` local. `state`
//! is moving to per-call-path keying (docs/dev/state-callsite-keying-plan.md):
//! each callsite, and each loop iteration around it, gets its own slot. A
//! helper that today launders one shared value through several callers will
//! quietly become several independent values.
//!
//! This pass reports those declarations ahead of the flip and points at the
//! migration idiom (plan §2.4): a **top-level** `state var` cell, read and
//! written with `get`/`set`, which means one value under both the old and the
//! new rules.
//!
//! Precision over recall, like the rest of the warning passes:
//! - Only declarations with a statically visible reason to change behaviour
//!   warn: the enclosing function needs two or more callsites in this module,
//!   or one inside a `for`/`while`. A single straight-line callsite behaves
//!   the same before and after, and never warns.
//! - `state(key) name = …` never warns. An explicit key is **absolute** under
//!   the new rules too (plan §2.2) — it ignores the call path — so those
//!   declarations are already saying which slot they want.
//! - Top-level `state` never warns: module scope is the root path, whose
//!   behaviour is unchanged.
//! - A declaration inside a lambda is left alone. Its slot follows the
//!   lambda's own call path, which is not something a lexical scan can count
//!   callsites for.
//!
//! Known gaps (deliberate, to keep false positives out): calls from *other*
//! modules and from the host are invisible here, so an exported one-callsite
//! helper stays quiet; and a function passed by name to `map`/`filter` is a
//! reference, not a callsite, so it is not counted as an in-loop call.

use std::collections::HashMap;

use crate::ast::{self, Expr, ExprKind, ExprVisitor, Stmt, StmtKind};
use crate::diagnostic::Diagnostic;
use crate::source_map::SourceSpan;

/// Walk a module's statements and report every in-function `state` whose
/// enclosing function is reached from more than one callsite, or from a loop.
pub fn check_state_callsites(stmts: &[Stmt]) -> Vec<Diagnostic> {
    let decls = collect_fn_decls(stmts);
    if decls.iter().all(|d| d.states.is_empty()) {
        return Vec::new();
    }

    let mut calls = CallCollector {
        loop_depth: 0,
        calls: Vec::new(),
    };
    for s in stmts {
        calls.visit_stmt(s);
    }

    // A bare name declared more than once is an overload set: `f(1)` and
    // `f(1, 2)` are different functions, so arity separates their callsites.
    let mut decls_per_name: HashMap<&str, usize> = HashMap::new();
    for d in &decls {
        *decls_per_name.entry(d.bare_name()).or_insert(0) += 1;
    }

    let mut diags = Vec::new();
    for decl in &decls {
        if decl.states.is_empty() {
            continue;
        }
        let overloaded = decls_per_name.get(decl.bare_name()).copied().unwrap_or(0) > 1;
        let sites: Vec<&Callsite> = calls
            .calls
            .iter()
            .filter(|c| c.name == decl.bare_name() && (!overloaded || c.argc == decl.arity))
            .collect();
        let in_loop = sites.iter().any(|c| c.in_loop);
        if sites.len() < 2 && !in_loop {
            continue;
        }
        for state in &decl.states {
            diags.push(Diagnostic {
                span: state.span,
                message: message(&state.name, &decl.name, sites.len(), in_loop),
            });
        }
    }
    diags
}

fn message(state: &str, function: &str, sites: usize, in_loop: bool) -> String {
    let reason = match (sites, in_loop) {
        (n, true) if n >= 2 => format!("which is called from {n} places, one of them in a loop"),
        (_, true) => "which is called inside a loop".to_string(),
        (n, false) => format!("which is called from {n} places"),
    };
    format!(
        "state-shared-callsites: `{state}` is declared inside `{function}`, {reason}. \
         Every one of those calls shares this one slot today, but `state` is moving to \
         per-call-path keying, where each callsite — and each loop iteration around it — \
         gets its own. If they are meant to share one value, hoist it out: declare \
         `state var {state} = …` at the top level and read and write it with \
         `get {state}` / `set {state} = …`."
    )
}

/// One `state` declaration eligible for the lint.
struct StateDecl {
    name: String,
    span: SourceSpan,
}

/// A named function and the `state` declarations directly inside it.
struct FnDeclInfo {
    /// As written: `Class.method` for a method.
    name: String,
    arity: usize,
    states: Vec<StateDecl>,
}

impl FnDeclInfo {
    /// The name a call writes: the method name for `fn Rect.area(r)`.
    fn bare_name(&self) -> &str {
        match self.name.rsplit_once('.') {
            Some((_, method)) => method,
            None => &self.name,
        }
    }
}

/// Every `fn` in the module — top level or nested — with its own `state`
/// declarations attached.
fn collect_fn_decls(stmts: &[Stmt]) -> Vec<FnDeclInfo> {
    struct Collector {
        out: Vec<FnDeclInfo>,
    }
    impl ExprVisitor for Collector {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::FnDecl {
                name, params, body, ..
            } = &s.kind
            {
                self.out.push(FnDeclInfo {
                    name: name.clone(),
                    arity: params.len(),
                    states: direct_states(body),
                });
            }
            // Keep descending: a nested `fn` is its own declaration, and its
            // states belong to it rather than to the function around it.
            ast::walk_stmt(self, s);
        }
    }
    let mut c = Collector { out: Vec::new() };
    for s in stmts {
        c.visit_stmt(s);
    }
    c.out
}

/// The unkeyed `state` declarations lexically inside one function body,
/// stopping at every nested function and lambda boundary.
fn direct_states(body: &[Stmt]) -> Vec<StateDecl> {
    struct Walker {
        out: Vec<StateDecl>,
    }
    impl ExprVisitor for Walker {
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                // A nested function owns its own states.
                StmtKind::FnDecl { .. } => {}
                StmtKind::State { name, key, .. } => {
                    // An explicit key is absolute: unaffected by the flip.
                    if key.is_none() {
                        self.out.push(StateDecl {
                            name: name.clone(),
                            span: s.span,
                        });
                    }
                }
                _ => ast::walk_stmt(self, s),
            }
        }
        fn visit_expr(&mut self, e: &Expr) {
            // A lambda's states are keyed by the lambda's own call path.
            if matches!(e.kind, ExprKind::Lambda { .. }) {
                return;
            }
            ast::walk_expr(self, e);
        }
    }
    let mut w = Walker { out: Vec::new() };
    for s in body {
        w.visit_stmt(s);
    }
    w.out
}

/// One call written in the module, by callee name.
struct Callsite {
    /// The name at the callsite: `f` for `f(x)`, `area` for `r.area()`.
    name: String,
    /// Argument count, with the receiver counted for a method call, so it
    /// lines up with the declaration's parameter list.
    argc: usize,
    in_loop: bool,
}

struct CallCollector {
    loop_depth: usize,
    calls: Vec<Callsite>,
}

impl CallCollector {
    fn walk_loop_body(&mut self, body: &[Stmt]) {
        self.loop_depth += 1;
        for s in body {
            self.visit_stmt(s);
        }
        self.loop_depth -= 1;
    }
}

impl ExprVisitor for CallCollector {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.walk_loop_body(body);
            }
            StmtKind::While { condition, body } => {
                self.visit_expr(condition);
                self.walk_loop_body(body);
            }
            StmtKind::FnDecl { body, .. } => {
                // A `fn` declared inside a loop is not *called* by it: the
                // body starts a fresh loop context.
                let outer = std::mem::replace(&mut self.loop_depth, 0);
                for s in body {
                    self.visit_stmt(s);
                }
                self.loop_depth = outer;
            }
            _ => ast::walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.walk_loop_body(body);
                return;
            }
            ExprKind::Call { function, args } => {
                let site = match &function.kind {
                    ExprKind::Ident(name) => Some((name.clone(), args.len())),
                    // `r.area()` — the receiver is the method's first param.
                    ExprKind::FieldAccess { field, .. } => Some((field.clone(), args.len() + 1)),
                    _ => None,
                };
                if let Some((name, argc)) = site {
                    self.calls.push(Callsite {
                        name,
                        argc,
                        in_loop: self.loop_depth > 0,
                    });
                }
            }
            _ => {}
        }
        ast::walk_expr(self, e);
    }
}
