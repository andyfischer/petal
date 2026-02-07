//! Env - The foundational data structure for the Petal runtime.
//!
//! Owns all programs and stacks. Most operations require an Env as context.
//! See docs/tech_outline/data_structures/Env.md

use std::collections::HashMap;

use crate::eval::Interpreter;
use crate::lexer::Lexer;
use crate::parse::Parser;
use crate::program::{Program, ProgramId};
use crate::stack::{Stack, StackKey};
use crate::value::Value;

pub struct Env {
    programs: HashMap<ProgramId, Program>,
    stacks: HashMap<StackKey, Stack>,
    interpreter: Interpreter,
    next_program_id: u32,
    next_stack_id: u32,
}

#[derive(Debug)]
pub enum StepResult {
    Continue,
    Complete(Value),
    Error(String),
}

impl Env {
    /// Create a new environment
    pub fn new() -> Self {
        Self {
            programs: HashMap::new(),
            stacks: HashMap::new(),
            interpreter: Interpreter::new(),
            next_program_id: 1,
            next_stack_id: 1,
        }
    }

    /// Load a program from source code
    pub fn load_program(&mut self, source: &str) -> Result<ProgramId, String> {
        let mut lexer = Lexer::new(source);
        lexer.tokenize()?;
        let mut parser = Parser::new(lexer.tokens);
        let stmts = parser.parse_program()?;

        let id = ProgramId(self.next_program_id);
        self.next_program_id += 1;

        let program = Program::new(id, source.to_string(), stmts);
        self.programs.insert(id, program);
        Ok(id)
    }

    /// Create a new execution stack for a program
    pub fn create_stack(&mut self, program_id: ProgramId) -> Result<StackKey, String> {
        if !self.programs.contains_key(&program_id) {
            return Err("Program not found".to_string());
        }
        let key = StackKey(self.next_stack_id);
        self.next_stack_id += 1;
        let stack = Stack::new(key, program_id);
        self.stacks.insert(key, stack);
        Ok(key)
    }

    /// Run a program to completion
    pub fn run(&mut self, stack_id: StackKey) -> Result<Value, String> {
        let stack = self.stacks.get(&stack_id)
            .ok_or("Stack not found")?;
        let program = self.programs.get(&stack.program_id)
            .ok_or("Program not found")?;
        self.interpreter.run(&program.stmts)
    }

    /// Run a program from source directly (convenience method)
    pub fn run_source(&mut self, source: &str) -> Result<Value, String> {
        let pid = self.load_program(source)?;
        let sid = self.create_stack(pid)?;
        self.run(sid)
    }

    /// Get a reference to a loaded program
    pub fn get_program(&self, id: ProgramId) -> Option<&Program> {
        self.programs.get(&id)
    }

    /// Reset a stack to re-run while keeping state
    pub fn reset_stack(&mut self, _stack_id: StackKey) -> Result<(), String> {
        // State persists in the interpreter's state_store
        Ok(())
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}
