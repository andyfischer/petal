// Petal language runtime - Rust implementation
//
// See docs/Architecture.md for the module layout and the term-graph IR design.

pub mod ast;
pub mod builtins;
pub mod cli;
pub mod compiler;
pub mod constant_table;
pub mod env;
pub mod eval;
pub mod extract;
pub mod hot_reload;
pub mod heap;
pub mod ir_display;
pub mod ir_serialize;
pub mod lexer;
pub mod native_fn;
pub mod parse;
pub mod program;
pub mod rewrite;
pub mod source_map;
pub mod stack;
pub mod trace;
pub mod value;

#[cfg(feature = "wasm")]
pub mod wasm;
