// Petal language runtime - Rust implementation
//
// See docs/Architecture.md for the module layout and the term-graph IR design.

pub mod ast;
pub mod backend;
pub mod builtins;
pub mod classes;
pub mod cli;
pub mod compiler;
pub mod constant_table;
pub mod cst;
pub mod cst_project;
pub mod desugar;
pub mod diagnostic;
pub mod direct_manipulation;
pub mod dot_graph;
pub mod env;
pub mod error;
pub mod execution_context;
pub mod extract;
pub mod goal_based_editing;
pub mod handle;
pub mod heap;
pub mod inspect;
pub mod ir_display;
pub mod ir_serialize;
pub mod ir_validate;
pub mod lexer;
pub mod lint;
pub mod lsp;
pub mod module;
pub mod native_fn;
pub mod observe;
pub mod parse;
pub mod program;
pub mod program_analysis;
pub mod provenance;
pub mod resource_table;
pub mod rewrite;
pub mod source_map;
pub mod stack;
pub mod static_value;
pub mod stats;
pub mod symbol;
pub mod trace;
pub mod transfer_state;
pub mod trivia;
pub mod typecheck;
pub mod types;
pub mod value;

pub use handle::{HandleClass, HandleClassId, HandleVal};

#[cfg(feature = "wasm")]
pub mod wasm;
