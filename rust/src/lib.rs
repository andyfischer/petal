// Petal language runtime - Rust implementation
//
// See docs/Architecture.md for the module layout and the term-graph IR design.

pub mod ast;
pub mod backend;
pub mod builtins;
pub mod cli;
pub mod compiler;
pub mod constant_table;
pub mod desugar;
pub mod dot_graph;
pub mod env;
pub mod execution_context;
pub mod extract;
pub mod handle;
pub mod heap;
pub mod ir_display;
pub mod ir_serialize;
pub mod lexer;
pub mod module;
pub mod native_fn;
pub mod parse;
pub mod program;
pub mod rewrite;
pub mod source_map;
pub mod stack;
pub mod stats;
pub mod symbol;
pub mod trace;
pub mod transfer_state;
pub mod trivia;
pub mod value;

pub use handle::{HandleClass, HandleClassId, HandleVal};

#[cfg(feature = "wasm")]
pub mod wasm;
