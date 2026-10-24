//! Running a program's test blocks (`test "title" do … end`), for `petal
//! test` and for a host that runs them with its own natives present.
//!
//! A test block compiles to a top-level function of no parameters that
//! nothing calls ([`crate::ast::Stmt::test`]); the program's table of them is
//! [`Env::tests`]. Running one is a host call of that function with two
//! differences from [`Env::call_function`]:
//!
//! - **Its expectations are its own.** While the test executes, the
//!   context's [`ExpectLog`](crate::expect::ExpectLog) is the test's, so
//!   [`TestResult::failures`] holds what the test recorded and nothing a
//!   frame of the program recorded around it.
//! - **It can be set aside and picked up again.** A native that calls
//!   [`PetalCxt::suspend`](crate::native_fn::PetalCxt::suspend) stops the test
//!   after that call; [`Env::test_resume`] answers [`TestStep::Suspended`],
//!   the test's frames are parked on the stack (rooted for the collector),
//!   and the stack is free to [`run`](Env::run) the program's ordinary frames
//!   until the host resumes the test. That is how a test spans frames of a
//!   host's loop: `wait_frames(n)` is a host native that notes `n` and
//!   suspends.
//!
//! The intended loop of a host with a frame clock:
//!
//! ```text
//! load the program; create a stack; run one frame      (functions exist now)
//! for test in env.tests(pid):
//!     env.test_begin(stack, &test.function)?
//!     loop:
//!         match env.test_resume(stack):
//!             Finished(result) -> report it; break
//!             Suspended        -> run the frames the native asked for
//!                                 (reset_stack + run, as every frame), then loop
//!             OutOfSteps       -> unreachable without a step bound
//! ```
//!
//! What a suspended test sees when it resumes is what any function called
//! later sees: a `var` or `state var` read with `get` is live, a `let` or
//! plain `state` it captured is the value from when the test began.

use super::run::{host_call_site, make_vm};
use super::*;

use crate::backend::StepResult;
use crate::error::CallError;
use crate::expect::{ExpectFailure, ExpectLog};
use crate::program::TestDecl;
use crate::stack::TestRun;

/// What a finished test did.
#[derive(Debug, Clone, PartialEq)]
pub struct TestResult {
    /// The function that was run ([`TestDecl::function`]).
    pub function: String,
    /// No expectation failed and the body did not raise.
    pub passed: bool,
    /// The expectations that did not hold, in the order they were evaluated
    /// (at most [`crate::expect::MAX_RECORDED_FAILURES`]).
    pub failures: Vec<ExpectFailure>,
    /// How many expectations did not hold; more than `failures.len()` only
    /// when the log filled up.
    pub failed: u64,
    /// How many expectations were evaluated.
    pub checks: u64,
    /// The runtime error that ended the test, with its position and stack
    /// trace: an `assert`, a failing builtin, a reload under a waiting test.
    pub error: Option<String>,
}

/// Where [`Env::test_resume`] left the test.
#[derive(Debug, Clone, PartialEq)]
pub enum TestStep {
    /// The body returned or raised. The test is over and the stack is ready
    /// for the next one, or for a run.
    Finished(TestResult),
    /// A native asked to wait ([`PetalCxt::suspend`]). The test's frames are
    /// set aside; run whatever it is waiting for and call
    /// [`Env::test_resume`] again.
    ///
    /// [`PetalCxt::suspend`]: crate::native_fn::PetalCxt::suspend
    Suspended,
    /// [`Env::test_resume_bounded`] ran out of steps. The test is set aside
    /// exactly as for [`Suspended`](Self::Suspended): resume it to carry on,
    /// or [`Env::test_abort`] it.
    OutOfSteps,
}

impl Env {
    /// The test blocks of a program: the entry file's and every module's, in
    /// load order then source order. Empty for an unknown program. A runner
    /// that wants only the project's own tests filters on
    /// [`TestDecl::module`] / [`TestDecl::path`].
    pub fn tests(&self, program_id: ProgramId) -> &[TestDecl] {
        self.programs
            .get(&program_id)
            .map(|p| p.tests.as_slice())
            .unwrap_or(&[])
    }

    /// The expectation failures recorded in `stack_id`'s context since they
    /// were last taken, by code that was not a test: the program's ordinary
    /// runs and the host's own calls. (A test's failures are in its
    /// [`TestResult`].) See [`crate::expect`].
    pub fn take_expect_failures(&mut self, stack_id: StackKey) -> Vec<ExpectFailure> {
        match self.ctx_for(stack_id) {
            Some(ck) => self.ctx_mut(ck).expects.take(),
            None => Vec::new(),
        }
    }

    /// The expectation counts of `stack_id`'s context: how many were
    /// evaluated outside tests and how many of those did not hold, since the
    /// context was made. `(0, 0)` for an unknown stack.
    pub fn expect_counts(&self, stack_id: StackKey) -> (u64, u64) {
        match self.ctx_for(stack_id) {
            Some(ck) => {
                let log = &self.ctx(ck).expects;
                (log.checked, log.failed)
            }
            None => (0, 0),
        }
    }

