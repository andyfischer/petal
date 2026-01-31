//! Error types for Petal

use thiserror::Error;

use crate::program::TermId;
use crate::source_map::SourceSpan;

/// Result type for Petal operations
pub type Result<T> = std::result::Result<T, Error>;

/// Petal error types
#[derive(Error, Debug, Clone)]
pub enum Error {
    #[error("Parse error at {span:?}: {message}")]
    Parse { message: String, span: SourceSpan },

    #[error("Runtime error: {message}")]
    Runtime { message: String },

    #[error("Type error: expected {expected}, got {got}")]
    Type { expected: String, got: String },

    #[error("Undefined variable: {name}")]
    UndefinedVariable { name: String },

    #[error("Undefined function: {name}")]
    UndefinedFunction { name: String },

    #[error("Invalid operation: {message}")]
    InvalidOperation { message: String },

    #[error("Division by zero")]
    DivisionByZero,

    #[error("Index out of bounds: {index} for length {length}")]
    IndexOutOfBounds { index: i64, length: usize },

    #[error("Invalid program key")]
    InvalidProgramKey,

    #[error("Invalid stack key")]
    InvalidStackKey,

    #[error("Invalid term id: {0:?}")]
    InvalidTermId(TermId),

    #[error("Stack overflow")]
    StackOverflow,

    #[error("Arity mismatch: expected {expected} arguments, got {got}")]
    ArityMismatch { expected: usize, got: usize },
}
