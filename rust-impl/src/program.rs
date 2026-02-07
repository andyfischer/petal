//! Program - A block of code represented as a collection of terms and blocks.
//!
//! See docs/tech_outline/data_structures/Program.md

use crate::ast::Stmt;
use crate::source_map::SourceMap;

/// Unique identifier for a program within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramId(pub u32);

/// Unique identifier for a term within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TermId(pub u32);

/// Unique identifier for a block within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// Global term identifier - unique within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalTermId {
    pub program: ProgramId,
    pub term: TermId,
}

/// A parsed program ready for execution.
pub struct Program {
    pub id: ProgramId,
    pub source: String,
    pub stmts: Vec<Stmt>,
    pub source_map: SourceMap,
    pub has_errors: bool,
}

impl Program {
    pub fn new(id: ProgramId, source: String, stmts: Vec<Stmt>) -> Self {
        Self {
            id,
            source,
            stmts,
            source_map: SourceMap::new(),
            has_errors: false,
        }
    }
}
