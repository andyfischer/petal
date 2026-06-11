//! For / numeric-for / while loop execution.
//!
//! A loop term re-executes once per iteration: each visit either pushes a
//! body frame (for the next iteration) or finishes the loop. Per-loop
//! progress lives in the owning frame's `loop_states`, so it is discarded
//! automatically if the frame pops early (e.g. on `return`).

use crate::stack::LoopState;

use super::*;

impl<'a> Evaluator<'a> {
    /// Shared loop prelude: consume a pending break (which finishes the
    /// loop) and clear a pending continue (which just means "proceed to the
    /// next iteration"). Returns `Some` when the loop is done.
    fn check_break_continue(&mut self, term: &Term) -> Option<ControlFlow> {
        if self.stack.break_flag {
            self.stack.break_flag = false;
            if let Some(frame) = self.stack.frames.last_mut() {
                frame.remove_loop_state(&term.id);
            }
            return Some(self.produce(term, Value::Nil));
        }
        self.stack.continue_flag = false;
        None
    }

    /// Push a body frame for one loop iteration, optionally placing the
    /// loop variable in the body block's first register.
    fn push_loop_body(
        &mut self,
        term: &Term,
        body_block: BlockId,
        loop_var: Option<Value>,
    ) -> ControlFlow {
        let block = self.program.get_block(body_block);
        let parent_frame_idx = self.stack.frames.len() - 1;
        self.stack.push_frame(
            Frame::new(
                body_block,
                block.entry,
                block.register_count as usize,
                Some(term.id),
                Some(parent_frame_idx),
            )
            .as_loop_body(),
        );
        if let Some(val) = loop_var {
            if let Some(frame) = self.stack.frames.last_mut() {
                if !frame.registers.is_empty() {
                    frame.registers[0] = val;
                }
            }
        }
        ControlFlow::FramePushed
    }

    /// Yield the next element / counter value for a for-style loop, removing
    /// the loop state when iteration is complete.
    fn next_for_value(&mut self, term: &Term) -> Option<Value> {
        let frame = self.stack.frames.last_mut().unwrap();
        let next = match frame.get_loop_state_mut(&term.id) {
            Some(LoopState::For { elements, index }) => {
                if *index < elements.len() {
                    let elem = elements[*index];
                    *index += 1;
                    Some(elem)
                } else {
                    None
                }
            }
            Some(LoopState::NumericFor { current, end, index }) => {
                if *current < *end {
                    let val = Value::Int(*current);
                    *current += 1;
                    *index += 1;
                    Some(val)
                } else {
                    None
                }
            }
            _ => return None,
        };
        if next.is_none() {
            frame.remove_loop_state(&term.id);
        }
        next
    }

    /// `for x in list { ... }` — snapshots the list on first visit, then
    /// pushes one body frame per element.
    pub(super) fn exec_for_loop(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        if let Some(done) = self.check_break_continue(term) {
            return done;
        }

        // Initialize loop state on the first visit.
        let needs_init = self
            .stack
            .frames
            .last()
            .map(|f| !f.has_loop_state(&term.id))
            .unwrap_or(false);
        if needs_init {
            let Value::List(list_id) = inputs[0] else {
                return ControlFlow::Error(format!(
                    "Cannot iterate over {}",
                    inputs[0].type_name()
                ));
            };
            let elements = self.heap.get_list(list_id).to_vec();
            if let Some(frame) = self.stack.frames.last_mut() {
                frame.set_loop_state(term.id, LoopState::For { elements, index: 0 });
            }
        }

        match self.next_for_value(term) {
            Some(elem) => self.push_loop_body(term, term.child_blocks[0], Some(elem)),
            // All iterations complete.
            None => self.produce(term, Value::Nil),
        }
    }

    /// `for i in range(a, b)` — identical control flow to `exec_for_loop`,
    /// but the loop value is an integer counter and no list is materialized.
    pub(super) fn exec_numeric_for_loop(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        if let Some(done) = self.check_break_continue(term) {
            return done;
        }

        // Initialize loop state on the first visit: read the integer bounds.
        let needs_init = self
            .stack
            .frames
            .last()
            .map(|f| !f.has_loop_state(&term.id))
            .unwrap_or(false);
        if needs_init {
            let (Value::Int(start), Value::Int(end)) = (inputs[0], inputs[1]) else {
                return ControlFlow::Error(
                    "numeric for-loop bounds must be integers".to_string(),
                );
            };
            if let Some(frame) = self.stack.frames.last_mut() {
                frame.set_loop_state(
                    term.id,
                    LoopState::NumericFor { current: start, end, index: 0 },
                );
            }
        }

        match self.next_for_value(term) {
            Some(val) => self.push_loop_body(term, term.child_blocks[0], Some(val)),
            None => self.produce(term, Value::Nil),
        }
    }

    /// `while cond { ... }` — alternates between running the condition block
    /// and the body block, tracking the phase in loop state.
    pub(super) fn exec_while_loop(&mut self, term: &Term) -> ControlFlow {
        if let Some(done) = self.check_break_continue(term) {
            return done;
        }

        let cond_block = term.child_blocks[0];
        let body_block = term.child_blocks[1];

        // Which phase is this visit in? (condition just returned / body just
        // returned / fresh start)
        let phase = self
            .stack
            .frames
            .last()
            .and_then(|f| f.get_loop_state(&term.id))
            .map(|ls| match ls {
                LoopState::WhileCondition { iteration } => (true, *iteration),
                LoopState::WhileBody { iteration } => (false, *iteration),
                _ => (false, 0),
            });

        match phase {
            Some((true, iteration)) => {
                // Condition block just returned; check its result.
                let cond_val = self.read_register(term.id);
                if !cond_val.is_truthy() {
                    // Condition false — loop done.
                    if let Some(frame) = self.stack.frames.last_mut() {
                        frame.remove_loop_state(&term.id);
                    }
                    return self.produce(term, Value::Nil);
                }
                // Transition to WhileBody so state keys see the iteration.
                if let Some(frame) = self.stack.frames.last_mut() {
                    frame.set_loop_state(term.id, LoopState::WhileBody { iteration });
                }
                self.push_loop_body(term, body_block, None)
            }
            Some((false, iteration)) => {
                // Body just returned — run the condition for the next iteration.
                self.push_while_condition(term, cond_block, iteration + 1)
            }
            None => {
                // Fresh start — run the condition for iteration 0.
                self.push_while_condition(term, cond_block, 0)
            }
        }
    }

    /// Push the while-condition block and record the upcoming iteration.
    fn push_while_condition(
        &mut self,
        term: &Term,
        cond_block: BlockId,
        iteration: usize,
    ) -> ControlFlow {
        let block = self.program.get_block(cond_block);
        let parent_frame_idx = self.stack.frames.len() - 1;
        self.stack.push_frame(Frame::new(
            cond_block,
            block.entry,
            block.register_count as usize,
            Some(term.id),
            Some(parent_frame_idx),
        ));
        if let Some(frame) = self.stack.frames.get_mut(parent_frame_idx) {
            frame.set_loop_state(term.id, LoopState::WhileCondition { iteration });
        }
        ControlFlow::FramePushed
    }
}
