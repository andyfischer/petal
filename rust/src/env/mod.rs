//! Env - The foundational data structure for the Petal runtime.
//!
//! Owns all programs and stacks. Most operations require an Env as context.
//! See docs/Architecture.md for the surrounding runtime design.

use std::collections::HashMap;

use crate::compiler::Compiler;
use crate::backend::{Evaluator, StepResult};
use crate::execution_context::{ContextKey, ExecutionContext};
use crate::heap::Heap;
use crate::lexer::Lexer;
use crate::native_fn::{NativeFn, NativeFnId, NativeFnTable};
use crate::parse::Parser;
use crate::program::{Program, ProgramId, StateKey};
use crate::stack::{Frame, RuntimeStateKey, Stack, StackKey, StackStatus};
use crate::stats::{AllocStats, DupStats};
use crate::symbol::{SymbolId, SymbolTable};
use crate::trace::TraceBuffer;
use crate::value::Value;

pub struct Env {
    programs: HashMap<ProgramId, Program>,
    stacks: HashMap<StackKey, Stack>,
    native_fns: NativeFnTable,
    /// Interned symbol names ↔ ordinals, shared with the embedding host.
    symbols: SymbolTable,
    /// Execution-local state bundles (heap + runtime registries), one per
    /// isolated execution. Each `Stack` links to its context by key. See
    /// `execution_context::ExecutionContext`.
    contexts: HashMap<ContextKey, ExecutionContext>,
    /// The context used by no-stack accessor methods and by newly created
    /// stacks.
    default_context: ContextKey,
    next_context_id: u32,
    trace: TraceBuffer,
    next_program_id: u32,
    next_stack_id: u32,
}

impl Env {
    /// Create a new environment
    pub fn new() -> Self {
        let mut native_fns = NativeFnTable::new();
        crate::builtins::register_builtins(&mut native_fns);
        let default_context = ContextKey(1);
        let mut contexts = HashMap::new();
        contexts.insert(default_context, ExecutionContext::new());
        Self {
            programs: HashMap::new(),
            stacks: HashMap::new(),
            native_fns,
            symbols: SymbolTable::new(),
            contexts,
            default_context,
            next_context_id: 2,
            trace: TraceBuffer::new(),
            next_program_id: 1,
            next_stack_id: 1,
        }
    }

    /// Shared access to one execution context.
    fn ctx(&self, ck: ContextKey) -> &ExecutionContext {
        self.contexts.get(&ck).expect("context exists")
    }

    /// Mutable access to one execution context.
    fn ctx_mut(&mut self, ck: ContextKey) -> &mut ExecutionContext {
        self.contexts.get_mut(&ck).expect("context exists")
    }

    /// Resolve the context a stack is bound to. `None` if the stack is unknown.
    /// The basis of the `*_for(stack)` accessors that reach a fork's own heap /
    /// output / bindings rather than the default context.
    fn ctx_for(&self, stack_id: StackKey) -> Option<ContextKey> {
        self.stacks.get(&stack_id).map(|s| s.context)
    }

    /// Compile source code into a Program without loading it.
    /// Use this to prepare a program for `transfer_state`.
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

    /// Load a program from its JSON IR form (the shape `show-ir --json` emits)
    /// rather than from source. Validates the graph and assigns it a fresh
    /// ProgramId. See `docs/ir-as-target.md`.
    pub fn load_program_ir(&mut self, json: &str) -> Result<ProgramId, String> {
        let mut program = Program::from_json(json)?;
        let id = ProgramId(self.next_program_id);
        self.next_program_id += 1;
        program.id = id;
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

        let mut stack = Stack::new(key, program_id, self.default_context);

        // Push initial frame for the root block
        Self::push_root_frame(&self.native_fns,&mut stack, program);

        self.stacks.insert(key, stack);
        Ok(key)
    }

