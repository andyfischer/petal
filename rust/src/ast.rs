use serde::{Deserialize, Serialize};

use crate::source_map::SourceSpan;

// The serialized AST (`show-ast --json`) omits fields holding their default
// value — `false` flags, `None` annotations, empty variant field lists — via
// `skip_serializing_if`. Absence means the default. The two AST types that are
// also *de*serialized (`Pattern`/`Literal`, embedded in the IR JSON via
// `Program.match_arms`) pair each skip with `#[serde(default)]` so both the
// compact and the explicit spellings load.
fn is_false(b: &bool) -> bool {
    !*b
}

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
    /// `get name` — an explicit read of a `var` cell's current contents.
    ///
    /// Required wherever the read crosses a function boundary from the
    /// declaration, because that is exactly where a bare name would be
    /// ambiguous: a captured `let`/`state` reads the value as of the
    /// function's definition point, while a cell reads the value now. Inside
    /// the declaring scope both spellings mean the same thing and the bare one
    /// is still allowed.
    CellGet(String),
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
        #[serde(skip_serializing_if = "Option::is_none")]
        else_body: Option<ElseBranch>,
    },
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// A `for` loop used in value position (`x = for … do … end`): evaluates to
    /// a list built from the last expression of each iteration (a mapping). The
    /// statement form ([`StmtKind::For`]) runs purely for side effects and
    /// collects nothing. `while` has no expression form — it is statement-only.
    For {
        var: String,
        iter: Box<Expr>,
        body: Vec<Stmt>,
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
    /// An access chain written with `?.` somewhere in it (`cfg?.window.width`,
    /// `rows?.[0]`). The inner expression is an ordinary `FieldAccess`/
    /// `IndexAccess` spine; the wrapper marks that the *whole* spine reads
    /// absence-tolerantly, so a record that simply does not carry a link yields
    /// `Nil` rather than aborting — the same tolerance a `??` left-hand side
    /// already gets, made available without writing a fallback.
    ///
    /// One `?.` covers the chain it appears in, matching JavaScript's
    /// short-circuit: in `a?.b.c` a missing `b` yields nil instead of erroring
    /// on the `.c`.
    OptionalAccess(Box<Expr>),
    Block(Vec<Stmt>),
    Lambda {
        params: Vec<Param>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<Pattern>,
    },
    List {
        elements: Vec<Pattern>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
}

/// A written type annotation: the source name plus its resolution.
/// `resolved` is `None` when `name` is not a recognized type — the checker
/// warns on that but treats it as `any`.
///
/// `span` covers the *name as written* (`nosuch` in `fn f(a: nosuch)`), which is
/// what a diagnostic about the annotation should underline. It is deliberately
/// outside `PartialEq`: two annotations are the same annotation when they spell
/// the same type, wherever they were written.
#[derive(Debug, Clone, Serialize)]
pub struct TypeAnn {
    /// The type name exactly as written (`int`, `str`, `banana`).
    pub name: String,
    /// The resolved static type, or `None` for an unrecognized name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<crate::types::Type>,
    /// Where the type name was written. [`ZERO_SPAN`] when synthesized.
    #[serde(skip)]
    pub span: SourceSpan,
}

impl PartialEq for TypeAnn {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.resolved == other.resolved
    }
}

impl TypeAnn {
    /// An annotation with no recorded position — for synthesized annotations
    /// and for tests. Real parses go through [`TypeAnn::at`].
    pub fn new(name: String) -> Self {
        Self::at(name, crate::source_map::ZERO_SPAN)
    }

    /// The annotation `name` as written at `span`.
    pub fn at(name: String, span: SourceSpan) -> Self {
        let resolved = crate::types::Type::from_name(&name);
        TypeAnn {
            name,
            resolved,
            span,
        }
    }
}

/// One field of a `class` declaration: `x: int`. The annotation is optional
/// (an un-annotated field is `any`) and is kept verbatim even when the name is
/// unrecognized, exactly like a parameter's.
#[derive(Debug, Clone, Serialize)]
pub struct ClassFieldDecl {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeAnn>,
    /// The whole field declaration, `x: int` — what a diagnostic about *this
    /// field* (a duplicate name, say) underlines. Without it every field-level
    /// message fell back to the class's own span, i.e. line 1, column 1.
    #[serde(skip)]
    pub span: SourceSpan,
}

/// A function/lambda parameter with an optional declared type.
/// `ty` is `None` when the parameter is un-annotated. A written annotation is
/// preserved even when its name is unrecognized (`resolved: None`).
#[derive(Debug, Clone, Serialize)]
pub struct Param {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeAnn>,
}

