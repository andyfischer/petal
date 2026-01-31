//! Petal Programming Language
//!
//! A dataflow-first language with inline state management, projectional views,
//! and live editing support.

pub mod differentiate;
pub mod env;
pub mod error;
pub mod eval;
pub mod heap;
pub mod live_edit;
pub mod parse;
pub mod program;
pub mod projection;
pub mod provenance;
pub mod source_map;
pub mod stack;
pub mod value;

// Re-export main types
pub use env::{value_to_string, Env};
pub use error::{Error, Result};
pub use program::{Program, ProgramKey, Term, TermId, TermOp};
pub use stack::{Stack, StackKey, StepResult};
pub use value::Value;
