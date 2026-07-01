//! Persistent `state` variables.
//!
//! A state slot is identified at runtime by its compile-time `StateKey` plus
//! a loop-iteration context — either the indices of the enclosing loops or
//! an explicit `state(key)` value — so each iteration can own its own slot.

use smallvec::SmallVec;

use crate::stack::{LoopKeyPart, RuntimeStateKey};

use super::*;

impl<'a> Evaluator<'a> {
    /// Walk the stack frames collecting the current iteration index of every
    /// active loop, outermost first. All loop kinds share the same counter, so
    /// no per-kind handling is needed here.
    fn loop_key_parts(&self) -> SmallVec<[LoopKeyPart; 2]> {
        let mut parts = SmallVec::new();
        for frame in &self.stack.frames {
            for (_, loop_state) in &frame.loop_states {
                parts.push(LoopKeyPart::Index(loop_state.iteration));
            }
        }
        parts
    }

    /// Build the RuntimeStateKey for a state term, taking into account loop
    /// context and explicit keys.
    ///
    /// `explicit_key` is the runtime value the source-level `state(expr) name`
    /// resolved to, or `None` for the default "key by loop index" form. When
    /// it's `Some`, the value is hashed and used instead of the loop index;
    /// this is what makes per-iteration state survive list reordering.
    fn resolve_runtime_state_key(
        &self,
        term: &Term,
        explicit_key: Option<&Value>,
    ) -> RuntimeStateKey {
        let base = term.state_key.unwrap();
        let loop_indices = match explicit_key {
            Some(key_val) => {
                let hash = value::hash_value(key_val, self.heap);
                smallvec::smallvec![LoopKeyPart::Explicit(hash)]
            }
            None if term.in_loop => self.loop_key_parts(),
            None => SmallVec::new(),
        };
        RuntimeStateKey { base, loop_indices }
    }

    /// `state name = init` — evaluate the init block lazily, only when the
    /// runtime key has no value yet.
    pub(super) fn exec_state_init(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        // The explicit key (if any) is the only input; the init value lives
        // in child_blocks[0] and is computed lazily.
        let runtime_key = self.resolve_runtime_state_key(term, inputs.first());
        self.stack.touched_state_keys.insert(runtime_key.clone());

        if let Some(&existing) = self.stack.state.get(&runtime_key) {
            // Cache hit: skip the init expression entirely.
            self.produce(term, existing)
        } else if let Some(&init_block) = term.child_blocks.first() {
            // Cache miss: push a frame for the init block. On pop, its last
            // term value is written into this StateInit's register (via
            // return_term) and `finish_state_init` stores it into the
            // persistent state map.
            self.push_child_frame(init_block, term);
            ControlFlow::FramePushed
        } else {
            // No init block (legacy / synthetic StateInit): seed nil.
            self.stack.state.insert(runtime_key, Value::Nil);
            self.produce(term, Value::Nil)
        }
    }

    /// Called by `pop_frame` when a StateInit's init block finishes: bind the
    /// computed init value into the persistent state map. The runtime key is
    /// computed here because the explicit-key input lives in the parent
    /// frame's registers.
    pub(super) fn finish_state_init(&mut self, init_term: &Term, value: Value) {
        let explicit_key = init_term
            .inputs
            .first()
            .map(|&tid| self.read_register(tid));
        let runtime_key = self.resolve_runtime_state_key(init_term, explicit_key.as_ref());
        self.stack.touched_state_keys.insert(runtime_key.clone());
        self.stack.state.insert(runtime_key, value);
    }

    pub(super) fn exec_state_read(&mut self, term: &Term) -> ControlFlow {
        let runtime_key = self.resolve_runtime_state_key(term, None);
        self.stack.touched_state_keys.insert(runtime_key.clone());
        let val = self
            .stack
            .state
            .get(&runtime_key)
            .copied()
            .unwrap_or(Value::Nil);
        self.produce(term, val)
    }

    pub(super) fn exec_state_write(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        // Inputs: [value] or [value, explicit_key]. The explicit key
        // (if present) is always the last input.
        let val = inputs.first().copied().unwrap_or(Value::Nil);
        let explicit_key = if inputs.len() > 1 { inputs.last() } else { None };
        let runtime_key = self.resolve_runtime_state_key(term, explicit_key);
        self.stack.touched_state_keys.insert(runtime_key.clone());
        self.stack.state.insert(runtime_key, val);
        self.produce(term, val)
    }
}
