//! Program - A block of code represented as a collection of terms and blocks.
//!
//! See docs/tech_outline/data_structures/Program.md

use std::collections::HashMap;

use serde::Serialize;
use smallvec::SmallVec;

use crate::ast::Pattern;
use crate::constant_table::{ConstantId, ConstantTable};
use crate::ir_serialize::serialize_termid_map;
use crate::source_map::SourceMap;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique identifier for a program within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ProgramId(pub u32);

/// Unique identifier for a term within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TermId(pub u32);

/// Unique identifier for a block within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct BlockId(pub u32);

/// Global term identifier - unique within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct GlobalTermId {
    pub program: ProgramId,
    pub term: TermId,
}

/// Register index within a Frame's register file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct RegisterIndex(pub u16);

/// Unique key for persistent state values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct StateKey(pub u64);

/// Identifier for a function definition within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FunctionId(pub u32);

/// Identifier for a runtime closure instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ClosureId(pub u32);

// ---------------------------------------------------------------------------
// TermOp
// ---------------------------------------------------------------------------

/// The operation a term performs.
#[derive(Debug, Clone, Serialize)]
pub enum TermOp {
    // --- Core (from docs/tech_outline/data_structures/Term.md) ---

    /// Load a constant from the constant table
    Constant(ConstantId),
    /// A parse error - message stored as a constant
    Error(ConstantId),

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,

    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // Logical
    Not,
    /// Short-circuit AND: inputs=[left], child_blocks=[rhs_block]
    And,
    /// Short-circuit OR: inputs=[left], child_blocks=[rhs_block]
    Or,

    // String
    Concat,

    // Binding & identity
    /// Variable reference / identity copy: inputs=[source_term]
    Copy,
    /// Write to an outer scope's register: inputs=[value], target term specified
    Assign(TermId),

    // Control flow
    /// if/else: inputs=[cond], child_blocks=[then_block, else_block]
    Branch,
    /// for-in loop: inputs=[iterable], child_blocks=[body_block]
    ForLoop,
    /// while loop: child_blocks=[cond_block, body_block]
    WhileLoop,
    Break,
    Continue,
    /// Return from function: inputs=[value] or empty for bare return
    Return,

    // Functions
    /// Create a closure: inputs=[captured values]
    MakeClosure(FunctionId),
    /// Dynamic call: inputs=[callable, arg0, arg1, ...]
    Call,
    /// Method call: inputs=[object, arg0, arg1, ...], method name as constant
    /// At runtime: tries record field first, then builtin/scope lookup with obj prepended
    MethodCall(ConstantId),

    // State
    /// Initialize state if not yet set: inputs=[init_value], state_key set
    StateInit,
    /// Read persistent state: state_key set
    StateRead,
    /// Write persistent state: inputs=[value], state_key set
    StateWrite,

    // Data structures
    /// Allocate a list: inputs=[elem0, elem1, ...]
    AllocList,
    /// Allocate a map/record: inputs=[val0, val1, ...], field names stored here
    AllocMap { fields: Vec<ConstantId> },
    /// Read a field: inputs=[object], field name as constant
    GetField(ConstantId),
    /// Write a field: inputs=[object, value]
    SetField(ConstantId),
    /// Read by index: inputs=[object, index]
    GetIndex,
    /// Write by index: inputs=[object, index, value]
    SetIndex,

    // Elements (JSX-like)
    /// Allocate an element: inputs=[prop_val0, ..., child0, ...]
    /// prop_keys.len() determines where prop values end and children begin
    AllocElement { tag: ConstantId, prop_keys: Vec<ConstantId> },

    // Enums
    /// Construct an enum variant: inputs=[field values], variant name as constant
    MakeEnumVariant(ConstantId),

    // Pattern matching
    /// Match expression: inputs=[subject], child_blocks=[arm body blocks]
    /// Arm metadata stored in Program.match_arms
    Match,
}

// ---------------------------------------------------------------------------
// Term
// ---------------------------------------------------------------------------

/// A single expression/node in the program graph.
#[derive(Serialize)]
pub struct Term {
    pub id: TermId,
    pub op: TermOp,
    /// Input terms (dataflow edges)
    pub inputs: SmallVec<[TermId; 4]>,
    /// The block this term belongs to
    pub block_id: BlockId,
    /// Linked list ordering within the block
    pub block_next: Option<TermId>,
    pub block_prev: Option<TermId>,
    /// Optional name for binding terms (variable declarations)
    pub name: Option<String>,
    /// Register assignment for evaluation
    pub register: RegisterIndex,
    /// For state terms: unique identifier for state reconciliation
    pub state_key: Option<StateKey>,
    /// Child blocks for control flow terms (Branch, ForLoop, WhileLoop, Match, And, Or)
    pub child_blocks: SmallVec<[BlockId; 2]>,
}

// ---------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------

/// A control flow block within a Program.
#[derive(Serialize)]
pub struct Block {
    pub id: BlockId,
    /// The term that creates this block's scope. None for the root block.
    pub parent_term_id: Option<TermId>,
    /// Entry point for this block's term list. None for empty blocks.
    pub entry: Option<TermId>,
    /// Parameter names for function body blocks and for-loop bodies
    pub param_names: Vec<String>,
    /// Total registers needed for this block's frame
    pub register_count: u16,
}

// ---------------------------------------------------------------------------
// FunctionDef
// ---------------------------------------------------------------------------

/// Compile-time function metadata.
#[derive(Serialize)]
pub struct FunctionDef {
    pub id: FunctionId,
    pub name: Option<String>,
    pub params: Vec<String>,
    pub body_block: BlockId,
    pub capture_names: Vec<String>,
    /// Which body registers get capture values (indexed same as captures)
    pub capture_registers: Vec<RegisterIndex>,
    /// Which body register gets the self-reference for recursion
    pub self_ref_register: Option<RegisterIndex>,
    pub register_count: u16,
}

// ---------------------------------------------------------------------------
// MatchArmMeta
// ---------------------------------------------------------------------------

/// Metadata for a compiled match arm.
#[derive(Serialize)]
pub struct MatchArmMeta {
    pub pattern: Pattern,
    pub guard_block: Option<BlockId>,
    pub body_block: BlockId,
}

// ---------------------------------------------------------------------------
// Program
// ---------------------------------------------------------------------------

/// A compiled program ready for execution.
#[derive(Serialize)]
pub struct Program {
    pub id: ProgramId,
    pub source: String,

    // IR data
    pub terms: Vec<Term>,
    pub blocks: Vec<Block>,
    pub root_block: BlockId,
    pub constants: ConstantTable,
    pub source_map: SourceMap,
    pub has_errors: bool,
    pub functions: Vec<FunctionDef>,
    #[serde(serialize_with = "serialize_termid_map")]
    pub match_arms: HashMap<TermId, Vec<MatchArmMeta>>,
}

impl Program {
    pub fn get_term(&self, id: TermId) -> &Term {
        &self.terms[id.0 as usize]
    }

    pub fn get_block(&self, id: BlockId) -> &Block {
        &self.blocks[id.0 as usize]
    }
}
