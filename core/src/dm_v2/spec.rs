//! The speculative runner: run a program again, with literals overridden,
//! with no side effects and with every input frozen, so that two runs differ
//! only by the override (docs/dev/direct-manipulation-v2-plan.md §2.1, M1).
//!
//! A [`SpecSession`] is opened on a *checkpoint*, a stack ready to run the
//! frame being edited (inputs bound, state as it was before the frame). It
//! forks the checkpoint into a stack it owns, gives each candidate literal a
//! constant slot of its own ([`Env::slot_literals`]), and runs the baseline
//! twice: the two must agree bit for bit, or the program is not deterministic
//! enough to measure ([`SpecError::Nondeterministic`]). Every
//! [`run`](SpecSession::run) after that forks the session's checkpoint again,
//! loads the overridden values in place of the literals, and records what the
//! run emitted ([`SpecRun`]). The checkpoint and the live stack are never
//! touched.
//!
//! What makes two runs comparable:
//!
//! - **Effects.** A native whose effect is not
//!   [`contained`](crate::native_fn::NativeEffects::contained) is not called;
//!   it answers `nil` and the run lists it in [`SpecRun::suppressed`].
//!   Contained effects (`print`, counters, output buffers) land in the fork
//!   and are dropped with it. A handle method fails the run unless its class
//!   was marked safe ([`Env::mark_handle_class_spec_safe`]).
//! - **State** is frozen: each run starts from the checkpoint's state
//!   ([`StateMode::Frozen`]), and its writes go with the fork.
//! - **Randomness** is replayed by place ([`RngMode::ReplayByCallSite`]):
//!   the draw at a given call chain, loop path and occurrence is the one the
//!   baseline drew there, mapped through the native's current arguments.
//! - **Host data** (`reads(HOST_DATA)`) is answered from what the baseline
//!   was told, by native and arguments; a question the baseline never asked
//!   answers `nil` and counts in [`SpecRun::new_host_keys`], so a perturbed
//!   run cannot start a fetch.
//! - **Time and input** are bindings, which the fork copies: frozen.
//!
//! Emits are recorded with the native's raw arguments, so an observable can
//! read a draw call's `x` before the native truncates it to an int.

use std::collections::HashMap;
use std::time::Duration;

use smallvec::SmallVec;

use crate::backend::bytecode::vm::spec::{Portable, ReplayPhase, RngKey, SpecHooks};
use crate::constant_table::ConstantId;
use crate::env::Env;
use crate::env::spec_run::SpecRunEnd;
use crate::program::{FunctionId, ProgramId, TermId};
use crate::stack::StackKey;
use crate::symbol::SymbolId;
use crate::value::Value;

pub use crate::backend::bytecode::vm::spec::{EmitRecord, RngMode, SuppressedCall, WatchRecord};
pub use crate::dm_v2::observe::{EmitKey, LiveObservation, ObsValue, Observable, PathStep};

/// Where a run's `state` starts from (§2.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateMode {
    /// Every run starts from the checkpoint's state: state is an input.
    #[default]
    Frozen,
    /// Every run starts with no state, so `state` initializers execute.
    Fresh,
}

/// How an override reaches the program (§2.1.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverrideStrategy {
    /// Load the overridden value in place of the literal's slot.
    #[default]
    Slots,
    /// Test-only oracle: splice the value into the source text, compile it,
    /// and run that. Entry-file literals only.
    Recompile,
}

/// What a run records besides its emits.
#[derive(Debug, Clone)]
pub struct CaptureSpec {
    /// The output channels to record (`None` records every one).
    pub channels: Option<Vec<SymbolId>>,
    /// Record each emitting native's raw arguments.
    pub native_args: bool,
    /// Terms whose values to record each time one is computed.
    pub watch: Vec<TermId>,
}

impl Default for CaptureSpec {
    fn default() -> Self {
        CaptureSpec {
            channels: None,
            native_args: true,
            watch: Vec::new(),
        }
    }
}

