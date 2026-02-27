//! Env - The foundational data structure for the Petal runtime.
//!
//! Owns all programs and stacks. Most operations require an Env as context.
//! See docs/tech_outline/data_structures/Env.md

use std::collections::HashMap;

use crate::compiler::Compiler;
use crate::eval::{Evaluator, RuntimeClosure, StepResult};
use crate::heap::Heap;
use crate::lexer::Lexer;
use crate::native_fn::{NativeFn, NativeFnId, NativeFnTable};
use crate::parse::Parser;
use crate::program::{Program, ProgramId};
use crate::stack::{Frame, Stack, StackKey, StackStatus};
use crate::value::Value;

/// Result of a hot-reload operation.
pub struct HotReloadResult {
    /// Number of state values preserved across the reload.
    pub state_preserved: usize,
    /// Number of state values dropped (no matching key in new program).
    pub state_dropped: usize,
}

pub struct Env {
    programs: HashMap<ProgramId, Program>,
    stacks: HashMap<StackKey, Stack>,
    heap: Heap,
    native_fns: NativeFnTable,
    closures: Vec<RuntimeClosure>,
    output: Vec<String>,
    next_program_id: u32,
    next_stack_id: u32,
}

impl Env {
    /// Create a new environment
    pub fn new() -> Self {
        let mut native_fns = NativeFnTable::new();
        crate::builtins::register_builtins(&mut native_fns);
        Self {
            programs: HashMap::new(),
            stacks: HashMap::new(),
            heap: Heap::new(),
            native_fns,
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
        let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
        let stmts = parser.parse_program()?;

        let id = ProgramId(self.next_program_id);
        self.next_program_id += 1;

        let compiler = Compiler::new();
        let program = compiler.compile(&stmts, source.to_string(), id, &self.native_fns);
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
        Self::push_root_frame(&self.native_fns,&mut stack, program);

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
            &self.native_fns,
            &mut self.output,
        );

        Ok(result)
    }

