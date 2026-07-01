//! For / numeric-for / while loop execution.
//!
//! A loop term re-executes once per iteration: each visit either pushes a
//! body frame (for the next iteration) or finishes the loop. Per-loop
//! progress lives in the owning frame's `loop_states`, so it is discarded
//! automatically if the frame pops early (e.g. on `return`).

use crate::stack::{LoopKind, LoopState};

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
        if let Some(val) = loop_var
            && let Some(frame) = self.stack.frames.last_mut()
            && !frame.registers.is_empty()
        {
            frame.registers[0] = val;
        }
        ControlFlow::FramePushed
    }

    /// Advance a for-style loop (for-each or numeric range) to the iteration
    /// it should run next, returning that iteration's loop value — or `None`
    /// when the loop is finished, in which case the loop state is removed.
    ///
    /// `first_visit` is true only on the loop term's initial execution, where
    /// iteration 0 runs without advancing; every later visit steps the
    /// iteration counter forward by one.
    fn next_for_value(&mut self, term: &Term, first_visit: bool) -> Option<Value> {
        let frame = self.stack.frames.last_mut().unwrap();
        let state = frame.get_loop_state_mut(&term.id)?;
        if !first_visit {
            state.iteration += 1;
        }
        let i = state.iteration;
        let next = match &state.kind {
            LoopKind::ForEach { elements } => elements.get(i).copied(),
            LoopKind::Range { start, end } => {
                let value = *start + i as i64;
                (value < *end).then_some(Value::Int(value))
            }
            LoopKind::While { .. } => None,
        };
        if next.is_none() {
            frame.remove_loop_state(&term.id);
        }
        next
    }

    /// True only on a loop term's first execution, before its loop state has
    /// been initialized.
    fn first_loop_visit(&self, term: &Term) -> bool {
        self.stack
            .frames
            .last()
            .map(|f| !f.has_loop_state(&term.id))
            .unwrap_or(false)
    }

    /// Store a freshly initialized loop state (iteration 0) on the current
    /// frame for `term`.
    fn init_loop_state(&mut self, term: &Term, kind: LoopKind) {
        if let Some(frame) = self.stack.frames.last_mut() {
            frame.set_loop_state(term.id, LoopState { iteration: 0, kind });
        }
    }

    /// Drive a for-style loop one step: advance to the next iteration's value
    /// and push its body frame, or finish the loop once values run out. Shared
    /// by the for-each and numeric-range handlers once their kind is set up.
    fn advance_for_loop(&mut self, term: &Term, first_visit: bool) -> ControlFlow {
        match self.next_for_value(term, first_visit) {
            Some(val) => self.push_loop_body(term, term.child_blocks[0], Some(val)),
            // All iterations complete.
            None => self.produce(term, Value::Nil),
        }
    }

    /// `for x in list { ... }` — snapshots the list on first visit, then
    /// pushes one body frame per element.
    pub(super) fn exec_for_loop(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        if let Some(done) = self.check_break_continue(term) {
            return done;
        }

        let first_visit = self.first_loop_visit(term);
        if first_visit {
            let Value::List(list_id) = inputs[0] else {
                return ControlFlow::Error(format!(
                    "Cannot iterate over {}",
                    inputs[0].type_name()
                ));
            };
            let elements = self.heap.get_list(list_id).to_vec();
            self.init_loop_state(term, LoopKind::ForEach { elements });
        }

        self.advance_for_loop(term, first_visit)
    }

    /// `for i in range(a, b)` — identical control flow to `exec_for_loop`,
    /// but the loop value is an integer counter and no list is materialized.
    pub(super) fn exec_numeric_for_loop(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        if let Some(done) = self.check_break_continue(term) {
            return done;
        }

        let first_visit = self.first_loop_visit(term);
        if first_visit {
            let (Value::Int(start), Value::Int(end)) = (inputs[0], inputs[1]) else {
                return ControlFlow::Error(
                    "numeric for-loop bounds must be integers".to_string(),
                );
            };
            self.init_loop_state(term, LoopKind::Range { start, end });
        }

        self.advance_for_loop(term, first_visit)
    }

    /// `while cond { ... }` — alternates between running the condition block
    /// and the body block, tracking the phase in loop state.
    pub(super) fn exec_while_loop(&mut self, term: &Term) -> ControlFlow {
        if let Some(done) = self.check_break_continue(term) {
            return done;
        }

        let cond_block = term.child_blocks[0];
        let body_block = term.child_blocks[1];

        // Read the current phase: whether the body (rather than the condition)
        // was the block that just returned, plus the iteration counter. Absent
        // when this is a fresh start with no loop state yet.
        let phase = self
            .stack
            .frames
            .last()
            .and_then(|f| f.get_loop_state(&term.id))
            .map(|ls| {
                let running_body = matches!(ls.kind, LoopKind::While { running_body: true });
                (running_body, ls.iteration)
            });

        match phase {
            // Fresh start — run the condition for iteration 0.
            None => self.push_while_condition(term, cond_block, 0),
            // Condition block just returned; check its result.
            Some((false, iteration)) => {
                let cond_val = self.read_register(term.id);
                if !cond_val.is_truthy() {
                    // Condition false — loop done.
                    if let Some(frame) = self.stack.frames.last_mut() {
                        frame.remove_loop_state(&term.id);
                    }
                    return self.produce(term, Value::Nil);
                }
                // Enter the body phase so per-iteration state keys see this iteration.
                if let Some(frame) = self.stack.frames.last_mut() {
                    frame.set_loop_state(
                        term.id,
                        LoopState { iteration, kind: LoopKind::While { running_body: true } },
                    );
                }
                self.push_loop_body(term, body_block, None)
            }
            // Body just returned — run the condition for the next iteration.
            Some((true, iteration)) => {
                self.push_while_condition(term, cond_block, iteration + 1)
            }
        }
    }

    /// Push the while-condition block and record that we are evaluating the
    /// condition for `iteration`.
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
            frame.set_loop_state(
                term.id,
                LoopState { iteration, kind: LoopKind::While { running_body: false } },
            );
        }
        ControlFlow::FramePushed
    }
}
