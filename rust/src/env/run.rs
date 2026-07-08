//! The run/step loop: the shared driver that dispatches to the bytecode VM and
//! the bounded/unbounded/speculative run entry points built on top of it.
//!
//! Split out of `env/mod.rs`; see that module for the `Env` struct and core
//! accessors. Extra imports here are the ones only this cluster needs.

use super::*;

use crate::backend::StepResult;
use crate::backend::bytecode::Vm;
use crate::stack::StackStatus;

impl Env {
    /// Run one step of execution, dispatching to the active backend. Both
    /// backends return the same [`StepResult`], so the run loops
    /// ([`run`](Self::run) / [`run_bounded`](Self::run_bounded)) are shared.
    pub fn step(&mut self, stack_id: StackKey) -> Result<StepResult, String> {
        Ok(self.step_n(stack_id, 1)?.0)
    }

    /// How many bytecode instructions execute per `Env`-level dispatch. Each
    /// dispatch pays several map lookups plus VM construction — more than a
    /// typical instruction costs to execute — so the run loops hand the VM a
    /// budget and it runs an inner loop, yielding early for GC, completion,
    /// error, or budget exhaustion. The value only caps how stale the GC check
    /// inside a batch can get relative to `run_bounded` budgets; the VM itself
    /// polls `Heap::should_collect` after every instruction.
    const BYTECODE_BATCH: u64 = 65_536;

    /// Run up to `budget` steps, returning the final [`StepResult`] and the
    /// number of steps consumed. The bytecode VM consumes up to the whole
    /// budget in one dispatch.
    fn step_n(
        &mut self,
        stack_id: StackKey,
        budget: u64,
    ) -> Result<(StepResult, u64), String> {
        self.step_bytecode(stack_id, budget)
    }

    /// Run the bytecode VM for up to `budget` instructions. Lowers the program
    /// on first use, pushes the root frame on the first step of a run, then
    /// executes the batch (see [`Vm::run_batch`]).
    fn step_bytecode(
        &mut self,
        stack_id: StackKey,
        budget: u64,
    ) -> Result<(StepResult, u64), String> {
        let ck = self.stacks.get(&stack_id).ok_or("Stack not found")?.context;
        let pid = self.stacks.get(&stack_id).unwrap().program_id;
        self.ensure_bytecode(pid)?;

        let bc = &self.bytecode.get(&pid).unwrap().1;
        let program = self.programs.get(&pid).ok_or("Program not found")?;
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        let ctx = self.contexts.get_mut(&ck).ok_or("Context not found")?;

        let mut vm = make_vm(
            program,
            bc,
            stack,
            ctx,
            &self.native_fns,
            &self.handle_classes,
            &mut self.symbols,
            &mut self.trace,
        );
        if !vm.stack.vm_started {
            vm.push_root_frame();
            vm.stack.vm_started = true;
        }
        Ok(vm.run_batch(budget))
    }

