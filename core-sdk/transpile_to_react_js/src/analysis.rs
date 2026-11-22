//! Questions about the program that decide its React shape: which functions
//! return elements (components), which keep `state` (components or hooks),
//! which names a block writes.

use std::collections::{BTreeSet, HashMap, HashSet};

use petal::ast::{AssignTarget, ElseBranch, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_expr, walk_stmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    /// Returns an element: a React function component, called as `<Name />`.
    Component,
    /// Keeps `state` (or calls a hook) but returns a value: a custom hook.
    Hook,
    /// Anything else: a plain function.
    Plain,
}

#[derive(Debug, Clone)]
pub struct FnInfo {
    /// The JS name: PascalCase for a component, `useX` for a hook.
    pub js: String,
    pub params: Vec<String>,
    pub kind: FnKind,
    /// The design component this function renders (`component("card", card)`).
    pub design_component: Option<String>,
}

/// The last expression of a block, which is its value.
pub fn tail_expr(body: &[Stmt]) -> Option<&Expr> {
    match &body.last()?.kind {
        StmtKind::Expr(e) => Some(e),
        _ => None,
    }
}

/// Whether evaluating `e` gives an element, judged from its shape: an element
/// literal, a call of a function that returns one, or a branch whose every
/// arm does. `elem_fns` are the functions known to return elements;
/// `elem_names` the local names bound to one.
pub fn is_element_expr(e: &Expr, elem_fns: &HashSet<String>, elem_names: &HashSet<String>) -> bool {
    match &e.kind {
        ExprKind::Element { .. } => true,
        ExprKind::Call { function, .. } => match &function.kind {
            ExprKind::Ident(n) => elem_fns.contains(n),
            _ => false,
        },
        ExprKind::Ident(n) => elem_names.contains(n),
        ExprKind::If { then_body, else_body, .. } => {
            let then_ok = block_is_element(then_body, elem_fns, elem_names);
            match else_body {
                None => then_ok,
                Some(ElseBranch::Block(b)) => then_ok || block_is_element(b, elem_fns, elem_names),
                Some(ElseBranch::ElseIf(e)) => then_ok || is_element_expr(e, elem_fns, elem_names),
            }
        }
        ExprKind::Match { arms, .. } => arms.iter().any(|a| block_is_element(&a.body, elem_fns, elem_names)),
        _ => false,
    }
}

pub fn block_is_element(body: &[Stmt], elem_fns: &HashSet<String>, elem_names: &HashSet<String>) -> bool {
    let mut names = elem_names.clone();
    for s in body {
        if let StmtKind::Let { name, value, .. } = &s.kind {
            if is_element_expr(value, elem_fns, &names) {
                names.insert(name.clone());
            }
        }
    }
    match body.last().map(|s| &s.kind) {
        Some(StmtKind::Expr(e)) => is_element_expr(e, elem_fns, &names),
        Some(StmtKind::Return(Some(e))) => is_element_expr(e, elem_fns, &names),
        _ => body.iter().any(|s| matches!(&s.kind, StmtKind::Return(Some(e)) if is_element_expr(e, elem_fns, &names))),
    }
}

/// Whether a function body declares `state` (not counting nested functions
/// and lambdas, which are their own places).
pub fn declares_state(body: &[Stmt]) -> bool {
    struct V(bool);
    impl ExprVisitor for V {
        fn visit_expr(&mut self, e: &Expr) {
            if matches!(e.kind, ExprKind::Lambda { .. }) {
                return;
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::State { .. } => self.0 = true,
                StmtKind::FnDecl { .. } => return,
                _ => {}
            }
            walk_stmt(self, s);
        }
    }
    let mut v = V(false);
    body.iter().for_each(|s| v.visit_stmt(s));
    v.0
}

/// The names of the functions a body calls directly (bare `f(...)`).
pub fn called_names(body: &[Stmt]) -> BTreeSet<String> {
    struct V(BTreeSet<String>);
    impl ExprVisitor for V {
        fn visit_expr(&mut self, e: &Expr) {
            if let ExprKind::Call { function, .. } = &e.kind {
                if let ExprKind::Ident(n) = &function.kind {
                    self.0.insert(n.clone());
                }
            }
            if matches!(e.kind, ExprKind::Lambda { .. }) {
                return;
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            if matches!(s.kind, StmtKind::FnDecl { .. }) {
                return;
            }
            walk_stmt(self, s);
        }
    }
    let mut v = V(BTreeSet::new());
    body.iter().for_each(|s| v.visit_stmt(s));
    v.0
}

/// The base name an assignment target writes: `r` for `r.a.b = …`.
pub fn target_base(t: &AssignTarget) -> Option<&str> {
    match t {
        AssignTarget::Name(n) => Some(n),
        AssignTarget::Field(obj, _) | AssignTarget::Index(obj, _) => expr_base(obj),
    }
}

fn expr_base(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Ident(n) | ExprKind::CellGet(n) => Some(n),
        ExprKind::FieldAccess { object, .. } | ExprKind::IndexAccess { object, .. } => expr_base(object),
        _ => None,
    }
}