/// A session's settings (§6.1).
#[derive(Debug, Clone)]
pub struct SpecConfig {
    pub state: StateMode,
    pub rng: RngMode,
    /// The step budget of a run; `None` is ten times the baseline's steps
    /// (at least 10,000).
    pub max_steps: Option<u64>,
    /// The wall-clock cap of a run.
    pub max_wall: Duration,
    pub capture: CaptureSpec,
    pub strategy: OverrideStrategy,
    /// Seeds the draws at places the baseline never drew at.
    pub session_seed: u64,
}

impl Default for SpecConfig {
    fn default() -> Self {
        SpecConfig {
            state: StateMode::Frozen,
            rng: RngMode::ReplayByCallSite,
            max_steps: None,
            max_wall: Duration::from_secs(2),
            capture: CaptureSpec::default(),
            strategy: OverrideStrategy::Slots,
            session_seed: 0,
        }
    }
}

/// The value an overridden literal loads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverrideValue {
    Num(f64),
    Int(i64),
    /// A dual number (value, derivative), for forward-mode probes.
    Dual(f64, f64),
}

impl OverrideValue {
    fn to_value(self) -> Value {
        match self {
            OverrideValue::Num(f) => Value::Float(f),
            OverrideValue::Int(i) => Value::Int(i),
            OverrideValue::Dual(value, derivative) => Value::Dual { value, derivative },
        }
    }
}

/// Literal overrides for one run, by the literal's term.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overrides(pub SmallVec<[(TermId, OverrideValue); 8]>);

impl Overrides {
    /// No overrides: the baseline.
    pub fn none() -> Overrides {
        Overrides::default()
    }

    /// One literal overridden.
    pub fn one(term: TermId, value: OverrideValue) -> Overrides {
        Overrides::none().with(term, value)
    }

    /// These overrides and one more.
    pub fn with(mut self, term: TermId, value: OverrideValue) -> Overrides {
        self.0.push((term, value));
        self
    }
}

/// Why a run failed. A failed run is data, not a failed session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The program raised an error (or the run could not be set up).
    Runtime(String),
    /// The step budget ran out (deterministic).
    StepBudget,
    /// The wall-clock cap ran out (not deterministic).
    TimedOut,
    /// A handle method of a class not marked safe for speculative runs.
    Unsafe(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Runtime(e) => write!(f, "{e}"),
            RunError::StepBudget => write!(f, "the run went over its step budget"),
            RunError::TimedOut => write!(f, "the run went over its time limit"),
            RunError::Unsafe(class) => {
                write!(f, "the run called a method of handle class `{class}`, which is not marked safe")
            }
        }
    }
}

/// Which of a session's parameters a run loaded, by parameter index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamSet(Vec<bool>);

impl ParamSet {
    pub fn contains(&self, param: usize) -> bool {
        self.0.get(param).copied().unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The indices that are set.
    pub fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i)
    }
}

/// What one speculative run did (§2.1.6).
#[derive(Debug, Clone)]
pub struct SpecRun {
    pub error: Option<RunError>,
    /// Every recorded emit, in the order the run made them.
    pub emits: Vec<EmitRecord>,
    /// Every recorded value of a watched term, in order.
    pub watched: Vec<WatchRecord>,
    /// A hash of every conditional's outcome (branches, loop steps, match
    /// arms, `state` hits and misses). Equal fingerprints: the same path.
    pub fingerprint: u64,
    pub loaded: ParamSet,
    pub suppressed: Vec<SuppressedCall>,
    /// Random draws made at places the baseline never drew at.
    pub new_random_keys: u32,
    /// Host-data questions the baseline never asked (each answered `nil`).
    pub new_host_keys: u32,
    /// The user-function calls the run made, by call site (recorded by the
    /// baseline only).
    pub call_edges: Vec<(TermId, FunctionId)>,
    pub steps: u64,
    pub elapsed: Duration,
}

impl SpecRun {
    fn failed(error: RunError) -> SpecRun {
        SpecRun {
            error: Some(error),
            emits: Vec::new(),
            watched: Vec::new(),
            fingerprint: 0,
            loaded: ParamSet::default(),
            suppressed: Vec::new(),
            new_random_keys: 0,
            new_host_keys: 0,
            call_edges: Vec::new(),
            steps: 0,
            elapsed: Duration::ZERO,
        }
    }

