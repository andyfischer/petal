//! Stack - Runtime evaluation context.
//!
//! See docs/Architecture.md for the surrounding runtime design.

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use crate::execution_context::ContextKey;
use crate::program::{BlockId, ProgramId, StateKey, TermId};
use crate::value::Value;

/// Part of a compound runtime state key representing one loop nesting level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LoopKeyPart {
    /// Keyed by iteration index (default for loops).
    Index(usize),
    /// Keyed by an explicit hashed value (Phase 2: `state(expr)`).
    Explicit(u64),
}

/// Runtime state key combining the static StateKey with loop iteration context.
/// Top-level state (not in a loop) has an empty `loop_indices`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeStateKey {
    pub base: StateKey,
    pub loop_indices: SmallVec<[LoopKeyPart; 2]>,
}

/// Unique identifier for a stack within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StackKey(pub u32);

/// Per-loop progress stored in the frame that owns the loop term.
///
/// Because this lives on the Frame, it is automatically discarded when the
/// frame is popped (e.g. by an early `return`), which prevents stale state
/// from leaking across function calls.
///
/// Every loop kind tracks the same 0-based `iteration` counter, so
/// per-iteration `state` keying treats them uniformly (see [`LoopKeyPart`]).
/// The kind-specific data needed to produce the next value lives in [`kind`].
#[derive(Clone)]
pub struct LoopState {
    /// 0-based index of the iteration currently executing.
    pub iteration: usize,
    /// What this loop iterates over, and any per-kind progress.
    pub kind: LoopKind,
}

/// The source a loop draws its values from, plus any kind-specific phase.
#[derive(Clone)]
pub enum LoopKind {
    /// `for x in list` — iterates a snapshot of the list's elements; the
    /// current element is `elements[iteration]`.
    ForEach { elements: Vec<Value> },
    /// `for i in range(start, end)` — iterates integers with no list
    /// allocation. Yields `start + iteration` while it stays below `end`.
    Range { start: i64, end: i64 },
    /// `while cond` — alternates between evaluating the condition block and
    /// running the body. `running_body` is false while the condition is being
    /// evaluated and true while the body executes.
    While { running_body: bool },
}

/// Runtime execution state for a program.
#[derive(Clone)]
pub struct Stack {
    pub id: StackKey,
    pub program_id: ProgramId,
    /// The ExecutionContext this stack draws its heap and registries from.
    pub context: ContextKey,
    pub frames: Vec<Frame>,
    pub state: HashMap<RuntimeStateKey, Value>,
    pub status: StackStatus,
    pub break_flag: bool,
    pub continue_flag: bool,
    /// Temporary storage for the result of the last popped frame.
    /// Used by synchronous closure calls (map/filter/reduce) to capture return values.
    pub last_pop_result: Option<Value>,
    /// RuntimeStateKeys touched (read or written) since the last call to
    /// `start_run_tracking`. Used to garbage-collect persistent state entries
    /// whose source-level declaration was not visited this run — for example
    /// per-iteration state for an item that was removed from the iterated
    /// list, or a top-level `state` declaration that was deleted on hot
    /// reload. Cleared at the start of each top-level `run`.
    pub touched_state_keys: HashSet<RuntimeStateKey>,
    /// Top-level named functions (and lambdas bound to a name) captured from
    /// the root block when the program runs. Lets the host invoke a named
    /// Petal function via `Env::call_function` without re-running the whole
    /// program. Refreshed each time the root frame completes; cleared on hot
    /// reload since the underlying closure IDs are invalidated.
    pub functions: HashMap<String, Value>,
    /// Activation records for the bytecode backend (`Backend::Bytecode`). Empty
    /// and unused under the graph backend, which uses `frames`. Stored here (not
    /// on the VM, which is rebuilt per step) so execution state survives across
    /// steps and is reachable as GC roots, exactly like `frames`.
    pub vm_frames: Vec<crate::backend::bytecode::VmFrame>,
    /// Whether the bytecode root frame has been pushed for the current run.
    /// Distinguishes "not started" (push root) from "completed" (`vm_frames`
    /// empty again → done). Reset by [`Stack::reset_execution`].
    pub vm_started: bool,
    /// Recycled VM activation records: popped frames land here (registers,
    /// cursors, and loop context cleared) and calls reuse their allocations
    /// instead of hitting the allocator per call. Cleared frames hold no
    /// values, so this is deliberately *not* a GC root — keep it that way.
    pub vm_frame_pool: Vec<crate::backend::bytecode::VmFrame>,
}

/// A single activation frame on the stack.
#[derive(Clone)]
pub struct Frame {
    pub block_id: BlockId,
    pub current_term: Option<TermId>,
    pub registers: Vec<Value>,
    /// Term in the parent frame to resume at when this frame completes.
    pub return_term: Option<TermId>,
    /// Index into stack.frames of the parent frame (for cross-frame register reads).
    /// None for function call frames (captures are in local registers).
    pub parent_frame: Option<usize>,
    /// True if this frame is the direct body of a loop (for/while body).
    /// When `break_flag` is set, the evaluator immediately pops loop-body
    /// frames so the parent loop term can handle the break.
    pub is_loop_body: bool,
    /// Loop state for any loop terms executing within this frame.
    /// Keyed by TermId of the ForLoop / WhileLoop term.
    /// SmallVec since most frames have 0-1 active loops; linear scan beats hashing.
    pub loop_states: SmallVec<[(TermId, LoopState); 2]>,
    /// Function name for this frame (if it's a function call frame).
    /// Used for stack traces in error messages.
    pub fn_name: Option<String>,
}

