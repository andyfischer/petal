//! Runtime evaluation stack

use std::collections::HashMap;

use slotmap::new_key_type;

use crate::program::{ProgramKey, StateKey, TermId};
use crate::value::Value;

new_key_type! {
    /// Key for accessing stacks in the Env
    pub struct StackKey;
}

/// Result of a single step of execution
#[derive(Debug, Clone)]
pub enum StepResult {
    /// Execution should continue
    Continue,
    /// Execution completed with a value
    Complete(Value),
    /// Hit a breakpoint
    Breakpoint(TermId),
    /// An error occurred
    Error(String),
}

/// Runtime state key that includes iteration context
/// This allows each loop iteration to have independent state
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeStateKey {
    /// The base state key from parse time
    pub base: StateKey,
    /// Iteration indices for each enclosing loop (innermost last)
    pub iterations: Vec<i64>,
}

impl RuntimeStateKey {
    pub fn new(base: StateKey, iterations: Vec<i64>) -> Self {
        Self { base, iterations }
    }

    pub fn simple(base: StateKey) -> Self {
        Self {
            base,
            iterations: Vec::new(),
        }
    }
}

/// Loop iteration context
#[derive(Debug, Clone)]
pub struct LoopIteration {
    /// Current iteration index
    pub index: i64,
    /// The term ID of the loop (for debugging)
    pub loop_term: TermId,
}

/// A single activation frame
#[derive(Debug, Clone)]
pub struct Frame {
    /// Program being executed
    pub program_id: ProgramKey,
    /// Local variables in this frame
    pub locals: HashMap<String, Value>,
    /// Return value slot
    pub return_value: Option<Value>,
    /// Stack of loop iterations (for state key generation)
    pub loop_iterations: Vec<LoopIteration>,
}

impl Frame {
    pub fn new(program_id: ProgramKey) -> Self {
        Self {
            program_id,
            locals: HashMap::new(),
            return_value: None,
            loop_iterations: Vec::new(),
        }
    }

    /// Get the current iteration context as a vector of indices
    pub fn get_iteration_context(&self) -> Vec<i64> {
        self.loop_iterations.iter().map(|li| li.index).collect()
    }
}

/// Runtime execution stack
#[derive(Debug, Clone)]
pub struct Stack {
    pub id: StackKey,
    /// The program being executed
    pub program_id: ProgramKey,
    /// Stack of activation frames
    pub frames: Vec<Frame>,
    /// Persistent state storage with iteration-aware keys
    pub state_storage: HashMap<RuntimeStateKey, Value>,
    /// Tracks the loop depth at which each state variable was declared
    /// This allows us to use the correct iteration context for state access
    pub state_binding_depth: HashMap<StateKey, usize>,
    /// Whether execution has completed
    pub completed: bool,
    /// Final result value
    pub result: Option<Value>,
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramKey) -> Self {
        let mut stack = Self {
            id,
            program_id,
            frames: Vec::new(),
            state_storage: HashMap::new(),
            state_binding_depth: HashMap::new(),
            completed: false,
            result: None,
        };
        stack.frames.push(Frame::new(program_id));
        stack
    }

    pub fn current_frame(&self) -> Option<&Frame> {
        self.frames.last()
    }

    pub fn current_frame_mut(&mut self) -> Option<&mut Frame> {
        self.frames.last_mut()
    }

    pub fn push_frame(&mut self, program_id: ProgramKey) {
        self.frames.push(Frame::new(program_id));
    }

    pub fn pop_frame(&mut self) -> Option<Frame> {
        self.frames.pop()
    }

    /// Push a loop iteration context
    pub fn push_loop_iteration(&mut self, loop_term: TermId, index: i64) {
        if let Some(frame) = self.current_frame_mut() {
            frame.loop_iterations.push(LoopIteration { index, loop_term });
        }
    }

    /// Pop a loop iteration context
    pub fn pop_loop_iteration(&mut self) {
        if let Some(frame) = self.current_frame_mut() {
            frame.loop_iterations.pop();
        }
    }

    /// Update the current loop iteration index
    pub fn set_loop_iteration_index(&mut self, index: i64) {
        if let Some(frame) = self.current_frame_mut() {
            if let Some(li) = frame.loop_iterations.last_mut() {
                li.index = index;
            }
        }
    }

    /// Get the current iteration context from all frames
    pub fn get_iteration_context(&self) -> Vec<i64> {
        // Collect iteration context from all frames (outer to inner)
        let mut context = Vec::new();
        for frame in &self.frames {
            context.extend(frame.get_iteration_context());
        }
        context
    }

    /// Make a runtime state key using the bound iteration depth
    pub fn make_runtime_key(&self, base: StateKey) -> RuntimeStateKey {
        let context = self.get_iteration_context();
        // Use only the iteration context up to the binding depth
        if let Some(&depth) = self.state_binding_depth.get(&base) {
            let truncated: Vec<i64> = context.into_iter().take(depth).collect();
            RuntimeStateKey::new(base, truncated)
        } else {
            // Not yet bound, use full context
            RuntimeStateKey::new(base, context)
        }
    }

    /// Bind a state variable at the current iteration depth
    pub fn bind_state(&mut self, key: StateKey) {
        let depth = self.get_iteration_context().len();
        self.state_binding_depth.insert(key, depth);
    }

    /// Check if a state variable is already bound
    pub fn is_state_bound(&self, key: StateKey) -> bool {
        self.state_binding_depth.contains_key(&key)
    }

    pub fn get_local(&self, name: &str) -> Option<&Value> {
        // Search from innermost to outermost frame
        for frame in self.frames.iter().rev() {
            if let Some(value) = frame.locals.get(name) {
                return Some(value);
            }
        }
        None
    }

    pub fn set_local(&mut self, name: String, value: Value) {
        if let Some(frame) = self.current_frame_mut() {
            frame.locals.insert(name, value);
        }
    }

    /// Get state with iteration-aware key
    pub fn get_state(&self, key: StateKey) -> Option<&Value> {
        let runtime_key = self.make_runtime_key(key);
        self.state_storage.get(&runtime_key)
    }

    /// Set state with iteration-aware key
    pub fn set_state(&mut self, key: StateKey, value: Value) {
        let runtime_key = self.make_runtime_key(key);
        self.state_storage.insert(runtime_key, value);
    }

    /// Get all state keys (for debugging/inspection)
    pub fn get_all_state_keys(&self) -> Vec<&RuntimeStateKey> {
        self.state_storage.keys().collect()
    }

    pub fn reset(&mut self) {
        self.frames.clear();
        self.frames.push(Frame::new(self.program_id));
        self.completed = false;
        self.result = None;
        // Note: state_storage is intentionally preserved for inline state
    }
}