    /// Whether the run completed.
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    /// The emit with `key`.
    pub fn emit(&self, key: &EmitKey) -> Option<&EmitRecord> {
        self.emits.iter().find(|e| e.key == *key)
    }

    /// The emits into `channel`, in order.
    pub fn channel(&self, channel: SymbolId) -> impl Iterator<Item = &EmitRecord> {
        self.emits.iter().filter(move |e| e.channel == channel)
    }

    /// Read one number of the run.
    pub fn read(&self, o: &Observable) -> Option<f64> {
        match o {
            Observable::EmitArg { key, arg, path } => {
                self.emit(key)?.native_args.get(*arg)?.at(path)?.as_f64()
            }
            Observable::EmitValue { key, path } => self.emit(key)?.value.at(path)?.as_f64(),
            Observable::Watch { term, at, path } => {
                let before = self.emit(at)?.seq;
                self.watched
                    .iter()
                    .rev()
                    .find(|w| w.term == *term && w.seq < before)?
                    .value
                    .at(path)?
                    .as_f64()
            }
        }
    }

    /// The first way this run's observables differ from `other`'s (emits,
    /// watched values, fingerprint, outcome), or `None` when they agree bit
    /// for bit.
    pub fn first_difference(&self, other: &SpecRun) -> Option<String> {
        if self.error != other.error {
            return Some(format!("outcome: {:?} vs {:?}", self.error, other.error));
        }
        if self.emits.len() != other.emits.len() {
            return Some(format!("{} emits vs {}", self.emits.len(), other.emits.len()));
        }
        for (i, (a, b)) in self.emits.iter().zip(&other.emits).enumerate() {
            if a.channel != b.channel || a.key != b.key {
                return Some(format!("emit {i}: a different place"));
            }
            if a.value != b.value {
                return Some(format!("emit {i}: value {:?} vs {:?}", a.value, b.value));
            }
            if a.native_args != b.native_args {
                return Some(format!("emit {i}: arguments {:?} vs {:?}", a.native_args, b.native_args));
            }
        }
        if self.watched != other.watched {
            return Some("watched values differ".to_string());
        }
        if self.fingerprint != other.fingerprint {
            return Some("a branch went the other way".to_string());
        }
        None
    }

    /// Whether two runs emitted the same values with the same raw arguments,
    /// compared by channel and position: what the Recompile oracle checks
    /// (§2.1.8), since a recompiled program may number its terms differently,
    /// which moves every [`EmitKey`].
    pub fn same_emits_by_position(&self, other: &SpecRun) -> Result<(), String> {
        if self.error != other.error {
            return Err(format!("outcome: {:?} vs {:?}", self.error, other.error));
        }
        if self.emits.len() != other.emits.len() {
            return Err(format!("{} emits vs {}", self.emits.len(), other.emits.len()));
        }
        for (i, (a, b)) in self.emits.iter().zip(&other.emits).enumerate() {
            if a.channel != b.channel || a.value != b.value || a.native_args != b.native_args {
                return Err(format!(
                    "emit {i}: {:?} {:?} vs {:?} {:?}",
                    a.value, a.native_args, b.value, b.native_args
                ));
            }
        }
        Ok(())
    }
}

/// Why a session could not be opened or used.
#[derive(Debug, Clone, PartialEq)]
pub enum SpecError {
    /// The baseline, run twice, did not agree with itself.
    Nondeterministic { first_difference: String },
    /// The speculative baseline does not show what the live frame showed
    /// for goal `goal`; `suppressed` names the natives it did not call.
    BaselineMismatch { goal: usize, live: f64, spec: f64, suppressed: Vec<String> },
    /// The baseline called a handle method of a class not marked safe.
    Unsafe { native: String },
    /// The baseline failed, or the session could not be set up.
    BaselineFailed(String),
    /// The checkpoint stack or its program is gone.
    Stale,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::Nondeterministic { first_difference } => write!(
                f,
                "the program does not run the same way twice ({first_difference}); \
                 a native it calls may read something it does not declare"
            ),
            SpecError::BaselineMismatch { goal, live, spec, suppressed } => {
                write!(f, "goal {goal}: the live frame shows {live}, a speculative run gives {spec}")?;
                if !suppressed.is_empty() {
                    write!(f, " (natives not called in a speculative run: {})", suppressed.join(", "))?;
                }
                Ok(())
            }
            SpecError::Unsafe { native } => write!(
                f,
                "the program calls methods of handle class `{native}`, which is not marked safe for speculative runs"
            ),
            SpecError::BaselineFailed(e) => write!(f, "the baseline run failed: {e}"),
            SpecError::Stale => write!(f, "the checkpoint stack is gone"),
        }
    }
}

