#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Fn,
    Let,
    Return,
    If,
    Else,
    While,
    For,
    In,
    True,
    False,
    Null,
    Struct,
    Enum,
    State,
    Match,
    Loop,
    Break,
    Continue,

    // Literals
    Integer(i64),
    Float(f64),
    String(String),
    Symbol(String),

    // Identifier
    Identifier(String),

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar,       // **
    Equal,
    EqualEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,            // &&
    Or,             // ||
    Bang,
    At,             // @
    Dot,
    DotDot,         // ..
    Question,       // ?
    QuestionDot,    // ?.
    Arrow,          // ->
    FatArrow,       // =>
    ColonColon,     // ::

    // Assignment operators
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    PercentEqual,

    // Delimiters
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Colon,
    Semicolon,

    // Special
    Eof,
}

#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub token: Token,
    pub line: usize,
    pub column: usize,
}

impl TokenInfo {
    pub fn new(token: Token, line: usize, column: usize) -> Self {
        TokenInfo { token, line, column }
    }
}
