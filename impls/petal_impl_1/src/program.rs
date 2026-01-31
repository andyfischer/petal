//! Program representation - Terms and Programs

use slotmap::new_key_type;
use smallvec::SmallVec;

use crate::source_map::SourceMap;

new_key_type! {
    /// Key for accessing programs in the Env
    pub struct ProgramKey;
}

/// Local term identifier - unique within a Program
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct TermId(pub u32);

/// Function identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

/// Constant identifier - index into constant table
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantId(pub u32);

/// State key for state reconciliation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateKey(pub u64);

/// A single expression/node in the program graph
#[derive(Debug, Clone)]
pub struct Term {
    pub id: TermId,
    /// The operation this term performs
    pub op: TermOp,
    /// Input terms (dataflow edges)
    pub inputs: SmallVec<[TermId; 4]>,
}

/// Term operations
#[derive(Debug, Clone)]
pub enum TermOp {
    /// Load a constant from the constant table
    Constant(ConstantId),

    /// Parse error placeholder
    Error(String),

    /// Variable reference
    Var(String),

    /// Variable assignment
    Assign { name: String },

    /// Let binding
    Let { name: String },

    /// State declaration
    StateDecl { name: String, key: StateKey },

    /// State read
    StateRead { name: String, key: StateKey },

    /// State write
    StateWrite { name: String, key: StateKey },

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,

    // Comparison
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,

    // Logical
    And,
    Or,
    Not,

    // Control flow
    If,          // inputs: [condition, then_branch, else_branch]
    Block,       // inputs: [statements..., result]
    Loop,        // For/while loop
    Return,

    // Functions
    FnDef {
        name: String,
        params: Vec<String>,
        body: TermId,
    },
    Call {
        function: String,
        arg_count: usize,
    },

    // Data structures
    List,        // inputs: elements
    Map,         // inputs: alternating keys and values (as constant strings)
    Index,       // inputs: [collection, index]
    Field { name: String }, // inputs: [object]
    SetField { name: String }, // inputs: [object, value]

    // Iteration
    Range,       // inputs: [start, end]
    ForLoop {
        var_name: String,
        body: TermId,
    },
    WhileLoop {
        body: TermId,
    },

    // Print (built-in)
    Print,
}

/// Constant values stored in the constant table
#[derive(Debug, Clone, PartialEq)]
pub enum ConstantValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

/// Constant table for a program
#[derive(Debug, Clone, Default)]
pub struct ConstantTable {
    values: Vec<ConstantValue>,
}

impl ConstantTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, value: ConstantValue) -> ConstantId {
        let id = ConstantId(self.values.len() as u32);
        self.values.push(value);
        id
    }

    pub fn get(&self, id: ConstantId) -> Option<&ConstantValue> {
        self.values.get(id.0 as usize)
    }
}

/// User-defined function
#[derive(Debug, Clone)]
pub struct UserFunction {
    pub name: String,
    pub params: Vec<String>,
    pub body: TermId,
}

/// A compiled program
#[derive(Debug, Clone)]
pub struct Program {
    pub id: ProgramKey,
    /// All terms in this program
    terms: Vec<Term>,
    /// Entry point term
    entry: TermId,
    /// Constant table
    pub constants: ConstantTable,
    /// Source map for debugging
    pub source_map: SourceMap,
    /// User-defined functions
    pub functions: Vec<UserFunction>,
    /// Whether this program has parse errors
    pub has_errors: bool,
}

impl Program {
    pub fn new(id: ProgramKey) -> Self {
        Self {
            id,
            terms: Vec::new(),
            entry: TermId(0),
            constants: ConstantTable::new(),
            source_map: SourceMap::new(),
            functions: Vec::new(),
            has_errors: false,
        }
    }

    pub fn add_term(&mut self, op: TermOp, inputs: SmallVec<[TermId; 4]>) -> TermId {
        let id = TermId(self.terms.len() as u32);
        self.terms.push(Term { id, op, inputs });
        id
    }

    pub fn set_entry(&mut self, entry: TermId) {
        self.entry = entry;
    }

    pub fn entry(&self) -> TermId {
        self.entry
    }

    pub fn get_term(&self, id: TermId) -> Option<&Term> {
        self.terms.get(id.0 as usize)
    }

    pub fn terms(&self) -> impl Iterator<Item = &Term> {
        self.terms.iter()
    }

    pub fn add_function(&mut self, func: UserFunction) -> FunctionId {
        let id = FunctionId(self.functions.len() as u32);
        self.functions.push(func);
        id
    }

    pub fn get_function(&self, id: FunctionId) -> Option<&UserFunction> {
        self.functions.get(id.0 as usize)
    }

    pub fn find_function(&self, name: &str) -> Option<(FunctionId, &UserFunction)> {
        self.functions
            .iter()
            .enumerate()
            .find(|(_, f)| f.name == name)
            .map(|(i, f)| (FunctionId(i as u32), f))
    }
}