impl std::error::Error for SpecError {}

/// A host's run wrapper (R11): whatever thread-local providers the host
/// installs around a live frame (host data, fonts, a clipboard), it installs
/// around a speculative run the same way, and calls `f` inside.
pub trait SpecHost {
    fn with_run_env(&mut self, env: &mut Env, f: &mut dyn FnMut(&mut Env));
}

/// The host of plain core: nothing to install.
pub struct NoHost;

impl SpecHost for NoHost {
    fn with_run_env(&mut self, env: &mut Env, f: &mut dyn FnMut(&mut Env)) {
        f(env)
    }
}

/// How a run reads the replay tables.
#[derive(Clone, Copy)]
struct RunMode {
    phase: ReplayPhase,
    rng: RngMode,
    record_edges: bool,
}

/// One manipulation request's speculative runs (§2.1.1, §6.1).
pub struct SpecSession {
    /// The session's own fork of the checkpoint.
    checkpoint: StackKey,
    program: ProgramId,
    params: Vec<TermId>,
    slots: Vec<ConstantId>,
    cfg: SpecConfig,
    baseline: SpecRun,
    rng_table: HashMap<RngKey, u64>,
    host_table: HashMap<(u32, u64), Option<Portable>>,
    rigid_table: HashMap<(u64, u64, u32), Option<Portable>>,
    runs: usize,
}

impl SpecSession {
    /// Open a session on `checkpoint`, a stack ready to run the frame
    /// (inputs bound, pre-frame state). Forks it into a stack the session
    /// owns, slots `params` (literal terms), runs the baseline twice and
    /// checks it agrees with itself and, when `live` is given, with the live
    /// frame.
    pub fn begin(
        env: &mut Env,
        host: &mut dyn SpecHost,
        checkpoint: StackKey,
        params: &[TermId],
        cfg: SpecConfig,
        live: Option<&LiveObservation>,
    ) -> Result<SpecSession, SpecError> {
        let program = env.stack_program(checkpoint).ok_or(SpecError::Stale)?;
        let own = env.fork_execution(checkpoint).map_err(|_| SpecError::Stale)?;
        let mut session = SpecSession {
            checkpoint: own,
            program,
            params: Vec::new(),
            slots: Vec::new(),
            cfg,
            baseline: SpecRun::failed(RunError::Runtime("no baseline yet".into())),
            rng_table: HashMap::new(),
            host_table: HashMap::new(),
            rigid_table: HashMap::new(),
            runs: 0,
        };
        if let Err(e) = session.slot(env, params) {
            env.drop_fork(own);
            return Err(e);
        }
        match session.run_baselines(env, host, live) {
            Ok(()) => Ok(session),
            Err(e) => {
                env.drop_fork(own);
                Err(e)
            }
        }
    }

    fn slot(&mut self, env: &mut Env, params: &[TermId]) -> Result<(), SpecError> {
        let new: Vec<TermId> = params.iter().copied().filter(|t| !self.params.contains(t)).collect();
        let slots = env.slot_literals(self.program, &new).map_err(SpecError::BaselineFailed)?;
        self.params.extend(new);
        self.slots.extend(slots);
        Ok(())
    }