    /// Run a program to completion
    pub fn run(&mut self, stack_id: StackKey) -> Result<Value, String> {
        loop {
            match self.step(stack_id)? {
                StepResult::Continue => {
                    if self.heap.should_collect() {
                        self.collect_garbage();
                    }
                }
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

        // Keep state, reset frames and any in-progress loop tracking
        stack.frames.clear();
        stack.status = StackStatus::Ready;
        stack.break_flag = false;

        Self::push_root_frame(&self.native_fns,stack, program);

        Ok(())
    }

    /// Hot-reload: recompile source and replace the program for a running stack.
    /// State values with matching StateKeys are preserved across the reload.
    /// Returns the count of state values preserved.
    pub fn hot_reload(
        &mut self,
        stack_id: StackKey,
        new_source: &str,
    ) -> Result<HotReloadResult, String> {
        let stack = self
            .stacks
            .get(&stack_id)
            .ok_or("Stack not found")?;
        let old_program_id = stack.program_id;

        // Compile new program
        let mut lexer = Lexer::new(new_source);
        lexer.tokenize()?;
        let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
        let stmts = parser.parse_program()?;

        let compiler = Compiler::new();
        let new_program = compiler.compile(&stmts, new_source.to_string(), old_program_id, &self.native_fns);

        // Collect state keys from the new program to know which state to keep
        let new_state_keys: std::collections::HashSet<_> = new_program.terms.iter()
            .filter_map(|t| t.state_key)
            .collect();

        // Determine which old state values will be preserved
        let stack = self.stacks.get(&stack_id).unwrap();
        let preserved: usize = stack.state.keys()
            .filter(|k| new_state_keys.contains(k))
            .count();
        let dropped: usize = stack.state.len() - preserved;

        // Replace program
        self.programs.insert(old_program_id, new_program);

        // Clear closures (they reference the old program's function defs)
        self.closures.clear();

        // Reset stack: keep state but restart execution with new program
        {
            let stack = self.stacks.get_mut(&stack_id).unwrap();
            // Remove state keys that no longer exist in the new program
            stack.state.retain(|k, _| new_state_keys.contains(k));
            stack.frames.clear();
            stack.status = StackStatus::Ready;
            stack.break_flag = false;
            stack.continue_flag = false;
            stack.last_pop_result = None;
        }

        let program = self.programs.get(&old_program_id).unwrap();
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        Self::push_root_frame(&self.native_fns,stack, program);

        Ok(HotReloadResult {
            state_preserved: preserved,
            state_dropped: dropped,
        })
    }

    /// Register a native function that can be called from Petal code.
    /// Must be called before `load_program` so the compiler knows about it.
    pub fn register_native(&mut self, name: &str, func: NativeFn) -> NativeFnId {
        self.native_fns.register(name, func)
    }

    /// Build and push the initial root frame for a program, with native function
    /// values pre-populated in registers.
    fn push_root_frame(
        native_fns: &NativeFnTable,
        stack: &mut Stack,
        program: &Program,
    ) {
        let root_block = program.get_block(program.root_block);
        let mut frame = Frame::new(
            program.root_block, root_block.entry,
            root_block.register_count as usize, None, None,
        );
        for i in 0..native_fns.count() {
            if i < frame.registers.len() {
                frame.registers[i] = Value::NativeFunction(NativeFnId(i as u32));
            }
        }
        stack.push_frame(frame);
    }

    /// Run a mark-and-sweep garbage collection cycle.
    /// Marks all values reachable from roots (stack registers, state, closures,
    /// loop state), then sweeps unmarked heap objects.
    fn collect_garbage(&mut self) {
        // Mark phase: trace all roots

        // 1. Stack frame registers and state
        for stack in self.stacks.values() {
            for frame in &stack.frames {
                for val in &frame.registers {
                    self.heap.mark_value(*val);
                }
                // Loop state elements (ForLoop stores Vec<Value>)
                for loop_state in frame.loop_states.values() {
                    if let crate::stack::LoopState::For { elements, .. } = loop_state {
                        for val in elements {
                            self.heap.mark_value(*val);
                        }
                    }
                }
            }
            // Persistent state values
            for val in stack.state.values() {
                self.heap.mark_value(*val);
            }
            // Last pop result (used by synchronous closure calls)
            if let Some(val) = stack.last_pop_result {
                self.heap.mark_value(val);
            }
        }

        // 2. Closure captures
        for closure in &self.closures {
            for val in &closure.captures {
                self.heap.mark_value(*val);
            }
        }

        // 3. Output buffer (contains strings, but they're Rust Strings not heap-allocated)
        // — no heap values to mark

        // Sweep phase
        self.heap.sweep();
    }

    /// Get the output buffer contents and clear it.
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.output)
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_reload_preserves_state() {
        let mut env = Env::new();

        // Run initial program that sets state via StateWrite
        let source_v1 = r#"
state counter = 0
counter += 5
print(counter)
"#;
        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let output_v1 = env.take_output();
        assert_eq!(output_v1, vec!["5"]);

        // Hot-reload with new source that reads the same state
        let source_v2 = r#"
state counter = 0
counter += 10
print(counter)
"#;
        let result = env.hot_reload(sid, source_v2).unwrap();
        assert_eq!(result.state_preserved, 1);
        assert_eq!(result.state_dropped, 0);

        // Run the reloaded program: counter=5 (preserved), +=10 -> 15
        env.run(sid).unwrap();
        let output_v2 = env.take_output();
        assert_eq!(output_v2, vec!["15"]);
    }

    #[test]
    fn hot_reload_drops_removed_state() {
        let mut env = Env::new();

        // Run with two state variables
        let source_v1 = r#"
state a = 1
state b = 2
print(a + b)
"#;
        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let output = env.take_output();
        assert_eq!(output, vec!["3"]);

        // Reload with only one state variable
        let source_v2 = r#"
state a = 1
print(a)
"#;
        let result = env.hot_reload(sid, source_v2).unwrap();
        assert_eq!(result.state_preserved, 1); // 'a' preserved
        assert_eq!(result.state_dropped, 1);   // 'b' dropped

        env.run(sid).unwrap();
        let output = env.take_output();
        // a was 1 (from init, not modified), state init skips, prints 1
        assert_eq!(output, vec!["1"]);
    }

    #[test]
    fn hot_reload_preserves_state_after_reordering() {
        let mut env = Env::new();

        // Run with a=1, b=2, modify both
        let source_v1 = r#"
state a = 0
state b = 0
a += 10
b += 20
print(a, b)
"#;
        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let output = env.take_output();
        assert_eq!(output, vec!["10 20"]);

        // Reload with state declarations in reversed order
        let source_v2 = r#"
state b = 0
state a = 0
print(a, b)
"#;
        let result = env.hot_reload(sid, source_v2).unwrap();
        assert_eq!(result.state_preserved, 2); // both preserved
        assert_eq!(result.state_dropped, 0);

        env.run(sid).unwrap();
        let output = env.take_output();
        // Both values preserved despite reordering
        assert_eq!(output, vec!["10 20"]);
    }

    #[test]
    fn hot_reload_fresh_state_gets_initialized() {
        let mut env = Env::new();

        // Run with one state
        let source_v1 = r#"
state x = 10
print(x)
"#;
        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        env.take_output(); // discard

        // Reload adding a new state variable
        let source_v2 = r#"
state x = 10
state y = 20
print(x + y)
"#;
        let result = env.hot_reload(sid, source_v2).unwrap();
        assert_eq!(result.state_preserved, 1); // 'x' preserved

        env.run(sid).unwrap();
        let output = env.take_output();
        // x=10 (preserved), y=20 (newly initialized), sum=30
        assert_eq!(output, vec!["30"]);
    }
}
