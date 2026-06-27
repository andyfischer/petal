//! Transfer a running stack's state onto a different program.
//!
//! Replaces the program backing a running stack with a new compiled program,
//! preserving state values with matching StateKeys across the transfer.
//! Hot-reloading is one use of this, but it can reshape a stack for any new
//! program that shares the same StateKeys.

use crate::env::Env;
use crate::program::Program;
use crate::stack::StackStatus;

/// Result of transferring a stack's state onto a new program.
pub struct TransferStateResult {
    /// Number of state values preserved across the transfer.
    pub state_preserved: usize,
    /// Number of state values dropped (no matching key in new program).
    pub state_dropped: usize,
}

impl Env {
    /// Transfer a running stack's state onto a pre-compiled program.
    /// State values with matching StateKeys are preserved across the transfer;
    /// the rest are dropped. The new program's ProgramId must match the
    /// stack's existing program.
    pub fn transfer_state(
        &mut self,
        stack_id: crate::stack::StackKey,
        new_program: Program,
    ) -> Result<TransferStateResult, String> {
        let stack = self
            .stack(stack_id)
            .ok_or("Stack not found")?;
        let old_program_id = stack.program_id;

        // Collect base state keys from the new program to know which state to keep
        let new_state_keys: std::collections::HashSet<_> = new_program.terms.iter()
            .filter_map(|t| t.state_key)
            .collect();

        // Determine which old state values will be preserved (match on base key)
        let preserved: usize = stack.state.keys()
            .filter(|k| new_state_keys.contains(&k.base))
            .count();
        let dropped: usize = stack.state.len() - preserved;

        // Replace program
        self.insert_program(old_program_id, new_program);

        // Clear closures (they reference the old program's function defs)
        self.clear_closures();

        // Reset stack: keep state but restart execution with new program
        {
            let stack = self.stack_mut(stack_id).unwrap();
            // Remove state keys that no longer exist in the new program
            stack.state.retain(|k, _| new_state_keys.contains(&k.base));
            stack.frames.clear();
            stack.status = StackStatus::Ready;
            stack.break_flag = false;
            stack.continue_flag = false;
            stack.last_pop_result = None;
            // The old captured closures point into the now-cleared closures
            // vec; they get recaptured on the next run.
            stack.functions.clear();
        }

        self.push_root_frame_for(stack_id)?;

        Ok(TransferStateResult {
            state_preserved: preserved,
            state_dropped: dropped,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::env::Env;

    #[test]
    fn transfer_state_preserves_state() {
        let mut env = Env::new();

        // Run initial program that sets state via StateWrite
        let source_v1 = r#"
state counter = 0
counter += 5
print(counter)
"#;
        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let output_v1 = env.take_output();
        assert_eq!(output_v1, vec!["5"]);

        // Transfer state onto new source that reads the same state
        let source_v2 = r#"
state counter = 0
counter += 10
print(counter)
"#;
        let new_program = env.compile_program(pid, source_v2).unwrap();
        let result = env.transfer_state(sid, new_program).unwrap();
        assert_eq!(result.state_preserved, 1);
        assert_eq!(result.state_dropped, 0);

        // Run the program after transfer: counter=5 (preserved), +=10 -> 15
        env.run(sid).unwrap();
        let output_v2 = env.take_output();
        assert_eq!(output_v2, vec!["15"]);
    }

    #[test]
    fn transfer_state_drops_removed_state() {
        let mut env = Env::new();

        // Run with two state variables
        let source_v1 = r#"
state a = 1
state b = 2
print(a + b)
"#;
        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let output = env.take_output();
        assert_eq!(output, vec!["3"]);

        // Transfer onto a program with only one state variable
        let source_v2 = r#"
state a = 1
print(a)
"#;
        let new_program = env.compile_program(pid, source_v2).unwrap();
        let result = env.transfer_state(sid, new_program).unwrap();
        assert_eq!(result.state_preserved, 1); // 'a' preserved
        assert_eq!(result.state_dropped, 1);   // 'b' dropped

        env.run(sid).unwrap();
        let output = env.take_output();
        // a was 1 (from init, not modified), state init skips, prints 1
        assert_eq!(output, vec!["1"]);
    }

    #[test]
    fn transfer_state_preserves_state_after_reordering() {
        let mut env = Env::new();

        // Run with a=1, b=2, modify both
        let source_v1 = r#"
state a = 0
state b = 0
a += 10
b += 20
print(a, b)
"#;
        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let output = env.take_output();
        assert_eq!(output, vec!["10 20"]);

        // Transfer onto state declarations in reversed order
        let source_v2 = r#"
state b = 0
state a = 0
print(a, b)
"#;
        let new_program = env.compile_program(pid, source_v2).unwrap();
        let result = env.transfer_state(sid, new_program).unwrap();
        assert_eq!(result.state_preserved, 2); // both preserved
        assert_eq!(result.state_dropped, 0);

        env.run(sid).unwrap();
        let output = env.take_output();
        // Both values preserved despite reordering
        assert_eq!(output, vec!["10 20"]);
    }

    #[test]
    fn transfer_state_fresh_state_gets_initialized() {
        let mut env = Env::new();

        // Run with one state
        let source_v1 = r#"
state x = 10
print(x)
"#;
        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        env.take_output(); // discard

        // Transfer onto a program that adds a new state variable
        let source_v2 = r#"
state x = 10
state y = 20
print(x + y)
"#;
        let new_program = env.compile_program(pid, source_v2).unwrap();
        let result = env.transfer_state(sid, new_program).unwrap();
        assert_eq!(result.state_preserved, 1); // 'x' preserved

        env.run(sid).unwrap();
        let output = env.take_output();
        // x=10 (preserved), y=20 (newly initialized), sum=30
        assert_eq!(output, vec!["30"]);
    }
}
