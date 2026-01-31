//! Program representation - a collection of terms

use slotmap::new_key_type;
use smallvec::SmallVec;

new_key_type! {
    pub struct ProgramKey;
    pub struct TermId;
    pub struct ConstantId;
    pub struct FunctionId;
    pub struct FieldId;
}

pub struct Program {
    pub id: ProgramKey,

    /// All terms in this program, indexed by TermId
    pub terms: Vec<Term>,

    /// Entry point term for control flow
    pub entry: TermId,

    /// Constant value table for literals
    pub constants: ConstantTable,

    /// Whether this program contains parse errors
    pub has_errors: bool,
}

pub struct ConstantTable {
    pub values: Vec<ConstantValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstantValue {
    Int(i64),
    Float(f64),
    String(String),
}

pub struct Term {
    pub id: TermId,

    /// The operation this term performs
    pub op: TermOp,

    /// Input terms (dataflow edges)
    pub inputs: SmallVec<[TermId; 4]>,

    /// Control flow ordering (for effectful terms only)
    pub control_flow_next: Option<TermId>,
    pub control_flow_prev: Option<TermId>,

    /// For state terms: unique identifier for state reconciliation
    pub state_key: Option<StateKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateKey(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub enum TermOp {
    // Constants (reference into Program's constant table)
    Constant(ConstantId),

    // Parse error (message is a constant)
    Error(ConstantId),

    // Variables and bindings
    Variable(String),  // Variable name (for now)
    NoOp,  // No operation

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,

    // Comparison
    Eq,
    Lt,
    Gt,
    Lte,
    Gte,
    Neq,

    // Control flow
    Branch { then_term: TermId, else_term: TermId },
    Jump { target: TermId },
    Return,

    // State
    StateRead,
    StateWrite,

    // Functions
    Call { function: FunctionId },
    CallDirect { function: String }, // For built-in and user functions

    // Data access
    GetField { field: FieldId },
    SetField { field: FieldId },

    // IO and built-ins
    Print,
    PrintLn,

    // Heap allocation
    AllocList,
    AllocMap,
}

impl Program {
    pub fn new(id: ProgramKey) -> Self {
        Self {
            id,
            terms: Vec::new(),
            entry: TermId::default(),
            constants: ConstantTable::new(),
            has_errors: false,
        }
    }

    pub fn add_term(&mut self, op: TermOp) -> TermId {
        let id = TermId::from(self.terms.len() as u32);
        let term = Term {
            id,
            op,
            inputs: SmallVec::new(),
            control_flow_next: None,
            control_flow_prev: None,
            state_key: None,
        };
        self.terms.push(term);
        id
    }

    pub fn get_term(&self, id: TermId) -> Option<&Term> {
        self.terms.get(id.0 as usize)
    }

    pub fn get_term_mut(&mut self, id: TermId) -> Option<&mut Term> {
        self.terms.get_mut(id.0 as usize)
    }

    pub fn add_constant(&mut self, value: ConstantValue) -> ConstantId {
        self.constants.add(value)
    }
}

impl ConstantTable {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
        }
    }

    pub fn add(&mut self, value: ConstantValue) -> ConstantId {
        let id = ConstantId::from(self.values.len() as u32);
        self.values.push(value);
        id
    }

    pub fn get(&self, id: ConstantId) -> Option<&ConstantValue> {
        self.values.get(id.0 as usize)
    }
}

impl Term {
    pub fn new(id: TermId, op: TermOp) -> Self {
        Self {
            id,
            op,
            inputs: SmallVec::new(),
            control_flow_next: None,
            control_flow_prev: None,
            state_key: None,
        }
    }

    pub fn add_input(&mut self, input: TermId) {
        self.inputs.push(input);
    }

    pub fn set_control_flow(&mut self, next: Option<TermId>, prev: Option<TermId>) {
        self.control_flow_next = next;
        self.control_flow_prev = prev;
    }
}

// Default implementations
impl Default for ConstantTable {
    fn default() -> Self {
        Self::new()
    }
}
