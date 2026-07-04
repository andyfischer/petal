//! Stack - Runtime evaluation context.
//!
//! See docs/Architecture.md for the surrounding runtime design.

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use crate::execution_context::ContextKey;
use crate::program::{ProgramId, StateKey};
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

/// Runtime execution state for a program.
#[derive(Clone)]
pub struct Stack {
    pub id: StackKey,
    pub program_id: ProgramId,
    /// The ExecutionContext this stack draws its heap and registries from.
    pub context: ContextKey,
    pub state: HashMap<RuntimeStateKey, Value>,
    pub status: StackStatus,
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
    /// Activation records for the bytecode VM. Stored on the stack (not on the
    /// VM, which is rebuilt per step) so execution state survives across steps
    /// and is reachable as GC roots.
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

#[derive(Debug, Clone)]
pub enum StackStatus {
    Ready,
    Running,
    Complete(Value),
    Error(String),
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramId, context: ContextKey) -> Self {
        Self {
            id,
            program_id,
            context,
            state: HashMap::new(),
            status: StackStatus::Ready,
            last_pop_result: None,
            touched_state_keys: HashSet::new(),
            functions: HashMap::new(),
            vm_frames: Vec::new(),
            vm_started: false,
            vm_frame_pool: Vec::new(),
        }
    }

    /// Clear all per-run execution state, leaving the stack `Ready` with no
    /// frames. Persistent `state` and captured `functions` are kept (callers
    /// that invalidate them, like hot reload, clear them separately). The
    /// bytecode VM pushes its own root frame on the first step of the next run
    /// (`vm_started == false` signals it).
    ///
    /// This is the single reset point shared by `Env::reset_stack` and
    /// `Env::transfer_state`.
    pub fn reset_execution(&mut self) {
        self.vm_frames.clear();
        self.vm_started = false;
        self.status = StackStatus::Ready;
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
}
