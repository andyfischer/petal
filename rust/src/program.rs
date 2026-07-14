//! Program - A block of code represented as a collection of terms and blocks.
//!
//! See docs/Architecture.md for the surrounding compiler design.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::ast::Pattern;
use crate::constant_table::{ConstantId, ConstantTable, ConstantValue};
use crate::ir_serialize::{deserialize_termid_map, serialize_termid_map};
use crate::source_map::SourceMap;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique identifier for a program within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramId(pub u32);

/// Unique identifier for a term within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TermId(pub u32);

/// Unique identifier for a block within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockId(pub u32);

/// Global term identifier - unique within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GlobalTermId {
    pub program: ProgramId,
    pub term: TermId,
}

/// Register index within a Frame's register file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegisterIndex(pub u16);

/// Unique key for persistent state values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StateKey(pub u64);

/// Identifier for a function definition within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionId(pub u32);

/// Identifier for a runtime closure instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClosureId(pub u32);

/// Identifier for a runtime overload set (multi-arity function dispatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OverloadSetId(pub u32);

/// Entry in a map-with-spread allocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MapSpreadEntry {
    /// Spread all fields from the input at the given index
    Spread(usize),
    /// Set a named field from the input at the given index
    Named(ConstantId, usize),
}

// ---------------------------------------------------------------------------
// TermOp
// ---------------------------------------------------------------------------

/// The operation a term performs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TermOp {
    // --- Core ---
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
    /// Short-circuit coalesce `??`: inputs=[left], child_blocks=[rhs_block].
    /// Yields the RHS when the left is absent (`Nil` or `Pending`), else the left.
    Coalesce,

    // String
    Concat,

    // Binding & identity
    /// Variable reference / identity copy: inputs=[source_term]
    Copy,
    /// Pure-dataflow join point for values rebound inside a child block
    /// (conditional branches, loop bodies). Sits in the parent block *before*
    /// its associated control-flow term (`Branch`, `Match`, `ForLoop`,
    /// `WhileLoop`). On exec it initializes its register from `inputs[0]` —
    /// the pre-control-flow value of the name being joined. Child frames
    /// that rebind the name overwrite the phi's register on pop via
    /// `Block.phi_outs`; branches that don't rebind leave the init value in
    /// place. For loops, each iteration's pop updates the register, and
    /// subsequent iterations read the updated value.
    Phi,

    // Control flow
    /// if/else: inputs=[cond], child_blocks=[then_block, else_block]
    Branch,
    /// for-in loop: inputs=[iterable], child_blocks=[body_block]
    ForLoop,
    /// Numeric for-loop over an integer range (`for i in range(a, b)`):
    /// inputs=[start, end] (both Int-producing terms), child_blocks=[body_block].
    /// Iterates the half-open range [start, end) with no list allocation.
    /// For single-arg `range(n)` the compiler supplies a constant 0 as start.
    NumericForLoop,
    /// while loop: child_blocks=[cond_block, body_block]
    WhileLoop,
    Break,
    Continue,
    /// Return from function: inputs=[value] or empty for bare return
    Return,

    // Functions
    /// Create a closure: inputs=[captured values]
    MakeClosure(FunctionId),
    /// Create an overload set from multiple closures: inputs=[closure0, closure1, ...]
    /// Each closure handles a different arity.
    MakeOverloadSet,
    /// Dynamic call: inputs=[callable, arg0, arg1, ...]
    Call,
    /// Method call: inputs=[object, arg0, arg1, ...], method name as constant
    /// At runtime: tries record field first, then builtin/scope lookup with obj prepended
    MethodCall(ConstantId),
    /// Static builtin call: inputs=[arg0, arg1, ...], builtin name as constant.
    /// Emitted when a bare, unshadowed builtin (e.g. `print`) is called directly,
    /// replacing the dynamic `Call` through a phantom `Copy` of the builtin.
    BuiltinCall(ConstantId),

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
    AllocMap {
        fields: Vec<ConstantId>,
    },
    /// Allocate a map with spread: entries describe the order of spreads and named fields.
    /// inputs = [spread_source_0, ..., named_value_0, ...]
    /// Each entry is either Spread (index into inputs for the spread source map)
    /// or Named (field name constant + index into inputs for the value).
    AllocMapSpread {
        entries: Vec<MapSpreadEntry>,
    },
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
    AllocElement {
        tag: ConstantId,
        prop_keys: Vec<ConstantId>,
    },

    // Enums
    /// Construct an enum variant: inputs=[field values], variant name as constant
    MakeEnumVariant(ConstantId),

    // Pattern matching
    /// Match expression: inputs=[subject], child_blocks=[arm body blocks]
    /// Arm metadata stored in Program.match_arms
    Match,
}

