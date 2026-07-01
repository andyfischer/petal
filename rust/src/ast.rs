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
