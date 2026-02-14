//! Stack - Runtime evaluation context.
//!
//! See docs/tech_outline/data_structures/Stack.md

use std::collections::HashMap;

use crate::program::{BlockId, ProgramId, StateKey, TermId};
use crate::value::Value;

/// Unique identifier for a stack within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StackKey(pub u32);

/// Runtime execution state for a program.
pub struct Stack {
    pub id: StackKey,
    pub program_id: ProgramId,
    pub frames: Vec<Frame>,
    pub state: HashMap<StateKey, Value>,
    pub status: StackStatus,
    pub break_flag: bool,
}

/// A single activation frame on the stack.
pub struct Frame {
    pub block_id: BlockId,
    pub current_term: Option<TermId>,
    pub registers: Vec<Value>,
    /// Term in the parent frame to resume at when this frame completes
    pub return_term: Option<TermId>,
    /// Index into stack.frames of the parent frame (for cross-frame register reads).
    /// None for function call frames (captures are in local registers).
    pub parent_frame: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum StackStatus {
    Ready,
    Running,
    Complete(Value),
    Error(String),
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
