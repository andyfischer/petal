//! Env - The foundational data structure for the Petal runtime.
//!
//! Owns all programs and stacks. Most operations require an Env as context.
//! See docs/Architecture.md for the surrounding runtime design.

use std::collections::HashMap;

use crate::compiler::Compiler;
use crate::eval::{Evaluator, RuntimeClosure, StepResult};
use crate::heap::Heap;
use crate::lexer::Lexer;
use crate::native_fn::{NativeFn, NativeFnId, NativeFnTable};
use crate::parse::Parser;
use crate::program::{Program, ProgramId, OverloadEntry, StateKey};
use crate::stack::{Frame, RuntimeStateKey, Stack, StackKey, StackStatus};
use crate::trace::TraceBuffer;
use crate::value::Value;

pub struct Env {
    programs: HashMap<ProgramId, Program>,
    stacks: HashMap<StackKey, Stack>,
    heap: Heap,
    native_fns: NativeFnTable,
    closures: Vec<RuntimeClosure>,
    overload_sets: Vec<Vec<OverloadEntry>>,
    output: Vec<String>,
    trace: TraceBuffer,
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
            overload_sets: Vec::new(),
            output: Vec::new(),
            trace: TraceBuffer::new(),
            next_program_id: 1,
            next_stack_id: 1,
        }
    }

    /// Compile source code into a Program without loading it.
    /// Use this to prepare a program for `hot_reload`.
    pub fn compile_program(
        &self,
        program_id: ProgramId,
        source: &str,
    ) -> Result<Program, String> {
        let mut lexer = Lexer::new(source);
        lexer.tokenize()?;
        let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
        let stmts = parser.parse_program()?;
        let compiler = Compiler::new();
        Ok(compiler.compile(&stmts, source.to_string(), program_id, &self.native_fns))
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
            &mut self.overload_sets,
            &self.native_fns,
            &mut self.output,
            &mut self.trace,
        );

        Ok(result)
    }

    /// Access the shared trace buffer (for recording/queries).
    pub fn trace(&self) -> &TraceBuffer {
        &self.trace
    }

    /// Mutable access to the trace buffer (to enable/clear/configure).
    pub fn trace_mut(&mut self) -> &mut TraceBuffer {
        &mut self.trace
    }

    /// Run a program to completion. Tracks which `RuntimeStateKey`s are
    /// touched and sweeps untouched entries from the persistent state map
    /// when the program completes — so per-iteration state for a removed
    /// list item, or a top-level state declaration deleted on hot reload,
    /// is reclaimed instead of leaking.
    pub fn run(&mut self, stack_id: StackKey) -> Result<Value, String> {
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.start_run_tracking();
        }
        loop {
            match self.step(stack_id)? {
                StepResult::Continue => {
                    if self.heap.should_collect() {
                        self.collect_garbage();
                    }
                }
                StepResult::Complete(val) => {
                    if let Some(stack) = self.stacks.get_mut(&stack_id) {
                        stack.sweep_untouched_state();
                    }
                    return Ok(val);
                }
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

    /// Register a native function that can be called from Petal code.
    /// Must be called before `load_program` so the compiler knows about it.
    pub fn register_native(&mut self, name: &str, func: NativeFn) -> NativeFnId {
        self.native_fns.register(name, func)
    }

    // ── Heap access ──────────────────────────────────────────────

    /// Get a shared reference to the heap.
    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    /// Get a mutable reference to the heap.
    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    // ── State inspection ─────────────────────────────────────────

    /// Get the current value of a single top-level state variable.
    /// For per-iteration state, use `get_all_state` and filter by base key.
    pub fn get_state(&self, stack_id: StackKey, key: StateKey) -> Option<Value> {
        let stack = self.stacks.get(&stack_id)?;
        // Find the first entry with matching base key (top-level state has empty loop_indices)
        let runtime_key = RuntimeStateKey {
            base: key,
            loop_indices: smallvec::SmallVec::new(),
        };
        stack.state.get(&runtime_key).copied()
    }

    /// Get all current state as a reference to the HashMap.
    pub fn get_all_state(&self, stack_id: StackKey) -> Option<&HashMap<RuntimeStateKey, Value>> {
        self.stacks.get(&stack_id).map(|s| &s.state)
    }

    /// Set a top-level state variable's value directly.
    pub fn set_state(&mut self, stack_id: StackKey, key: StateKey, value: Value) {
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            let runtime_key = RuntimeStateKey {
                base: key,
                loop_indices: smallvec::SmallVec::new(),
            };
            stack.state.insert(runtime_key, value);
        }
    }

    // ── State key name resolution ────────────────────────────────

    /// Build a map from StateKey → variable name by scanning program terms.
    /// O(n) over terms, call once or on hot-reload.
    pub fn state_key_names(&self, program_id: ProgramId) -> HashMap<StateKey, String> {
        let mut map = HashMap::new();
        if let Some(program) = self.programs.get(&program_id) {
            for term in &program.terms {
                if let (Some(sk), Some(name)) = (term.state_key, &term.name) {
                    map.entry(sk).or_insert_with(|| name.clone());
                }
            }
        }
        map
    }

    // ── State snapshots ──────────────────────────────────────────

    /// Clone all state values. Cheap since Value is Copy.
    pub fn snapshot_state(&self, stack_id: StackKey) -> Option<HashMap<RuntimeStateKey, Value>> {
        self.stacks.get(&stack_id).map(|s| s.state.clone())
    }

    /// Restore state from a previous snapshot, replacing all current state.
    pub fn restore_state(&mut self, stack_id: StackKey, snapshot: HashMap<RuntimeStateKey, Value>) {
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.state = snapshot;
        }
    }

    // ── Speculative execution ────────────────────────────────────

    /// Run one frame without committing state changes.
    /// Snapshots state → reset_stack → run → restore snapshot.
    /// Side effects (print output) still occur. Heap allocations persist
    /// but get GC'd naturally.
    pub fn run_speculative(&mut self, stack_id: StackKey) -> Result<Value, String> {
        let snapshot = self
            .snapshot_state(stack_id)
            .ok_or("Stack not found")?;
        self.reset_stack(stack_id)?;
        let result = self.run(stack_id);
        self.restore_state(stack_id, snapshot);
        result
    }

    // ── Internal accessors (used by hot_reload module) ─────────

    /// Get a shared reference to a stack.
    pub(crate) fn stack(&self, key: StackKey) -> Option<&Stack> {
        self.stacks.get(&key)
    }

    /// Get a mutable reference to a stack.
    pub(crate) fn stack_mut(&mut self, key: StackKey) -> Option<&mut Stack> {
        self.stacks.get_mut(&key)
    }

    /// Insert or replace a program.
    pub(crate) fn insert_program(&mut self, id: ProgramId, program: Program) {
        self.programs.insert(id, program);
    }

    /// Clear all runtime closures and overload sets.
    pub(crate) fn clear_closures(&mut self) {
        self.closures.clear();
        self.overload_sets.clear();
    }

    /// Push the root frame for a stack's program.
    pub(crate) fn push_root_frame_for(&mut self, stack_id: StackKey) -> Result<(), String> {
        let stack = self.stacks.get(&stack_id).ok_or("Stack not found")?;
        let program = self.programs.get(&stack.program_id).ok_or("Program not found")?;
        let root_block = program.get_block(program.root_block);
        let mut frame = Frame::new(
            program.root_block, root_block.entry,
            root_block.register_count as usize, None, None,
        );
        for i in 0..self.native_fns.count() {
            if i < frame.registers.len() {
                frame.registers[i] = Value::NativeFunction(NativeFnId(i as u32));
            }
        }
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        stack.push_frame(frame);
        Ok(())
    }

    // ── State JSON helpers ──────────────────────────────────────

    /// Serialize all state variables to a JSON map keyed by variable name.
    /// Per-iteration state entries are suffixed with their loop indices.
    pub fn get_state_json(
        &self,
        program_id: ProgramId,
        stack_id: StackKey,
    ) -> serde_json::Map<String, serde_json::Value> {
        let names = self.state_key_names(program_id);
        let heap = &self.heap;
        let mut map = serde_json::Map::new();
        if let Some(state) = self.get_all_state(stack_id) {
            for (key, val) in state {
                let base_name = names
                    .get(&key.base)
                    .cloned()
                    .unwrap_or_else(|| format!("unknown_{}", key.base.0));
                let name = if key.loop_indices.is_empty() {
                    base_name
                } else {
                    let suffix: Vec<String> = key.loop_indices.iter().map(|p| match p {
                        crate::stack::LoopKeyPart::Index(i) => i.to_string(),
                        crate::stack::LoopKeyPart::Explicit(h) => format!("k{}", h),
                    }).collect();
                    format!("{}[{}]", base_name, suffix.join(","))
                };
                map.insert(name, crate::value::value_to_json(val, heap));
            }
        }
        map
    }

    /// Set a top-level state variable by name from a JSON value.
    pub fn set_state_from_json(
        &mut self,
        program_id: ProgramId,
        stack_id: StackKey,
        name: &str,
        json_val: &serde_json::Value,
    ) -> Result<(), String> {
        let names = self.state_key_names(program_id);
        let state_key = names
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(k, _)| *k)
            .ok_or_else(|| format!("No state variable named '{}'", name))?;

        let val = crate::value::json_to_value(json_val, &mut self.heap)?;
        self.set_state(stack_id, state_key, val);
        Ok(())
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
                for (_, loop_state) in &frame.loop_states {
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