    fn run_baselines(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        live: Option<&LiveObservation>,
    ) -> Result<(), SpecError> {
        let record = RunMode {
            phase: ReplayPhase::Record,
            rng: self.cfg.rng,
            record_edges: true,
        };
        let cap = self.cfg.max_steps.unwrap_or(1_000_000_000);
        // The first baseline fills the replay tables; the second records
        // into tables of its own and must come out the same.
        let first = self.exec(env, host, &Overrides::none(), record, cap);
        let first = Self::baseline_ok(first)?;
        let (rng, hosts) = (std::mem::take(&mut self.rng_table), std::mem::take(&mut self.host_table));
        let rigid = std::mem::take(&mut self.rigid_table);
        let second = self.exec(env, host, &Overrides::none(), record, cap);
        self.rng_table = rng;
        self.host_table = hosts;
        self.rigid_table = rigid;
        let second = Self::baseline_ok(second)?;
        if let Some(first_difference) = first.first_difference(&second) {
            return Err(SpecError::Nondeterministic { first_difference });
        }
        if let Some(live) = live {
            for (goal, (o, want)) in live.goals.iter().enumerate() {
                let got = first.read(o);
                if got.is_none_or(|g| (g - want).abs() > 1e-9) {
                    return Err(SpecError::BaselineMismatch {
                        goal,
                        live: *want,
                        spec: got.unwrap_or(f64::NAN),
                        suppressed: first.suppressed.iter().map(|s| s.native.clone()).collect(),
                    });
                }
            }
        }
        self.baseline = first;
        Ok(())
    }

    fn baseline_ok(run: SpecRun) -> Result<SpecRun, SpecError> {
        match &run.error {
            None => Ok(run),
            Some(RunError::Unsafe(class)) => Err(SpecError::Unsafe { native: class.clone() }),
            Some(e) => Err(SpecError::BaselineFailed(e.to_string())),
        }
    }

    /// Slot more literals. The baseline is run again (once, inside `host`'s
    /// run wrapper) so its loaded set covers them.
    pub fn add_params(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        params: &[TermId],
    ) -> Result<(), SpecError> {
        self.slot(env, params)?;
        let record = RunMode {
            phase: ReplayPhase::Record,
            rng: self.cfg.rng,
            record_edges: true,
        };
        let cap = self.cfg.max_steps.unwrap_or(1_000_000_000);
        let run = self.exec(env, host, &Overrides::none(), record, cap);
        let run = Self::baseline_ok(run)?;
        if let Some(first_difference) = self.baseline.first_difference(&run) {
            return Err(SpecError::Nondeterministic { first_difference });
        }
        self.baseline = run;
        Ok(())
    }

    /// One speculative run with `overrides`. A failed run is data: its
    /// [`SpecRun::error`] says why, and the session stays usable.
    pub fn run(&mut self, env: &mut Env, host: &mut dyn SpecHost, overrides: &Overrides) -> SpecRun {
        self.run_with(env, host, overrides, self.cfg.rng)
    }

    /// [`run`](Self::run) with random draws taken from the forked stream in
    /// order, as the edited program will really draw them (§2.1.4).
    pub fn run_fork_stream(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        overrides: &Overrides,
    ) -> SpecRun {
        self.run_with(env, host, overrides, RngMode::ForkStream)
    }

    fn run_with(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        overrides: &Overrides,
        rng: RngMode,
    ) -> SpecRun {
        let mode = RunMode {
            phase: ReplayPhase::Replay,
            rng,
            record_edges: false,
        };
        let cap = self
            .cfg
            .max_steps
            .unwrap_or_else(|| self.baseline.steps.saturating_mul(10).max(10_000));
        let mut run = self.exec_strategy(env, host, overrides, mode, cap);
        // The wall clock is not deterministic: one more try.
        if run.error == Some(RunError::TimedOut) {
            run = self.exec_strategy(env, host, overrides, mode, cap);
        }
        self.runs += 1;
        run
    }

    fn exec_strategy(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        overrides: &Overrides,
        mode: RunMode,
        cap: u64,
    ) -> SpecRun {
        match self.cfg.strategy {
            OverrideStrategy::Slots => self.exec(env, host, overrides, mode, cap),
            OverrideStrategy::Recompile => self.exec_recompiled(env, host, overrides, mode, cap),
        }
    }

