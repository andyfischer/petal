pub mod env;
pub mod error;
pub mod eval;
pub mod parse;
pub mod program;
pub mod stack;
pub mod term;
pub mod value;

pub use env::Env;
pub use error::Error;
pub use program::{Program, ProgramKey};
pub use stack::{Stack, StackKey};
pub use term::{Term, TermId, TermOp};
pub use value::Value;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepResult {
    Continue,
    Complete,
    Error,
}
