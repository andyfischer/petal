//! Transfer a running stack's state onto a different program.
//!
//! Replaces the program backing a running stack with a new compiled program,
//! preserving state values with matching StateKeys across the transfer.
//! Hot-reloading is one use of this, but it can reshape a stack for any new
//! program that shares the same StateKeys.

use std::collections::HashSet;

use crate::env::Env;
use crate::program::{Program, StateKey};
use crate::stack::Stack;

/// Result of transferring a stack's state onto a new program.
pub struct TransferStateResult {
    /// Number of state values preserved across the transfer.
    pub state_preserved: usize,
    /// Number of state values dropped (no matching key in new program).
    pub state_dropped: usize,
}

/// Reshape one stack for a program whose state declarations are `new_state_keys`:
/// keep the state values that still have a declaration, drop the rest, and
/// restart execution. Returns the preserved/dropped counts.
///
/// This is the whole stack-local half of the transfer, taking the `&mut Stack`
/// directly — no `Env`, no program table, no stack lookup that can fail — so it
/// is testable against a bare `Stack`. [`Env::transfer_state`] wraps it with the
/// Env-level half (swapping the program in and invalidating closures).
pub fn transfer_stack_state(
    stack: &mut Stack,
    new_state_keys: &HashSet<StateKey>,
) -> TransferStateResult {
    // Match on the base key: a runtime key also carries the call path that
    // reached it, which the new program's declarations say nothing about. Paths
    // that no longer occur are swept by the untouched-key GC after the next run.
    let preserved = stack
        .state
        .keys()
        .filter(|k| new_state_keys.contains(&k.base))
        .count();
    let dropped = stack.state.len() - preserved;

    stack.state.retain(|k, _| new_state_keys.contains(&k.base));
    stack.reset_execution();
    // The old captured closures point into the caller's now-cleared closures
    // vec; they get recaptured on the next run.
    stack.functions.clear();
    stack.methods.clear();
    // `reset_execution` cleared `vm_started`, so the VM re-pushes its root frame
    // (against the new program's lowering) on the next run.

    TransferStateResult {
        state_preserved: preserved,
        state_dropped: dropped,
    }
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
        let old_program_id = self.stack(stack_id).ok_or("Stack not found")?.program_id;
        let new_state_keys: HashSet<StateKey> = new_program.state_terms().map(|(k, _)| k).collect();

        self.insert_program(old_program_id, new_program);
        // Closures reference the old program's function defs.
        self.clear_closures();

        let stack = self.stack_mut(stack_id).expect("stack found above");
        Ok(transfer_stack_state(stack, &new_state_keys))
    }
}

/// Unit tests for the stack-local half, driven against a bare `Stack` — no
/// program, no Env, no run loop. The `transfer_state_*` tests below cover the
/// same reshaping end-to-end through real programs; these pin the reshaping
/// rules themselves, including cases (loop-indexed keys, a mid-run stack with
/// captured functions) that are awkward to set up through a source program.
#[cfg(test)]
mod stack_state_tests {
    use super::*;
    use crate::execution_context::ContextKey;
    use crate::program::ProgramId;
    use crate::stack::{RuntimeStateKey, StackKey, StackStatus};
    use crate::value::Value;

    /// A stack holding one state entry per given base key, all on the root path.
    fn stack_with_state(keys: &[u64]) -> Stack {
        let mut stack = Stack::new(StackKey(0), ProgramId(0), ContextKey(0));
        for (i, k) in keys.iter().enumerate() {
            stack.state.insert(key(*k), Value::Int(i as i64));
        }
        stack
    }

    fn key(base: u64) -> RuntimeStateKey {
        RuntimeStateKey {
            base: StateKey(base),
            path: Default::default(),
        }
    }

    fn keys(bases: &[u64]) -> HashSet<StateKey> {
        bases.iter().copied().map(StateKey).collect()
    }

    #[test]
    fn state_with_a_matching_declaration_survives_and_the_rest_is_dropped() {
        let mut stack = stack_with_state(&[1, 2, 3]);

        // Key 2 has no declaration in the new program; keys 1 and 3 do. Key 9 is
        // newly declared and simply has nothing to preserve yet.
        let result = transfer_stack_state(&mut stack, &keys(&[1, 3, 9]));

        assert_eq!(result.state_preserved, 2);
        assert_eq!(result.state_dropped, 1);
        assert_eq!(stack.state.len(), 2);
        assert!(stack.state.contains_key(&key(1)));
        assert!(stack.state.contains_key(&key(3)));
        assert!(!stack.state.contains_key(&key(2)));
    }

