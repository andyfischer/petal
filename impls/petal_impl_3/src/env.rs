//! Environment - owns programs and stacks

use slotmap::{SlotMap, new_key_type};
use std::collections::HashMap;
use std::path::Path;

use crate::{
    program::{Program, ProgramKey, TermId, StateKey, ConstantId, TermOp, ConstantValue},
    stack::{Stack, StackKey, StepResult},
    value::Value,
    parse::Parser,
    eval::Evaluator,
};

new_key_type! {
    pub struct GlobalTermId;
}

/// The foundational environment, owns all programs and stacks
pub struct Env {
    /// All loaded programs
    programs: SlotMap<ProgramKey, Program>,

    /// All running stacks
    stacks: SlotMap<StackKey, Stack>,

    /// Built-in function registry
    builtins: HashMap<String, BuiltinFn>,

    /// Next state key for state declarations
    next_state_key: u64,

    /// Next function ID
    next_function_id: u64,
}

type BuiltinFn = fn(&[Value]) -> Result<Value, crate::Error>;

impl Env {
    /// Create a new environment
    pub fn new() -> Self {
        Self {
            programs: SlotMap::new(),
            stacks: SlotMap::new(),
            builtins: HashMap::new(),
            next_state_key: 0,
            next_function_id: 0,
        }
    }

    /// Register a built-in function
    pub fn register_builtin<F>(&mut self, name: &str, func: F)
    where
        F: Fn(&[Value]) -> Result<Value, crate::Error> + 'static,
    {
        // Box the closure and convert to function pointer
        let func_ptr = Box::into_raw(Box::new(func));
        unsafe {
            self.builtins.insert(name.to_string(), std::mem::transmute_copy(&*func_ptr));
        }
    }

    /// Get a built-in function
    pub fn get_builtin(&self, name: &str) -> Option<&BuiltinFn> {
        self.builtins.get(name)
    }

    /// Load a program from source
    pub fn load_program(&mut self, source: &str) -> Result<ProgramKey, crate::Error> {
        let mut parser = Parser::new(source);
        let program = parser.parse()?;
        let key = self.programs.insert(program);
        Ok(key)
    }

    /// Get a program
    pub fn get_program(&self, key: ProgramKey) -> Option<&Program> {
        self.programs.get(key)
    }

    /// Get a program mutably
    pub fn get_program_mut(&mut self, key: ProgramKey) -> Option<&mut Program> {
        self.programs.get_mut(key)
    }

    /// Create a new state key
    pub fn new_state_key(&mut self) -> StateKey {
        let key = StateKey(self.next_state_key);
        self.next_state_key += 1;
        key
    }

    /// Create a new function ID
    pub fn new_function_id(&mut self) -> u64 {
        let id = self.next_function_id;
        self.next_function_id += 1;
        id
    }

    /// Create a new execution stack for a program
    pub fn create_stack(&mut self, program_key: ProgramKey) -> Result<StackKey, crate::Error> {
        let program = self.programs.get(program_key)
            .ok_or_else(|| crate::Error::Runtime("Program not found".to_string()))?;

        let stack_key = self.stacks.insert(Stack::new(
            StackKey::default(),
            program_key,
        ));

        Ok(stack_key)
    }

    /// Get a stack
    pub fn get_stack(&self, key: StackKey) -> Option<&Stack> {
        self.stacks.get(key)
    }

    /// Get a stack mutably
    pub fn get_stack_mut(&mut self, key: StackKey) -> Option<&mut Stack> {
        self.stacks.get_mut(key)
    }

    /// Run one step of execution
    pub fn step(&mut self, stack_key: StackKey) -> Result<StepResult, crate::Error> {
        let mut evaluator = Evaluator::new(self);
        evaluator.step(stack_key)
    }

    /// Run until completion or breakpoint
    pub fn run(&mut self, stack_key: StackKey) -> Result<Value, crate::Error> {
        loop {
            match self.step(stack_key)? {
                StepResult::Complete(value) => return Ok(value),
                StepResult::Continue => continue,
                StepResult::Breakpoint(_) => {
                    return Err(crate::Error::Runtime("Breakpoint hit".to_string()))
                }
                StepResult::Error(err) => return Err(err),
            }
        }
    }

    /// Get the current value of a term (for inspection)
    pub fn get_term_value(&self, stack_key: StackKey, term_id: TermId) -> Option<&Value> {
        self.stacks.get(stack_key)
            .and_then(|stack| stack.current_frame())
            .and_then(|frame| frame.registers.get(term_id.0 as usize))
    }

    /// Get provenance: what terms influenced this term?
    pub fn get_provenance(&self, program_key: ProgramKey, term_id: TermId) -> Vec<TermId> {
        // Simplified implementation - just return direct inputs
        // In a full implementation, this would include transitive dependencies
        self.programs.get(program_key)
            .and_then(|prog| prog.terms.get(term_id.0 as usize))
            .map(|term| term.inputs.to_vec())
            .unwrap_or_default()
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_env() {
        let env = Env::new();
        assert_eq!(env.programs.len(), 0);
        assert_eq!(env.stacks.len(), 0);
    }

    #[test]
    fn test_state_key_generation() {
        let mut env = Env::new();
        let key1 = env.new_state_key();
        let key2 = env.new_state_key();
        assert_ne!(key1.0, key2.0);
    }
}