impl TermOp {
    /// The constant-table ids this op references into `Program.constants`.
    /// Single source of truth for the (previously duplicated) enumeration of
    /// which variants carry constants — used by IR validation to range-check
    /// them.
    pub fn constant_ids(&self) -> Vec<ConstantId> {
        match self {
            TermOp::Constant(c)
            | TermOp::Error(c)
            | TermOp::GetField(c)
            | TermOp::SetField(c)
            | TermOp::MethodCall(c)
            | TermOp::BuiltinCall(c)
            | TermOp::MakeEnumVariant(c) => vec![*c],
            TermOp::AllocMap { fields } => fields.clone(),
            TermOp::AllocElement { tag, prop_keys } => {
                let mut v = vec![*tag];
                v.extend(prop_keys.iter().copied());
                v
            }
            TermOp::AllocMapSpread { entries } => entries
                .iter()
                .filter_map(|e| match e {
                    MapSpreadEntry::Named(c, _) => Some(*c),
                    MapSpreadEntry::Spread(_) => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Term
// ---------------------------------------------------------------------------

/// A single expression/node in the program graph.
#[derive(Serialize, Deserialize)]
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
    /// True if this state term is inside a loop body (for per-iteration state).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_loop: bool,
    /// For a loop control term (`ForLoop`/`NumericForLoop`/`WhileLoop`): collect
    /// each iteration's body result into a list and yield it as the term's
    /// value. Set only when the loop is used in value position (`x = for …`);
    /// a bare statement loop leaves this false so it allocates nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collect: bool,
}

// ---------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------

/// A control flow block within a Program.
#[derive(Serialize, Deserialize)]
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
    /// Phi carry-outs: when this block's frame pops, copy each `src_term`'s
    /// register value to the parent frame at each `dest_term`'s register.
    /// Emitted when a conditional branch rebinds a name that was bound in
    /// an outer scope — see the phi-join discussion in `docs/dev/Architecture.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phi_outs: Vec<PhiOut>,
}

/// One phi-slot copy: read `src_term`'s value and write to `dest_term`'s
/// register in the parent frame when a child frame pops.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhiOut {
    pub src_term: TermId,
    pub dest_term: TermId,
}

// ---------------------------------------------------------------------------
// FunctionDef
// ---------------------------------------------------------------------------

/// Compile-time function metadata.
#[derive(Serialize, Deserialize)]
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

/// Strip the internal `#arity` overload suffix from a function's internal name
/// (e.g. `"foo#2"` → `"foo"`), returning the base source name. Function names
/// carry at most one `#` (source identifiers can't contain it), so splitting on
/// the last one recovers the original source name.
pub fn base_fn_name(name: &str) -> &str {
    match name.rfind('#') {
        Some(pos) => &name[..pos],
        None => name,
    }
}

// ---------------------------------------------------------------------------
// OverloadSet
// ---------------------------------------------------------------------------

/// A set of function closures dispatched by argument count.
/// Created at runtime by MakeOverloadSet terms.
#[derive(Debug, Clone)]
pub struct OverloadEntry {
    pub arity: usize,
    pub closure_id: ClosureId,
}

// ---------------------------------------------------------------------------
// MatchArmMeta
// ---------------------------------------------------------------------------

/// Metadata for a compiled match arm.
#[derive(Serialize, Deserialize)]
pub struct MatchArmMeta {
    pub pattern: Pattern,
    pub guard_block: Option<BlockId>,
    pub body_block: BlockId,
}

// ---------------------------------------------------------------------------
// Program
// ---------------------------------------------------------------------------

/// A compiled program ready for execution.
#[derive(Serialize, Deserialize)]
pub struct Program {
    pub id: ProgramId,
    /// Original source text. Optional for imported IR (see docs/ir-as-target.md).
    #[serde(default)]
    pub source: String,

    // IR data
    pub terms: Vec<Term>,
    pub blocks: Vec<Block>,
    pub root_block: BlockId,
    pub constants: ConstantTable,
    /// Source spans. Optional for imported IR.
    #[serde(default)]
    pub source_map: SourceMap,
    #[serde(default)]
    pub has_errors: bool,
    #[serde(default)]
    pub functions: Vec<FunctionDef>,
    #[serde(
        default,
        serialize_with = "serialize_termid_map",
        deserialize_with = "deserialize_termid_map"
    )]
    pub match_arms: HashMap<TermId, Vec<MatchArmMeta>>,
    /// Index from block to all terms in that block (including phantoms).
    /// Built once at compile time to avoid O(N) scans over all terms.
    #[serde(skip)]
    pub block_terms: HashMap<BlockId, Vec<TermId>>,
}

impl Program {
    pub fn get_term(&self, id: TermId) -> &Term {
        &self.terms[id.0 as usize]
    }

    /// Iterate the program's state-bearing terms as `(state key, optional
    /// variable name)`. The single scan behind both `Env::state_key_names`
    /// (which keeps the named keys) and cross-run state transfer (which keeps
    /// every key) — each caller applies its own filter to this.
    pub fn state_terms(&self) -> impl Iterator<Item = (StateKey, Option<&String>)> {
        self.terms
            .iter()
            .filter_map(|t| t.state_key.map(|k| (k, t.name.as_ref())))
    }

    pub fn get_block(&self, id: BlockId) -> &Block {
        &self.blocks[id.0 as usize]
    }

    /// Resolve a ConstantId that's expected to be a string. Returns None if not a string.
    pub fn get_string_constant(&self, cid: ConstantId) -> Option<&str> {
        match self.constants.get(cid) {
            ConstantValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }
}
