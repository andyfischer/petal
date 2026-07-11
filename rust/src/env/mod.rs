//! Env - The foundational data structure for the Petal runtime.
//!
//! Owns all programs and stacks. Most operations require an Env as context.
//! See docs/Architecture.md for the surrounding runtime design.

use std::collections::HashMap;

use crate::compiler::Compiler;
use crate::backend::bytecode::BytecodeProgram;
use crate::backend::OptFlags;
use crate::execution_context::{ContextKey, ExecutionContext};
use crate::handle::{HandleClass, HandleClassId, HandleVal};
use crate::heap::Heap;
use crate::module::ModuleRegistry;
use crate::native_fn::{NativeFn, NativeFnId, NativeFnTable};
use crate::program::{Program, ProgramId, StateKey};
use crate::stack::{RuntimeStateKey, Stack, StackKey};
use crate::stats::{AllocStats, DupStats};
use crate::symbol::SymbolTable;
use crate::trace::TraceBuffer;
use crate::value::Value;

mod run;
mod gc;
mod fork;
mod state_json;
mod host_io;

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
    /// Per-run optimization toggles for the bytecode VM.
    opt_flags: OptFlags,
    /// Lazily-lowered bytecode, cached next to each `Program`. Populated on the
    /// first bytecode run of a program; an entry's presence means lowering
    /// succeeded. The stored `OptFlags` is the flag set the lowering was built
    /// with — when the active flags change (e.g. `--no-opt`), the cache is
    /// re-lowered so in-place opcodes match the current optimization gate.
    bytecode: HashMap<ProgramId, (OptFlags, BytecodeProgram)>,
    /// Module resolution: in-memory registrations, search paths, and implicit
    /// imports. See docs/module-system.md.
    modules: ModuleRegistry,
    /// Host-registered foreign-object classes, indexed by `HandleClassId`.
    handle_classes: Vec<HandleClass>,
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
            opt_flags: Self::opt_flags_from_env(),
            bytecode: HashMap::new(),
            modules: ModuleRegistry::default(),
            handle_classes: Vec::new(),
        }
    }

    /// Default opt flags from the `PETAL_OPT` env var: `none`/`0`/`off` disables
    /// all opts, `all`/`1` enables all; anything else uses the compiled default.
    /// Public so `show-bytecode` can mirror exactly what a run would execute.
    pub fn opt_flags_from_env() -> OptFlags {
        match std::env::var("PETAL_OPT").ok().as_deref() {
            Some("none") | Some("0") | Some("off") => OptFlags::none(),
            Some("all") | Some("1") | Some("on") => OptFlags::all(),
            _ => OptFlags::default(),
        }
    }

    /// Set the bytecode VM's optimization flags for subsequent runs.
    pub fn set_opt_flags(&mut self, flags: OptFlags) {
        self.opt_flags = flags;
    }

    /// The active optimization flags.
    pub fn opt_flags(&self) -> OptFlags {
        self.opt_flags
    }

    /// Ensure `pid`'s program is lowered to bytecode and cached. Returns the
    /// lowering error (naming the first unlowered op) if it cannot be lowered.
    fn ensure_bytecode(&mut self, pid: ProgramId) -> Result<(), String> {
        // Serve the cache only if it was lowered with the flags in effect now.
        if self.bytecode.get(&pid).map(|(f, _)| *f) == Some(self.opt_flags) {
            return Ok(());
        }
        let program = self.programs.get(&pid).ok_or("Program not found")?;
        // Escape analysis (M4) is a pure function of the program; honoring its
        // in-place set is gated on the flag, so "opts off" reproduces the
        // clone-and-alloc oracle byte-for-byte.
        let in_place = if self.opt_flags.in_place_mutation {
            crate::backend::bytecode::analyze_escapes(program)
        } else {
            crate::backend::bytecode::InPlaceSet::default()
        };
        let mut bc = crate::backend::bytecode::lower_program_opt(program, &in_place)
            .map_err(|e| format!("bytecode lowering failed: {e}"))?;
        // Route A (M4): straight-line last-use rewriting runs on the lowered
        // code, after route B's opcode selection.
        if self.opt_flags.in_place_straight_line {
            crate::backend::bytecode::apply_last_use(&mut bc, program);
        }
        self.bytecode.insert(pid, (self.opt_flags, bc));
        Ok(())
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

    // ── Module registration & resolution ─────────────────────────

    /// Register an in-memory module: `import name` resolves to `source`
    /// without touching the filesystem. This is how an embedder ships a
    /// Petal-source library (e.g. `env.register_module("ui", include_str!(
    /// "ui.ptl"))`) and how wasm hosts with no filesystem provide modules.
    /// Takes priority over file resolution. Call before `load_program`.
    pub fn register_module(&mut self, name: &str, source: &str) {
        self.modules.register(name, source);
    }

    /// Append a directory to the module search path (searched after the
    /// importing file's own directory). The CLI's `-I <dir>` lands here;
    /// `PETAL_PATH` directories are searched after these.
    pub fn add_module_path(&mut self, dir: std::path::PathBuf) {
        self.modules.add_path(dir);
    }

    /// Declare modules that every loaded program imports implicitly, as if by
    /// a selective import of all their exports — a host prelude with zero
    /// ceremony in user scripts. A script's own bindings still win, and an
    /// explicit `import` of the same module is a no-op on top of this.
    pub fn set_implicit_imports(&mut self, names: &[&str]) {
        self.modules.implicit_imports = names.iter().map(|s| s.to_string()).collect();
    }

    /// Resolve imports and compile: the shared back half of
    /// [`load_program`](Self::load_program) / [`compile_program`](Self::compile_program).
    /// `origin` is the entry source's file path when it has one — the anchor
    /// for resolving imports relative to the importing file.
    fn compile_source(
        &self,
        program_id: ProgramId,
        source: &str,
        origin: Option<&std::path::Path>,
    ) -> Result<Program, String> {
        let modules = crate::module::load_modules(
            source,
            origin,
            &self.modules,
            &self.modules.implicit_imports,
        )?;
        Compiler::new().compile_modules(&modules, program_id, &self.native_fns)
    }

    /// Compile source code into a Program without loading it.
    /// Use this to prepare a program for `transfer_state`.
    pub fn compile_program(
        &self,
        program_id: ProgramId,
        source: &str,
    ) -> Result<Program, String> {
        self.compile_source(program_id, source, None)
    }

    /// [`compile_program`](Self::compile_program) for source that lives at a
    /// filesystem path: imports resolve relative to `origin`'s directory
    /// first. Hosts hot-reloading a script file should use this.
    pub fn compile_program_at(
        &self,
        program_id: ProgramId,
        source: &str,
        origin: &std::path::Path,
    ) -> Result<Program, String> {
        self.compile_source(program_id, source, Some(origin))
    }

    /// Load a program from source code
    pub fn load_program(&mut self, source: &str) -> Result<ProgramId, String> {
        let id = ProgramId(self.next_program_id);
        let program = self.compile_source(id, source, None)?;
        self.next_program_id += 1;
        self.programs.insert(id, program);
        Ok(id)
    }

    /// [`load_program`](Self::load_program) for source read from a file:
    /// imports resolve relative to `origin`'s directory first.
    pub fn load_program_at(
        &mut self,
        source: &str,
        origin: &std::path::Path,
    ) -> Result<ProgramId, String> {
        let id = ProgramId(self.next_program_id);
        let program = self.compile_source(id, source, Some(origin))?;
        self.next_program_id += 1;
        self.programs.insert(id, program);
        Ok(id)
    }

    /// The module manifest of a loaded program: one entry per source file it
    /// was compiled from (entry file first). Hosts use this to answer "does
    /// editing file X invalidate program P?" and to watch every file a
    /// program depends on, not just its entry (see petal-sdl's watcher).
    /// Single-file programs get their one entry with `origin: None` — the
    /// host already knows the entry path it loaded from.
    pub fn module_manifest(&self, program_id: ProgramId) -> Vec<ModuleManifestEntry> {
        let Some(program) = self.programs.get(&program_id) else {
            return Vec::new();
        };
        let hash = |s: &str| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        };
        if program.source_map.files.is_empty() {
            return vec![ModuleManifestEntry {
                name: "<entry>".to_string(),
                origin: None,
                content_hash: hash(&program.source),
            }];
        }
        program
            .source_map
            .files
            .iter()
            .map(|f| ModuleManifestEntry {
                name: f.name.clone(),
                origin: f.origin.clone(),
                content_hash: hash(&f.source),
            })
            .collect()
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
        // Validate the program exists (the stack references it by id).
        self.programs
            .get(&program_id)
            .ok_or("Program not found")?;

        let key = StackKey(self.next_stack_id);
        self.next_stack_id += 1;

        // The bytecode VM pushes its own root frame on the first step of a run
        // (gated by `vm_started`), so a freshly created stack needs no frame.
        let stack = Stack::new(key, program_id, self.default_context);

        self.stacks.insert(key, stack);
        Ok(key)
    }

    /// Access the shared trace buffer (for recording/queries).
    pub fn trace(&self) -> &TraceBuffer {
        &self.trace
    }

    /// Mutable access to the trace buffer (to enable/clear/configure).
    pub fn trace_mut(&mut self) -> &mut TraceBuffer {
        &mut self.trace
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

        // Keep state, reset execution; the VM re-pushes its root frame on the
        // next run (gated by `vm_started`, cleared by `reset_execution`).
        let ck = stack.context;
        stack.reset_execution();

        // A stack reset is the per-frame boundary (a host resets before each
        // frame's run). Clear the per-frame absorption state — the debug log and
        // the always-on `absorbed_count`s — so both describe just the next frame.
        // The cross-frame resource table itself is kept.
        if let Some(ctx) = self.contexts.get_mut(&ck) {
            ctx.reset_frame_absorption();
        }

        Ok(())
    }

    /// Register a native function that can be called from Petal code.
    /// Must be called before `load_program`.
    pub fn register_native(&mut self, name: &str, func: NativeFn) -> NativeFnId {
        self.native_fns.register(name, func)
    }

    /// Register a class of host-owned foreign objects, returning its id.
    /// Unlike `register_native`, this may be called at any time — handle
    /// classes are not referenced by compiled programs.
    pub fn register_handle_class(&mut self, class: HandleClass) -> HandleClassId {
        let id = HandleClassId(self.handle_classes.len() as u16);
        self.handle_classes.push(class);
        id
    }

    /// Mint a `Value::Handle` for a (slot, serial) address in `class`.
    pub fn make_handle(&self, class: HandleClassId, slot: u32, serial: u32) -> Value {
        Value::Handle(HandleVal { class, slot, serial })
    }

    /// The registered handle classes, indexed by `HandleClassId`.
    pub fn handle_classes(&self) -> &[HandleClass] {
        &self.handle_classes
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
            for (sk, name) in program.state_terms() {
                if let Some(name) = name {
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

    // ── Internal accessors (used by transfer_state module) ─────────

    /// Get a shared reference to a stack.
    pub(crate) fn stack(&self, key: StackKey) -> Option<&Stack> {
        self.stacks.get(&key)
    }

    /// Get a mutable reference to a stack.
    pub(crate) fn stack_mut(&mut self, key: StackKey) -> Option<&mut Stack> {
        self.stacks.get_mut(&key)
    }

    /// Insert or replace a program. Replacing under the same id (hot reload /
    /// `transfer_state`) drops the cached bytecode lowering, which is derived
    /// from the program and would otherwise be served stale by `ensure_bytecode`.
    pub(crate) fn insert_program(&mut self, id: ProgramId, program: Program) {
        self.bytecode.remove(&id);
        self.programs.insert(id, program);
    }

    /// Clear all runtime closures and overload sets.
    pub(crate) fn clear_closures(&mut self) {
        let ck = self.default_context;
        let ctx = self.ctx_mut(ck);
        ctx.closures.clear();
        ctx.overload_sets.clear();
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

/// One source file a program was compiled from (see
/// [`Env::module_manifest`]). `origin` is the filesystem path when the module
/// was resolved from disk (`None` for in-memory registrations and inline
/// entry sources); `content_hash` is a stable hash of the source text so
/// hosts can detect real changes without re-reading files.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleManifestEntry {
    pub name: String,
    pub origin: Option<std::path::PathBuf>,
    pub content_hash: u64,
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
