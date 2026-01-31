//! Execution stack and runtime context

use slotmap::new_key_type;
use std::collections::HashMap;

use crate::{value::Value, program::{TermId, ProgramKey, StateKey}};

new_key_type! {
    pub struct StackKey;
}

/// Execution result after stepping
#[derive(Debug, Clone)]
pub enum StepResult {
    /// Continue execution
    Continue,
    /// Execution completed with a value
    Complete(Value),
    /// Hit a breakpoint at this term
    Breakpoint(TermId),
    /// Error occurred
    Error(crate::Error),
}

/// The execution stack
pub struct Stack {
    pub id: StackKey,
    pub program_id: ProgramKey,

    /// Stack of activation frames
    pub frames: Vec<Frame>,

    /// Persistent state storage (for `state` declarations)
    pub state_storage: HashMap<StateKey, Value>,

    /// Current point in control flow (term ID)
    pub current_term: Option<TermId>,
}

/// An activation frame
pub struct Frame {
    /// Register file for this frame
    pub registers: Vec<Value>,

    /// Return address (term to jump to when this frame completes)
    pub return_term: Option<TermId>,

    /// For loops: iteration context
    pub loop_context: Option<LoopContext>,
}

/// Context for loop iterations
#[derive(Debug, Clone)]
pub struct LoopContext {
    /// Current iteration index
    pub iteration_index: usize,

    /// State prefix for this iteration
    pub state_prefix: StateKey,
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramKey) -> Self {
        Self {
            id,
            program_id,
            frames: Vec::new(),
            state_storage: HashMap::new(),
            current_term: None,
        }
    }

    /// Create a new frame and push it onto the stack
    pub fn push_frame(&mut self, return_term: Option<TermId>) {
        let frame = Frame {
            registers: Vec::new(),
            return_term,
            loop_context: None,
        };
        self.frames.push(frame);
    }

    /// Pop the current frame from the stack
    pub fn pop_frame(&mut self) -> Option<Frame> {
        self.frames.pop()
    }

    /// Get the current frame
    pub fn current_frame(&self) -> Option<&Frame> {
        self.frames.last()
    }

    /// Get the current frame mutably
    pub fn current_frame_mut(&mut self) -> Option<&mut Frame> {
        self.frames.last_mut()
    }

    /// Write a value to a register in the current frame
    pub fn write_register(&mut self, index: usize, value: Value) {
        if let Some(frame) = self.current_frame_mut() {
            if index >= frame.registers.len() {
                frame.registers.resize(index + 1, Value::Nil);
            }
            frame.registers[index] = value;
        }
    }

    /// Read a value from a register in the current frame
    pub fn read_register(&self, index: usize) -> Option<&Value> {
        self.current_frame()
            .and_then(|frame| frame.registers.get(index))
    }

    /// Get state value by key
    pub fn get_state(&self, key: &StateKey) -> Option<&Value> {
        self.state_storage.get(key)
    }

    /// Set state value by key
    pub fn set_state(&mut self, key: StateKey, value: Value) {
        self.state_storage.insert(key, value);
    }

    /// Get all state keys
    pub fn state_keys(&self) -> impl Iterator<Item = &StateKey> {
        self.state_storage.keys()
    }

    /// Reset the stack to initial state but preserve state storage
    pub fn reset(&mut self) {
        self.frames.clear();
        self.current_term = None;
        // Note: state_storage is preserved
    }
}

impl Frame {
    pub fn new() -> Self {
        Self {
            registers: Vec::new(),
            return_term: None,
            loop_context: None,
        }
    }

    /// Set up loop context
    pub fn with_loop_context(mut self, iteration_index: usize, state_prefix: StateKey) -> Self {
        self.loop_context = Some(LoopContext {
            iteration_index,
            state_prefix,
        });
        self
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}
