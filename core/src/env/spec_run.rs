//! The `Env` half of a speculative run (docs/dev/direct-manipulation-v2-plan.md
//! §2.1.1): forking a checkpoint into a run-ready stack, and running a fork
//! under a step budget and a wall-clock cap with the Env-wide recorders
//! (trace, observations, profile, static audit) out of the way, so a live
//! host never sees a speculative event. [`crate::dm_v2::spec`] drives these.

use super::*;

use crate::backend::bytecode::vm::spec::ActiveSpec;

/// How a speculative run ended, as the Env sees it.
pub(crate) enum SpecRunEnd {
    Done,
    Error(String),
    StepBudget,
    TimedOut,
}

/// A wall clock, where there is one (`wasm32-unknown-unknown` has none, and a
/// run there has no wall-clock cap, only the step budget).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods, reason = "wasm32 builds take the version below")]
pub(crate) fn wall_now() -> Option<std::time::Instant> {
    Some(std::time::Instant::now())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn wall_now() -> Option<std::time::Instant> {
    None
}

impl Env {
    /// Fork `from` into a stack ready to run one speculative frame: silent
    /// (`print` does not echo), with everything the last run derived dropped
    /// (R4: a static value built from a literal must not outlive an override
    /// of it), and reset to run from the top. With `fresh_state` the fork
    /// starts with no `state` at all, so initializers run.
    pub(crate) fn spec_fork(&mut self, from: StackKey, fresh_state: bool) -> Result<StackKey, String> {
        let fork = self.fork_execution(from)?;
        let ck = self.ctx_for(fork).ok_or("Stack not found")?;
        self.ctx_mut(ck).set_echo(false);
        let stack = self.stacks.get_mut(&fork).ok_or("Stack not found")?;
        stack.drop_derived("a speculative run forked the stack");
        if fresh_state {
            stack.state.clear();
        }
        self.reset_stack(fork)?;
        Ok(fork)
    }

    /// [`spec_fork`](Self::spec_fork) onto another program: the fork keeps
    /// `from`'s context (bindings, random stream, heap) and the state the
    /// other program still declares, and runs `program`. The Recompile
    /// oracle (§2.1.8) runs an edited copy of the source this way.
    pub(crate) fn spec_fork_onto(
        &mut self,
        from: StackKey,
        program: ProgramId,
        fresh_state: bool,
    ) -> Result<StackKey, String> {
        let fork = self.spec_fork(from, fresh_state)?;
        let keys: std::collections::HashSet<crate::program::StateKey> = self
            .get_program(program)
            .ok_or("Program not found")?
            .state_terms()
            .map(|(k, _)| k)
            .collect();
        let ck = self.ctx_for(fork).ok_or("Stack not found")?;
        // Closures point into the program that made them.
        self.ctx_mut(ck).closures.clear();
        let stack = self.stacks.get_mut(&fork).ok_or("Stack not found")?;
        stack.program_id = program;
        crate::transfer_state::transfer_stack_state(stack, &keys);
        Ok(fork)
    }

    /// Run `fork` from the top under `hooks`, for at most `max_steps`
    /// instructions and (where there is a clock) `max_wall`. Returns how it
    /// ended, the hooks with what they recorded, the instructions it took and
    /// the time. The Env's trace, observations, profile and static audit are
    /// swapped for idle ones for the duration and put back after, so the
    /// live trace ring and observation buffer are exactly as they were.
    pub(crate) fn run_spec_fork(
        &mut self,
        fork: StackKey,
        hooks: Box<crate::backend::bytecode::vm::spec::SpecHooks>,
        max_steps: u64,
        max_wall: std::time::Duration,
    ) -> (SpecRunEnd, Box<crate::backend::bytecode::vm::spec::SpecHooks>, u64, std::time::Duration) {
        let trace = std::mem::take(&mut self.trace);
        let observations = std::mem::take(&mut self.observations);
        let profile = std::mem::replace(&mut self.profile, VmProfile::new());
        let static_audit = std::mem::replace(&mut self.static_audit, crate::static_audit::StaticAudit::new());
        let outer = self.spec.replace(ActiveSpec { stack: fork, hooks });

        let start = wall_now();
        let insts_before = self.stacks.get(&fork).map_or(0, |s| s.insts);
        let mut used: u64 = 0;
        let end = loop {
            let remaining = max_steps.saturating_sub(used);
            if remaining == 0 {
                break SpecRunEnd::StepBudget;
            }
            match self.run_bounded(fork, remaining.min(1 << 18)) {
                Ok(RunOutcome::Done(_)) => break SpecRunEnd::Done,
                Ok(RunOutcome::Yielded { steps }) => {
                    used += steps.max(1);
                    if let Some(t) = start
                        && t.elapsed() > max_wall
                    {
                        break SpecRunEnd::TimedOut;
                    }
                }
                Err(e) => break SpecRunEnd::Error(e),
            }
        };
        let elapsed = start.map(|t| t.elapsed()).unwrap_or_default();
        let steps = self.stacks.get(&fork).map_or(0, |s| s.insts).saturating_sub(insts_before);

        let hooks = std::mem::replace(&mut self.spec, outer)
            .expect("the speculative run's hooks are still installed")
            .hooks;
        self.trace = trace;
        self.observations = observations;
        self.profile = profile;
        self.static_audit = static_audit;
        (end, hooks, steps, elapsed)
    }

    /// How many constants `pid`'s table holds (the override table's size).
    pub(crate) fn constants_len(&self, pid: ProgramId) -> usize {
        self.programs.get(&pid).map_or(0, |p| p.constants.len())
    }

    /// The program a stack runs, if the stack exists.
    pub(crate) fn stack_program(&self, stack: StackKey) -> Option<ProgramId> {
        self.stacks.get(&stack).map(|s| s.program_id)
    }

    /// The handle classes marked safe for speculative runs.
    pub(crate) fn spec_safe_handle_classes(&self) -> std::collections::HashSet<u16> {
        self.spec_safe_handles.clone()
    }
}
