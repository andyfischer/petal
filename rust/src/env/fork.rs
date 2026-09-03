//! Forked (side-by-side) execution: cloning a stack into an isolated context,
//! dropping a fork, and value-diffing two executions' committed state.
//!
//! Split out of `env/mod.rs`; see that module for the `Env` struct and core
//! accessors. `run_speculative` (built on `fork_execution`) lives in
//! `env::run` next to the other run entry points.

use super::*;

impl Env {
    /// Drop a forked execution: remove its stack and, if no other stack still
    /// references it, its (exclusively-owned, non-default) context — releasing
    /// the forked heap and registries.
    ///
    /// A host that holds a fork open for side-by-side comparison calls this to
    /// release it once done (the source stack/context is left untouched). Safe
    /// to call on the default context's stacks too: a stack bound to the default
    /// context is removed but the shared default context is never dropped.
    pub fn drop_fork(&mut self, stack_id: StackKey) {
        if let Some(stack) = self.stacks.remove(&stack_id) {
            let ck = stack.context;
            if ck != self.default_context && !self.stacks.values().any(|s| s.context == ck) {
                self.contexts.remove(&ck);
            }
        }
    }

    /// Fork an execution into a fully isolated side-by-side copy. The new stack
    /// gets its own [`ExecutionContext`] (heap + registries deep-cloned, output
    /// sinks fresh) and a clone of the source stack's frames/state, so the two
    /// share no mutable heap state: the fork can advance freely without
    /// disturbing the source, and vice versa. Pre-fork heap ids resolve to equal
    /// objects in both contexts. This is the public API the host/CLI/WASM will
    /// build speculative side-by-side runs on. See
    /// the "Speculative execution" section of docs/program-modification.md.
    pub fn fork_execution(&mut self, src: StackKey) -> Result<StackKey, String> {
        // Read the source's context key (and validate the stack exists).
        let src_ck = self.stacks.get(&src).ok_or("Stack not found")?.context;

        // Fork the source context into a fresh context key.
        let new_ck = ContextKey(self.next_context_id);
        self.next_context_id += 1;
        let forked = self
            .contexts
            .get(&src_ck)
            .ok_or("Context not found")?
            .fork();
        self.contexts.insert(new_ck, forked);

        // Clone the source stack into a fresh stack key, rebinding it to the new
        // context.
        let new_key = StackKey(self.next_stack_id);
        self.next_stack_id += 1;
        let mut new_stack = self.stacks.get(&src).ok_or("Stack not found")?.clone();
        new_stack.id = new_key;
        new_stack.context = new_ck;
        self.stacks.insert(new_key, new_stack);

        Ok(new_key)
    }

    /// Restore a live execution from a fork taken earlier: the inverse of
    /// [`fork_execution`](Self::fork_execution). `dst`'s context (heap,
    /// registries, bindings, RNG, resources) becomes a fresh copy of `src`'s,
    /// and `dst`'s stack takes `src`'s state and frames — under `dst`'s own
    /// keys, so everything a host holds (`stack_id`, the default context)
    /// stays valid. `src` is untouched and can be restored from again.
    ///
    /// This is what a rewind is: a host that forks after every frame keeps a
    /// history of executions, and restoring one puts the program back at that
    /// moment. The restored stack is reshaped onto the program that is loaded
    /// *now* — state whose declaration no longer exists is dropped, captured
    /// closures are recaptured on the next run — so a snapshot taken before a
    /// hot reload restores cleanly into the edited program.
    pub fn restore_execution(&mut self, dst: StackKey, src: StackKey) -> Result<(), String> {
        let dst_ck = self.stacks.get(&dst).ok_or("Stack not found")?.context;
        let src_ck = self.stacks.get(&src).ok_or("Source stack not found")?.context;
        let program_id = self.stacks.get(&dst).ok_or("Stack not found")?.program_id;

        let mut ctx = self.contexts.get(&src_ck).ok_or("Source context not found")?.fork();
        // Closures point into the program that captured them; the program
        // running now may be a different one, so recapture on the next run
        // (exactly what `transfer_state` does on a reload).
        ctx.closures.clear();
        self.contexts.insert(dst_ck, ctx);

        let mut stack = self.stacks.get(&src).ok_or("Source stack not found")?.clone();
        stack.id = dst;
        stack.context = dst_ck;
        stack.program_id = program_id;
        let keys: std::collections::HashSet<crate::program::StateKey> = self
            .get_program(program_id)
            .map(|p| p.state_terms().map(|(k, _)| k).collect())
            .unwrap_or_default();
        crate::transfer_state::transfer_stack_state(&mut stack, &keys);
        self.stacks.insert(dst, stack);
        Ok(())
    }

    /// Diff two executions' committed state by *value* (never by heap id — see
    /// hazard 4: ids are not comparable across contexts). Each stack's state is
    /// rendered to JSON against its own context heap, then compared key-by-key;
    /// only differing or one-sided variables are returned. This is the
    /// side-by-side comparison primitive a host uses after running a fork:
    /// `diff_state(pid, source, fork)`. `program_id` supplies the state-key →
    /// name mapping (both stacks should share the same program).
    pub fn diff_state(
        &self,
        program_id: ProgramId,
        source: StackKey,
        fork: StackKey,
    ) -> Vec<StateDiff> {
        let a = self.get_state_json(program_id, source);
        let b = self.get_state_json(program_id, fork);
        let mut names: Vec<&String> = a.keys().chain(b.keys()).collect();
        names.sort_unstable();
        names.dedup();
        names
            .into_iter()
            .filter_map(|name| {
                let av = a.get(name);
                let bv = b.get(name);
                if av == bv {
                    None
                } else {
                    Some(StateDiff {
                        name: name.clone(),
                        source: av.cloned(),
                        fork: bv.cloned(),
                    })
                }
            })
            .collect()
    }
}