    /// The baseline run.
    pub fn baseline(&self) -> &SpecRun {
        &self.baseline
    }

    /// How many runs [`run`](Self::run) and
    /// [`run_fork_stream`](Self::run_fork_stream) have made.
    pub fn runs_used(&self) -> usize {
        self.runs
    }

    pub fn program(&self) -> ProgramId {
        self.program
    }

    /// Whether the baseline drew random numbers (then a [`ForkStream`]
    /// run can disagree with the replayed one, §2.1.4).
    ///
    /// [`ForkStream`]: RngMode::ForkStream
    pub fn drew_random(&self) -> bool {
        !self.rng_table.is_empty()
    }

    /// The session's parameters, in parameter-index order.
    pub fn params(&self) -> &[TermId] {
        &self.params
    }

    /// The parameter index of `term`.
    pub fn param_index(&self, term: TermId) -> Option<usize> {
        self.params.iter().position(|&t| t == term)
    }

    /// Close the session, dropping its fork. The slots stay: they change
    /// nothing about how the program runs.
    pub fn end(self, env: &mut Env) {
        env.drop_fork(self.checkpoint);
    }

    /// Build the hooks for one run.
    fn hooks(&mut self, env: &Env, overrides: &Overrides, mode: RunMode) -> Result<SpecHooks, RunError> {
        let n = env.constants_len(self.program);
        let mut hooks = SpecHooks {
            param_of: vec![u32::MAX; n],
            loaded: vec![false; self.params.len()],
            capture_args: self.cfg.capture.native_args,
            host_phase: Some(mode.phase),
            safe_handles: env.spec_safe_handle_classes(),
            watch: self.cfg.capture.watch.iter().copied().collect(),
            record_call_edges: mode.record_edges,
            ..Default::default()
        };
        for (i, slot) in self.slots.iter().enumerate() {
            if let Some(p) = hooks.param_of.get_mut(slot.0 as usize) {
                *p = i as u32;
            }
        }
        for (term, value) in &overrides.0 {
            let Some(i) = self.param_index(*term) else {
                return Err(RunError::Runtime(format!(
                    "term {} is not a parameter of this session",
                    term.0
                )));
            };
            let slot = self.slots[i].0 as usize;
            if hooks.overrides.len() <= slot {
                hooks.overrides.resize(slot + 1, None);
            }
            hooks.overrides[slot] = Some(value.to_value());
        }
        hooks.tap.rng_mode = mode.rng;
        hooks.tap.phase = Some(mode.phase);
        hooks.tap.session_seed = self.cfg.session_seed;
        hooks.tap.channels = self.cfg.capture.channels.clone();
        if mode.phase == ReplayPhase::Replay {
            hooks.tap.rng_table = std::mem::take(&mut self.rng_table);
            hooks.host_table = std::mem::take(&mut self.host_table);
            hooks.rigid_table = std::mem::take(&mut self.rigid_table);
        }
        Ok(hooks)
    }

    /// Run a fork of the checkpoint under `overrides`.
    fn exec(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        overrides: &Overrides,
        mode: RunMode,
        cap: u64,
    ) -> SpecRun {
        let fresh = self.cfg.state == StateMode::Fresh;
        let fork = match env.spec_fork(self.checkpoint, fresh) {
            Ok(f) => f,
            Err(e) => return SpecRun::failed(RunError::Runtime(e)),
        };
        let run = self.exec_fork(env, host, fork, overrides, mode, cap);
        env.drop_fork(fork);
        run
    }

