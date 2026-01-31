use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TermId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateKey(pub u64);

#[derive(Debug, Clone)]
pub struct Term {
    pub id: TermId,
    pub op: TermOp,
    pub inputs: Vec<TermId>,
    pub control_flow_next: Option<TermId>,
    pub state_key: Option<StateKey>,
}

#[derive(Debug, Clone)]
pub enum TermOp {
    // Literals
    Constant(Value),

    // Parse error placeholder
    Error(String),

    // Variables
    LoadVar(String),
    StoreVar(String),

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
    Gt,
    LtEq,
    GtEq,

    // Logical
    And,
    Or,
    Not,

    // Control flow
    Branch {
        condition: TermId,
        then_block: Vec<TermId>,
        else_block: Vec<TermId>,
    },
    ForLoop {
        var_name: String,
        iterable: TermId,
        body: Vec<TermId>,
    },
    WhileLoop {
        condition: TermId,
        body: Vec<TermId>,
    },
    Return,
    Break,
    Continue,

    // State
    StateRead(StateKey),
    StateWrite(StateKey),
    StateInit(StateKey), // Initialize state only if not already set
    StateDeclare {
        state_key: StateKey,
        var_name: String,
    }, // Combined init + read + store

    // Functions
    Call {
        function: TermId,
        args: Vec<TermId>,
    },
    DefineFunction {
        name: String,
        params: Vec<String>,
        body: Vec<TermId>,
    },

    // Data access
    Index {
        target: TermId,
        index: TermId,
    },
    FieldAccess {
        target: TermId,
        field: String,
    },

    // Collections
    MakeList(Vec<TermId>),
    MakeMap(Vec<(String, TermId)>),

    // No-op / placeholder
    Nop,
}

impl Term {
    pub fn new(id: TermId, op: TermOp) -> Self {
        Self {
            id,
            op,
            inputs: Vec::new(),
            control_flow_next: None,
            state_key: None,
        }
    }

    pub fn with_inputs(mut self, inputs: Vec<TermId>) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn with_next(mut self, next: TermId) -> Self {
        self.control_flow_next = Some(next);
        self
    }

    pub fn with_state_key(mut self, key: StateKey) -> Self {
        self.state_key = Some(key);
        self
    }
}
