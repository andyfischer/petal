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
    fn step_n(&mut self, stack_id: StackKey, budget: u64) -> Result<(StepResult, u64), String> {
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
        // Observed values are ids into *this* context's heap; moving execution
        // to another context invalidates whatever is buffered. Cheap: one
        // `Option<ContextKey>` compare per batch dispatch.
        self.observations.enter_context(ck);
        if self.static_audit.enabled {
            self.static_audit.enter(pid);
        }

        let memo = self.memo_enabled();
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
            &mut self.observations,
            &mut self.profile,
            memo,
            &mut self.effect_audit,
            &mut self.static_audit,
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
        // Static values a drag left stale are rebuilt by the first run after
        // it pauses (`Env::set_static_debounce`).
        if self.static_rebuilds_due(stack_id) {
            self.flush_static_rebuilds(stack_id);
        }
        self.begin_run_deps(stack_id, ck);
        // Each run reports its own bindings: clear at the start, so a name the
        // program no longer reaches can't report a value from a previous run.
        self.observations.start_run(ck);
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
                        stack.memo.sweep();
                    }
                    self.finish_run_deps(stack_id, ck);
                    self.announce_static_audit();
                    return Ok(val);
                }
                StepResult::Error(e) => {
                    // A failed run still has a read-set: re-running it with
                    // the same inputs would fail the same way, so the host
                    // may skip until something it read moves.
                    self.finish_run_deps(stack_id, ck);
                    self.note_static_audit_error(stack_id, &e);
                    return Err(e);
                }
            }
        }
    }

    /// A run ended in an error. Under the static audit's static-only build
    /// that is a finding: the statement that raised was called static.
    fn note_static_audit_error(&mut self, stack_id: StackKey, error: &str) {
        if !self.static_audit.enabled {
            return;
        }
        let ip = self
            .stacks
            .get(&stack_id)
            .and_then(|s| s.vm_frames.first())
            .filter(|f| f.func.is_none())
            .and_then(|f| f.ip.checked_sub(1));
        self.static_audit.raised(ip, error);
        self.announce_static_audit();
    }

    /// Under `PETAL_STATIC_AUDIT`, print the findings the run that just
    /// ended added (see [`crate::static_audit`]).
    fn announce_static_audit(&mut self) {
        if !self.static_audit.enabled {
            return;
        }
        let Some(from) = self.static_audit.unannounced() else { return };
        let report = self.static_audit.report_from(
            from,
            |pid| self.programs.get(&pid),
            &self.native_fns,
            &self.symbols,
        );
        for finding in &report.findings {
            eprintln!("static audit failure: {finding}");
        }
    }

    /// Start the dependency record of a run (see [`crate::run_deps`]).
    fn begin_run_deps(&mut self, stack_id: StackKey, ck: ContextKey) {
        let rng = self.ctx(ck).rng_state;
        if let (Some(stack), Some(ctx)) =
            (self.stacks.get_mut(&stack_id), self.contexts.get_mut(&ck))
        {
            stack.begin_run_deps(&mut ctx.heap, rng);
        }
    }

    /// Complete the dependency record of a run that ended.
    fn finish_run_deps(&mut self, stack_id: StackKey, ck: ContextKey) {
        if let (Some(stack), Some(ctx)) = (self.stacks.get_mut(&stack_id), self.contexts.get(&ck)) {
            stack.finish_run_deps(
                &ctx.bindings,
                &ctx.binding_revisions,
                &ctx.heap,
                &ctx.closures,
                ctx.rng_state,
                ctx.resources.revision(),
            );
        }
    }

    /// Whether running `stack_id` now could produce a different result from
    /// its last run — and if so, why. `None` means every binding the last run
    /// read still holds the value it read, the run consumed no randomness,
    /// consulted no host data the host has since reported changed, left every
    /// `state` slot as it found it, and no pending resource has moved: running
    /// again would reproduce the last run's output exactly, so a host may skip
    /// the frame and keep what it drew.
    ///
    /// Ask *after* binding the frame's inputs and *before* `reset_stack`/`run`.
    /// A stack that has never completed a run always needs one. See
    /// [`crate::run_deps`] for what is tracked and
    /// [`RunReason`](crate::run_deps::RunReason) for the answers.
    pub fn run_needed_reason(&self, stack_id: StackKey) -> Option<crate::run_deps::RunReason> {
        let Some(stack) = self.stacks.get(&stack_id) else {
            return Some(crate::run_deps::RunReason::NoRecord);
        };
        let Some(ctx) = self.contexts.get(&stack.context) else {
            return Some(crate::run_deps::RunReason::NoRecord);
        };
        // A drag paused with rebuilds waiting: the run that makes them.
        if self.static_rebuilds_due(stack_id) {
            return Some(crate::run_deps::RunReason::Forced);
        }
        stack
            .run_deps
            .run_needed(&ctx.bindings, &ctx.binding_revisions, &ctx.heap, ctx.resources.revision())
    }

    /// [`run_needed_reason`](Self::run_needed_reason) as a plain yes/no.
    pub fn run_needed(&self, stack_id: StackKey) -> bool {
        self.run_needed_reason(stack_id).is_some()
    }

    /// Ask that the next [`run_needed`](Self::run_needed) answer yes, whatever
    /// the recorded dependencies say. For a host that changed something the
    /// record cannot see: replaced a data provider, edited a document a native
    /// reads, or wants a frame for its own reasons (a debug step). Cleared by
    /// the next run.
    pub fn invalidate_run(&mut self, stack_id: StackKey) {
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.run_deps.force();
        }
    }

    /// Tell the gate that host-owned data behind a `host_data`-style native
    /// changed. Cheaper than [`invalidate_run`](Self::invalidate_run) when the
    /// host does not know whether the script reads such data: a run that never
    /// called a native that [`note_host_read`](crate::native_fn::PetalCxt::note_host_read)
    /// is unaffected.
    pub fn note_host_data_changed(&mut self, stack_id: StackKey) {
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.run_deps.note_host_data_changed();
        }
    }

    /// The dependency record of `stack_id`'s most recent run, for hosts and
    /// tools that want more than the yes/no (which bindings were read, whether
    /// host data was consulted).
    pub fn run_deps(&self, stack_id: StackKey) -> Option<&crate::run_deps::RunDeps> {
        self.stacks.get(&stack_id).map(|s| &s.run_deps)
    }

    /// Whether runs memoize user-function calls right now: the
    /// [`RunPolicy::memo`](crate::policy::RunPolicy::memo) switch,
    /// unless the `explain` trace is on — a trace of a replayed scope would
    /// have no instructions in it, and the tools reading it want every one.
    pub fn memo_enabled(&self) -> bool {
        self.policy.memo && !self.trace.enabled
    }

    /// The memo counters of `stack_id`'s table (hits, misses, records, …),
    /// cumulative since the stack was created.
    pub fn memo_stats(&self, stack_id: StackKey) -> Option<crate::memo::MemoStats> {
        self.stacks.get(&stack_id).map(|s| s.memo.stats)
    }

    /// `stack_id`'s enum-data revision: bumped when a run publishes constant
    /// data for a variant that differs from what the run before published
    /// (see [`Stack::enum_data_revision`](crate::stack::Stack)). Constant
    /// data leaves it where it is, run after run.
    pub fn enum_data_revision(&self, stack_id: StackKey) -> Option<u64> {
        self.stacks.get(&stack_id).map(|s| s.enum_data_revision)
    }

    /// How many scope records `stack_id`'s table holds.
    pub fn memo_slots(&self, stack_id: StackKey) -> usize {
        self.stacks.get(&stack_id).map_or(0, |s| s.memo.len())
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
        let mut fresh_entry = false;
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            if matches!(stack.status, StackStatus::Ready) {
                stack.start_run_tracking();
                stack.status = StackStatus::Running;
                fresh_entry = true;
            }
        }
        if fresh_entry {
            self.begin_run_deps(stack_id, ck);
        }
        // Observations follow the same fresh-entry rule, and for the same
        // reason: a resume is the *middle* of one run, so clearing here would
        // discard everything observed before the yield. A host driving a frame
        // in small budgets (an editor at 60fps) takes the resume path on nearly
        // every call.
        if fresh_entry {
            self.observations.start_run(ck);
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
                        stack.memo.sweep();
                        stack.status = StackStatus::Complete(val);
                    }
                    self.finish_run_deps(stack_id, ck);
                    self.announce_static_audit();
                    return Ok(RunOutcome::Done(val));
                }
                StepResult::Error(e) => {
                    if let Some(stack) = self.stacks.get_mut(&stack_id) {
                        stack.status = StackStatus::Error(e.clone());
                    }
                    self.finish_run_deps(stack_id, ck);
                    self.note_static_audit_error(stack_id, &e);
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
    ///
    /// A host call runs with no caller frame, so the function's own `state`
    /// declarations are keyed under a root path derived from the name called
    /// (docs/dev/state-call-paths.md §2.5): repeated host calls of one function
    /// share their slots with each other, but not with any in-program call of
    /// the same function. The workaround, as always, is a top-level `state var`.
    ///
    /// The error is a string; [`call_function_diag`](Self::call_function_diag)
    /// is the same call with a typed error that tells "no such function"
    /// apart from "it ran and failed".
    pub fn call_function(
        &mut self,
        stack_id: StackKey,
        name: &str,
        args: &[Value],
    ) -> Result<Value, String> {
        self.call_function_diag(stack_id, name, args)
            .map_err(String::from)
    }

    /// [`call_function`](Self::call_function) with a typed
    /// [`CallError`](crate::error::CallError): a host calling an optional
    /// hook (`on_key`, `update`) matches
    /// [`FunctionNotFound`](crate::error::CallError::FunctionNotFound) instead
    /// of sniffing the message. Whether a function exists can also be asked
    /// up front with [`has_function`](Self::has_function).
    pub fn call_function_diag(
        &mut self,
        stack_id: StackKey,
        name: &str,
        args: &[Value],
    ) -> Result<Value, crate::error::CallError> {
        self.call_function_named(stack_id, name, args, &[])
    }

    /// [`call_function_diag`](Self::call_function_diag) with some arguments
    /// given by parameter name, which is how a host calls a function with a
    /// record of arguments (a JSON object from a tool call, say).
    ///
    /// `arg_names` is parallel to `args` — `None` for a positional argument,
    /// and every positional one before every named one, as in a written call
    /// `f(1, y: 2)` — or empty when nothing is named. Binding is the
    /// language's own: a parameter with a default may be left out, and an
    /// unknown name, a parameter given twice or a required one missing is a
    /// [`Runtime`](crate::error::CallError::Runtime) error naming it.
    pub fn call_function_named(
        &mut self,
        stack_id: StackKey,
        name: &str,
        args: &[Value],
        arg_names: &[Option<&str>],
    ) -> Result<Value, crate::error::CallError> {
        use crate::error::CallError;
        let stack = self
            .stacks
            .get(&stack_id)
            .ok_or(CallError::StackNotFound)?;
        let callable = stack.functions.get(name).copied().ok_or_else(|| {
            CallError::FunctionNotFound {
                name: name.to_string(),
            }
        })?;

        let ck = stack.context;
        let pid = stack.program_id;
        self.ensure_bytecode(pid).map_err(CallError::Runtime)?;
        self.observations.enter_context(ck);
        if self.static_audit.enabled {
            self.static_audit.enter(pid);
        }

        let bc = &self.bytecode.get(&pid).unwrap().1;
        let program = self
            .programs
            .get(&pid)
            .ok_or_else(|| CallError::Runtime("Program not found".into()))?;
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        let ctx = self
            .contexts
            .get_mut(&ck)
            .ok_or_else(|| CallError::Runtime("Context not found".into()))?;

        // A call that fails stops where it failed, with its frames still
        // pushed (a run is the same, and the next run's reset clears them).
        // Nothing resets between two host calls, so they are dropped here:
        // left in place they would sit under every later call, be listed in
        // its stack trace, and count against its depth limit.
        let base_depth = stack.vm_frames.len();

        // A host call runs outside any frame's run: nothing it does is
        // recorded into a scope, and nothing is replayed for it.
        let result = make_vm(
            program,
            bc,
            stack,
            ctx,
            &self.native_fns,
            &self.handle_classes,
            &mut self.symbols,
            &mut self.trace,
            &mut self.observations,
            &mut self.profile,
            false,
            &mut self.effect_audit,
            &mut self.static_audit,
        )
        .call_closure_sync_named(callable, args, arg_names, host_call_site(name));
        if result.is_err()
            && let Some(stack) = self.stacks.get_mut(&stack_id)
        {
            stack.vm_frames.truncate(base_depth);
            stack.last_pop_result = None;
        }
        // The call may have written state the next run reads; the run's
        // dependency record cannot tell, so the next frame must run.
        self.invalidate_run(stack_id);
        result.map_err(CallError::Runtime)
    }

    /// The parameters of the top-level function [`call_function`](Self::call_function)
    /// would call, as its source declares them: what a host needs to check or
    /// convert arguments that arrive loosely typed (JSON). `None` when there
    /// is no such function, or it has no single declaration to read (an
    /// overload set, a builtin). A parameter's `ty` is `None` when it has no
    /// annotation or the program has no source text.
    pub fn function_params(&self, stack_id: StackKey, name: &str) -> Option<Vec<FunctionParam>> {
        let stack = self.stacks.get(&stack_id)?;
        let Value::Closure(cid) = stack.functions.get(name).copied()? else {
            return None;
        };
        let ctx = self.contexts.get(&stack.context)?;
        let program = self.programs.get(&stack.program_id)?;
        let fid = ctx.closures.closure(cid).function_id;
        let def = program.functions.get(fid.0 as usize)?;
        let meta = program.introspect.meta(program, fid);
        let required = def.required_params();
        Some(
            def.params
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let pm = meta.as_ref().and_then(|m| m.params.get(i));
                    FunctionParam {
                        name: name.clone(),
                        ty: pm.and_then(|p| p.ty.clone()),
                        has_default: i >= required,
                        default_source: pm.and_then(|p| p.default_source.clone()),
                    }
                })
                .collect(),
        )
    }

    /// Whether [`call_function`](Self::call_function) would find a top-level
    /// function named `name` on this stack — i.e. the stack's last run
    /// defined it. `false` for an unknown stack.
    pub fn has_function(&self, stack_id: StackKey, name: &str) -> bool {
        self.stacks
            .get(&stack_id)
            .is_some_and(|s| s.functions.contains_key(name))
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
    ///
    /// Silencing stdout is this function's policy, not `fork_execution`'s: a
    /// speculative run's output is thrown away, so echoing it would print lines
    /// the program never committed. A host driving a fork by hand keeps whatever
    /// echo setting the source had.
    pub fn run_speculative(&mut self, stack_id: StackKey) -> Result<Value, String> {
        let fork = self.fork_execution(stack_id)?;
        let ck = self.ctx_for(fork).ok_or("Stack not found")?;
        self.ctx_mut(ck).set_echo(false);
        self.reset_stack(fork)?;
        let result = self.run(fork);
        self.drop_fork(fork);
        result
    }
}

