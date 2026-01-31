use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum Error {
    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Runtime error: {0}")]
    RuntimeError(String),

    #[error("Type error: {0}")]
    TypeError(String),

    #[error("Unknown variable: {0}")]
    UnknownVariable(String),

    #[error("Unknown function: {0}")]
    UnknownFunction(String),

    #[error("Division by zero")]
    DivisionByZero,

    #[error("Invalid program key")]
    InvalidProgramKey,

    #[error("Invalid stack key")]
    InvalidStackKey,

    #[error("Stack underflow")]
    StackUnderflow,

    #[error("Out of bounds access")]
    OutOfBounds,

    #[error("Loop break")]
    LoopBreak,

    #[error("Loop continue")]
    LoopContinue,
}