    fn exec_fork(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        fork: StackKey,
        overrides: &Overrides,
        mode: RunMode,
        cap: u64,
    ) -> SpecRun {
        let hooks = match self.hooks(env, overrides, mode) {
            Ok(h) => Box::new(h),
            Err(e) => return SpecRun::failed(e),
        };
        let max_wall = self.cfg.max_wall;
        let mut hooks = Some(hooks);
        let mut out = None;
        host.with_run_env(env, &mut |env| {
            if let Some(h) = hooks.take() {
                out = Some(env.run_spec_fork(fork, h, cap, max_wall));
            }
        });
        let Some((end, hooks, steps, elapsed)) = out else {
            return SpecRun::failed(RunError::Runtime("the host did not run the speculative run".into()));
        };
        let mut hooks = *hooks;
        // The tables go back to the session (a baseline's become the ones
        // later runs answer from).
        self.rng_table = std::mem::take(&mut hooks.tap.rng_table);
        self.host_table = std::mem::take(&mut hooks.host_table);
        self.rigid_table = std::mem::take(&mut hooks.rigid_table);
        let error = if let Some(class) = hooks.unsafe_hit.take() {
            Some(RunError::Unsafe(class))
        } else {
            match end {
                SpecRunEnd::Done => None,
                SpecRunEnd::Error(e) => Some(RunError::Runtime(e)),
                SpecRunEnd::StepBudget => Some(RunError::StepBudget),
                SpecRunEnd::TimedOut => Some(RunError::TimedOut),
            }
        };
        SpecRun {
            error,
            emits: hooks.tap.emits,
            watched: hooks.watched,
            fingerprint: hooks.fingerprint,
            loaded: ParamSet(hooks.loaded),
            suppressed: hooks.suppressed,
            new_random_keys: hooks.tap.new_random_keys,
            new_host_keys: hooks.new_host_keys,
            call_edges: hooks.call_edges,
            steps,
            elapsed,
        }
    }

    /// The Recompile oracle: splice each override into the entry file's
    /// text, compile it, and run that program from the checkpoint's context
    /// with no overrides.
    fn exec_recompiled(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        overrides: &Overrides,
        mode: RunMode,
        cap: u64,
    ) -> SpecRun {
        let source = match self.spliced_source(env, overrides) {
            Ok(s) => s,
            Err(e) => return SpecRun::failed(RunError::Runtime(e)),
        };
        let pid = match env.load_program(&source) {
            Ok(p) => p,
            Err(e) => return SpecRun::failed(RunError::Runtime(e)),
        };
        let fresh = self.cfg.state == StateMode::Fresh;
        let fork = match env.spec_fork_onto(self.checkpoint, pid, fresh) {
            Ok(f) => f,
            Err(e) => return SpecRun::failed(RunError::Runtime(e)),
        };
        // The recompiled program has no slots and its own constants: run it
        // with no overrides and no parameters.
        let (params, slots) = (std::mem::take(&mut self.params), std::mem::take(&mut self.slots));
        let program = std::mem::replace(&mut self.program, pid);
        let run = self.exec_fork(env, host, fork, &Overrides::none(), mode, cap);
        self.params = params;
        self.slots = slots;
        self.program = program;
        env.drop_fork(fork);
        run
    }

    fn spliced_source(&self, env: &Env, overrides: &Overrides) -> Result<String, String> {
        let program = env.get_program(self.program).ok_or("Program not found")?;
        let mut source = program.source_map.files.first().map(|f| f.source.clone()).unwrap_or_default();
        if source.is_empty() {
            source = program.source.clone();
        }
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        for (term, value) in &overrides.0 {
            let span = program
                .source_map
                .get(*term)
                .ok_or_else(|| format!("term {} has no source position", term.0))?;
            if span.file.0 != 0 {
                return Err("the Recompile oracle edits the entry file only".into());
            }
            let text = match value {
                OverrideValue::Int(i) if *i < 0 => format!("({i})"),
                OverrideValue::Int(i) => i.to_string(),
                OverrideValue::Num(f) if *f < 0.0 => format!("({f:?})"),
                OverrideValue::Num(f) => format!("{f:?}"),
                OverrideValue::Dual(..) => return Err("a dual override has no source spelling".into()),
            };
            edits.push((span.start.offset as usize, span.end.offset as usize, text));
        }
        edits.sort_by(|a, b| b.0.cmp(&a.0));
        for (start, end, text) in edits {
            if end > source.len() || start > end {
                return Err("a literal's span is outside its file".into());
            }
            source.replace_range(start..end, &text);
        }
        Ok(source)
    }
}
