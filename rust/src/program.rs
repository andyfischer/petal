//! Program - A block of code represented as a collection of terms and blocks.
//!
//! See docs/tech_outline/data_structures/Program.md

use std::collections::HashMap;

use serde::Serialize;
use smallvec::SmallVec;

use crate::ast::Pattern;
use crate::constant_table::{ConstantId, ConstantTable, ConstantValue};
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
    /// Index from block to all terms in that block (including phantoms).
    /// Built once at compile time to avoid O(N) scans over all terms.
    #[serde(skip)]
    pub block_terms: HashMap<BlockId, Vec<TermId>>,
}

impl Program {
    pub fn get_term(&self, id: TermId) -> &Term {
        &self.terms[id.0 as usize]
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

    /// Find a term by name (e.g. variable name like "x") or by id string (e.g. "t24").
    pub fn find_term(&self, query: &str) -> Option<TermId> {
        // Try "tN" id format first
        if let Some(id_str) = query.strip_prefix('t') {
            if let Ok(id) = id_str.parse::<u32>() {
                if (id as usize) < self.terms.len() {
                    return Some(TermId(id));
                }
            }
        }
        // Search by name (last match wins — like variable shadowing)
        let mut found = None;
        for term in &self.terms {
            if term.name.as_deref() == Some(query) {
                found = Some(term.id);
            }
        }
        found
    }

    /// Trace provenance: collect all transitive input ancestors of a term.
    /// Returns (ancestor_ids_in_order, edges) where each edge is (from, to)
    /// meaning `from` is an input of `to`.
    pub fn trace_provenance(&self, root_id: TermId) -> (Vec<TermId>, Vec<(TermId, TermId)>) {
        use std::collections::{HashSet, VecDeque};

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut ancestors = Vec::new();
        let mut edges = Vec::new();

        // Seed with the root's direct inputs
        let root_term = self.get_term(root_id);
        for &input_id in &root_term.inputs {
            if visited.insert(input_id) {
                queue.push_back(input_id);
            }
            edges.push((input_id, root_id));
        }

        // BFS backward through inputs
        while let Some(term_id) = queue.pop_front() {
            ancestors.push(term_id);
            let term = self.get_term(term_id);
            for &input_id in &term.inputs {
                edges.push((input_id, term_id));
                if visited.insert(input_id) {
                    queue.push_back(input_id);
                }
            }
        }

        (ancestors, edges)
    }

    /// Forward slice: collect all terms that transitively depend on the given term.
    /// This is the complement of `trace_provenance` (backward slice).
    /// Returns (dependent_ids_in_order, edges) where each edge is (from, to)
    /// meaning `from` is an input of `to`.
    pub fn trace_dependents(&self, root_id: TermId) -> (Vec<TermId>, Vec<(TermId, TermId)>) {
        use std::collections::{HashMap as StdHashMap, HashSet, VecDeque};

        // Build a reverse index: term_id -> list of terms that use it as input
        let mut users: StdHashMap<TermId, Vec<TermId>> = StdHashMap::new();
        for term in &self.terms {
            for &input_id in &term.inputs {
                users.entry(input_id).or_default().push(term.id);
            }
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut dependents = Vec::new();
        let mut edges = Vec::new();

        // Seed with the root's direct users
        if let Some(direct_users) = users.get(&root_id) {
            for &user_id in direct_users {
                if visited.insert(user_id) {
                    queue.push_back(user_id);
                }
                edges.push((root_id, user_id));
            }
        }

        // BFS forward through users
        while let Some(term_id) = queue.pop_front() {
            dependents.push(term_id);
            if let Some(term_users) = users.get(&term_id) {
                for &user_id in term_users {
                    edges.push((term_id, user_id));
                    if visited.insert(user_id) {
                        queue.push_back(user_id);
                    }
                }
            }
        }

        (dependents, edges)
    }

    /// Compute a dataflow slice: the minimal subgraph needed to compute the
    /// given target terms from their transitive inputs. Returns term IDs in
    /// topological order (inputs before outputs).
    pub fn slice(&self, targets: &[TermId]) -> Vec<TermId> {
        use std::collections::HashSet;

        // Collect all ancestors of all targets (backward slice)
        let mut needed: HashSet<TermId> = HashSet::new();
        for &target in targets {
            needed.insert(target);
            let (ancestors, _) = self.trace_provenance(target);
            for id in ancestors {
                needed.insert(id);
            }
        }

        // Return in program order (which is topological for a well-formed IR)
        let mut result: Vec<TermId> = needed.into_iter().collect();
        result.sort_by_key(|id| id.0);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constant_table::ConstantTable;
    use crate::source_map::SourceMap;

    /// Build a minimal program with the given terms for testing.
    fn test_program(terms: Vec<Term>) -> Program {
        let root_block = BlockId(0);
        let blocks = vec![Block {
            id: root_block,
            parent_term_id: None,
            entry: terms.first().map(|t| t.id),
            param_names: vec![],
            register_count: terms.len() as u16,
        }];
        Program {
            id: ProgramId(0),
            source: String::new(),
            terms,
            blocks,
            root_block,
            constants: ConstantTable::new(),
            source_map: SourceMap::new(),
            has_errors: false,
            functions: vec![],
            match_arms: HashMap::new(),
            block_terms: HashMap::new(),
        }
    }

    fn make_term(id: u32, op: TermOp, inputs: Vec<u32>, name: Option<&str>) -> Term {
        Term {
            id: TermId(id),
            op,
            inputs: inputs.into_iter().map(TermId).collect(),
            block_id: BlockId(0),
            block_next: None,
            block_prev: None,
            name: name.map(|s| s.to_string()),
            register: RegisterIndex(id as u16),
            state_key: None,
            child_blocks: SmallVec::new(),
        }
    }

    #[test]
    fn find_term_by_name() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Copy, vec![0], Some("y")),
        ]);
        assert_eq!(prog.find_term("x"), Some(TermId(0)));
        assert_eq!(prog.find_term("y"), Some(TermId(1)));
        assert_eq!(prog.find_term("z"), None);
    }

    #[test]
    fn find_term_by_id_string() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], None),
        ]);
        assert_eq!(prog.find_term("t0"), Some(TermId(0)));
        assert_eq!(prog.find_term("t99"), None);
    }

    #[test]
    fn find_term_last_name_wins() {
        // Like variable shadowing: last definition with same name is found
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Constant(ConstantId(1)), vec![], Some("x")),
        ]);
        assert_eq!(prog.find_term("x"), Some(TermId(1)));
    }

    #[test]
    fn trace_provenance_leaf_has_no_ancestors() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
        ]);
        let (ancestors, edges) = prog.trace_provenance(TermId(0));
        assert!(ancestors.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn trace_provenance_single_input() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
        ]);
        let (ancestors, edges) = prog.trace_provenance(TermId(1));
        assert_eq!(ancestors, vec![TermId(0)]);
        assert_eq!(edges, vec![(TermId(0), TermId(1))]);
    }

    #[test]
    fn trace_provenance_diamond() {
        // c depends on a and b, both depend on const
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], None),
            make_term(1, TermOp::Copy, vec![0], Some("a")),
            make_term(2, TermOp::Copy, vec![0], Some("b")),
            make_term(3, TermOp::Add, vec![1, 2], Some("c")),
        ]);
        let (ancestors, edges) = prog.trace_provenance(TermId(3));
        // BFS order: 1, 2, 0 (1 and 2 are direct inputs, 0 is shared ancestor)
        assert_eq!(ancestors.len(), 3);
        assert!(ancestors.contains(&TermId(1)));
        assert!(ancestors.contains(&TermId(2)));
        assert!(ancestors.contains(&TermId(0)));
        // Should have 4 edges: (1,3), (2,3), (0,1), (0,2)
        assert_eq!(edges.len(), 4);
    }

    #[test]
    fn trace_dependents_leaf_has_no_dependents() {
        // Terminal node with no users
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Copy, vec![0], Some("y")),
        ]);
        let (dependents, edges) = prog.trace_dependents(TermId(1));
        assert!(dependents.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn trace_dependents_single_user() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
        ]);
        let (dependents, edges) = prog.trace_dependents(TermId(0));
        assert_eq!(dependents, vec![TermId(1)]);
        assert_eq!(edges, vec![(TermId(0), TermId(1))]);
    }

    #[test]
    fn trace_dependents_transitive() {
        // a -> b -> c
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Copy, vec![1], Some("c")),
        ]);
        let (dependents, _edges) = prog.trace_dependents(TermId(0));
        assert_eq!(dependents.len(), 2);
        assert!(dependents.contains(&TermId(1)));
        assert!(dependents.contains(&TermId(2)));
    }

    #[test]
    fn trace_dependents_fan_out() {
        // a used by both b and c
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Copy, vec![0], Some("c")),
        ]);
        let (dependents, edges) = prog.trace_dependents(TermId(0));
        assert_eq!(dependents.len(), 2);
        assert!(dependents.contains(&TermId(1)));
        assert!(dependents.contains(&TermId(2)));
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn slice_returns_minimal_subgraph() {
        // a(0) -> b(1), c(2) -> d(3) = b + c, e(4) = unrelated
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Constant(ConstantId(1)), vec![], Some("c")),
            make_term(3, TermOp::Add, vec![1, 2], Some("d")),
            make_term(4, TermOp::Constant(ConstantId(2)), vec![], Some("e")),
        ]);
        let slice = prog.slice(&[TermId(3)]);
        // Should include a, b, c, d but NOT e
        assert!(slice.contains(&TermId(0))); // a
        assert!(slice.contains(&TermId(1))); // b
        assert!(slice.contains(&TermId(2))); // c
        assert!(slice.contains(&TermId(3))); // d
        assert!(!slice.contains(&TermId(4))); // e is unrelated
        assert_eq!(slice.len(), 4);
        // Should be in topological order
        assert_eq!(slice, vec![TermId(0), TermId(1), TermId(2), TermId(3)]);
    }

    #[test]
    fn slice_multiple_targets() {
        // a(0) -> b(1), c(2) -> d(3)
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Constant(ConstantId(1)), vec![], Some("c")),
            make_term(3, TermOp::Copy, vec![2], Some("d")),
        ]);
        // Slice for both b and d should include a, b, c, d
        let slice = prog.slice(&[TermId(1), TermId(3)]);
        assert_eq!(slice.len(), 4);
    }
}
