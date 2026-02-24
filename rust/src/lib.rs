// Petal language runtime - Rust implementation
//
// Module structure follows docs/tech_outline/Outline.md

pub mod ast;
pub mod builtins;
pub mod cli;
pub mod compiler;
pub mod constant_table;
pub mod env;
pub mod eval;
pub mod heap;
pub mod ir_display;
pub mod ir_serialize;
pub mod lexer;
pub mod parse;
pub mod program;
pub mod source_map;
pub mod stack;
pub mod value;
