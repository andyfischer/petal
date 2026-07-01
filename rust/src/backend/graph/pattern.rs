//! `match` execution and runtime pattern matching.

use crate::backend::pattern::match_pattern;

use super::*;

impl<'a> Evaluator<'a> {
    /// Try each arm in order: match the pattern, run the guard (if any),
    /// then execute the first matching arm's body block with the pattern's
    /// bindings applied.
    pub(super) fn exec_match(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        let program = self.program;
        let subject = inputs[0];
        let arm_metas = match program.match_arms.get(&term.id) {
            Some(arms) => arms,
            None => return ControlFlow::Error("Match: no arm metadata".into()),
        };

        for arm_meta in arm_metas {
            let mut bindings = Vec::new();
            if !match_pattern(&arm_meta.pattern, subject, self.heap, &mut bindings) {
                continue;
            }

            // Check guard if present (with pattern bindings available)
            if let Some(guard_block) = arm_meta.guard_block {
                match self.run_match_guard(term, guard_block, &bindings) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => return ControlFlow::Error(e),
                }
            }

            // Advance the parent frame past the Match term, then execute the
            // arm body with the pattern bindings applied.
            if let Some(parent_frame) = self.stack.frames.last_mut() {
                parent_frame.current_term = term.block_next;
            }
            let body_block = arm_meta.body_block;
            let block = program.get_block(body_block);
            let parent_frame_idx = self.stack.frames.len() - 1;
            self.stack.push_frame(Frame::new(
                body_block,
                block.entry,
                block.register_count as usize,
                Some(term.id),
                Some(parent_frame_idx),
            ));
            self.apply_pattern_bindings(body_block, &bindings);
            return ControlFlow::FramePushed;
        }

        ControlFlow::Error(format!(
            "No matching pattern for value: {}",
            value::value_to_display_string(&subject, self.heap)
        ))
    }

    /// Run a match arm's guard block to completion (nested stepping) and
    /// return its truthiness. The guard's result lands in the Match term's
    /// register in the parent frame via the return_term mechanism.
    fn run_match_guard(
        &mut self,
        term: &Term,
        guard_block: BlockId,
        bindings: &[(String, Value)],
    ) -> Result<bool, String> {
        let gb = self.program.get_block(guard_block);
        let parent_idx = self.stack.frames.len() - 1;
        self.stack.push_frame(Frame::new(
            guard_block,
            gb.entry,
            gb.register_count as usize,
            Some(term.id),
            Some(parent_idx),
        ));
        self.apply_pattern_bindings(guard_block, bindings);

        let target_depth = parent_idx + 1;
        loop {
            if self.stack.frames.len() <= target_depth {
                let result = self
                    .stack
                    .frames
                    .last()
                    .map(|f| f.get_register(term.register.0 as usize))
                    .unwrap_or(Value::Bool(false));
                return Ok(result.is_truthy());
            }
            match self.step() {
                StepResult::Continue => {}
                StepResult::Complete(v) => return Ok(v.is_truthy()),
                StepResult::Error(e) => return Err(e),
            }
        }
    }

    /// Apply pattern bindings to the top frame's registers by matching names
    /// to terms in the block (including phantom terms not in the linked
    /// list). Uses the precomputed block_terms index for O(B) lookup instead
    /// of O(N) where B is the number of terms in the block and N is total
    /// program terms.
    fn apply_pattern_bindings(&mut self, block_id: BlockId, bindings: &[(String, Value)]) {
        let program = self.program;
        let Some(frame) = self.stack.frames.last_mut() else {
            return;
        };
        let Some(term_ids) = program.block_terms.get(&block_id) else {
            return;
        };
        for tid in term_ids {
            let term = program.get_term(*tid);
            let Some(ref term_name) = term.name else {
                continue;
            };
            for (bind_name, bind_val) in bindings {
                if term_name == bind_name {
                    let reg = term.register.0 as usize;
                    if reg < frame.registers.len() {
                        frame.registers[reg] = *bind_val;
                    }
                }
            }
        }
    }
}