    /// Start the test compiled as `function` (a [`TestDecl::function`]) on
    /// `stack_id`. Nothing of its body runs yet: [`test_resume`](Self::test_resume)
    /// does that.
    ///
    /// The stack must have completed a run, so the program's functions and
    /// module-level cells exist, and must not be in the middle of one. The
    /// test then sees the program as that run left it; a runner that wants
    /// each test to start clean gives each its own stack
    /// ([`create_stack`](Self::create_stack) and one `run`), which is what
    /// `petal test` does.
    ///
    /// `function` may name any top-level function of no required parameters,
    /// not only a test block's; a host can drive a script's own long-running
    /// entry point the same way.
    pub fn test_begin(&mut self, stack_id: StackKey, function: &str) -> Result<(), CallError> {
        let stack = self.stacks.get(&stack_id).ok_or(CallError::StackNotFound)?;
        if let Some(test) = &stack.test {
            return Err(CallError::Runtime(format!(
                "the test `{}` is still in progress on this stack; resume it to the end or \
                 abort it first",
                test.function
            )));
        }
        let callable = stack.functions.get(function).copied().ok_or_else(|| {
            CallError::FunctionNotFound {
                name: function.to_string(),
            }
        })?;
        let ck = stack.context;
        let pid = stack.program_id;
        self.ensure_bytecode(pid).map_err(CallError::Runtime)?;
        self.observations.enter_context(ck);
        if self.static_audit.enabled {
            self.static_audit.enter(pid);
        }
        let lowered = self.lower_count.get();

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
        // Whatever a finished or failed run left on the frame stack is not
        // the test's: start from none, as the next run would.
        stack.reset_execution();
        let pushed = make_vm(
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
            None,
        )
        .push_host_frame(callable, host_call_site(function));
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        let parked = std::mem::take(&mut stack.vm_frames);
        stack.reset_execution();
        pushed.map_err(CallError::Runtime)?;
        stack.test = Some(TestRun {
            function: function.to_string(),
            parked,
            failures: Vec::new(),
            checked: 0,
            failed: 0,
            lowered,
            invalidated: None,
        });
        Ok(())
    }

    /// The function of the test in progress on `stack_id`, if there is one.
    pub fn test_in_progress(&self, stack_id: StackKey) -> Option<&str> {
        self.stacks
            .get(&stack_id)?
            .test
            .as_ref()
            .map(|t| t.function.as_str())
    }

    /// Run the test begun on `stack_id` until it finishes or a native
    /// suspends it. See the [module docs](self) for the loop a host writes.
    ///
    /// Call it between the stack's runs, never during one: whatever frames
    /// the stack holds are dropped, as [`reset_stack`](Self::reset_stack)
    /// would drop them. With no test in progress the answer is a finished,
    /// failed [`TestResult`] saying so.
    pub fn test_resume(&mut self, stack_id: StackKey) -> TestStep {
        self.test_resume_bounded(stack_id, u64::MAX)
    }

