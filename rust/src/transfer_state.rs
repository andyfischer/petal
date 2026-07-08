//! Transfer a running stack's state onto a different program.
//!
//! Replaces the program backing a running stack with a new compiled program,
//! preserving state values with matching StateKeys across the transfer.
//! Hot-reloading is one use of this, but it can reshape a stack for any new
//! program that shares the same StateKeys.

use crate::env::Env;
use crate::program::Program;

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
        let new_state_keys: std::collections::HashSet<_> =
            new_program.state_terms().map(|(k, _)| k).collect();

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
            stack.reset_execution();
            // The old captured closures point into the now-cleared closures
            // vec; they get recaptured on the next run.
            stack.functions.clear();
        }
        // `reset_execution` cleared `vm_started`, so the VM re-pushes its root
        // frame (against the new program's lowering) on the next run.

        Ok(TransferStateResult {
            state_preserved: preserved,
            state_dropped: dropped,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::OptFlags;
    use crate::env::Env;

    /// Run a transfer scenario under one optimization level: run `source_v1`,
    /// assert its output, transfer onto `source_v2` (asserting preserved/dropped
    /// counts), re-run, and assert the post-transfer output. Each test runs
    /// under both the clone-and-alloc baseline and the in-place path — hot
    /// reload crosses the program-replacement seam (bytecode cache invalidation,
    /// VM run-state reset), and in-place mutation reaches heap state that
    /// survives the reload, so both paths must be exercised.
    fn check_transfer(
        opts: OptFlags,
        source_v1: &str,
        expect_v1: &[&str],
        source_v2: &str,
        expect_preserved: usize,
        expect_dropped: usize,
        expect_v2: &[&str],
    ) {
        let mut env = Env::new();
        env.set_opt_flags(opts);

        let pid = env.load_program(source_v1).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), expect_v1, "[{opts:?}] v1 output");

        let new_program = env.compile_program(pid, source_v2).unwrap();
        let result = env.transfer_state(sid, new_program).unwrap();
        assert_eq!(result.state_preserved, expect_preserved, "[{opts:?}] preserved");
        assert_eq!(result.state_dropped, expect_dropped, "[{opts:?}] dropped");

        env.run(sid).unwrap();
        assert_eq!(env.take_output(), expect_v2, "[{opts:?}] v2 output");
    }

    fn check_transfer_both_opt_levels(
        source_v1: &str,
        expect_v1: &[&str],
        source_v2: &str,
        expect_preserved: usize,
        expect_dropped: usize,
        expect_v2: &[&str],
    ) {
        for opts in [OptFlags::none(), OptFlags::all()] {
            check_transfer(
                opts,
                source_v1,
                expect_v1,
                source_v2,
                expect_preserved,
                expect_dropped,
                expect_v2,
            );
        }
    }

    #[test]
    fn transfer_state_preserves_state() {
        check_transfer_both_opt_levels(
            // Run initial program that sets state via StateWrite
            "state counter = 0\ncounter += 5\nprint(counter)",
            &["5"],
            // Transfer onto new source that reads the same state:
            // counter=5 (preserved), +=10 -> 15
            "state counter = 0\ncounter += 10\nprint(counter)",
            1,
            0,
            &["15"],
        );
    }

    #[test]
    fn transfer_state_drops_removed_state() {
        check_transfer_both_opt_levels(
            // Run with two state variables
            "state a = 1\nstate b = 2\nprint(a + b)",
            &["3"],
            // Transfer onto a program with only one state variable:
            // 'a' preserved (init skips, prints 1), 'b' dropped
            "state a = 1\nprint(a)",
            1,
            1,
            &["1"],
        );
    }

    #[test]
    fn transfer_state_preserves_state_after_reordering() {
        check_transfer_both_opt_levels(
            // Run with a=0, b=0, modify both
            "state a = 0\nstate b = 0\na += 10\nb += 20\nprint(a, b)",
            &["10 20"],
            // Transfer onto state declarations in reversed order:
            // both values preserved despite reordering
            "state b = 0\nstate a = 0\nprint(a, b)",
            2,
            0,
            &["10 20"],
        );
    }

    #[test]
    fn transfer_state_fresh_state_gets_initialized() {
        check_transfer_both_opt_levels(
            // Run with one state
            "state x = 10\nprint(x)",
            &["10"],
            // Transfer onto a program that adds a new state variable:
            // x=10 (preserved), y=20 (newly initialized), sum=30
            "state x = 10\nstate y = 20\nprint(x + y)",
            1,
            0,
            &["30"],
        );
    }
}