/// One parameter of a function, as declared (see [`Env::function_params`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionParam {
    pub name: String,
    /// The type annotation's name exactly as written (`float`, `Kind`).
    pub ty: Option<String>,
    /// Whether a call may leave the parameter out.
    pub has_default: bool,
    /// The source text of the default expression.
    pub default_source: Option<String>,
}

/// The root path part for a host-invoked function (`Env::call_function`).
/// Derived from the name the host asked for, so every host call of one function
/// lands on the same slots; the `host ` marker (a space cannot appear in an
/// identifier) keeps it disjoint from every compiled callsite id.
pub(super) fn host_call_site(name: &str) -> u64 {
    crate::compiler::Compiler::hash_state_name(&format!("host {name}"))
}

/// Bundle `Env`'s runtime borrows into a [`Vm`] for one dispatch. The borrows
/// come from disjoint fields of `Env` (plus the resolved program / bytecode /
/// stack / context), so the caller splits them out before calling. Both run
/// paths — [`Env::step_bytecode`] and [`Env::call_function`] — construct an
/// identical VM this way.
pub(super) fn make_vm<'a>(
    program: &'a Program,
    bc: &'a BytecodeProgram,
    stack: &'a mut Stack,
    ctx: &'a mut ExecutionContext,
    native_fns: &'a NativeFnTable,
    handle_classes: &'a [HandleClass],
    symbols: &'a mut SymbolTable,
    trace: &'a mut TraceBuffer,
    observations: &'a mut Observations,
    profile: &'a mut crate::profile::VmProfile,
    memo: bool,
    audit: &'a mut crate::effect_audit::EffectAudit,
    static_audit: &'a mut crate::static_audit::StaticAudit,
) -> Vm<'a> {
    // Read the Copy fields before splitting `ctx`'s fields into disjoint borrows.
    let frame = ctx.frame();
    let trace_pending = ctx.trace_pending;
    let trace_emit = ctx.trace_emit;
    // One flag standing for "some per-instruction hook is armed" (see `Vm::hooks`).
    let hooks = trace.enabled || observations.enabled || profile.enabled;
    // The stack's enum table is the one for this lowering's layout: a pointer
    // comparison, except on the first dispatch after a load or a reload.
    stack.enums.sync(&bc.enum_layout, &mut ctx.heap);
    // Likewise the static values: two comparisons unless the lowering or a
    // literal changed under the stack, and then it starts empty.
    stack.statics.sync(&bc.statics, program.constants.revision());
    stack.statics.set_check(static_audit.check_kept);
    stack.statics.set_check_announced(static_audit.announce_check);
    Vm {
        program,
        bc,
        stack,
        heap: &mut ctx.heap,
        closures: &mut ctx.closures,
        native_fns,
        handle_classes,
        output: &mut ctx.output,
        symbols,
        output_buffers: &mut ctx.output_buffers,
        trace_emit,
        emit_origins: &mut ctx.emit_origins,
        bindings: &mut ctx.bindings,
        binding_revisions: &ctx.binding_revisions,
        counters: &mut ctx.counters,
        rng_state: &mut ctx.rng_state,
        noise_seed: &mut ctx.noise_seed,
        frame,
        resources: &mut ctx.resources,
        trace_pending,
        absorption_log: &mut ctx.absorption_log,
        echo: ctx.echo,
        trace,
        observations,
        profile,
        hooks,
        error_already_annotated: false,
        memo,
        audit,
        static_audit,
        expects: &mut ctx.expects,
    }
}