    /// [`test_resume`](Self::test_resume), stopping with
    /// [`TestStep::OutOfSteps`] once about `max_steps` instructions have run
    /// in this call. A runner that enforces a time limit resumes in slices
    /// and looks at the clock between them.
    pub fn test_resume_bounded(&mut self, stack_id: StackKey, max_steps: u64) -> TestStep {
        let Some(stack) = self.stacks.get_mut(&stack_id) else {
            return TestStep::Finished(TestResult {
                function: String::new(),
                passed: false,
                failures: Vec::new(),
                failed: 0,
                checks: 0,
                error: Some("Stack not found".to_string()),
            });
        };
        let Some(test) = stack.test.as_mut() else {
            return TestStep::Finished(TestResult {
                function: String::new(),
                passed: false,
                failures: Vec::new(),
                failed: 0,
                checks: 0,
                error: Some("no test is in progress on this stack".to_string()),
            });
        };
        let ck = stack.context;
        let pid = stack.program_id;
        if let Some(why) = test.invalidated {
            return TestStep::Finished(self.finish_test(stack_id, Some(why.to_string())));
        }
        let lowered = test.lowered;
        if let Err(e) = self.ensure_bytecode(pid) {
            return TestStep::Finished(self.finish_test(stack_id, Some(e)));
        }
        if self.lower_count.get() != lowered {
            return TestStep::Finished(self.finish_test(
                stack_id,
                Some("the program was compiled again while the test was waiting".to_string()),
            ));
        }

        // Put the test's frames back and make the context's log the test's
        // for as long as it executes.
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        stack.reset_execution();
        let test = stack.test.as_mut().unwrap();
        stack.vm_frames = std::mem::take(&mut test.parked);
        stack.vm_started = true;
        let around = std::mem::take(&mut self.ctx_mut(ck).expects);
        self.ctx_mut(ck).expects.suspendable = true;

        /// Why the test stopped executing.
        enum Stop {
            Bound,
            Suspend,
            Done(Option<String>),
        }
        let mut steps: u64 = 0;
        let stop = loop {
            if steps >= max_steps {
                break Stop::Bound;
            }
            let budget = (max_steps - steps).min(Self::TEST_BATCH);
            let stepped = self.step_test(stack_id, ck, pid, budget);
            // A native that suspended raised the heap's interrupt to get
            // here; lower it before anything looks at the collector.
            self.ctx_mut(ck).heap.end_interrupt();
            match stepped {
                Ok((StepResult::Continue, consumed)) => {
                    steps += consumed;
                    if std::mem::take(&mut self.ctx_mut(ck).expects.suspend_requested) {
                        break Stop::Suspend;
                    }
                    if self.ctx(ck).heap.should_collect() {
                        self.collect_garbage(ck);
                    }
                }
                Ok((StepResult::Complete(_), _)) => break Stop::Done(None),
                Ok((StepResult::Error(e), _)) | Err(e) => break Stop::Done(Some(e)),
            }
        };

        // Hand the context its own log back and keep what the test recorded.
        let log: ExpectLog = std::mem::replace(&mut self.ctx_mut(ck).expects, around);
        let stack = self.stacks.get_mut(&stack_id).unwrap();
        let test = stack.test.as_mut().unwrap();
        test.failures.extend(log.failures);
        test.checked += log.checked;
        test.failed += log.failed;
        // The call may have written state the next run reads.
        stack.run_deps.force();
        match stop {
            Stop::Done(error) => TestStep::Finished(self.finish_test(stack_id, error)),
            waiting => {
                test.parked = std::mem::take(&mut stack.vm_frames);
                stack.reset_execution();
                match waiting {
                    Stop::Suspend => TestStep::Suspended,
                    _ => TestStep::OutOfSteps,
                }
            }
        }
    }

    /// How many instructions a test runs between looks at the collector and
    /// the step bound.
    const TEST_BATCH: u64 = 65_536;

    /// One batch of the test whose frames are on `stack_id`. As
    /// `step_bytecode`, but a host call: no root frame is pushed and nothing
    /// is memoized.
    fn step_test(
        &mut self,
        stack_id: StackKey,
        ck: ContextKey,
        pid: ProgramId,
        budget: u64,
    ) -> Result<(StepResult, u64), String> {
        self.observations.enter_context(ck);
        if self.static_audit.enabled {
            self.static_audit.enter(pid);
        }
        let bc = &self.bytecode.get(&pid).ok_or("Program not lowered")?.1;
        let program = self.programs.get(&pid).ok_or("Program not found")?;
        let stack = self.stacks.get_mut(&stack_id).ok_or("Stack not found")?;
        let ctx = self.contexts.get_mut(&ck).ok_or("Context not found")?;
        Ok(make_vm(
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
            None,
        )
        .run_batch(budget))
    }

    /// End the test on `stack_id` and build its result.
    fn finish_test(&mut self, stack_id: StackKey, error: Option<String>) -> TestResult {
        let stack = self.stacks.get_mut(&stack_id).expect("stack of the test");
        let test = stack.test.take().expect("test in progress");
        stack.reset_execution();
        stack.run_deps.force();
        TestResult {
            function: test.function,
            passed: error.is_none() && test.failed == 0,
            failures: test.failures,
            failed: test.failed,
            checks: test.checked,
            error,
        }
    }

    /// Give up on the test in progress on `stack_id`: its frames are dropped
    /// and what it recorded so far comes back as a failed result whose error
    /// is `why`. `None` when no test was in progress.
    pub fn test_abort(&mut self, stack_id: StackKey, why: &str) -> Option<TestResult> {
        self.stacks.get(&stack_id)?.test.as_ref()?;
        Some(self.finish_test(stack_id, Some(why.to_string())))
    }

    /// Run one test from start to finish: [`test_begin`](Self::test_begin)
    /// and [`test_resume`](Self::test_resume) for a runner with no frames to
    /// wait for. A test that suspends is aborted, with an error that says
    /// it needs a host that runs frames.
    pub fn run_test(&mut self, stack_id: StackKey, function: &str) -> TestResult {
        if let Err(e) = self.test_begin(stack_id, function) {
            return TestResult {
                function: function.to_string(),
                passed: false,
                failures: Vec::new(),
                failed: 0,
                checks: 0,
                error: Some(e.to_string()),
            };
        }
        match self.test_resume(stack_id) {
            TestStep::Finished(result) => result,
            TestStep::Suspended | TestStep::OutOfSteps => self
                .test_abort(
                    stack_id,
                    "the test asked to wait, and this runner has no frames to run in the \
                     meantime; run it with the host's own test runner",
                )
                .expect("test in progress"),
        }
    }
}