    /// Run a program to completion. Tracks which `RuntimeStateKey`s are
    /// touched and sweeps untouched entries from the persistent state map
    /// when the program completes — so per-iteration state for a removed
    /// list item, or a top-level state declaration deleted on hot reload,
    /// is reclaimed instead of leaking.
    pub fn run(&mut self, stack_id: StackKey) -> Result<Value, String> {
        let ck = self.stacks.get(&stack_id).ok_or("Stack not found")?.context;
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.start_run_tracking();
        }
        loop {
            match self.step_n(stack_id, Self::BYTECODE_BATCH)?.0 {
                StepResult::Continue => {
                    if self.ctx(ck).heap.should_collect() {
                        self.collect_garbage(ck);
                    }
                }
                StepResult::Complete(val) => {
                    if let Some(stack) = self.stacks.get_mut(&stack_id) {
                        stack.sweep_untouched_state();
                    }
                    return Ok(val);
                }
                StepResult::Error(e) => return Err(e),
            }
        }
    }

    /// Run a program for at most `max_steps` evaluation steps.
    ///
    /// The bounded counterpart to [`run`](Self::run), for in-process hosts that
    /// must keep control of the thread — e.g. an editor driving Petal-scripted
    /// panels at ~60fps, where a runaway script (`while true`) would otherwise
    /// hang the UI on the main thread. Returns [`RunOutcome::Done`] with the
    /// program's value if it completes within the budget, or
    /// [`RunOutcome::Yielded`] (carrying the number of steps actually consumed)
    /// if the budget is exhausted first.
    ///
    /// A yielded stack is left runnable: call `run_bounded` again to resume it
    /// from exactly where it stopped (e.g. on the next frame), or give up and
    /// [`reset_stack`](Self::reset_stack) / report an error to the user. Because
    /// resumption re-enters the same eval loop, splitting a run across many
    /// `run_bounded` calls produces the same result as a single [`run`](Self::run).
    ///
    /// Run-state tracking (which drives the untouched-state sweep, see
    /// [`run`](Self::run)) is started once when a fresh or `reset` stack first
    /// enters here, and the sweep happens only on completion — so a resumed run
    /// tracks the same key set a single uninterrupted run would.
    pub fn run_bounded(
        &mut self,
        stack_id: StackKey,
        max_steps: u64,
    ) -> Result<RunOutcome, String> {
        let ck = self.stacks.get(&stack_id).ok_or("Stack not found")?.context;

        // Start run tracking only on a fresh entry, not when resuming a
        // previously-yielded run (which would clear the touched-key set and
        // defeat the sweep). reset_stack/create_stack leave the stack `Ready`.
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            if matches!(stack.status, StackStatus::Ready) {
                stack.start_run_tracking();
                stack.status = StackStatus::Running;
            }
        }

        let mut steps = 0;
        while steps < max_steps {
            let budget = (max_steps - steps).min(Self::BYTECODE_BATCH);
            let (result, consumed) = self.step_n(stack_id, budget)?;
            steps += consumed;
            match result {
                StepResult::Continue => {
                    if self.ctx(ck).heap.should_collect() {
                        self.collect_garbage(ck);
                    }
                }
                StepResult::Complete(val) => {
                    if let Some(stack) = self.stacks.get_mut(&stack_id) {
                        stack.sweep_untouched_state();
                        stack.status = StackStatus::Complete(val);
                    }
                    return Ok(RunOutcome::Done(val));
                }
                StepResult::Error(e) => {
                    if let Some(stack) = self.stacks.get_mut(&stack_id) {
                        stack.status = StackStatus::Error(e.clone());
                    }
                    return Err(e);
                }
            }
        }
        Ok(RunOutcome::Yielded { steps })
    }

    /// Run a program from source directly (convenience method)
    pub fn run_source(&mut self, source: &str) -> Result<Value, String> {
        let pid = self.load_program(source)?;
        let sid = self.create_stack(pid)?;
        self.run(sid)
    }

    /// Call a top-level Petal function by name on an already-run stack,
    /// returning its result. The program must have been `run` at least once so
    /// its top-level functions are defined; the captured function table is
    /// refreshed on every run and dropped on `transfer_state`.
    ///
    /// This is the host-facing alternative to the "re-run the whole program
    /// and capture a side effect in a thread-local" pattern: it invokes one
    /// named function with `args` and hands back the return `Value` directly.
    /// Any heap `Value` passed in `args` must already live on this Env's heap.
    ///
    /// Note on state: a top-level `state` variable referenced inside a function
    /// is captured into its closure by value when the program runs, so a called
    /// function observes that variable as of the last `run` and cannot write it
    /// back into the persistent state map. To feed fresh state into a call, pass
    /// it through `args`, or `run`/`transfer_state` again to recapture.
    pub fn call_function(
        &mut self,
        stack_id: StackKey,
        name: &str,
        args: &[Value],
    ) -> Result<Value, String> {
        let stack = self.stacks.get(&stack_id).ok_or("Stack not found")?;
        let callable = stack.functions.get(name).copied().ok_or_else(|| {
            format!(
                "No top-level function named '{}' (define it and `run` the program before calling)",
                name
            )
        })?;

        let ck = self.stacks.get(&stack_id).ok_or("Stack not found")?.context;
        let pid = self.stacks.get(&stack_id).unwrap().program_id;
        self.ensure_bytecode(pid)?;

        let bc = &self.bytecode.get(&pid).unwrap().1;
        let program = self.programs.get(&pid).ok_or("Program not found")?;
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        let ctx = self.contexts.get_mut(&ck).ok_or("Context not found")?;

        make_vm(
            program,
            bc,
            stack,
            ctx,
            &self.native_fns,
            &self.handle_classes,
            &mut self.symbols,
            &mut self.trace,
        )
        .call_closure_sync(callable, args)
    }

    /// Run one frame without disturbing the source execution at all.
    ///
    /// Implemented on top of `fork_execution`: fork the stack into an isolated
    /// context, run the fork, return its result, then drop the fork. Because the
    /// fork owns its own heap and registries (including a *fresh* output sink),
    /// nothing about the source is touched — not its state, not its heap
    /// objects, and not its print output. (The previous snapshot/restore
    /// implementation left the source stack reset and leaked the speculative
    /// run's print output into the shared buffer; the fork-based version does
    /// neither.) The fork's own side effects are discarded when it is dropped.
    pub fn run_speculative(&mut self, stack_id: StackKey) -> Result<Value, String> {
        let fork = self.fork_execution(stack_id)?;
        self.reset_stack(fork)?;
        let result = self.run(fork);
        self.drop_fork(fork);
        result
    }
}

/// Bundle `Env`'s runtime borrows into a [`Vm`] for one dispatch. The borrows
/// come from disjoint fields of `Env` (plus the resolved program / bytecode /
/// stack / context), so the caller splits them out before calling. Both run
/// paths — [`Env::step_bytecode`] and [`Env::call_function`] — construct an
/// identical VM this way.
fn make_vm<'a>(
    program: &'a Program,
    bc: &'a BytecodeProgram,
    stack: &'a mut Stack,
    ctx: &'a mut ExecutionContext,
    native_fns: &'a NativeFnTable,
    handle_classes: &'a [HandleClass],
    symbols: &'a mut SymbolTable,
    trace: &'a mut TraceBuffer,
) -> Vm<'a> {
    Vm {
        program,
        bc,
        stack,
        heap: &mut ctx.heap,
        closures: &mut ctx.closures,
        overload_sets: &mut ctx.overload_sets,
        native_fns,
        handle_classes,
        output: &mut ctx.output,
        symbols,
        output_buffers: &mut ctx.output_buffers,
        bindings: &mut ctx.bindings,
        counters: &mut ctx.counters,
        trace,
        error_already_annotated: false,
    }
}
