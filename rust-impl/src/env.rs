//! Env - The foundational data structure for the Petal runtime.
//!
//! Owns all programs and stacks. Most operations require an Env as context.
//! See docs/tech_outline/data_structures/Env.md

use std::collections::HashMap;

use crate::builtins::BuiltinTable;
use crate::compiler::Compiler;
use crate::eval::{Evaluator, RuntimeClosure, StepResult};
use crate::heap::Heap;
use crate::lexer::Lexer;
use crate::parse::Parser;
use crate::program::{Program, ProgramId};
use crate::stack::{Frame, Stack, StackKey, StackStatus};
use crate::value::Value;

pub struct Env {
    programs: HashMap<ProgramId, Program>,
    stacks: HashMap<StackKey, Stack>,
    heap: Heap,
    builtins: BuiltinTable,
    closures: Vec<RuntimeClosure>,
    output: Vec<String>,
    next_program_id: u32,
    next_stack_id: u32,
}

impl Env {
    /// Create a new environment
    pub fn new() -> Self {
        Self {
            programs: HashMap::new(),
            stacks: HashMap::new(),
            heap: Heap::new(),
            builtins: BuiltinTable::new(),
            closures: Vec::new(),
            output: Vec::new(),
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

        let compiler = Compiler::new();
        let program = compiler.compile(&stmts, source.to_string(), id);
        self.programs.insert(id, program);
        Ok(id)
    }

    /// Create a new execution stack for a program
    pub fn create_stack(&mut self, program_id: ProgramId) -> Result<StackKey, String> {
        let program = self
            .programs
            .get(&program_id)
            .ok_or("Program not found")?;

        let key = StackKey(self.next_stack_id);
        self.next_stack_id += 1;

        let mut stack = Stack::new(key, program_id);

        // Push initial frame for the root block
        let root_block = program.get_block(program.root_block);
        let reg_count = root_block.register_count as usize;

        // Pre-populate builtin function values in the first N registers
        let mut registers = vec![Value::Nil; reg_count];
        for i in 0..self.builtins.count() {
            let builtin_id = crate::program::BuiltinId(i as u16);
            if i < registers.len() {
                registers[i] = Value::BuiltinFunction(builtin_id);
            }
        }

        stack.push_frame(Frame {
            block_id: program.root_block,
            current_term: root_block.entry,
            registers,
            return_term: None,
            parent_frame: None,
        });

        self.stacks.insert(key, stack);
        Ok(key)
    }

    /// Run one step of execution
    pub fn step(&mut self, stack_id: StackKey) -> Result<StepResult, String> {
        let stack = self
            .stacks
            .get_mut(&stack_id)
            .ok_or("Stack not found")?;
        let program = self
            .programs
            .get(&stack.program_id)
            .ok_or("Program not found")?;

        let result = Evaluator::step(
            program,
            stack,
            &mut self.heap,
            &mut self.closures,
            &self.builtins,
            &mut self.output,
        );

        Ok(result)
    }

    /// Run a program to completion
    pub fn run(&mut self, stack_id: StackKey) -> Result<Value, String> {
        loop {
            match self.step(stack_id)? {
                StepResult::Continue => continue,
                StepResult::Complete(val) => return Ok(val),
                StepResult::Error(e) => return Err(e),
            }
        }
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
    pub fn reset_stack(&mut self, stack_id: StackKey) -> Result<(), String> {
        let stack = self
            .stacks
            .get_mut(&stack_id)
            .ok_or("Stack not found")?;
        let program = self
            .programs
            .get(&stack.program_id)
            .ok_or("Program not found")?;

        // Keep state, reset frames
        stack.frames.clear();
        stack.status = StackStatus::Ready;
        stack.break_flag = false;

        let root_block = program.get_block(program.root_block);
        let reg_count = root_block.register_count as usize;

        let mut registers = vec![Value::Nil; reg_count];
        for i in 0..self.builtins.count() {
            let builtin_id = crate::program::BuiltinId(i as u16);
            if i < registers.len() {
                registers[i] = Value::BuiltinFunction(builtin_id);
            }
        }

        stack.push_frame(Frame {
            block_id: program.root_block,
            current_term: root_block.entry,
            registers,
            return_term: None,
            parent_frame: None,
        });

        Ok(())
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}