/// Names a block rebinds with `=` (not `set`), with `lambdas` saying whether
/// writes inside lambdas count.
pub fn assigned_names(body: &[Stmt], lambdas: bool) -> HashSet<String> {
    struct V {
        out: HashSet<String>,
        lambdas: bool,
    }
    impl ExprVisitor for V {
        fn visit_expr(&mut self, e: &Expr) {
            if !self.lambdas && matches!(e.kind, ExprKind::Lambda { .. }) {
                return;
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Assign { target, .. } => {
                    if let Some(b) = target_base(target) {
                        self.out.insert(b.to_string());
                    }
                }
                StmtKind::FnDecl { .. } => return,
                _ => {}
            }
            walk_stmt(self, s);
        }
    }
    let mut v = V { out: HashSet::new(), lambdas };
    body.iter().for_each(|s| v.visit_stmt(s));
    v.out
}

/// Names a block declares at its own level (and in nested blocks, which in
/// JS are their own scopes but cannot be told from Petal's here).
pub fn declared_names(body: &[Stmt]) -> HashSet<String> {
    struct V(HashSet<String>);
    impl ExprVisitor for V {
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Lambda { .. } => {}
                ExprKind::For { var, .. } => {
                    self.0.insert(var.clone());
                    walk_expr(self, e);
                }
                _ => walk_expr(self, e),
            }
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Let { name, .. } | StmtKind::State { name, .. } | StmtKind::Memo { name, .. } => {
                    self.0.insert(name.clone());
                }
                StmtKind::For { var, .. } => {
                    self.0.insert(var.clone());
                }
                StmtKind::FnDecl { name, .. } => {
                    self.0.insert(name.clone());
                    return;
                }
                _ => {}
            }
            walk_stmt(self, s);
        }
    }
    let mut v = V(HashSet::new());
    body.iter().for_each(|s| v.visit_stmt(s));
    v.0
}

pub fn idents_in_stmt(s: &Stmt, out: &mut HashSet<String>) {
    petal::ast::for_each_expr_in_stmt(s, &mut |x| {
        if let ExprKind::Ident(n) | ExprKind::CellGet(n) = &x.kind {
            out.insert(n.clone());
        }
    });
    match &s.kind {
        StmtKind::Assign { target, .. } | StmtKind::Set { target, .. } => {
            if let Some(b) = target_base(target) {
                out.insert(b.to_string());
            }
        }
        _ => {}
    }
}

/// Every `state var` / `var` read with `get`, or written with `set`, in a
/// function body (lambdas included: a handler's write is the function's).
pub fn cell_uses(body: &[Stmt]) -> (HashSet<String>, HashSet<String>) {
    struct V {
        reads: HashSet<String>,
        writes: HashSet<String>,
    }
    impl ExprVisitor for V {
        fn visit_expr(&mut self, e: &Expr) {
            if let ExprKind::CellGet(n) = &e.kind {
                self.reads.insert(n.clone());
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::Set { target, .. } = &s.kind {
                if let Some(b) = target_base(target) {
                    self.writes.insert(b.to_string());
                }
            }
            if matches!(s.kind, StmtKind::FnDecl { .. }) {
                return;
            }
            walk_stmt(self, s);
        }
    }
    let mut v = V { reads: HashSet::new(), writes: HashSet::new() };
    body.iter().for_each(|s| v.visit_stmt(s));
    (v.reads, v.writes)
}

/// Whether any part of an expression or statement satisfies `pred`.
pub fn any_expr_in_stmt(s: &Stmt, pred: &mut impl FnMut(&Expr) -> bool) -> bool {
    let mut found = false;
    petal::ast::for_each_expr_in_stmt(s, &mut |e| {
        if !found && pred(e) {
            found = true;
        }
    });
    found
}

/// The `component("name", render, …)` calls of a design document:
/// render function name → component name, and the component names.
pub fn design_components(stmts: &[Stmt]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for s in stmts {
        petal::ast::for_each_expr_in_stmt(s, &mut |e| {
            if let ExprKind::Call { function, args, .. } = &e.kind {
                if matches!(&function.kind, ExprKind::Ident(n) if n == "component") && args.len() >= 2 {
                    if let (ExprKind::Literal(petal::ast::Literal::String(name)), ExprKind::Ident(f)) = (&args[0].kind, &args[1].kind) {
                        out.insert(f.clone(), name.clone());
                    }
                }
            }
        });
    }
    out
}


/// Every name the program binds anywhere: declarations, parameters, loop
/// variables, pattern bindings.
pub fn all_bound_names(stmts: &[Stmt]) -> HashSet<String> {
    struct V(HashSet<String>);
    impl ExprVisitor for V {
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Lambda { params, .. } => self.0.extend(params.iter().map(|p| p.name.clone())),
                ExprKind::For { var, .. } => {
                    self.0.insert(var.clone());
                }
                ExprKind::Match { arms, .. } => {
                    for a in arms {
                        self.0.extend(a.pattern.bound_names());
                    }
                }
                _ => {}
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Let { name, .. } | StmtKind::State { name, .. } | StmtKind::Memo { name, .. } | StmtKind::EnumDecl { name, .. } | StmtKind::ClassDecl { name, .. } => {
                    self.0.insert(name.clone());
                }
                StmtKind::For { var, .. } => {
                    self.0.insert(var.clone());
                }
                StmtKind::FnDecl { name, params, .. } => {
                    self.0.insert(name.clone());
                    self.0.extend(params.iter().map(|p| p.name.clone()));
                }
                _ => {}
            }
            walk_stmt(self, s);
        }
    }
    let mut v = V(HashSet::new());
    stmts.iter().for_each(|s| v.visit_stmt(s));
    v.0
}
