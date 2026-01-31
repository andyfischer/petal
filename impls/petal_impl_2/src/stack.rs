use crate::program::ProgramKey;
use crate::term::{StateKey, TermId};
use crate::value::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StackKey(pub usize);

#[derive(Debug, Clone)]
pub struct Stack {
    pub id: StackKey,
    pub program_id: ProgramKey,
    pub frames: Vec<Frame>,
    pub state_storage: HashMap<StateKey, Value>,
    pub state_variables: HashMap<String, StateKey>,  // Maps variable names to state keys
    pub globals: HashMap<String, Value>,
    pub completed: bool,
    pub result: Value,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub current_term: TermId,
    pub locals: HashMap<String, Value>,
    pub return_term: Option<TermId>,
    pub loop_context: Option<LoopContext>,
}

#[derive(Debug, Clone)]
pub struct LoopContext {
    pub iteration_index: usize,
    pub state_prefix: StateKey,
    pub continue_target: TermId,
    pub break_target: Option<TermId>,
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramKey, entry: TermId) -> Self {
        let frame = Frame {
            current_term: entry,
            locals: HashMap::new(),
            return_term: None,
            loop_context: None,
        };

        Self {
            id,
            program_id,
            frames: vec![frame],
            state_storage: HashMap::new(),
            state_variables: HashMap::new(),
            globals: HashMap::new(),
            completed: false,
            result: Value::Nil,
        }
    }

    pub fn current_frame(&self) -> Option<&Frame> {
        self.frames.last()
    }

    pub fn current_frame_mut(&mut self) -> Option<&mut Frame> {
        self.frames.last_mut()
    }

    pub fn push_frame(&mut self, frame: Frame) {
        self.frames.push(frame);
    }

    pub fn pop_frame(&mut self) -> Option<Frame> {
        self.frames.pop()
    }

    pub fn get_state(&self, key: StateKey) -> Option<&Value> {
        self.state_storage.get(&key)
    }

    pub fn set_state(&mut self, key: StateKey, value: Value) {
        self.state_storage.insert(key, value);
    }

    pub fn get_variable(&self, name: &str) -> Option<&Value> {
        // Check local scope first
        if let Some(frame) = self.current_frame() {
            if let Some(value) = frame.locals.get(name) {
                return Some(value);
            }
        }

        // Then check globals
        self.globals.get(name)
    }

    pub fn set_variable(&mut self, name: String, value: Value) {
        if let Some(frame) = self.current_frame_mut() {
            frame.locals.insert(name, value);
        } else {
            self.globals.insert(name, value);
        }
    }

    pub fn set_global(&mut self, name: String, value: Value) {
        self.globals.insert(name, value);
    }

    pub fn register_state_variable(&mut self, name: String, state_key: StateKey) {
        self.state_variables.insert(name, state_key);
    }

    pub fn get_state_key_for_variable(&self, name: &str) -> Option<StateKey> {
        self.state_variables.get(name).copied()
    }
}
