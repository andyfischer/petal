use serde::{Deserialize, Serialize};

use crate::source_map::SourceSpan;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Literal {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    /// `??` — short-circuit coalescing: yields the RHS when the LHS is absent
    /// (`Nil` or `Pending`), otherwise the LHS.
    Coalesce,
    Concat,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// An expression with source location.
#[derive(Debug, Clone, Serialize)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize)]
pub enum ExprKind {
    Literal(Literal),
    Ident(String),
    /// `@name` — an in-out argument marker. Only ever produced by the parser;
    /// the [`crate::desugar`] pass rewrites `f(@x)` into `x = f(x)` and strips
    /// every `AtVar` before compilation. Any `AtVar` that survives to the
    /// compiler is an `@` used somewhere the desugar pass can't lift (e.g. not
    /// inside a call at statement level) and compiles to a deferred error.
    AtVar(String),
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Call {
        function: Box<Expr>,
        args: Vec<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_body: Vec<Stmt>,
        else_body: Option<ElseBranch>,
    },
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    List(Vec<Expr>),
    Record(Vec<RecordField>),
    FieldAccess {
        object: Box<Expr>,
        field: String,
    },
    IndexAccess {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Block(Vec<Stmt>),
    Lambda {
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    /// String interpolation: alternating string parts and expressions.
    /// parts has one more element than exprs (parts[0], exprs[0], parts[1], exprs[1], ..., parts[N]).
    StringInterp {
        parts: Vec<String>,
        exprs: Vec<Expr>,
    },
    /// JSX-like element: `<tag props...>children</tag>`
    Element {
        tag: String,
        props: Vec<(String, Expr)>,
        children: Vec<JsxChild>,
    },
}

/// A field in a record literal: either a named field or a spread expression.
#[derive(Debug, Clone, Serialize)]
pub enum RecordField {
    /// Named field: `key: value`
    Named(String, Expr),
    /// Spread: `...expr` — copies all fields from another record
    Spread(Expr),
}

#[derive(Debug, Clone, Serialize)]
pub enum JsxChild {
    Text(String),
    Expr(Expr),
}

#[derive(Debug, Clone, Serialize)]
pub enum ElseBranch {
    Block(Vec<Stmt>),
    ElseIf(Box<Expr>),
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pattern {
    Wildcard,
    Literal(Literal),
    Variable(String),
    Variant {
        name: String,
        fields: Vec<Pattern>,
    },
    List {
        elements: Vec<Pattern>,
        rest: Option<String>,
    },
    Record(Vec<(String, Pattern)>),
}

#[derive(Debug, Clone, Serialize)]
pub enum AssignTarget {
    Name(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Serialize)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<String>,
}

/// A statement with source location.
#[derive(Debug, Clone, Serialize)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize)]
pub enum StmtKind {
    Let {
        name: String,
        value: Expr,
    },
    Assign {
        target: AssignTarget,
        value: Expr,
    },
    Expr(Expr),
    FnDecl {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    EnumDecl {
        name: String,
        variants: Vec<EnumVariant>,
    },
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    State {
        name: String,
        init: Expr,
        id: usize,
        /// Optional explicit key expression for per-iteration state: `state(expr) name = init`
        key: Option<Expr>,
    },
    /// `import m` / `import m as u` / `import m: a, b`. Only allowed before
    /// any other statement in a file (the parser enforces this); consumed by
    /// the module loader (`crate::module`) before compilation — the compiler
    /// itself receives imports pre-resolved.
    Import(ImportDecl),
}

/// One parsed `import` statement.
#[derive(Debug, Clone, Serialize)]
pub struct ImportDecl {
    /// The module name as written (`import ui` → "ui").
    pub module: String,
    /// `import ui as u` → Some("u"). Defaults to the module name.
    pub alias: Option<String>,
    /// `import ui: button, clicked` → Some(["button", "clicked"]).
    /// `None` means qualified-only (`ui.button(...)`).
    pub names: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// AST traversal
//
// A single exhaustive traversal over `ExprKind`/`StmtKind` lives here so the
// handful of hand-rolled walkers across the compiler (desugar's `@`-lifter,
// the phi pre-scan, lint's rebind and total walks, …) no longer each
// re-enumerate every variant. Adding an AST variant means updating `walk_expr`
// / `walk_stmt` (and their `_mut` twins) once, here, and the compiler then
// points every visitor at the missing arm.
//
// The default `visit_*` methods descend into *every* child. A walker that
// needs a narrower policy — stop at a call boundary, skip match arms, treat a
// nested body as its own scope — overrides only the methods whose children it
// treats differently and calls `walk_*` for the rest.
// ---------------------------------------------------------------------------

/// Read-only AST visitor. Default methods perform a total traversal; override
/// `visit_expr`/`visit_stmt` for the nodes whose children need a different
/// policy and delegate to [`walk_expr`]/[`walk_stmt`] for everything else.
pub trait ExprVisitor {
    fn visit_expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
    fn visit_stmt(&mut self, s: &Stmt) {
        walk_stmt(self, s);
    }
}

/// Visit every direct child expression/statement of `e` with `v`.
pub fn walk_expr<V: ExprVisitor + ?Sized>(v: &mut V, e: &Expr) {
    match &e.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::AtVar(_) => {}
        ExprKind::BinaryOp { left, right, .. } => {
            v.visit_expr(left);
            v.visit_expr(right);
        }
        ExprKind::UnaryOp { operand, .. } => v.visit_expr(operand),
        ExprKind::Call { function, args } => {
            v.visit_expr(function);
            for a in args {
                v.visit_expr(a);
            }
        }
        ExprKind::If {
            condition,
            then_body,
            else_body,
        } => {
            v.visit_expr(condition);
            for s in then_body {
                v.visit_stmt(s);
            }
            match else_body {
                Some(ElseBranch::Block(stmts)) => {
                    for s in stmts {
                        v.visit_stmt(s);
                    }
                }
                Some(ElseBranch::ElseIf(e)) => v.visit_expr(e),
                None => {}
            }
        }
        ExprKind::Match { subject, arms } => {
            v.visit_expr(subject);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    v.visit_expr(g);
                }
                v.visit_expr(&arm.body);
            }
        }
        ExprKind::List(items) => {
            for e in items {
                v.visit_expr(e);
            }
        }
        ExprKind::Record(fields) => {
            for f in fields {
                match f {
                    RecordField::Named(_, e) | RecordField::Spread(e) => v.visit_expr(e),
                }
            }
        }
        ExprKind::FieldAccess { object, .. } => v.visit_expr(object),
        ExprKind::IndexAccess { object, index } => {
            v.visit_expr(object);
            v.visit_expr(index);
        }
        ExprKind::Block(stmts) => {
            for s in stmts {
                v.visit_stmt(s);
            }
        }
        ExprKind::Lambda { body, .. } => {
            for s in body {
                v.visit_stmt(s);
            }
        }
        ExprKind::StringInterp { exprs, .. } => {
            for e in exprs {
                v.visit_expr(e);
            }
        }
        ExprKind::Element { props, children, .. } => {
            for (_, e) in props {
                v.visit_expr(e);
            }
            for c in children {
                if let JsxChild::Expr(e) = c {
                    v.visit_expr(e);
                }
            }
        }
    }
}

/// Visit every direct child expression/statement of `s` with `v`.
pub fn walk_stmt<V: ExprVisitor + ?Sized>(v: &mut V, s: &Stmt) {
    match &s.kind {
        StmtKind::Let { value, .. } => v.visit_expr(value),
        StmtKind::Assign { target, value } => {
            match target {
                AssignTarget::Name(_) => {}
                AssignTarget::Field(object, _) => v.visit_expr(object),
                AssignTarget::Index(object, index) => {
                    v.visit_expr(object);
                    v.visit_expr(index);
                }
            }
            v.visit_expr(value);
        }
        StmtKind::Expr(e) => v.visit_expr(e),
        StmtKind::FnDecl { body, .. } => {
            for s in body {
                v.visit_stmt(s);
            }
        }
        StmtKind::EnumDecl { .. } => {}
        StmtKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            for s in body {
                v.visit_stmt(s);
            }
        }
        StmtKind::While { condition, body } => {
            v.visit_expr(condition);
            for s in body {
                v.visit_stmt(s);
            }
        }
        StmtKind::Return(value) => {
            if let Some(e) = value {
                v.visit_expr(e);
            }
        }
        StmtKind::Break | StmtKind::Continue => {}
        StmtKind::State { init, key, .. } => {
            v.visit_expr(init);
            if let Some(k) = key {
                v.visit_expr(k);
            }
        }
        StmtKind::Import(_) => {}
    }
}

/// Mutable mirror of [`ExprVisitor`] for passes that rewrite the AST in place.
pub trait ExprVisitorMut {
    fn visit_expr(&mut self, e: &mut Expr) {
        walk_expr_mut(self, e);
    }
    fn visit_stmt(&mut self, s: &mut Stmt) {
        walk_stmt_mut(self, s);
    }
}

/// Visit every direct child expression/statement of `e` with `v` (mutable).
pub fn walk_expr_mut<V: ExprVisitorMut + ?Sized>(v: &mut V, e: &mut Expr) {
    match &mut e.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::AtVar(_) => {}
        ExprKind::BinaryOp { left, right, .. } => {
            v.visit_expr(left);
            v.visit_expr(right);
        }
        ExprKind::UnaryOp { operand, .. } => v.visit_expr(operand),
        ExprKind::Call { function, args } => {
            v.visit_expr(function);
            for a in args.iter_mut() {
                v.visit_expr(a);
            }
        }
        ExprKind::If {
            condition,
            then_body,
            else_body,
        } => {
            v.visit_expr(condition);
            for s in then_body.iter_mut() {
                v.visit_stmt(s);
            }
            match else_body {
                Some(ElseBranch::Block(stmts)) => {
                    for s in stmts.iter_mut() {
                        v.visit_stmt(s);
                    }
                }
                Some(ElseBranch::ElseIf(e)) => v.visit_expr(e),
                None => {}
            }
        }
        ExprKind::Match { subject, arms } => {
            v.visit_expr(subject);
            for arm in arms.iter_mut() {
                if let Some(g) = &mut arm.guard {
                    v.visit_expr(g);
                }
                v.visit_expr(&mut arm.body);
            }
        }
        ExprKind::List(items) => {
            for e in items.iter_mut() {
                v.visit_expr(e);
            }
        }
        ExprKind::Record(fields) => {
            for f in fields.iter_mut() {
                match f {
                    RecordField::Named(_, e) | RecordField::Spread(e) => v.visit_expr(e),
                }
            }
        }
        ExprKind::FieldAccess { object, .. } => v.visit_expr(object),
        ExprKind::IndexAccess { object, index } => {
            v.visit_expr(object);
            v.visit_expr(index);
        }
        ExprKind::Block(stmts) => {
            for s in stmts.iter_mut() {
                v.visit_stmt(s);
            }
        }
        ExprKind::Lambda { body, .. } => {
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        ExprKind::StringInterp { exprs, .. } => {
            for e in exprs.iter_mut() {
                v.visit_expr(e);
            }
        }
        ExprKind::Element { props, children, .. } => {
            for (_, e) in props.iter_mut() {
                v.visit_expr(e);
            }
            for c in children.iter_mut() {
                if let JsxChild::Expr(e) = c {
                    v.visit_expr(e);
                }
            }
        }
    }
}

/// Visit every direct child expression/statement of `s` with `v` (mutable).
pub fn walk_stmt_mut<V: ExprVisitorMut + ?Sized>(v: &mut V, s: &mut Stmt) {
    match &mut s.kind {
        StmtKind::Let { value, .. } => v.visit_expr(value),
        StmtKind::Assign { target, value } => {
            match target {
                AssignTarget::Name(_) => {}
                AssignTarget::Field(object, _) => v.visit_expr(object),
                AssignTarget::Index(object, index) => {
                    v.visit_expr(object);
                    v.visit_expr(index);
                }
            }
            v.visit_expr(value);
        }
        StmtKind::Expr(e) => v.visit_expr(e),
        StmtKind::FnDecl { body, .. } => {
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        StmtKind::EnumDecl { .. } => {}
        StmtKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        StmtKind::While { condition, body } => {
            v.visit_expr(condition);
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        StmtKind::Return(value) => {
            if let Some(e) = value {
                v.visit_expr(e);
            }
        }
        StmtKind::Break | StmtKind::Continue => {}
        StmtKind::State { init, key, .. } => {
            v.visit_expr(init);
            if let Some(k) = key {
                v.visit_expr(k);
            }
        }
        StmtKind::Import(_) => {}
    }
}

/// Visit every expression in `e`'s subtree (pre-order), descending into nested
/// statements. The closure-shaped counterpart to a total [`ExprVisitor`]; used
/// where a caller just wants to inspect every expression.
pub fn for_each_expr(e: &Expr, f: &mut impl FnMut(&Expr)) {
    ForEachExpr(f).visit_expr(e);
}

/// Like [`for_each_expr`] but rooted at a statement.
pub fn for_each_expr_in_stmt(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    ForEachExpr(f).visit_stmt(s);
}

struct ForEachExpr<'a, F>(&'a mut F);

impl<F: FnMut(&Expr)> ExprVisitor for ForEachExpr<'_, F> {
    fn visit_expr(&mut self, e: &Expr) {
        (self.0)(e);
        walk_expr(self, e);
    }
}
