use std::collections::HashMap;
use std::rc::Rc;
use std::cell::RefCell;

pub mod parse;
pub mod eval;
pub mod types;

use types::*;

#[derive(Debug, Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<HashMap<String, Value>>>),
    Function(Function),
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<String>,
    pub body: Rc<Program>,
    pub body_term_id: usize,
    pub is_builtin: bool,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub terms: Vec<Term>,
    pub entry_term: usize,
}

#[derive(Debug, Clone)]
pub struct Term {
    pub id: usize,
    pub op: TermOp,
    pub inputs: Vec<usize>,
}

#[derive(Debug, Clone)]
pub enum TermOp {
    Constant(Value),
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Lt,
    Gt,
    Lte,
    Gte,
    And,
    Or,
    Not,
    Call(String),
    Branch { then_id: usize, else_id: usize },
    Return,
    GetField(String),
    SetField(String),
    ListIndex,
    ListConcat,
    Sequence { terms: Vec<usize> },
    // Variable binding: let var = init_term in body_term
    Let { var: String, init: usize, body: usize },
    // Variable reference
    Var(String),
    // State management: state var = init_term in body_term (with unique id for reconciliation)
    StateDef { var: String, init: usize, body: usize, state_id: u64 },
    // State read
    StateRead(String),
    // State write
    StateWrite { var: String, value: usize },
    // Function definition: fn name(params) { body }
    FunctionDef { name: String, params: Vec<String>, body: usize, next: usize },
    // For loop: for var in iter { body }
    For { var: String, iter: usize, body: usize },
    // While loop: while cond { body }
    While { cond: usize, body: usize },
    // Mutation: var += expr (desugared to var = var + expr)
    Mutate { var: String, op: String, value: usize },
}

pub struct Stack {
    pub registers: Vec<Value>,
    pub state: HashMap<String, Value>,
    pub current_term: usize,
    // Variable bindings at current scope
    pub bindings: HashMap<String, Value>,
    // Program this stack is executing
    pub program_key: ProgramKey,
}

pub struct Env {
    programs: HashMap<ProgramKey, Program>,
    stacks: HashMap<StackKey, Stack>,
    functions: HashMap<String, Function>,
    next_program_key: u32,
    next_stack_key: u32,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct ProgramKey(u32);

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct StackKey(u32);

impl Env {
    pub fn new() -> Self {
        let mut env = Env {
            programs: HashMap::new(),
            stacks: HashMap::new(),
            functions: HashMap::new(),
            next_program_key: 1,
            next_stack_key: 1,
        };

        // Register built-in functions
        env.register_builtins();

        env
    }

    fn register_builtins(&mut self) {
        // Built-ins will be handled in eval
    }

    pub fn load_program(&mut self, source: &str) -> Result<ProgramKey, String> {
        let program = parse::parse(source)?;
        let key = ProgramKey(self.next_program_key);
        self.next_program_key += 1;
        self.programs.insert(key, program);
        Ok(key)
    }

    pub fn create_stack(&mut self, program_key: ProgramKey) -> Result<StackKey, String> {
        if !self.programs.contains_key(&program_key) {
            return Err("Program not found".to_string());
        }

        let stack_key = StackKey(self.next_stack_key);
        self.next_stack_key += 1;

        let stack = Stack {
            registers: vec![],
            state: HashMap::new(),
            current_term: 0,
            bindings: HashMap::new(),
            program_key,
        };

        self.stacks.insert(stack_key, stack);
        Ok(stack_key)
    }

    pub fn run(&mut self, stack_key: StackKey) -> Result<Value, String> {
        eval::eval(self, stack_key)
    }

    pub fn get_program(&self, key: ProgramKey) -> Option<&Program> {
        self.programs.get(&key)
    }

    pub fn get_stack(&mut self, key: StackKey) -> Option<&mut Stack> {
        self.stacks.get_mut(&key)
    }

    pub fn add_function(&mut self, name: String, func: Function) {
        self.functions.insert(name, func);
    }

    pub fn get_function(&self, name: &str) -> Option<&Function> {
        self.functions.get(name)
    }
}

#[derive(Debug)]
pub enum Error {
    ParseError(String),
    RuntimeError(String),
    TypeError(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::ParseError(msg) => write!(f, "Parse error: {}", msg),
            Error::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            Error::TypeError(msg) => write!(f, "Type error: {}", msg),
        }
    }
}

impl Value {
    pub fn to_string(&self) -> String {
        match self {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => {
                if f.fract() == 0.0 {
                    format!("{:.1}", f)
                } else {
                    f.to_string()
                }
            }
            Value::String(s) => s.clone(),
            Value::List(list) => {
                let items: Vec<String> = list
                    .borrow()
                    .iter()
                    .map(|v| v.to_string())
                    .collect();
                format!("[{}]", items.join(", "))
            }
            Value::Map(map) => {
                let items: Vec<String> = map
                    .borrow()
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v.to_string()))
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
            Value::Function(f) => format!("fn {}", f.name),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::String(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn as_int(&self) -> Result<i64, String> {
        match self {
            Value::Int(n) => Ok(*n),
            Value::Float(f) => Ok(*f as i64),
            _ => Err(format!("Cannot convert {:?} to int", self)),
        }
    }

    pub fn as_float(&self) -> Result<f64, String> {
        match self {
            Value::Int(n) => Ok(*n as f64),
            Value::Float(f) => Ok(*f),
            _ => Err(format!("Cannot convert {:?} to float", self)),
        }
    }
}