    /// Run one step of execution
    pub fn step(&mut self, stack_id: StackKey) -> Result<StepResult, String> {
        let ck = self.stacks.get(&stack_id).ok_or("Stack not found")?.context;
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        let program = self
            .programs
            .get(&stack.program_id)
            .ok_or("Program not found")?;
        let ctx = self.contexts.get_mut(&ck).ok_or("Context not found")?;

        let result = Evaluator {
            program,
            stack,
            heap: &mut ctx.heap,
            closures: &mut ctx.closures,
            overload_sets: &mut ctx.overload_sets,
            native_fns: &self.native_fns,
            output: &mut ctx.output,
            trace: &mut self.trace,
            symbols: &mut self.symbols,
            output_buffers: &mut ctx.output_buffers,
            bindings: &mut ctx.bindings,
            counters: &mut ctx.counters,
        }
        .step();

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
        let ck = self.stacks.get(&stack_id).ok_or("Stack not found")?.context;
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.start_run_tracking();
        }
        loop {
            match self.step(stack_id)? {
                StepResult::Continue => {
                    if self.ctx(ck).heap.should_collect() {
                        self.collect_garbage(ck);
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

    /// Run a program for at most `max_steps` evaluation steps.
    ///
    /// The bounded counterpart to [`run`](Self::run), for in-process hosts that
    /// must keep control of the thread — e.g. an editor driving Petal-scripted
    /// panels at ~60fps, where a runaway script (`while true`) would otherwise
    /// hang the UI on the main thread. Returns [`RunOutcome::Done`] with the
    /// program's value if it completes within the budget, or
    /// [`RunOutcome::Yielded`] (carrying the number of steps actually consumed)
    /// if the budget is exhausted first.
    ///
    /// A yielded stack is left runnable: call `run_bounded` again to resume it
    /// from exactly where it stopped (e.g. on the next frame), or give up and
    /// [`reset_stack`](Self::reset_stack) / report an error to the user. Because
    /// resumption re-enters the same eval loop, splitting a run across many
    /// `run_bounded` calls produces the same result as a single [`run`](Self::run).
    ///
    /// Run-state tracking (which drives the untouched-state sweep, see
    /// [`run`](Self::run)) is started once when a fresh or `reset` stack first
    /// enters here, and the sweep happens only on completion — so a resumed run
    /// tracks the same key set a single uninterrupted run would.
    pub fn run_bounded(
        &mut self,
        stack_id: StackKey,
        max_steps: u64,
    ) -> Result<RunOutcome, String> {
        let ck = self.stacks.get(&stack_id).ok_or("Stack not found")?.context;

        // Start run tracking only on a fresh entry, not when resuming a
        // previously-yielded run (which would clear the touched-key set and
        // defeat the sweep). reset_stack/create_stack leave the stack `Ready`.
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            if matches!(stack.status, StackStatus::Ready) {
                stack.start_run_tracking();
                stack.status = StackStatus::Running;
            }
        }

        let mut steps = 0;
        while steps < max_steps {
            steps += 1;
            match self.step(stack_id)? {
                StepResult::Continue => {
                    if self.ctx(ck).heap.should_collect() {
                        self.collect_garbage(ck);
                    }
                }
                StepResult::Complete(val) => {
                    if let Some(stack) = self.stacks.get_mut(&stack_id) {
                        stack.sweep_untouched_state();
                        stack.status = StackStatus::Complete(val);
                    }
                    return Ok(RunOutcome::Done(val));
                }
                StepResult::Error(e) => {
                    if let Some(stack) = self.stacks.get_mut(&stack_id) {
                        stack.status = StackStatus::Error(e.clone());
                    }
                    return Err(e);
                }
            }
        }
        Ok(RunOutcome::Yielded { steps })
    }

    /// Run a program from source directly (convenience method)
    pub fn run_source(&mut self, source: &str) -> Result<Value, String> {
        let pid = self.load_program(source)?;
        let sid = self.create_stack(pid)?;
        self.run(sid)
    }

    /// Call a top-level Petal function by name on an already-run stack,
    /// returning its result. The program must have been `run` at least once so
    /// its top-level functions are defined; the captured function table is
    /// refreshed on every run and dropped on `transfer_state`.
    ///
    /// This is the host-facing alternative to the "re-run the whole program
    /// and capture a side effect in a thread-local" pattern: it invokes one
    /// named function with `args` and hands back the return `Value` directly.
    /// Any heap `Value` passed in `args` must already live on this Env's heap.
    ///
    /// Note on state: a top-level `state` variable referenced inside a function
    /// is captured into its closure by value when the program runs, so a called
    /// function observes that variable as of the last `run` and cannot write it
    /// back into the persistent state map. To feed fresh state into a call, pass
    /// it through `args`, or `run`/`transfer_state` again to recapture.
    pub fn call_function(
        &mut self,
        stack_id: StackKey,
        name: &str,
        args: &[Value],
    ) -> Result<Value, String> {
        let stack = self.stacks.get(&stack_id).ok_or("Stack not found")?;
        let callable = stack.functions.get(name).copied().ok_or_else(|| {
            format!(
                "No top-level function named '{}' (define it and `run` the program before calling)",
                name
            )
        })?;

        let ck = self.stacks.get(&stack_id).ok_or("Stack not found")?.context;
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        let program = self
            .programs
            .get(&stack.program_id)
            .ok_or("Program not found")?;
        let ctx = self.contexts.get_mut(&ck).ok_or("Context not found")?;

        Evaluator {
            program,
            stack,
            heap: &mut ctx.heap,
            closures: &mut ctx.closures,
            overload_sets: &mut ctx.overload_sets,
            native_fns: &self.native_fns,
            output: &mut ctx.output,
            trace: &mut self.trace,
            symbols: &mut self.symbols,
            output_buffers: &mut ctx.output_buffers,
            bindings: &mut ctx.bindings,
            counters: &mut ctx.counters,
        }
        .call_closure_sync(callable, args)
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

        Self::push_root_frame(&self.native_fns, stack, program);

        Ok(())
    }

    /// Register a native function that can be called from Petal code.
    /// Must be called before `load_program`.
    pub fn register_native(&mut self, name: &str, func: NativeFn) -> NativeFnId {
        self.native_fns.register(name, func)
    }

    // ── Heap access ──────────────────────────────────────────────

    pub fn heap(&self) -> &Heap {
        &self.ctx(self.default_context).heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        let ck = self.default_context;
        &mut self.ctx_mut(ck).heap
    }

    /// Heap of the context a specific stack is bound to. For a forked stack this
    /// is the fork's own heap — the one its state `Value` ids resolve against —
    /// not the default context's. Use this (not [`heap`](Self::heap)) to decode
    /// or diff a fork's objects. `None` if the stack is unknown.
    pub fn heap_for(&self, stack_id: StackKey) -> Option<&Heap> {
        self.ctx_for(stack_id).map(|ck| &self.ctx(ck).heap)
    }

    /// Mutable [`heap_for`](Self::heap_for): the heap of a specific stack's
    /// context, e.g. to allocate inputs directly into a fork before running it.
    pub fn heap_for_mut(&mut self, stack_id: StackKey) -> Option<&mut Heap> {
        let ck = self.ctx_for(stack_id)?;
        Some(&mut self.ctx_mut(ck).heap)
    }

    // ── Duplication statistics ───────────────────────────────────

    /// Value-duplication statistics for the default execution context. Counts
    /// copy-on-write duplications and fork copies; all zero in release builds
    /// unless the `dup-stats` feature is enabled. See [`crate::stats`].
    pub fn dup_stats(&self) -> &DupStats {
        self.ctx(self.default_context).dup_stats()
    }

    /// Duplication statistics for the context a specific stack is bound to (its
    /// own fork's heap, for a forked stack). `None` if the stack is unknown.
    pub fn dup_stats_for(&self, stack_id: StackKey) -> Option<&DupStats> {
        self.ctx_for(stack_id).map(|ck| self.ctx(ck).dup_stats())
    }

    /// Heap-allocation statistics (objects created per kind) for the default
    /// execution context. See [`crate::stats`].
    pub fn alloc_stats(&self) -> &AllocStats {
        self.ctx(self.default_context).alloc_stats()
    }

    /// Allocation statistics for the context a specific stack is bound to.
    /// `None` if the stack is unknown.
    pub fn alloc_stats_for(&self, stack_id: StackKey) -> Option<&AllocStats> {
        self.ctx_for(stack_id).map(|ck| self.ctx(ck).alloc_stats())
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

    /// Run one frame without disturbing the source execution at all.
    ///
    /// Implemented on top of `fork_execution`: fork the stack into an isolated
    /// context, run the fork, return its result, then drop the fork. Because the
    /// fork owns its own heap and registries (including a *fresh* output sink),
    /// nothing about the source is touched — not its state, not its heap
    /// objects, and not its print output. (The previous snapshot/restore
    /// implementation left the source stack reset and leaked the speculative
    /// run's print output into the shared buffer; the fork-based version does
    /// neither.) The fork's own side effects are discarded when it is dropped.
    pub fn run_speculative(&mut self, stack_id: StackKey) -> Result<Value, String> {
        let fork = self.fork_execution(stack_id)?;
        self.reset_stack(fork)?;
        let result = self.run(fork);
        self.drop_fork(fork);
        result
    }

    /// Drop a forked execution: remove its stack and, if no other stack still
    /// references it, its (exclusively-owned, non-default) context — releasing
    /// the forked heap and registries.
    ///
    /// A host that holds a fork open for side-by-side comparison calls this to
    /// release it once done (the source stack/context is left untouched). Safe
    /// to call on the default context's stacks too: a stack bound to the default
    /// context is removed but the shared default context is never dropped.
    pub fn drop_fork(&mut self, stack_id: StackKey) {
        if let Some(stack) = self.stacks.remove(&stack_id) {
            let ck = stack.context;
            if ck != self.default_context && !self.stacks.values().any(|s| s.context == ck) {
                self.contexts.remove(&ck);
            }
        }
    }

    /// Fork an execution into a fully isolated side-by-side copy. The new stack
    /// gets its own [`ExecutionContext`] (heap + registries deep-cloned, output
    /// sinks fresh) and a clone of the source stack's frames/state, so the two
    /// share no mutable heap state: the fork can advance freely without
    /// disturbing the source, and vice versa. Pre-fork heap ids resolve to equal
    /// objects in both contexts. This is the public API the host/CLI/WASM will
    /// build speculative side-by-side runs on. See
    /// docs/dev/speculative-execution-plan.md §3.
    pub fn fork_execution(&mut self, src: StackKey) -> Result<StackKey, String> {
        // Read the source's context key (and validate the stack exists).
        let src_ck = self.stacks.get(&src).ok_or("Stack not found")?.context;

        // Fork the source context into a fresh context key.
        let new_ck = ContextKey(self.next_context_id);
        self.next_context_id += 1;
        let forked = self.contexts.get(&src_ck).ok_or("Context not found")?.fork();
        self.contexts.insert(new_ck, forked);

        // Clone the source stack into a fresh stack key, rebinding it to the new
        // context.
        let new_key = StackKey(self.next_stack_id);
        self.next_stack_id += 1;
        let mut new_stack = self.stacks.get(&src).ok_or("Stack not found")?.clone();
        new_stack.id = new_key;
        new_stack.context = new_ck;
        self.stacks.insert(new_key, new_stack);

        Ok(new_key)
    }

    // ── Internal accessors (used by transfer_state module) ─────────

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
        let ck = self.default_context;
        let ctx = self.ctx_mut(ck);
        ctx.closures.clear();
        ctx.overload_sets.clear();
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
        // Resolve the stack's *own* context heap: a fork's state ids index its
        // forked heap, not the default context's.
        let ck = self.ctx_for(stack_id).unwrap_or(self.default_context);
        let heap = &self.ctx(ck).heap;
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

        // Allocate the value into the stack's own context heap so a fork's
        // state stays self-consistent (its ids must index its forked heap).
        let ck = self.ctx_for(stack_id).unwrap_or(self.default_context);
        let val = crate::value::json_to_value(json_val, &mut self.ctx_mut(ck).heap)?;
        self.set_state(stack_id, state_key, val);
        Ok(())
    }

    /// Diff two executions' committed state by *value* (never by heap id — see
    /// hazard 4: ids are not comparable across contexts). Each stack's state is
    /// rendered to JSON against its own context heap, then compared key-by-key;
    /// only differing or one-sided variables are returned. This is the
    /// side-by-side comparison primitive a host uses after running a fork:
    /// `diff_state(pid, source, fork)`. `program_id` supplies the state-key →
    /// name mapping (both stacks should share the same program).
    pub fn diff_state(
        &self,
        program_id: ProgramId,
        source: StackKey,
        fork: StackKey,
    ) -> Vec<StateDiff> {
        let a = self.get_state_json(program_id, source);
        let b = self.get_state_json(program_id, fork);
        let mut names: Vec<&String> = a.keys().chain(b.keys()).collect();
        names.sort_unstable();
        names.dedup();
        names
            .into_iter()
            .filter_map(|name| {
                let av = a.get(name);
                let bv = b.get(name);
                if av == bv {
                    None
                } else {
                    Some(StateDiff {
                        name: name.clone(),
                        source: av.cloned(),
                        fork: bv.cloned(),
                    })
                }
            })
            .collect()
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
    fn collect_garbage(&mut self, ck: ContextKey) {
        // Disjoint borrows: stacks (shared) + the one context (mut). Mark all
        // roots into THAT context's heap, then sweep it.
        let ctx = self.contexts.get_mut(&ck).expect("context exists");
        let heap = &mut ctx.heap;

        // 1. Stack frame registers and state — only stacks bound to this context.
        for stack in self.stacks.values() {
            if stack.context != ck {
                continue;
            }
            for frame in &stack.frames {
                for val in &frame.registers {
                    heap.mark_value(*val);
                }
                // Loop state elements (a for-each loop snapshots a Vec<Value>)
                for (_, loop_state) in &frame.loop_states {
                    if let crate::stack::LoopKind::ForEach { elements } = &loop_state.kind {
                        for val in elements {
                            heap.mark_value(*val);
                        }
                    }
                }
            }
            // Persistent state values
            for val in stack.state.values() {
                heap.mark_value(*val);
            }
            // Last pop result (used by synchronous closure calls)
            if let Some(val) = stack.last_pop_result {
                heap.mark_value(val);
            }
        }

        // 2. Closure captures
        for closure in &ctx.closures {
            for val in &closure.captures {
                heap.mark_value(*val);
            }
        }

        // 3. Print output buffer holds Rust Strings, not heap values — nothing
        //    to mark. The per-symbol output buffers, however, hold heap-backed
        //    Values (e.g. draw-command enum variants with string tags + list
        //    args), so they are GC roots: a frame can trip a collection mid-run
        //    while commands are still buffered.
        for buffer in ctx.output_buffers.values() {
            for val in buffer {
                heap.mark_value(*val);
            }
        }

        // 4. Host→script bindings hold heap-backed Values (e.g. a bound list of
        //    pressed keys), so they are GC roots too. Counters are plain u64s.
        for val in ctx.bindings.values() {
            heap.mark_value(*val);
        }

        // Sweep phase
        heap.sweep();
    }

    /// Get the output buffer contents and clear it.
    pub fn take_output(&mut self) -> Vec<String> {
        let ck = self.default_context;
        self.ctx_mut(ck).take_output()
    }

    /// Drain the print output of a specific stack's context. A fork accumulates
    /// its `print` output in its own (fresh) sink; this is how a host reads it
    /// before [`drop_fork`](Self::drop_fork). Empty `Vec` for an unknown stack.
    pub fn take_output_for(&mut self, stack_id: StackKey) -> Vec<String> {
        self.ctx_for(stack_id)
            .map(|ck| self.ctx_mut(ck).take_output())
            .unwrap_or_default()
    }

    // ── Symbols & buffered output (host side) ────────────────────

    /// Intern a symbol name, returning its stable id. Idempotent — the host and
    /// the script share an id by interning the same name. Use the returned id to
    /// address an output buffer with `take_output_buffer`.
    pub fn intern_symbol(&mut self, name: &str) -> SymbolId {
        self.symbols.intern(name)
    }

    /// Resolve a symbol id back to its name.
    pub fn symbol_name(&self, sym: SymbolId) -> Option<&str> {
        self.symbols.name(sym)
    }

    /// Drain and return everything pushed into the buffer bound to `sym` since
    /// the last drain. The buffer is left empty.
    pub fn take_output_buffer(&mut self, sym: SymbolId) -> Vec<Value> {
        let ck = self.default_context;
        self.ctx_mut(ck).take_output_buffer(sym)
    }

    /// Peek at the buffer bound to `sym` without draining it.
    pub fn output_buffer(&self, sym: SymbolId) -> &[Value] {
        self.ctx(self.default_context).output_buffer(sym)
    }

    /// [`take_output_buffer`](Self::take_output_buffer) for a specific stack's
    /// context. The drained `Value`s reference *that* context's heap — decode
    /// them with [`heap_for`](Self::heap_for), not [`heap`](Self::heap). This is
    /// how a host drains a fork's draw-command (or other) buffer.
    pub fn take_output_buffer_for(&mut self, stack_id: StackKey, sym: SymbolId) -> Vec<Value> {
        self.ctx_for(stack_id)
            .map(|ck| self.ctx_mut(ck).take_output_buffer(sym))
            .unwrap_or_default()
    }

    /// Peek at a specific stack's context buffer without draining it.
    pub fn output_buffer_for(&self, stack_id: StackKey, sym: SymbolId) -> &[Value] {
        self.ctx_for(stack_id)
            .map(|ck| self.ctx(ck).output_buffer(sym))
            .unwrap_or(&[])
    }

    /// Clear the buffer bound to `sym` (e.g. at the top of a frame).
    pub fn clear_output_buffer(&mut self, sym: SymbolId) {
        let ck = self.default_context;
        self.ctx_mut(ck).clear_output_buffer(sym);
    }

    // ── Bindings (host→script uniforms) ──────────────────────────

    /// Bind a `Value` to `sym`, readable by native fns/scripts (`binding`).
    /// Any heap `Value` passed must already live on this Env's heap.
    pub fn set_binding(&mut self, sym: SymbolId, value: Value) {
        let ck = self.default_context;
        self.ctx_mut(ck).set_binding(sym, value);
    }

    /// [`set_binding`](Self::set_binding) for a specific stack's context, e.g.
    /// to feed a fork different host inputs than its source. Any heap `Value`
    /// must already live on that stack's context heap
    /// ([`heap_for_mut`](Self::heap_for_mut)). No-op for an unknown stack.
    pub fn set_binding_for(&mut self, stack_id: StackKey, sym: SymbolId, value: Value) {
        if let Some(ck) = self.ctx_for(stack_id) {
            self.ctx_mut(ck).set_binding(sym, value);
        }
    }

    /// Read the value bound to `sym`, if any.
    pub fn binding(&self, sym: SymbolId) -> Option<Value> {
        self.ctx(self.default_context).binding(sym)
    }

    /// [`binding`](Self::binding) read from a specific stack's context.
    pub fn binding_for(&self, stack_id: StackKey, sym: SymbolId) -> Option<Value> {
        self.ctx_for(stack_id).and_then(|ck| self.ctx(ck).binding(sym))
    }

    /// Remove the binding for `sym`.
    pub fn clear_binding(&mut self, sym: SymbolId) {
        let ck = self.default_context;
        self.ctx_mut(ck).clear_binding(sym);
    }

    // ── Counters (per-run sequence allocation) ───────────────────

    /// Reset the counter for `sym` to `start` (call at frame start so
    /// `next_counter` hands out stable ids across the per-frame re-run model).
    pub fn reset_counter(&mut self, sym: SymbolId, start: u64) {
        let ck = self.default_context;
        self.ctx_mut(ck).reset_counter(sym, start);
    }

    /// Return the current counter value for `sym`, then increment it.
    /// An unset counter starts at 0.
    pub fn next_counter(&mut self, sym: SymbolId) -> u64 {
        let ck = self.default_context;
        self.ctx_mut(ck).next_counter(sym)
    }
}

/// Outcome of a bounded run (see [`Env::run_bounded`]).
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// The program ran to completion within the step budget, producing `Value`.
    Done(Value),
    /// The step budget was exhausted before the program completed. `steps` is
    /// how many steps were consumed (equal to the budget). The stack is left
    /// runnable — call [`Env::run_bounded`] again to resume.
    Yielded { steps: u64 },
}

/// One state variable that differs between two executions (see
/// [`Env::diff_state`]). `source`/`fork` hold the variable's JSON value in each
/// run, or `None` where that run has no such variable.
#[derive(Debug, Clone, PartialEq)]
pub struct StateDiff {
    pub name: String,
    pub source: Option<serde_json::Value>,
    pub fork: Option<serde_json::Value>,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Env {
    /// Summary view: counts rather than full contents, so a host struct that
    /// embeds an `Env` can `#[derive(Debug)]` (needed by `unwrap_err` /
    /// `expect_err` in tests, and for logging). The heap, closures, and trace
    /// buffer are intentionally elided via `finish_non_exhaustive`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let default_ctx = self.contexts.get(&self.default_context);
        f.debug_struct("Env")
            .field("programs", &self.programs.len())
            .field("stacks", &self.stacks.len())
            .field("native_fns", &self.native_fns.count())
            .field("contexts", &self.contexts.len())
            .field("closures", &default_ctx.map(|c| c.closures.len()).unwrap_or(0))
            .field(
                "pending_output_lines",
                &default_ctx.map(|c| c.output.len()).unwrap_or(0),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