/// A statement with source location.
#[derive(Debug, Clone, Serialize)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: SourceSpan,
    /// Whether this top-level declaration was written with the `export`
    /// modifier (`export fn`, `export let`, `export state`, `export enum`).
    /// Only meaningful for a module's top-level `fn`/`let`/`state`/`enum`: it
    /// gates what importers can see (see `docs/module-system.md`). `false`
    /// everywhere else — nested statements and the entry file never export.
    #[serde(skip_serializing_if = "is_false")]
    pub exported: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum StmtKind {
    Let {
        name: String,
        /// Optional declared type (`let x: int = …`). `None` only when
        /// un-annotated; a written but unrecognized name is preserved as a
        /// [`TypeAnn`] with `resolved: None`.
        #[serde(skip_serializing_if = "Option::is_none")]
        ty: Option<TypeAnn>,
        value: Expr,
        /// Written `var x = …` rather than `let x = …`: a mutable cell, which
        /// is written with `set` and rejects `=`.
        /// See docs/dev/var-next-steps.md (Two write keywords).
        #[serde(skip_serializing_if = "is_false")]
        is_var: bool,
        /// Written `config let x = …`: the binding is declared as a tuning
        /// knob. Direct manipulation prefers editing config bindings and
        /// leaves the rest alone (docs/direct-manipulation.md); hosts may
        /// also render them as sliders. No effect on evaluation.
        #[serde(skip_serializing_if = "is_false")]
        is_config: bool,
    },
    Assign {
        target: AssignTarget,
        value: Expr,
    },
    /// `set x = …` — a write through a `var` cell. Distinct from [`Self::Assign`]
    /// on purpose: `=` is a dataflow rebind and `set` is a mutation, and each
    /// binding kind accepts exactly one of them.
    Set {
        target: AssignTarget,
        value: Expr,
    },
    Expr(Expr),
    FnDecl {
        /// The bound name. For a method declaration this is the *qualified*
        /// name `Class.method` — the same string the class's method table and
        /// the runtime dispatcher key on — so every existing path that binds,
        /// overloads or captures a function keeps working unchanged.
        name: String,
        /// `Some("Rect")` for `fn Rect.center_x(...)`: this declaration is a
        /// method on that class, and the first parameter is its receiver.
        /// `None` for an ordinary function.
        #[serde(skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        params: Vec<Param>,
        /// Optional declared return type (`fn f(…) -> int`). `None` when
        /// un-annotated; a written but unrecognized name is preserved as a
        /// [`TypeAnn`] with `resolved: None`. Named functions only; lambdas have
        /// no return-type slot.
        #[serde(skip_serializing_if = "Option::is_none")]
        ret: Option<TypeAnn>,
        body: Vec<Stmt>,
    },
    EnumDecl {
        name: String,
        variants: Vec<EnumVariant>,
    },
    /// `class Rect ... end` — a named record type. The fields are in
    /// declaration order, which is also the order the generated constructor
    /// takes them in. See docs/language-guide.md (Classes & Methods).
    ClassDecl {
        name: String,
        fields: Vec<ClassFieldDecl>,
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
        /// Optional `state x: int = …` annotation. A reactive binding has no
        /// trustworthy inferred type (a later frame or `set` can replace it), so
        /// this is the only thing that lets the checker constrain a state name.
        #[serde(skip_serializing_if = "Option::is_none")]
        ty: Option<TypeAnn>,
        init: Expr,
        id: usize,
        /// Optional explicit key expression for per-iteration state: `state(expr) name = init`
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<Expr>,
        /// Written `state var x = …`: a cell that persists across frames.
        #[serde(skip_serializing_if = "is_false")]
        is_var: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// `import ui: button, clicked` → Some(["button", "clicked"]).
    /// `None` means qualified-only (`ui.button(...)`).
    #[serde(skip_serializing_if = "Option::is_none")]
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
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::AtVar(_) | ExprKind::CellGet(_) => {}
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
        ExprKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            for s in body {
                v.visit_stmt(s);
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
        ExprKind::OptionalAccess(inner) => v.visit_expr(inner),
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
        ExprKind::Element {
            props, children, ..
        } => {
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
        StmtKind::Assign { target, value } | StmtKind::Set { target, value } => {
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
        StmtKind::EnumDecl { .. } | StmtKind::ClassDecl { .. } => {}
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
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::AtVar(_) | ExprKind::CellGet(_) => {}
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
        ExprKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            for s in body.iter_mut() {
                v.visit_stmt(s);
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
        ExprKind::OptionalAccess(inner) => v.visit_expr(inner),
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
        ExprKind::Element {
            props, children, ..
        } => {
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
        StmtKind::Assign { target, value } | StmtKind::Set { target, value } => {
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
        StmtKind::EnumDecl { .. } | StmtKind::ClassDecl { .. } => {}
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
