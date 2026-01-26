#[derive(Debug, Clone)]
pub enum Expr {
    // Literals
    Integer(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Null,
    Symbol(String),

    // Collections
    Array(Vec<Expr>),
    Object(Vec<(String, Expr)>),

    // Variables
    Identifier(String),

    // Binary operations
    BinaryOp {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    // Unary operations
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },

    // Function call
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    // Method call (obj.method(args))
    MethodCall {
        object: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },

    // Property access
    PropertyAccess {
        object: Box<Expr>,
        property: String,
    },

    // Index access
    IndexAccess {
        object: Box<Expr>,
        index: Box<Expr>,
    },

    // Lambda expression
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
    },

    // Block expression
    Block(Vec<Stmt>),

    // If expression
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },

    // Match expression
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },

    // Dataflow operator (@)
    Dataflow {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    // Assignment
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },

    // Compound assignment (+=, -=, etc.)
    CompoundAssign {
        op: BinaryOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },

    // For loop as expression
    ForExpr {
        var: String,
        iter: Box<Expr>,
        body: Box<Expr>,
    },

    // Range expression (for use in for loops)
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        step: Option<Box<Expr>>,
    },

    // Enum variant access
    EnumVariant {
        enum_name: String,
        variant: String,
        args: Option<Vec<Expr>>,
    },
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Box<Expr>>,
    pub body: Box<Expr>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Literal(Expr),
    Variable(String),
    Array(Vec<Pattern>),
    Object(Vec<(String, Pattern)>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    // Expression statement
    Expr(Expr),

    // Variable declaration
    Let {
        name: String,
        value: Option<Expr>,
    },

    // State declaration
    State {
        name: String,
        value: Expr,
    },

    // Function declaration
    Function {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },

    // Return statement
    Return(Option<Expr>),

    // If statement
    If {
        condition: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Option<Vec<Stmt>>,
    },

    // While loop
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },

    // For loop
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },

    // Loop (infinite)
    Loop {
        body: Vec<Stmt>,
    },

    // Break
    Break,

    // Continue
    Continue,

    // Struct definition
    Struct {
        name: String,
        fields: Vec<(String, Option<String>)>,
    },

    // Enum definition
    Enum {
        name: String,
        variants: Vec<EnumVariant>,
    },
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub statements: Vec<Stmt>,
}