#[derive(Debug, Clone)]
pub enum StackStatus {
    Ready,
    Running,
    Complete(Value),
    Error(String),
}

impl Frame {
    /// Create a new frame for a block with default settings.
    pub fn new(
        block_id: BlockId,
        entry: Option<TermId>,
        register_count: usize,
        return_term: Option<TermId>,
        parent_frame: Option<usize>,
    ) -> Self {
        Self {
            block_id,
            current_term: entry,
            registers: vec![Value::Nil; register_count],
            return_term,
            parent_frame,
            is_loop_body: false,
            loop_states: SmallVec::new(),
            fn_name: None,
        }
    }

    /// Set this frame as a loop body frame.
    pub fn as_loop_body(mut self) -> Self {
        self.is_loop_body = true;
        self
    }

    /// Write `value` into register `reg`, growing the register file with
    /// Nil if it isn't large enough yet.
    pub fn set_register(&mut self, reg: usize, value: Value) {
        if reg >= self.registers.len() {
            self.registers.resize(reg + 1, Value::Nil);
        }
        self.registers[reg] = value;
    }

    /// Read register `reg`, returning Nil for never-written registers.
    pub fn get_register(&self, reg: usize) -> Value {
        self.registers.get(reg).copied().unwrap_or(Value::Nil)
    }

    /// Check if a loop state exists for the given term.
    pub fn has_loop_state(&self, term_id: &TermId) -> bool {
        self.loop_states.iter().any(|(id, _)| id == term_id)
    }

    /// Get a reference to a loop state by term id.
    pub fn get_loop_state(&self, term_id: &TermId) -> Option<&LoopState> {
        self.loop_states.iter().find(|(id, _)| id == term_id).map(|(_, ls)| ls)
    }

    /// Get a mutable reference to a loop state by term id.
    pub fn get_loop_state_mut(&mut self, term_id: &TermId) -> Option<&mut LoopState> {
        self.loop_states.iter_mut().find(|(id, _)| id == term_id).map(|(_, ls)| ls)
    }

    /// Insert or update a loop state for the given term.
    pub fn set_loop_state(&mut self, term_id: TermId, state: LoopState) {
        if let Some(entry) = self.loop_states.iter_mut().find(|(id, _)| *id == term_id) {
            entry.1 = state;
        } else {
            self.loop_states.push((term_id, state));
        }
    }

    /// Remove a loop state for the given term.
    pub fn remove_loop_state(&mut self, term_id: &TermId) {
        if let Some(pos) = self.loop_states.iter().position(|(id, _)| id == term_id) {
            self.loop_states.swap_remove(pos);
        }
    }
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramId, context: ContextKey) -> Self {
        Self {
            id,
            program_id,
            context,
            frames: Vec::new(),
            state: HashMap::new(),
            status: StackStatus::Ready,
            break_flag: false,
            continue_flag: false,
            last_pop_result: None,
            touched_state_keys: HashSet::new(),
            functions: HashMap::new(),
            vm_frames: Vec::new(),
            vm_started: false,
            vm_frame_pool: Vec::new(),
        }
    }

    /// Clear all per-run execution state — both backends' frames and the
    /// control/result flags — leaving the stack `Ready` with no frames.
    /// Persistent `state` and captured `functions` are kept (callers that
    /// invalidate them, like hot reload, clear them separately). The caller
    /// pushes a fresh graph root frame; the bytecode VM pushes its own on the
    /// first step (`vm_started == false` signals it).
    ///
    /// This is the single reset point shared by `Env::reset_stack` and
    /// `Env::transfer_state` — resetting fields by hand at each call site let
    /// the two lists drift (the VM fields were missed on transfer).
    pub fn reset_execution(&mut self) {
        self.frames.clear();
        self.vm_frames.clear();
        self.vm_started = false;
        self.status = StackStatus::Ready;
        self.break_flag = false;
        self.continue_flag = false;
        self.last_pop_result = None;
    }

    /// Reset the touched-keys set. Called at the start of a top-level run
    /// so that `sweep_untouched_state` can drop entries no longer reachable
    /// from current source.
    pub fn start_run_tracking(&mut self) {
        self.touched_state_keys.clear();
    }

    /// Drop persistent state entries that were not touched (read or written)
    /// since the last `start_run_tracking`. Returns the number of entries
    /// removed. Called once per top-level run after the program completes.
    pub fn sweep_untouched_state(&mut self) -> usize {
        let before = self.state.len();
        self.state
            .retain(|key, _| self.touched_state_keys.contains(key));
        before - self.state.len()
    }

    pub fn push_frame(&mut self, frame: Frame) {
        self.frames.push(frame);
    }

    pub fn pop_frame(&mut self) -> Option<Frame> {
        self.frames.pop()
    }

    pub fn current_frame(&self) -> Option<&Frame> {
        self.frames.last()
    }

    pub fn current_frame_mut(&mut self) -> Option<&mut Frame> {
        self.frames.last_mut()
    }
}
