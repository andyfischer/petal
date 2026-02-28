//! Stack - Runtime evaluation context.
//!
//! See docs/tech_outline/data_structures/Stack.md

use std::collections::HashMap;

use smallvec::SmallVec;

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

/// Per-loop state stored in the frame that owns the loop term.
///
/// Because this lives on the Frame, it is automatically discarded when the
/// frame is popped (e.g. by an early `return`), which prevents stale state
/// from leaking across function calls.
pub enum LoopState {
    /// For-each loop: elements copied from the list and the next index to process.
    For { elements: Vec<Value>, index: usize },
    /// While loop: the condition block has been pushed; waiting for its result.
    /// `iteration` tracks the current iteration (0-based) for per-iteration state.
    WhileCondition { iteration: usize },
}

/// Runtime execution state for a program.
pub struct Stack {
    pub id: StackKey,
    pub program_id: ProgramId,
    pub frames: Vec<Frame>,
    pub state: HashMap<RuntimeStateKey, Value>,
    pub status: StackStatus,
    pub break_flag: bool,
    pub continue_flag: bool,
    /// Temporary storage for the result of the last popped frame.
    /// Used by synchronous closure calls (map/filter/reduce) to capture return values.
    pub last_pop_result: Option<Value>,
}

/// A single activation frame on the stack.
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
    pub loop_states: HashMap<TermId, LoopState>,
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
            loop_states: HashMap::new(),
        }
    }

    /// Set this frame as a loop body frame.
    pub fn as_loop_body(mut self) -> Self {
        self.is_loop_body = true;
        self
    }
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramId) -> Self {
        Self {
            id,
            program_id,
            frames: Vec::new(),
            state: HashMap::new(),
            status: StackStatus::Ready,
            break_flag: false,
            continue_flag: false,
            last_pop_result: None,
        }
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
