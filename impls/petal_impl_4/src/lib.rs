//! Petal Programming Language - Rust Implementation
//!
//! Petal is a dataflow-first programming language with:
//! - First-class state management with the `state` keyword
//! - Live editing support
//! - Program projection and slicing
//! - Automatic differentiation

use std::collections::HashMap;
use slotmap::{SlotMap, new_key_type};

pub mod parser;
pub mod eval;
pub mod value;
pub mod heap;

use parser::{Program, TermId, FunctionId};
use eval::{Stack, StackKey};
pub use value::Value;
pub use heap::{Heap, StringId, ListId, MapId};

/// Type for built-in functions
pub type BuiltinFn = Box<dyn Fn(&mut Heap, Vec<Value>) -> Value>;

new_key_type! {
    pub struct ProgramKey;
}

/// The main environment that owns all programs and stacks
pub struct Env {
    programs: SlotMap<ProgramKey, Program>,
    stacks: SlotMap<StackKey, Stack>,
    heap: Heap,
    builtins: HashMap<String, BuiltinFn>,
    next_function_id: u32,
}

/// Error types for Petal operations
#[derive(Debug, Clone)]
pub enum Error {
    ParseError(String),
    RuntimeError(String),
    UndefinedVariable(String),
    TypeError(String),
    StackOverflow,
    InvalidProgram,
    InvalidStack,
}

/// Result of a single step execution
#[derive(Debug, Clone)]
pub enum StepResult {
    Continue,
    Complete(Value),
    Breakpoint(TermId),
    Error(Error),
}

impl Env {
    /// Create a new environment
    pub fn new() -> Self {
        Self {
            programs: SlotMap::with_key(),
            stacks: SlotMap::with_key(),
            heap: Heap::new(),
            builtins: HashMap::new(),
            next_function_id: 1,
        }
    }

    /// Load a program from source code
    pub fn load_program(&mut self, source: &str) -> Result<ProgramKey, Error> {
        let program = parser::parse(source)?;
        Ok(self.programs.insert(program))
    }

    /// Create a new execution stack for a program
    pub fn create_stack(&mut self, program_id: ProgramKey) -> Result<StackKey, Error> {
        let program = self.programs.get(program_id)
            .ok_or(Error::InvalidProgram)?;
        let stack = Stack::new(program_id, program.entry_point());
        Ok(self.stacks.insert(stack))
    }

    /// Run one step of execution
    pub fn step(&mut self, stack_id: StackKey) -> Result<StepResult, Error> {
        // First, get the program_id from the stack
        let program_id = self.stacks.get(stack_id)
            .ok_or(Error::InvalidStack)?
            .program_id();

        // Now we can safely get both mutable references
        let stack = self.stacks.get_mut(stack_id)
            .ok_or(Error::InvalidStack)?;
        let program = self.programs.get(program_id)
            .ok_or(Error::InvalidProgram)?;

        eval::step(stack, program, &mut self.heap, &self.builtins)
    }

    /// Run until completion or breakpoint
    pub fn run(&mut self, stack_id: StackKey) -> Result<Value, Error> {
        loop {
            match self.step(stack_id)? {
                StepResult::Continue => continue,
                StepResult::Complete(value) => return Ok(value),
                StepResult::Breakpoint(_) => continue,
                StepResult::Error(e) => return Err(e),
            }
        }
    }

    /// Register a built-in function
    pub fn register_builtin<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Heap, Vec<Value>) -> Value + 'static,
    {
        self.builtins.insert(name.to_string(), Box::new(f));
    }

    /// Get a built-in function
    pub fn get_builtin(&self, name: &str) -> Option<&BuiltinFn> {
        self.builtins.get(name)
    }

    /// Allocate a new function ID
    pub fn alloc_function_id(&mut self) -> FunctionId {
        let id = self.next_function_id;
        self.next_function_id += 1;
        FunctionId(id)
    }

    // Heap operations
    pub fn alloc_string(&mut self, s: &str) -> StringId {
        self.heap.alloc_string(s)
    }

    pub fn get_string(&self, id: StringId) -> Option<&str> {
        self.heap.get_string(id)
    }

    pub fn alloc_list(&mut self) -> ListId {
        self.heap.alloc_list()
    }

    pub fn get_list(&self, id: ListId) -> Option<&Vec<Value>> {
        self.heap.get_list(id)
    }

    pub fn push_to_list(&mut self, id: ListId, value: Value) {
        self.heap.push_to_list(id, value);
    }

    pub fn pop_from_list(&mut self, id: ListId) -> Option<Value> {
        self.heap.pop_from_list(id)
    }

    pub fn alloc_map(&mut self) -> MapId {
        self.heap.alloc_map()
    }

    pub fn get_map(&self, id: MapId) -> Option<&HashMap<Value, Value>> {
        self.heap.get_map(id)
    }

    pub fn get_map_mut(&mut self, id: MapId) -> Option<&mut HashMap<Value, Value>> {
        self.heap.get_map_mut(id)
    }

    pub fn set_in_map(&mut self, map_id: MapId, key: Value, value: Value) {
        self.heap.set_in_map(map_id, key, value);
    }

    /// Get a reference to a program
    pub fn get_program(&self, id: ProgramKey) -> Option<&Program> {
        self.programs.get(id)
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export parse function
pub use parser::parse;