    /// Keys are matched on `base` alone — the path is an opaque tail — so every
    /// path's entry for a surviving declaration is kept, and each counts once
    /// toward `state_preserved`.
    #[test]
    fn loop_indexed_entries_follow_their_base_declaration() {
        let mut stack = Stack::new(StackKey(0), ProgramId(0), ContextKey(0));
        for i in 0..3usize {
            let mut k = key(1);
            k.path.push(crate::stack::PathPart::Index(i));
            stack.state.insert(k, Value::Int(i as i64));
        }

        let result = transfer_stack_state(&mut stack, &keys(&[1]));

        assert_eq!(result.state_preserved, 3);
        assert_eq!(result.state_dropped, 0);
        assert_eq!(stack.state.len(), 3);
    }

    /// The same rule over the *other* two path parts: a `Call` chain and an
    /// absolute `Key` are as opaque to the transfer as a loop index is. The
    /// declaration surviving the edit is the whole test — which callsites
    /// reached it, and whether they still exist in the new program, is decided
    /// by the untouched-key sweep after the next run, not here.
    #[test]
    fn call_and_key_paths_are_opaque_to_the_transfer_too() {
        use crate::stack::PathPart;

        let mut stack = Stack::new(StackKey(0), ProgramId(0), ContextKey(0));
        let paths: [&[PathPart]; 4] = [
            &[],
            &[PathPart::Call(0xaa)],
            &[
                PathPart::Index(2),
                PathPart::Call(0xbb),
                PathPart::Call(0xbb),
            ],
            &[PathPart::Key(0xcc)],
        ];
        for (i, path) in paths.iter().enumerate() {
            let mut k = key(1);
            k.path.extend_from_slice(path);
            stack.state.insert(k, Value::Int(i as i64));
        }
        // A second declaration, on one path, that the new program has dropped.
        let mut gone = key(2);
        gone.path.push(PathPart::Call(0xdd));
        stack.state.insert(gone, Value::Int(99));

        let result = transfer_stack_state(&mut stack, &keys(&[1]));

        assert_eq!(result.state_preserved, 4, "every path of base 1 survives");
        assert_eq!(result.state_dropped, 1, "base 2 has no declaration left");
        assert_eq!(stack.state.len(), 4);
        assert!(stack.state.keys().all(|k| k.base == StateKey(1)));
    }

    /// A program with no `state` declarations drops everything.
    #[test]
    fn an_empty_declaration_set_drops_all_state() {
        let mut stack = stack_with_state(&[1, 2]);

        let result = transfer_stack_state(&mut stack, &HashSet::new());

        assert_eq!(result.state_preserved, 0);
        assert_eq!(result.state_dropped, 2);
        assert!(stack.state.is_empty());
    }

    /// Execution is restarted and the caches that point into the old program —
    /// captured functions and methods — are invalidated, whatever the state did.
    #[test]
    fn execution_restarts_and_program_bound_caches_are_cleared() {
        let mut stack = stack_with_state(&[1]);
        stack.functions.insert("main".to_string(), Value::Int(0));
        stack
            .methods
            .entry("Rect".to_string())
            .or_default()
            .insert("area".to_string(), Value::Int(0));
        stack.vm_started = true;
        stack.status = StackStatus::Running;

        transfer_stack_state(&mut stack, &keys(&[1]));

        assert!(stack.functions.is_empty(), "captured functions invalidated");
        assert!(stack.methods.is_empty(), "captured methods invalidated");
        assert!(!stack.vm_started, "the VM re-pushes its root frame");
        assert!(stack.vm_frames.is_empty());
        assert!(matches!(stack.status, StackStatus::Ready));
        // The surviving state is untouched by the reset.
        assert_eq!(stack.state.get(&key(1)), Some(&Value::Int(0)));
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
        assert_eq!(
            result.state_preserved, expect_preserved,
            "[{opts:?}] preserved"
        );
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
    fn transfer_state_preserves_a_pathed_slot_per_callsite() {
        // Hot reload with in-function state: one declaration, two callsites, so
        // two entries — both preserved, both resumed on their own path. The edit
        // between the versions touches neither callsite's spelling nor its
        // ordinal, which is exactly the stability the callsite hash promises
        // (plan §3.1).
        check_transfer_both_opt_levels(
            "\
fn counter()
  state n = 0
  n += 1
  n
end
print(counter())
print(counter())
",
            &["1", "1"],
            "\
fn counter()
  state n = 0
  n += 1
  n
end
let untouched = 1 + 1
print(counter())
print(counter())
",
            2,
            0,
            &["2", "2"],
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
