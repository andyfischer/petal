use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
pub enum Expr {
    Literal(Literal),
    Ident(String),
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
    Record(Vec<(String, Expr)>),
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

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
pub enum Stmt {
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
    State {
        name: String,
        init: Expr,
        id: usize,
    },
}
