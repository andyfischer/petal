//! The Petal programming language runtime
//!
//! Petal is a dataflow-first language with first-class state management,
//! projectional views, and live editing capabilities.

pub mod env;
pub mod program;
pub mod stack;
pub mod value;
pub mod eval;
pub mod parse;
pub mod source_map;
pub mod typing;
pub mod live_edit;
pub mod projection;
pub mod differentiate;
pub mod provenance;

// Re-export main types
pub use env::{Env, ProgramKey, StackKey};
pub use program::{Program, ConstantId, TermId, StateKey};
pub use stack::StepResult;
pub use value::Value;
pub use parse::ParseError;

// Error types
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("Type error: {0}")]
    TypeError(String),

    #[error("Runtime error: {0}")]
    Runtime(String),

    #[error("Division by zero")]
    DivisionByZero,

    #[error("Undefined variable: {0}")]
    Undefined(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("State key not found")]
    StateNotFound,

    #[error("Live edit error: {0}")]
    LiveEdit(String),
}

pub type Result<T> = std::result::Result<T, Error>;
