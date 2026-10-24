//! The public direct-manipulation v2 API (docs/dev/direct-manipulation-v2-plan.md
//! §4.1, §6.3, M3): from goals on observed outputs to ranked, verified
//! source edits.
//!
//! [`propose_edits_v2`] runs the whole pipeline once:
//!
//! 1. open a speculative session on the checkpoint (M1);
//! 2. discover the literals the goals depend on, slot them, drop the ones
//!    the baseline never evaluated, apply the request's filter and the hard
//!    eligibility rules (M2);
//! 3. screen the eligible ones by group testing and measure the Jacobian of
//!    the goals and of every other emitted number (the collateral rows);
//! 4. when the program reads the clock, measure the strongest columns again
//!    with the clock shifted, and penalize the ones that are an animation
//!    rate rather than a position (§2.5.4);
//! 5. form the candidate subsets (§2.5.3), solve each one linearly against
//!    the cached Jacobian (no runs), and send the best few to the
//!    verification loop ([`super::solve`]);
//! 6. round each to nice literals, verify the rendered text by one more run,
//!    measure collateral from that run, check the random-draw order, score
//!    and rank (§2.5.4).
//!
//! [`DmSession`] is the same pipeline split at the Jacobian, for a drag:
//! [`open`](DmSession::open) on mouse-down, [`preview`](DmSession::preview)
//! (linear, zero runs) on each move, [`finish`](DmSession::finish) on
//! mouse-up.
//!
//! A goal whose target is not a number (a string, a bool) has no derivative:
//! the request falls back to v1 (`crate::direct_manipulation`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::direct_manipulation::{self as v1, ManipulationGoal, SourceEdit, VarPolicy};
use crate::dm_v2::DmError;
use crate::dm_v2::hints::{Hint, read_hints};
use crate::dm_v2::jacobian::{ColumnQuality, FdOptions, Jacobian, collateral_rows, jacobian};
use crate::dm_v2::observe::{EmitKey, LiveObservation, ObsValue, Observable};
use crate::dm_v2::params::{
    Eligibility, EligibilityContext, Param, ParamFilter, ParamSelector, RunBudget, anchors_for,
    discover_params, eligibility, file_origins, screen_params,
};
use crate::dm_v2::rank::{
    ScoreFacts, ScoreParts, ScoreWeights, ScoredChange, SubsetKind, candidate_subsets, collateral_norm,
    collateral_shapes, param_weight, score, strength,
};
use crate::dm_v2::solve::{Bound, GoalRow, SolveCtx, SolveOptions, linear_step, max_error};
use crate::dm_v2::spec::{Overrides, SpecConfig, SpecHost, SpecRun, SpecSession};
use crate::env::Env;
use crate::native_fn::InputClasses;
use crate::program::{Program, TermId, TermOp};
use crate::source_map::{FileId, SourceSpan};
use crate::stack::StackKey;
use crate::static_value::StaticValue;
use crate::value::Value;

/// The most Jacobian columns a request measures (§9 M5, revision 4).
pub const MAX_MEASURE: usize = 32;

/// Past this many columns to measure, step escalation is cut to one.
pub const MANY_COLUMNS: usize = 8;

/// The most collateral rows a request reads (§2.3; §9 M5, revision 5).
pub const COLLATERAL_ROWS: usize = 8192;

// ── the request ────────────────────────────────────────────────────────────

/// What a goal wants its observable to be.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalTarget {
    Num(f64),
    /// A non-numeric value (a string, a bool): no derivative, so the request
    /// is answered by v1.
    Value(StaticValue),
}

/// One goal: an observable and where it should be (§6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct DmGoal {
    pub observable: Observable,
    pub target: GoalTarget,
    /// How close is close enough; `None` is the default of §2.4.3 (0.5 for a
    /// raw argument of a native that truncates to ints, else
    /// `1e-6 · max(|t|, 1)`).
    pub tolerance: Option<f64>,
    /// How much this goal counts against the others when they conflict.
    pub weight: f64,
}

impl DmGoal {
    /// The goal "`observable` should be `target`".
    pub fn to(observable: Observable, target: f64) -> DmGoal {
        DmGoal { observable, target: GoalTarget::Num(target), tolerance: None, weight: 1.0 }
    }

    /// A non-numeric goal (answered by v1).
    pub fn value(observable: Observable, target: StaticValue) -> DmGoal {
        DmGoal { observable, target: GoalTarget::Value(target), tolerance: None, weight: 1.0 }
    }

    /// The same goal with tolerance `tol`.
    pub fn within(mut self, tol: f64) -> DmGoal {
        self.tolerance = Some(tol);
        self
    }
}

/// How strongly `config let` and `tunable` are preferred (§2.5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Prefer {
    /// Config and tunable literals cost less, and alternatives made only of
    /// them are listed first when they verify; the others are still listed.
    #[default]
    Soft,
    /// v1's rule: once the program declares a `config let` (or a `tunable`
    /// that marks a literal), every other named binding is pinned, and when a
    /// config or tunable literal can move the goal, only those are used.
    ConfigStrict,
}

/// A request (§6.3).
#[derive(Debug, Clone)]
pub struct DmRequest {
    pub goals: Vec<DmGoal>,
    pub filter: ParamFilter,
    pub prefer: Prefer,
    pub max_alternatives: usize,
    /// Speculative runs the request may make (the session's two baselines
    /// and its re-baselines are not counted).
    pub max_runs: usize,
    pub max_screen: usize,
    pub allow_respell: bool,
    pub max_wall: Duration,
    /// Verification steps per subset.
    pub max_iters: usize,
    pub spec: SpecConfig,
    pub live: Option<LiveObservation>,
    pub explain: bool,
}

impl Default for DmRequest {
    fn default() -> Self {
        DmRequest {
            goals: Vec::new(),
            filter: ParamFilter::default(),
            prefer: Prefer::Soft,
            max_alternatives: 3,
            max_runs: 200,
            max_screen: 256,
            allow_respell: false,
            max_wall: Duration::from_secs(5),
            max_iters: 6,
            spec: SpecConfig::default(),
            live: None,
            explain: false,
        }
    }
}

impl DmRequest {
    /// A request for `goals` with the defaults.
    pub fn new(goals: Vec<DmGoal>) -> DmRequest {
        DmRequest { goals, ..Default::default() }
    }
}

// ── the response ───────────────────────────────────────────────────────────

/// One text replacement and the file it is in (R14).
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdit {
    pub file: FileId,
    /// Where that file came from, when it is on disk.
    pub path: Option<PathBuf>,
    pub edit: SourceEdit,
}

/// One literal changed by an alternative.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamChange {
    pub term: TermId,
    pub span: SourceSpan,
    pub file: FileId,
    /// The binding it defines, if any.
    pub binding: Option<String>,
    /// The function it is written in, if any.
    pub function: Option<String>,
    pub config: bool,
    /// The value as written before and after.
    pub old: f64,
    pub new: f64,
    pub old_text: String,
    pub new_text: String,
    /// An int literal written as a float now (§2.4.4).
    pub respelled: bool,
}

/// One other emit an alternative moved.
#[derive(Debug, Clone, PartialEq)]
pub struct CollateralItem {
    pub key: EmitKey,
    pub channel: String,
    /// The native that made the emit.
    pub callee: Option<String>,
    /// The line of the emit's call in user code (1-based).
    pub line: Option<u32>,
    /// The largest movement of any of its numbers (`inf`: it is gone).
    pub max_move: f64,
}

/// What else an alternative moved, measured by its verification run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Collateral {
    /// `‖Δ other rows‖ / ‖Δ goals‖`.
    pub norm: f64,
    /// How many other emits moved, each counted by how far it moved
    /// relative to the goals, at most 1 ([`collateral_shapes`]): what the
    /// score charges.
    pub shapes: f64,
    pub items: Vec<CollateralItem>,
}

/// Which engine produced an alternative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    V2,
    /// The v1 fallback for non-numeric goals; not re-run.
    V1,
}

/// One way to meet the goals (§6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Alternative {
    pub edits: Vec<FileEdit>,
    pub changes: Vec<ParamChange>,
    /// Each goal's value after the edit (from the verification run).
    pub achieved: Vec<f64>,
    /// What the linear model predicted for each goal before any run.
    pub predicted: Vec<f64>,
    /// `max_j |achieved - target| / τ_j`.
    pub max_error: f64,
    pub verified: bool,
    pub collateral: Collateral,
    /// Some change feeds back through `state` from earlier frames: the
    /// frozen-state measurement covers one frame only.
    pub stateful: bool,
    pub config_only: bool,
    /// The edit took another branch or re-initialized state somewhere.
    pub regime_changed: bool,
    /// Under sequential random draws the edited program draws differently.
    pub rng_order_changed: bool,
    /// The first difference the sequential-draw run showed.
    pub rng_difference: Option<String>,
    pub score: f64,
    pub score_parts: ScoreParts,
    pub kind: SubsetKind,
    /// Verification steps the solve took.
    pub iterations: usize,
    pub engine: Engine,
    pub description: String,
}

/// One discovered literal, and what became of it (§2.7).
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateReport {
    pub param: Param,
    /// May it be edited; `Err` says why not.
    pub eligible: Result<(), &'static str>,
    /// Its goal-row column, when measured.
    pub column: Option<Vec<f64>>,
    pub quality: Option<ColumnQuality>,
    /// Its cost weight `d_i`.
    pub weight: f64,
    pub clock_coupled: bool,
    /// The baseline evaluated it.
    pub loaded: bool,
}

/// What the request cost and anything worth telling the client.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DmDiagnostics {
    /// Speculative runs made (baselines not counted).
    pub runs: usize,
    pub wall: Option<Duration>,
    /// Natives a speculative run did not call.
    pub suppressed: Vec<String>,
    pub budget_exhausted: bool,
    pub warnings: Vec<String>,
    /// Literals found by discovery, eligible to screen, screened, measured.
    pub discovered: usize,
    pub eligible: usize,
    pub screened: usize,
    pub measured: usize,
    pub truncated: bool,
    /// Subsets solved linearly, and sent to the verification loop.
    pub subsets_predicted: usize,
    pub subsets_verified: usize,
    /// The goals' baseline values and tolerances.
    pub baseline: Vec<f64>,
    pub tolerances: Vec<f64>,
    pub engine: Option<Engine>,
}

/// The `explain` trace: what each stage did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExplainLog {
    pub lines: Vec<String>,
}

/// The answer (§6.3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DmResponse {
    /// Ranked, best first.
    pub alternatives: Vec<Alternative>,
    pub candidates: Vec<CandidateReport>,
    pub diagnostics: DmDiagnostics,
    pub explain: Option<ExplainLog>,
}

impl DmResponse {
    /// The best alternative, if it verified.
    pub fn best_verified(&self) -> Option<&Alternative> {
        self.alternatives.first().filter(|a| a.verified)
    }
}

// ── entry points ───────────────────────────────────────────────────────────

/// Propose ranked, verified edits that put `req.goals` where they want to
/// be, running the frame at `checkpoint` speculatively (§4.1).
pub fn propose_edits_v2(
    env: &mut Env,
    host: &mut dyn SpecHost,
    checkpoint: StackKey,
    req: &DmRequest,
) -> Result<DmResponse, DmError> {
    let targets: Vec<f64> = req
        .goals
        .iter()
        .map(|g| match g.target {
            GoalTarget::Num(t) => t,
            GoalTarget::Value(_) => f64::NAN,
        })
        .collect();
    DmSession::open(env, host, checkpoint, req)?.finish(env, host, &targets)
}

/// Apply alternatives' edits to the files they are in (R14): each file's
/// edits are spliced (by character offsets) as v1's `apply_edits` does;
/// files with no edits are left alone. An edit for a file not in `files` is
/// an error.
pub fn apply_edits_multi(files: &mut [(FileId, String)], edits: &[FileEdit]) -> Result<(), DmError> {
    let mut by_file: HashMap<FileId, Vec<SourceEdit>> = HashMap::new();
    for e in edits {
        if !files.iter().any(|(f, _)| *f == e.file) {
            return Err(DmError::Edit(format!("no source given for file {}", e.file.0)));
        }
        by_file.entry(e.file).or_default().push(e.edit.clone());
    }
    for (file, text) in files.iter_mut() {
        if let Some(es) = by_file.get(file) {
            *text = v1::apply_edits(text, es).map_err(|e| DmError::Edit(e.message))?;
        }
    }
    Ok(())
}

/// The text of every file of `program`, for [`apply_edits_multi`].
pub fn program_files(program: &Program) -> Vec<(FileId, String)> {
    program
        .source_map
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (FileId(i as u16), f.source.clone()))
        .collect()
}

// ── the session ────────────────────────────────────────────────────────────

/// What a parameter looks like in the source.
#[derive(Debug, Clone)]
struct ParamText {
    /// The literal as written (sign included when negated).
    text: String,
    /// The first non-blank character before it.
    prev: Option<char>,
    /// The first non-blank character after it.
    next: Option<char>,
    function: Option<String>,
    path: Option<PathBuf>,
}


/// A drag in progress: the pipeline up to the Jacobian, kept for previews
/// and the final solve (§6.3).
pub struct DmSession {
    /// `None` once ended, or when v1 answered.
    spec: Option<SpecSession>,
    req: DmRequest,
    params: Vec<Param>,
    texts: Vec<ParamText>,
    candidates: Vec<CandidateReport>,
    jac: Jacobian,
    /// Per Jacobian column.
    usable: Vec<bool>,
    weights: Vec<f64>,
    config: Vec<bool>,
    scales: Vec<f64>,
    base_rows: Vec<Option<f64>>,
    taus: Vec<f64>,
    budget: RunBudget,
    diag: DmDiagnostics,
    log: Vec<String>,
    started: Option<std::time::Instant>,
    /// The v1 answer, when a goal is not numeric.
    v1: Option<DmResponse>,
}

/// One subset's linear answer.
#[derive(Debug, Clone)]
struct Prediction {
    cols: Vec<usize>,
    kind: SubsetKind,
    values: Vec<f64>,
    goals: Vec<f64>,
    ok: bool,
    score: f64,
}

fn empty_jacobian() -> Jacobian {
    Jacobian {
        params: Vec::new(),
        rows: Vec::new(),
        goal_rows: 0,
        d: Vec::new(),
        step: Vec::new(),
        quality: Vec::new(),
        runs: 0,
        budget_exhausted: false,
    }
}

impl DmSession {
    /// Open a session: run the baseline, discover, screen and measure.
    pub fn open(
        env: &mut Env,
        host: &mut dyn SpecHost,
        checkpoint: StackKey,
        req: &DmRequest,
    ) -> Result<DmSession, DmError> {
        if req.goals.is_empty() {
            return Err(DmError::NoGoals);
        }
        let started = crate::env::spec_run::wall_now();
        let mut spec = SpecSession::begin(env, host, checkpoint, &[], req.spec.clone(), req.live.as_ref())?;
        let mut session = DmSession {
            spec: None,
            req: req.clone(),
            params: Vec::new(),
            texts: Vec::new(),
            candidates: Vec::new(),
            jac: empty_jacobian(),
            usable: Vec::new(),
            weights: Vec::new(),
            config: Vec::new(),
            scales: Vec::new(),
            base_rows: Vec::new(),
            taus: Vec::new(),
            budget: RunBudget::new(req.max_runs, req.max_wall),
            diag: DmDiagnostics::default(),
            log: Vec::new(),
            started,
            v1: None,
        };
        // Non-numeric goals: v1.
        if req.goals.iter().any(|g| matches!(g.target, GoalTarget::Value(_))) {
            let response = v1_fallback(env, &spec, req);
            spec.end(env);
            session.v1 = Some(response?);
            return Ok(session);
        }
        match session.prepare(env, host, checkpoint, &mut spec) {
            Ok(()) => {
                session.spec = Some(spec);
                Ok(session)
            }
            Err(e) => {
                spec.end(env);
                Err(e)
            }
        }
    }

    /// Everything [`open`](Self::open) does after the baseline.
    fn prepare(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        checkpoint: StackKey,
        spec: &mut SpecSession,
    ) -> Result<(), DmError> {
        let req = self.req.clone();
        let explain = req.explain;
        let pid = spec.program();
        let goals: Vec<Observable> = req.goals.iter().map(|g| g.observable.clone()).collect();
        for (i, g) in goals.iter().enumerate() {
            match spec.baseline().read(g) {
                Some(v) if v.is_finite() => self.diag.baseline.push(v),
                _ => {
                    return Err(DmError::BadObservable(format!(
                        "goal {i}: the baseline run has no number at {g:?}"
                    )));
                }
            }
        }
        let mut suppressed: Vec<String> = spec.baseline().suppressed.iter().map(|s| s.native.clone()).collect();
        suppressed.sort();
        suppressed.dedup();
        self.diag.suppressed = suppressed;
        self.taus = req
            .goals
            .iter()
            .map(|g| {
                let t = match g.target {
                    GoalTarget::Num(t) => t,
                    GoalTarget::Value(_) => 0.0,
                };
                g.tolerance.unwrap_or_else(|| default_tolerance(spec.baseline(), &g.observable, t))
            })
            .collect();
        self.diag.tolerances = self.taus.clone();

        // Discovery (§2.2).
        {
            let program = env.get_program(pid).ok_or(DmError::Spec(crate::dm_v2::SpecError::Stale))?;
            let hints = read_hints(program, env.native_table());
            let anchors = anchors_for(program, spec.baseline(), &goals);
            let found =
                discover_params(program, env.native_table(), &hints, &anchors, &spec.baseline().call_edges);
            self.texts = found.params.iter().map(|p| param_text(program, p)).collect();
            self.diag.warnings.extend(hints.warnings.iter().map(|w| w.message.clone()));
            self.diag.truncated = found.truncated;
            self.params = found.params;
        }
        self.diag.discovered = self.params.len();
        if self.params.is_empty() {
            return Err(DmError::NothingToMove { why: "no numeric literal in the program feeds the goals".into() });
        }
        let terms: Vec<TermId> = self.params.iter().map(|p| p.term).collect();
        spec.add_params(env, host, &terms)?;
        let loaded: Vec<bool> = self
            .params
            .iter()
            .map(|p| spec.param_index(p.term).is_some_and(|i| spec.baseline().loaded.contains(i)))
            .collect();

        // Eligibility (§2.5.1). The state rule is per slot: a `state`
        // initializer the checkpoint already holds is never evaluated by the
        // baseline, so the loaded filter takes it out (§9 M3).
        let (elig, strict): (Vec<Eligibility>, bool) = {
            let program = env.get_program(pid).unwrap();
            // The program declares knobs: a `config let`, or a `tunable`
            // that marks a literal (a hint-only program has knobs too).
            let strict = req.prefer == Prefer::ConfigStrict
                && (program.terms.iter().any(|t| t.is_config)
                    || read_hints(program, env.native_table())
                        .marks
                        .values()
                        .any(|h| matches!(h, Hint::Tunable { .. })));
            let e = self
                .params
                .iter()
                .zip(&loaded)
                .map(|(p, &ld)| {
                    let e = eligibility(program, p, &req.filter, EligibilityContext::default());
                    if !ld {
                        let why = if p.state_init {
                            "a `state` initializer whose state the checkpoint already holds"
                        } else {
                            "not evaluated by this frame"
                        };
                        return Eligibility { screen: false, edit: Err(why) };
                    }
                    if strict
                        && e.edit.is_ok()
                        && p.binding.is_some()
                        && !p.config
                        && !matches!(p.hint, Some(Hint::Tunable { .. }))
                    {
                        return Eligibility {
                            screen: false,
                            edit: Err("pinned: the program declares `config let` or `tunable` knobs (ConfigStrict)"),
                        };
                    }
                    e
                })
                .collect();
            (e, strict)
        };
        let eligible: Vec<usize> = (0..self.params.len()).filter(|&i| elig[i].screen).collect();
        self.diag.eligible = eligible.len();
        if explain {
            self.log.push(format!(
                "discovered {} literal(s), {} evaluated by the frame, {} eligible to screen",
                self.params.len(),
                loaded.iter().filter(|b| **b).count(),
                eligible.len()
            ));
        }

        // Screening may use at most 30% of the run budget and screening with
        // the Jacobian 60%, so the Jacobian and then the solve and its
        // verification always have runs left (§9 M5, revision 4).
        let full_budget = self.budget.max_runs;
        let start = self.budget.used;
        self.budget.max_runs = start + full_budget.saturating_mul(3) / 10;
        let mut screen = screen_params(
            spec,
            env,
            host,
            &self.params,
            &eligible,
            req.max_screen,
            &goals,
            &mut self.budget,
        );
        self.diag.screened = screen.screened;
        self.diag.budget_exhausted |= screen.budget_exhausted;
        if explain {
            self.log.push(format!(
                "screened {} of {} in {} run(s): {} move the goals",
                screen.screened,
                screen.eligible,
                screen.runs,
                screen.kept.len()
            ));
        }
        // At most MAX_MEASURE columns are measured: the cheapest by weight,
        // then the nearest (§9 M5, revision 4).
        if screen.kept.len() > MAX_MEASURE {
            let n = screen.kept.len();
            screen.kept.sort_by(|&a, &b| {
                let (pa, pb) = (&self.params[a], &self.params[b]);
                param_weight(pa, false)
                    .total_cmp(&param_weight(pb, false))
                    .then(pa.distance.cmp(&pb.distance))
                    .then(pa.span.start.offset.cmp(&pb.span.start.offset))
            });
            screen.kept.truncate(MAX_MEASURE);
            self.diag.warnings.push(format!(
                "{n} literals move the goals; only the {MAX_MEASURE} preferred ones were measured"
            ));
        }
        self.budget.max_runs = start + full_budget.saturating_mul(3) / 5;
        let mut rows = goals.clone();
        rows.extend(collateral_rows(spec.baseline(), &goals, COLLATERAL_ROWS));
        // With many columns, a column that does not move at the base step
        // is escalated once, not three times (§9 M5, revision 4): screening
        // infers some columns from their group without probing them alone,
        // and on a large program those cost the budget the solve needs.
        let mut fd = FdOptions { allow_respell: req.allow_respell, ..FdOptions::default() };
        if screen.kept.len() > MANY_COLUMNS {
            fd.max_escalations = fd.max_escalations.min(1);
        }
        let jac = jacobian(
            spec,
            env,
            host,
            &self.params,
            &screen.kept,
            rows,
            goals.len(),
            &Overrides::none(),
            &fd,
            &mut self.budget,
        );
        self.diag.measured = jac.params.len();
        self.diag.budget_exhausted |= jac.budget_exhausted;
        self.budget.max_runs = full_budget;
        let mut jac = jac;
        let mut base_rows = spec.baseline().read_all(&jac.rows);
        jac.drop_goal_aliases(&mut base_rows);
        self.base_rows = base_rows;

        // Columns.
        let params = &self.params;
        let ncols = jac.params.len();
        self.scales = jac.params.iter().map(|&i| params[i].value.abs().max(1.0)).collect();
        self.config = jac
            .params
            .iter()
            .map(|&i| params[i].config || matches!(params[i].hint, Some(Hint::Tunable { .. })))
            .collect();
        self.weights = jac.params.iter().map(|&i| param_weight(&params[i], false)).collect();
        self.usable = (0..ncols).map(|c| elig[jac.params[c]].edit.is_ok() && jac.quality[c].usable()).collect();
        if strict && (0..ncols).any(|c| self.usable[c] && self.config[c]) {
            for c in 0..ncols {
                self.usable[c] &= self.config[c];
            }
        }
        self.jac = jac;

        // Clock coupling (§2.5.4).
        let coupled = self.clock_coupling(env, host, checkpoint, pid);
        for c in 0..ncols {
            if coupled[c] {
                self.weights[c] = param_weight(&self.params[self.jac.params[c]], true);
            }
        }

        // The candidate report.
        let jac = &self.jac;
        self.candidates = self
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let col = jac.col_of(i);
                let dropped = eligible.contains(&i) && !screen.kept.contains(&i) && i_screened(&screen, &eligible, i);
                CandidateReport {
                    param: p.clone(),
                    eligible: elig[i].edit,
                    column: col.map(|c| jac.goal_column(c)).or_else(|| dropped.then(|| vec![0.0; goals.len()])),
                    quality: col.map(|c| jac.quality[c].clone()).or_else(|| dropped.then_some(ColumnQuality::Zero)),
                    weight: col.map_or_else(|| param_weight(p, false), |c| self.weights[c]),
                    clock_coupled: col.is_some_and(|c| coupled[c]),
                    loaded: loaded[i],
                }
            })
            .collect();
        if explain {
            for c in 0..ncols {
                let i = jac.params[c];
                self.log.push(format!(
                    "column `{}` (line {}): {:?} {:?}, weight {:.2}{}{}",
                    self.texts[i].text,
                    self.params[i].span.start.line,
                    jac.goal_column(c),
                    jac.quality[c],
                    self.weights[c],
                    if coupled[c] { ", clock-coupled" } else { "" },
                    if self.usable[c] { "" } else { " (not editable)" }
                ));
            }
        }
        Ok(())
    }

    /// Which usable columns are an animation rate (§2.5.4, as revised in §9
    /// M3): measured again with the clock bindings shifted (`time += 0.37`,
    /// `frame_count += 1`), a column is coupled when it changes by more than
    /// 25% (or flips sign) *and* points along the goal emits' own velocity
    /// (the way `time() * rate` moves a shape along its path). A radius or a
    /// position moves the shape across its path, or not with the clock.
    fn clock_coupling(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        checkpoint: StackKey,
        pid: crate::program::ProgramId,
    ) -> Vec<bool> {
        let ncols = self.jac.params.len();
        let mut out = vec![false; ncols];
        if ncols == 0 {
            return out;
        }
        let reads_clock = env.get_program(pid).is_some_and(|program| {
            program.terms.iter().any(|t| {
                let TermOp::BuiltinCall(c) = t.op else { return false };
                let crate::constant_table::ConstantValue::String(name) = program.constants.get(c) else {
                    return false;
                };
                env.native_table()
                    .lookup_name(name)
                    .is_some_and(|id| env.native_table().effects(id).reads.contains(InputClasses::CLOCK))
            })
        });
        if !reads_clock {
            return out;
        }
        let mut shifts: Vec<(crate::symbol::SymbolId, Value)> = Vec::new();
        for (name, by) in [("time", 0.37), ("frame_count", 1.0)] {
            let sym = env.intern_symbol(name);
            match env.binding_for(checkpoint, sym) {
                Some(Value::Float(f)) => shifts.push((sym, Value::Float(f + by))),
                Some(Value::Int(i)) => shifts.push((sym, Value::Int(i + by.ceil() as i64))),
                _ => {}
            }
        }
        if shifts.is_empty() {
            return out;
        }
        // The strongest few usable columns.
        let mut top: Vec<usize> = (0..ncols).filter(|&c| self.usable[c]).collect();
        top.sort_by(|&a, &b| {
            strength(&self.jac, b, self.scales[b], self.weights[b])
                .total_cmp(&strength(&self.jac, a, self.scales[a], self.weights[a]))
        });
        top.truncate(4);
        if top.is_empty() || self.budget.max_runs.saturating_sub(self.budget.used) < 2 * top.len() + 8 {
            return out;
        }
        let Ok(fork) = env.fork_execution(checkpoint) else { return out };
        for (sym, v) in &shifts {
            env.set_binding_for(fork, *sym, *v);
        }
        let terms: Vec<TermId> = top.iter().map(|&c| self.params[self.jac.params[c]].term).collect();
        let shifted = SpecSession::begin(env, host, fork, &terms, self.req.spec.clone(), None);
        let Ok(mut shifted) = shifted else {
            env.drop_fork(fork);
            return out;
        };
        let keep: Vec<usize> = top.iter().map(|&c| self.jac.params[c]).collect();
        let fd = FdOptions { allow_respell: self.req.allow_respell, ..FdOptions::default() };
        let j2 = jacobian(
            &mut shifted,
            env,
            host,
            &self.params,
            &keep,
            self.jac.rows.clone(),
            self.jac.goal_rows,
            &Overrides::none(),
            &fd,
            &mut self.budget,
        );
        // The rows of the goals' own emits, and their velocity.
        let keys: Vec<EmitKey> = self.req.goals.iter().filter_map(|g| emit_key_of(&g.observable)).collect();
        let own: Vec<usize> = (0..self.jac.rows.len())
            .filter(|&r| emit_key_of(&self.jac.rows[r]).is_some_and(|k| keys.contains(&k)))
            .collect();
        let vel: Vec<f64> = own
            .iter()
            .map(|&r| match (shifted.baseline().read(&self.jac.rows[r]), self.base_rows[r]) {
                (Some(a), Some(b)) => a - b,
                _ => 0.0,
            })
            .collect();
        let norm = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
        for (k, &c) in top.iter().enumerate() {
            if !j2.quality[k].usable() {
                continue;
            }
            let a: Vec<f64> = (0..self.jac.goal_rows).map(|r| self.jac.get(r, c)).collect();
            let b: Vec<f64> = (0..self.jac.goal_rows).map(|r| j2.get(r, k)).collect();
            let changed = a.iter().zip(&b).any(|(x, y)| {
                (x - y).abs() > 0.25 * x.abs().max(y.abs()) || (x * y < 0.0)
            });
            let col: Vec<f64> = own.iter().map(|&r| self.jac.get(r, c)).collect();
            let (nc, nv) = (norm(&col), norm(&vel));
            let along = nc > 0.0 && nv > 0.0 && {
                let dot: f64 = col.iter().zip(&vel).map(|(x, y)| x * y).sum();
                (dot / (nc * nv)).abs() > 0.95
            };
            out[c] = changed && along;
            if self.req.explain {
                let i = self.jac.params[c];
                self.log.push(format!(
                    "clock shift: `{}` column {:?} → {:?}, along the motion: {along}{}",
                    self.texts[i].text,
                    a,
                    b,
                    if out[c] { " (coupled)" } else { "" }
                ));
            }
        }
        shifted.end(env);
        env.drop_fork(fork);
        out
    }

    /// Speculative runs made so far (baselines not counted).
    pub fn runs_used(&self) -> usize {
        self.spec.as_ref().map_or(0, |s| s.runs_used())
    }

    /// The candidate report.
    pub fn candidates(&self) -> &[CandidateReport] {
        &self.candidates
    }

    /// The baseline value of each goal.
    pub fn baseline_goals(&self) -> &[f64] {
        &self.diag.baseline
    }

    /// Give [`finish`](Self::finish) a fresh budget: `max_runs` more runs
    /// within `max_wall` from now. A drag holds the session open for as long
    /// as the pointer is down, and that time is the user's, not the
    /// solver's: without this, a drag held longer than the request's
    /// `max_wall` would finish with no room to verify anything.
    pub fn renew_budget(&mut self, max_runs: usize, max_wall: Duration) {
        self.budget = RunBudget::new(max_runs, max_wall);
    }

    /// Close the session without solving (a cancelled drag), dropping its
    /// fork. Dropping a session without this or `finish` leaks the fork's
    /// heap until the Env is dropped.
    pub fn cancel(mut self, env: &mut Env) {
        if let Some(spec) = self.spec.take() {
            spec.end(env);
        }
    }

    fn goal_rows(&self, targets: &[f64]) -> Vec<GoalRow> {
        self.req
            .goals
            .iter()
            .zip(targets)
            .zip(&self.taus)
            .map(|((g, &t), &tau)| GoalRow { target: t, tolerance: tau.max(1e-300), weight: g.weight })
            .collect()
    }

    fn bounds(&self) -> Vec<Bound> {
        self.jac.params.iter().map(|&i| Bound::of(&self.params[i], self.req.allow_respell)).collect()
    }

    /// The linear answer of every candidate subset at `targets`, best first
    /// (§2.5.3): each subset's step against the cached Jacobian, projected,
    /// and the score it predicts. No runs.
    fn predictions(&self, targets: &[f64]) -> Vec<Prediction> {
        let goals = self.goal_rows(targets);
        let gr = self.jac.goal_rows;
        let client = self.req.filter.allow.as_ref().is_some_and(|a| {
            a.iter().any(|s| {
                !matches!(
                    s,
                    ParamSelector::AllUserCode
                        | ParamSelector::ReportLibrary
                        | ParamSelector::AllowPathDeps
                        | ParamSelector::IncludeStateInit
                        | ParamSelector::IncludeStructural
                        | ParamSelector::IncludeThroughRng
                )
            })
        });
        let subsets = candidate_subsets(&self.jac, &self.weights, &self.scales, &self.usable, &self.config, client);
        let bounds = self.bounds();
        let r: Vec<f64> =
            goals.iter().zip(&self.base_rows[..gr]).map(|(g, v)| g.target - v.unwrap_or(g.target)).collect();
        let wt: Vec<f64> = goals.iter().map(|g| g.weight / g.tolerance).collect();
        let value = |c: usize| self.params[self.jac.params[c]].value;
        let linear = |cols: &[usize]| -> (Vec<Vec<f64>>, Vec<f64>) {
            let local: Vec<Vec<f64>> =
                (0..self.jac.rows.len()).map(|row| cols.iter().map(|&c| self.jac.get(row, c)).collect()).collect();
            let cost: Vec<f64> = cols.iter().map(|&c| self.weights[c] / self.scales[c]).collect();
            let delta = linear_step(&local[..gr], &local[gr..], &r, &wt, &cost);
            (local, delta)
        };
        let rn = r.iter().map(|x| x * x).sum::<f64>().sqrt();
        let r1 = drag_norm(&r);
        let row_keys: Vec<Option<EmitKey>> = self.jac.rows[gr..].iter().map(emit_key_of).collect();
        let mut out: Vec<Prediction> = subsets
            .into_iter()
            .map(|mut s| {
                let (mut local, mut delta) = linear(&s.cols);
                // A spread over many literals keeps only the ones that do a
                // real share of the work (§9 M3): a literal moving the goals
                // by less than a tenth of the drag is left alone, and the
                // rest solved again.
                while s.kind != SubsetKind::Tuple && s.cols.len() > gr {
                    let keep: Vec<usize> = (0..s.cols.len())
                        .filter(|&k| {
                            let c: f64 = (0..gr).map(|g| (local[g][k] * delta[k]).powi(2)).sum::<f64>().sqrt();
                            c >= 0.1 * rn
                        })
                        .collect();
                    if keep.len() == s.cols.len() || keep.len() < gr {
                        break;
                    }
                    let cols: Vec<usize> = keep.iter().map(|&k| s.cols[k]).collect();
                    let jg: Vec<Vec<f64>> = (0..gr).map(|g| keep.iter().map(|&k| local[g][k]).collect()).collect();
                    if crate::dm_v2::solve::rank(&jg) < gr {
                        break;
                    }
                    s.cols = cols;
                    (local, delta) = linear(&s.cols);
                }
                let values: Vec<f64> =
                    s.cols.iter().zip(&delta).map(|(&c, &d)| bounds[c].project(value(c) + d)).collect();
                let moved: Vec<f64> = s.cols.iter().zip(&values).map(|(&c, &v)| v - value(c)).collect();
                let rows: Vec<Option<f64>> = local
                    .iter()
                    .zip(&self.base_rows)
                    .map(|(row, b)| b.map(|b| b + row.iter().zip(&moved).map(|(j, d)| j * d).sum::<f64>()))
                    .collect();
                let err = max_error(&goals, &rows[..gr]);
                let goal_moves: Vec<f64> = rows[..gr]
                    .iter()
                    .zip(&self.base_rows[..gr])
                    .map(|(a, b)| a.unwrap_or(0.0) - b.unwrap_or(0.0))
                    .collect();
                let coll: Vec<Option<f64>> = rows[gr..]
                    .iter()
                    .zip(&self.base_rows[gr..])
                    .map(|(a, b)| Some(a.unwrap_or(0.0) - b.unwrap_or(0.0)))
                    .collect();
                let respelled = s
                    .cols
                    .iter()
                    .zip(&values)
                    .filter(|&(&c, v)| self.params[self.jac.params[c]].is_int && v.fract() != 0.0)
                    .count();
                let changes: Vec<ScoredChange> = s
                    .cols
                    .iter()
                    .zip(&values)
                    .map(|(&c, &v)| ScoredChange {
                        weight: self.weights[c],
                        old: value(c),
                        new: v,
                        share: Some(share(&self.jac, c, v - value(c), r1)),
                    })
                    .collect();
                let facts = ScoreFacts {
                    collateral_norm: collateral_shapes(&row_keys, &coll, &goal_moves),
                    goals: gr,
                    respelled,
                    verified: err <= 1.0,
                    max_error: err,
                    ..Default::default()
                };
                let parts = score(&changes, &facts, &ScoreWeights::default());
                Prediction {
                    cols: s.cols,
                    kind: s.kind,
                    values,
                    goals: rows[..gr].iter().map(|v| v.unwrap_or(f64::NAN)).collect(),
                    ok: err <= 1.0,
                    score: parts.total(),
                }
            })
            .collect();
        out.sort_by(|a, b| b.ok.cmp(&a.ok).then(a.score.total_cmp(&b.score)));
        // A pruned spread that came down to a tuple is that tuple, and a
        // tuple whose prediction leaves one of its literals where it is is
        // the smaller tuple (§9 M5, revision 3): `(cy, len)` with `len`
        // unchanged is `cy` again, and solving it twice spends a verified
        // slot that a different alternative could have had.
        let mut seen: Vec<Vec<usize>> = Vec::new();
        out.retain(|p| {
            let mut c: Vec<usize> = p
                .cols
                .iter()
                .zip(&p.values)
                .filter(|&(&c, &v)| (v - value(c)).abs() > 1e-9 * value(c).abs().max(1.0))
                .map(|(&c, _)| c)
                .collect();
            if c.is_empty() {
                c = p.cols.clone();
            }
            c.sort_unstable();
            if seen.contains(&c) {
                return false;
            }
            seen.push(c);
            true
        });
        out
    }

    /// The best subset's linear answer at `targets`, with no runs: what a
    /// client shows while a drag is in progress.
    pub fn preview(&self, targets: &[f64]) -> Vec<ParamChange> {
        if self.v1.is_some() || targets.len() != self.req.goals.len() {
            return Vec::new();
        }
        let Some(best) = self.predictions(targets).into_iter().next() else { return Vec::new() };
        best.cols
            .iter()
            .zip(&best.values)
            .filter(|&(&c, &v)| v != self.params[self.jac.params[c]].value)
            .map(|(&c, &v)| {
                let i = self.jac.params[c];
                let v = crate::dm_v2::solve::round_to(v, 6);
                let (p, t) = (&self.params[i], &self.texts[i]);
                let (mut text, respelled) = render(p, t, v);
                if v < 0.0 && !p.negated && needs_parens_by_neighbours(t) {
                    text = format!("({text})");
                }
                change_of(p, t, v, (text, respelled))
            })
            .collect()
    }

    /// Solve at `targets`, verify, round, rank, and close the session.
    pub fn finish(mut self, env: &mut Env, host: &mut dyn SpecHost, targets: &[f64]) -> Result<DmResponse, DmError> {
        if let Some(mut response) = self.v1.take() {
            if self.req.explain {
                response.explain = Some(ExplainLog { lines: vec!["a goal is not numeric: answered by v1".into()] });
            }
            return Ok(response);
        }
        let Some(mut spec) = self.spec.take() else {
            return Err(DmError::Spec(crate::dm_v2::SpecError::Stale));
        };
        let result = self.solve_all(env, host, &mut spec, targets);
        spec.end(env);
        result
    }

    fn solve_all(
        &mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        spec: &mut SpecSession,
        targets: &[f64],
    ) -> Result<DmResponse, DmError> {
        if targets.len() != self.req.goals.len() || targets.iter().any(|t| !t.is_finite()) {
            return Err(DmError::BadObservable("one finite target per goal is needed".into()));
        }
        // A default tolerance scales with the target (§2.4.3), which a
        // session opened for a drag (or a `--by` goal) learns only now.
        for (i, g) in self.req.goals.iter().enumerate() {
            if g.tolerance.is_none() {
                self.taus[i] = default_tolerance(spec.baseline(), &g.observable, targets[i]);
            }
        }
        self.diag.tolerances = self.taus.clone();
        let goals = self.goal_rows(targets);
        let gr = self.jac.goal_rows;
        let explain = self.req.explain;
        let drag = drag_norm(
            &goals.iter().zip(&self.base_rows[..gr]).map(|(g, v)| g.target - v.unwrap_or(g.target)).collect::<Vec<_>>(),
        );
        let mut alternatives: Vec<Alternative> = Vec::new();
        if max_error(&goals, &self.base_rows[..gr]) <= 1.0 {
            self.diag.warnings.push("the goals already hold at the baseline: nothing to change".into());
        } else if !self.usable.iter().any(|b| *b) {
            if !self.diag.budget_exhausted {
                return Err(DmError::NothingToMove { why: nothing_to_move(&self.candidates) });
            }
            self.diag.warnings.push("the run budget ran out before any literal was measured".into());
        } else {
            let preds = self.predictions(targets);
            self.diag.subsets_predicted = preds.len();
            let free = self.req.max_alternatives + 2;
            let mut chosen: Vec<Prediction> = Vec::new();
            let mut n_free = 0;
            let mut rest: Vec<Prediction> = Vec::new();
            for p in preds {
                match p.kind {
                    SubsetKind::ConfigOnly | SubsetKind::Client => chosen.push(p),
                    _ if n_free < free => {
                        n_free += 1;
                        chosen.push(p);
                    }
                    _ => rest.push(p),
                }
            }
            // With several goals, the single literals whose linear prediction
            // comes closest among those not chosen yet are solved too, even
            // when that prediction misses: a curve (an angle) can reach
            // targets its tangent cannot (§9 M5, revision 3).
            if gr >= 2 {
                let miss = |p: &Prediction| -> f64 {
                    p.goals.iter().zip(&goals).map(|(a, g)| ((a - g.target) / g.tolerance).abs()).fold(0.0, f64::max)
                };
                // The closest, and the closest whose column is smooth (a
                // float literal under trig or a product: the kind whose
                // straight-line prediction understates what it can reach).
                for smooth_only in [false, true] {
                    let pick = (0..rest.len())
                        .filter(|&k| rest[k].cols.len() == 1 && rest[k].goals.iter().all(|v| v.is_finite()))
                        .filter(|&k| {
                            !smooth_only || matches!(self.jac.quality[rest[k].cols[0]], ColumnQuality::Smooth)
                        })
                        .min_by(|&a, &b| miss(&rest[a]).total_cmp(&miss(&rest[b])));
                    if let Some(k) = pick {
                        chosen.push(rest.swap_remove(k));
                    }
                }
            }
            let pid = spec.program();
            let emit_info = emit_info(env, pid, spec.baseline());
            let baseline_fp = spec.baseline().fingerprint;
            let drew_random = spec.drew_random();
            let bounds = self.bounds();
            let opts = SolveOptions {
                max_iters: self.req.max_iters,
                allow_respell: self.req.allow_respell,
                fd: FdOptions { allow_respell: self.req.allow_respell, ..FdOptions::default() },
            };
            let mut ctx = SolveCtx {
                session: spec,
                env,
                host,
                params: &self.params,
                jac: &self.jac,
                base_rows: self.base_rows.clone(),
                goals: &goals,
                weights: &self.weights,
                bounds,
                budget: &mut self.budget,
                opts,
                runs: 0,
                budget_exhausted: false,
                log: if explain { Some(&mut self.log) } else { None },
            };
            for pred in chosen {
                if !ctx.budget.has_room() {
                    ctx.budget_exhausted = true;
                    break;
                }
                self.diag.subsets_verified += 1;
                let names: Vec<&str> = pred.cols.iter().map(|&c| self.texts[self.jac.params[c]].text.as_str()).collect();
                if let Some(log) = ctx.log.as_deref_mut() {
                    log.push(format!("subset {:?} {names:?}: predicted {:?} → {:?}", pred.kind, pred.values, pred.goals));
                }
                let sol = ctx.solve_subset(&pred.cols);
                let texts: Vec<String> = pred.cols.iter().map(|&c| self.texts[self.jac.params[c]].text.clone()).collect();
                let rounded = ctx.round_solution(&sol, &texts);
                // Render, and read the text back: what is verified is what
                // will be written (§2.4.5).
                let mut rendered: Vec<(usize, f64, String, bool)> = Vec::new();
                for (k, &c) in pred.cols.iter().enumerate() {
                    let i = self.jac.params[c];
                    let p = &self.params[i];
                    let v = rounded.values[k];
                    if v == p.value {
                        continue;
                    }
                    let (mut text, respelled) = render(p, &self.texts[i], v);
                    if v < 0.0 && !p.negated && needs_parens(ctx.env, pid, p, &self.texts[i], &text) {
                        text = format!("({text})");
                    }
                    let back = text.trim_start_matches('(').trim_end_matches(')').parse::<f64>().unwrap_or(v);
                    rendered.push((i, back, text, respelled));
                }
                if rendered.is_empty() {
                    continue;
                }
                let values: Vec<f64> = pred
                    .cols
                    .iter()
                    .map(|&c| {
                        let i = self.jac.params[c];
                        rendered.iter().find(|r| r.0 == i).map_or(self.params[i].value, |r| r.1)
                    })
                    .collect();
                let (run, rows) = if values == rounded.values && rounded.run.is_some() {
                    (rounded.run, rounded.rows)
                } else {
                    match ctx.run_at(&pred.cols, &values) {
                        Some(r) => {
                            let rows = ctx.read_rows(&r);
                            (Some(r), rows)
                        }
                        None => (None, vec![None; self.jac.rows.len()]),
                    }
                };
                let Some(run) = run else { continue };
                let achieved: Vec<f64> = rows[..gr].iter().map(|v| v.unwrap_or(f64::NAN)).collect();
                let err = if run.ok() { max_error(&goals, &rows[..gr]) } else { f64::INFINITY };
                let verified = err <= 1.0;
                let regime_changed = run.fingerprint != baseline_fp;
                // The sequential-draw check (§2.1.4).
                let (mut rng_order_changed, mut rng_difference) = (false, None);
                if drew_random && verified && ctx.budget.take() {
                    ctx.runs += 1;
                    let ov = ctx.overrides(&pred.cols, &values);
                    let fork = ctx.session.run_fork_stream(ctx.env, ctx.host, &ov);
                    if let Some(d) = run.first_difference(&fork) {
                        rng_order_changed = true;
                        rng_difference = Some(d);
                    }
                }
                // Collateral, measured.
                let goal_moves: Vec<f64> = (0..gr)
                    .map(|g| rows[g].unwrap_or(f64::NAN) - self.base_rows[g].unwrap_or(f64::NAN))
                    .map(|x| if x.is_finite() { x } else { 0.0 })
                    .collect();
                let moves: Vec<Option<f64>> = (gr..self.jac.rows.len())
                    .map(|r| match (rows[r], self.base_rows[r]) {
                        (Some(a), Some(b)) => Some(a - b),
                        (None, None) => Some(0.0),
                        _ => None,
                    })
                    .collect();
                let collateral = collateral_items(&self.jac, gr, &moves, &emit_info, &goal_moves);
                let changes: Vec<ParamChange> = rendered
                    .iter()
                    .map(|(i, v, text, respelled)| {
                        change_of(&self.params[*i], &self.texts[*i], *v, (text.clone(), *respelled))
                    })
                    .collect();
                let scored: Vec<ScoredChange> = rendered
                    .iter()
                    .map(|(i, v, _, _)| {
                        let c = self.jac.col_of(*i).unwrap();
                        let old = self.params[*i].value;
                        ScoredChange {
                            weight: self.weights[c],
                            old,
                            new: *v,
                            share: Some(share(&self.jac, c, *v - old, drag)),
                        }
                    })
                    .collect();
                let facts = ScoreFacts {
                    collateral_norm: collateral.shapes,
                    goals: gr,
                    respelled: changes.iter().filter(|c| c.respelled).count(),
                    regime_changed,
                    rng_order_changed,
                    verified,
                    max_error: err,
                };
                let parts = score(&scored, &facts, &ScoreWeights::default());
                let edits: Vec<FileEdit> = changes
                    .iter()
                    .map(|c| FileEdit {
                        file: c.file,
                        path: self.texts[self.params.iter().position(|p| p.term == c.term).unwrap()].path.clone(),
                        edit: SourceEdit { span: c.span, new_text: c.new_text.clone() },
                    })
                    .collect();
                let stateful = rendered.iter().any(|(i, ..)| self.params[*i].through_state);
                let config_only = changes.iter().all(|c| c.config);
                let description = describe(&changes, &collateral, verified, &achieved, &goals);
                if let Some(log) = ctx.log.as_deref_mut() {
                    log.push(format!(
                        "  → {description}: achieved {achieved:?}, error {err:.3}, score {:.3} ({} step(s))",
                        parts.total(),
                        sol.iterations
                    ));
                }
                alternatives.push(Alternative {
                    edits,
                    changes,
                    achieved,
                    predicted: pred.goals.clone(),
                    max_error: err,
                    verified,
                    collateral,
                    stateful,
                    config_only,
                    regime_changed,
                    rng_order_changed,
                    rng_difference,
                    score: parts.total(),
                    score_parts: parts,
                    kind: pred.kind,
                    iterations: sol.iterations,
                    engine: Engine::V2,
                    description,
                });
            }
            self.diag.budget_exhausted |= ctx.budget_exhausted;
        }
        // The count penalty is charged against the leanest verified
        // alternative, not the goal count (§9 M5, revision 2): when one
        // literal can do it, a spread over three pays for two extra edits.
        if let Some(lean) = alternatives.iter().filter(|a| a.verified).map(|a| a.changes.len()).min() {
            let k = ScoreWeights::default();
            for a in &mut alternatives {
                a.score_parts.count = k.kn * a.changes.len().saturating_sub(lean.max(1)) as f64;
                a.score = a.score_parts.total();
            }
        }
        // Rank (§2.5.4): verified first; under the soft config preference,
        // verified config-only alternatives first among those; then score.
        let any_verified = alternatives.iter().any(|a| a.verified);
        let soft = self.req.prefer == Prefer::Soft;
        alternatives.sort_by(|a, b| {
            let tier = |x: &Alternative| -> u8 {
                match (x.verified, soft && x.config_only) {
                    (true, true) => 0,
                    (true, false) => 1,
                    (false, _) if any_verified => 3,
                    (false, _) => 2,
                }
            };
            tier(a).cmp(&tier(b)).then(a.score.total_cmp(&b.score))
        });
        let mut seen: Vec<Vec<(u16, u32, u32, String)>> = Vec::new();
        alternatives.retain(|a| {
            let mut key: Vec<(u16, u32, u32, String)> = a
                .edits
                .iter()
                .map(|e| (e.file.0, e.edit.span.start.offset, e.edit.span.end.offset, e.edit.new_text.clone()))
                .collect();
            key.sort();
            if seen.contains(&key) {
                return false;
            }
            seen.push(key);
            true
        });
        alternatives.truncate(self.req.max_alternatives.max(1));
        self.diag.runs = spec.runs_used();
        self.diag.wall = match (self.started, crate::env::spec_run::wall_now()) {
            (Some(a), Some(b)) => Some(b.duration_since(a)),
            _ => None,
        };
        self.diag.engine = Some(Engine::V2);
        Ok(DmResponse {
            alternatives,
            candidates: std::mem::take(&mut self.candidates),
            diagnostics: std::mem::take(&mut self.diag),
            explain: explain.then(|| ExplainLog { lines: std::mem::take(&mut self.log) }),
        })
    }
}

/// The size of a drag in output units, `‖target - baseline‖₁` over the goals.
fn drag_norm(residual: &[f64]) -> f64 {
    residual.iter().filter(|x| x.is_finite()).map(|x| x.abs()).sum()
}

/// The share of a drag of size `drag` that moving column `c` by `delta`
/// carries, by the linearization: `‖J_g[:, c] · delta‖₁ / drag` (§9 M5,
/// revision 1). In the L1 norm a pair of literals that each move their own
/// goal (`cx`, `cy`) carries exactly the drag, while a pair where one undoes
/// the other's side effect (`angle`, then `cx` to correct x) carries more.
fn share(jac: &Jacobian, c: usize, delta: f64, drag: f64) -> f64 {
    if drag <= 1e-12 {
        return 0.0;
    }
    let moved: f64 = (0..jac.goal_rows).map(|r| (jac.get(r, c) * delta).abs()).sum();
    if moved.is_finite() { moved / drag } else { 0.0 }
}

/// Whether `screen` got to parameter `i` (screening stops at `max_screen`,
/// nearest first).
fn i_screened(screen: &crate::dm_v2::params::ScreenResult, eligible: &[usize], i: usize) -> bool {
    screen.screened >= eligible.len() || eligible.iter().position(|&e| e == i).is_some_and(|k| k < screen.screened)
}

/// Why nothing can move the goals, from the candidate report.
fn nothing_to_move(candidates: &[CandidateReport]) -> String {
    let mut reasons: Vec<String> = Vec::new();
    for c in candidates.iter().take(8) {
        let why = match (&c.eligible, &c.quality) {
            (Err(e), _) => (*e).to_string(),
            (Ok(()), Some(q)) if !q.usable() => format!("{q:?}"),
            (Ok(()), Some(ColumnQuality::Zero)) | (Ok(()), None) => "does not move the goals".into(),
            (Ok(()), Some(_)) => "not measured".into(),
        };
        reasons.push(format!("line {}: {why}", c.param.span.start.line));
    }
    if reasons.is_empty() {
        "no literal moves the goals".into()
    } else {
        format!("no editable literal moves the goals ({})", reasons.join("; "))
    }
}

/// The emit key an observable reads.
fn emit_key_of(o: &Observable) -> Option<EmitKey> {
    match o {
        Observable::EmitArg { key, .. } | Observable::EmitValue { key, .. } => Some(*key),
        Observable::Watch { at, .. } => Some(*at),
    }
}

/// The default tolerance of §2.4.3: half a pixel for a raw argument of a
/// native whose emitted value holds only ints (it truncates), else
/// `1e-6 · max(|t|, 1)`.
fn default_tolerance(run: &SpecRun, o: &Observable, target: f64) -> f64 {
    if let Observable::EmitArg { key, .. } = o
        && let Some(e) = run.emit(key)
    {
        fn leaves(v: &ObsValue, ints: &mut usize, floats: &mut usize) {
            match v {
                ObsValue::Int(_) => *ints += 1,
                ObsValue::Num(_) => *floats += 1,
                ObsValue::List(items) => items.iter().for_each(|x| leaves(x, ints, floats)),
                ObsValue::Variant { data, .. } => data.iter().for_each(|x| leaves(x, ints, floats)),
                ObsValue::Record(fields) => fields.iter().for_each(|(_, x)| leaves(x, ints, floats)),
                _ => {}
            }
        }
        let (mut ints, mut floats) = (0, 0);
        leaves(&e.value, &mut ints, &mut floats);
        if ints > 0 && floats == 0 {
            return 0.5;
        }
    }
    1e-6 * target.abs().max(1.0)
}

/// The literal's text and surroundings.
fn param_text(program: &Program, p: &Param) -> ParamText {
    let file = program.source_map.files.get(p.file.0 as usize);
    let src: Vec<char> = file.map(|f| f.source.chars().collect()).unwrap_or_default();
    let (s, e) = (p.span.start.offset as usize, p.span.end.offset as usize);
    let text: String = src.get(s.min(src.len())..e.min(src.len())).map(|c| c.iter().collect()).unwrap_or_default();
    let prev = src[..s.min(src.len())].iter().rev().find(|c| !c.is_whitespace()).copied();
    let next = src[e.min(src.len())..].iter().find(|c| !c.is_whitespace()).copied();
    let function = p.function.and_then(|f| program.functions.get(f.0 as usize)).and_then(|d| d.name.clone()).map(|n| {
        let n = crate::program::base_fn_name(&n).to_string();
        n.rsplit("::").next().unwrap_or(&n).to_string()
    });
    let path = file.and_then(|f| f.origin.clone()).filter(|o| !o.to_string_lossy().starts_with('<'));
    ParamText { text, prev, next, function, path }
}

/// `v` written the way `p` was (§2.4.4): an int stays an int when whole; a
/// float keeps its padded decimals (`3.50`), else the shortest spelling with
/// a decimal point. Returns the text and whether an int became a float.
fn render(p: &Param, t: &ParamText, v: f64) -> (String, bool) {
    let respelled = p.is_int && v.fract() != 0.0;
    let v = if v == 0.0 { 0.0 } else { v };
    if p.is_int && !respelled {
        return (format!("{}", v as i64), false);
    }
    let model = t.text.trim_start_matches('-');
    (crate::literal_edit::float_like(v, if p.is_int { "" } else { model }), respelled)
}

/// Whether a negative replacement for the non-negated literal `p` must be
/// parenthesized: it would read `x - -3` (or `x + -3`), a postfix operator
/// follows it (`5.f()` as `-2.f()` is `-(2.f())`, and `5[i]` likewise), or
/// the file would not parse with it.
fn needs_parens(env: &Env, pid: crate::program::ProgramId, p: &Param, t: &ParamText, text: &str) -> bool {
    if needs_parens_by_neighbours(t) {
        return true;
    }
    let Some(program) = env.get_program(pid) else { return false };
    let Some(file) = program.source_map.files.get(p.file.0 as usize) else { return false };
    let edited = crate::rewrite::splice(&file.source, p.span, text);
    crate::cst::parse_source(&edited, p.file).is_err()
}

/// The half of [`needs_parens`] that reads only the literal's neighbours.
fn needs_parens_by_neighbours(t: &ParamText) -> bool {
    matches!(t.prev, Some('-' | '+')) || matches!(t.next, Some('.' | '['))
}

fn change_of(p: &Param, t: &ParamText, v: f64, (new_text, respelled): (String, bool)) -> ParamChange {
    ParamChange {
        term: p.term,
        span: p.span,
        file: p.file,
        binding: p.binding.as_ref().map(|b| b.rsplit("::").next().unwrap_or(b).to_string()),
        function: t.function.clone(),
        config: p.config || matches!(p.hint, Some(Hint::Tunable { .. })),
        old: p.value,
        new: v,
        old_text: t.text.clone(),
        new_text,
        respelled,
    }
}

/// What each baseline emit is, for collateral items: channel, native, line.
fn emit_info(env: &Env, pid: crate::program::ProgramId, base: &SpecRun) -> HashMap<EmitKey, (String, Option<String>, Option<u32>)> {
    let program = env.get_program(pid);
    let origins = program.map(file_origins).unwrap_or_default();
    base.emits
        .iter()
        .map(|e| {
            let channel = env.symbol_name(e.channel).unwrap_or("?").to_string();
            let callee = e.native.map(|id| env.native_table().get_name(id).to_string());
            let line = program.and_then(|program| {
                e.chain.chain.iter().find_map(|t| {
                    let span = program.source_map.get(*t)?;
                    let user = origins.get(span.file.0 as usize).is_some_and(|o| *o != crate::dm_v2::CodeOrigin::Library);
                    user.then_some(span.start.line)
                })
            });
            (e.key, (channel, callee, line))
        })
        .collect()
}

/// The collateral of a verification run: rows grouped by emit.
fn collateral_items(
    jac: &Jacobian,
    gr: usize,
    moves: &[Option<f64>],
    info: &HashMap<EmitKey, (String, Option<String>, Option<u32>)>,
    goal_moves: &[f64],
) -> Collateral {
    let mut items: Vec<CollateralItem> = Vec::new();
    for (k, m) in moves.iter().enumerate() {
        let moved = match m {
            Some(x) => x.abs() > 1e-9 * (1.0 + x.abs()) && x.abs() > 1e-9,
            None => true,
        };
        if !moved {
            continue;
        }
        let Some(key) = emit_key_of(&jac.rows[gr + k]) else { continue };
        let mag = m.map_or(f64::INFINITY, f64::abs);
        if let Some(it) = items.iter_mut().find(|i| i.key == key) {
            it.max_move = it.max_move.max(mag);
            continue;
        }
        let (channel, callee, line) = info.get(&key).cloned().unwrap_or(("?".into(), None, None));
        items.push(CollateralItem { key, channel, callee, line, max_move: mag });
    }
    let keys: Vec<Option<EmitKey>> = jac.rows[gr..].iter().map(emit_key_of).collect();
    Collateral {
        norm: collateral_norm(goal_moves, moves),
        shapes: collateral_shapes(&keys, moves, goal_moves),
        items,
    }
}

/// "set `gap` to 14 (line 3); moves 4 other emits".
fn describe(changes: &[ParamChange], coll: &Collateral, verified: bool, achieved: &[f64], goals: &[GoalRow]) -> String {
    let parts: Vec<String> = changes
        .iter()
        .map(|c| {
            let what = match (&c.binding, &c.function) {
                (Some(b), _) => format!("set `{b}` to {}", c.new_text),
                (None, Some(f)) => format!("change `{}` to {} in `{f}`", c.old_text, c.new_text),
                (None, None) => format!("change `{}` to {}", c.old_text, c.new_text),
            };
            let line = c.span.start.line;
            if c.file.0 == 0 { format!("{what} (line {line})") } else { format!("{what} (file {}, line {line})", c.file.0) }
        })
        .collect();
    let mut s = parts.join(", ");
    match coll.items.len() {
        0 => s.push_str("; moves nothing else"),
        1 => s.push_str("; also moves 1 other emit"),
        n => s.push_str(&format!("; also moves {n} other emits")),
    }
    if !verified {
        let worst = achieved
            .iter()
            .zip(goals)
            .map(|(a, g)| (a - g.target).abs())
            .fold(0.0, f64::max);
        s.push_str(&format!("; misses the goal by {worst:.3}"));
    }
    s
}

/// v1 for a request with a non-numeric goal: each goal is mapped to the
/// argument of the call that made its emit, and v1's proposals (not re-run)
/// become the alternatives.
fn v1_fallback(env: &Env, spec: &SpecSession, req: &DmRequest) -> Result<DmResponse, DmError> {
    let program = env.get_program(spec.program()).ok_or(DmError::Spec(crate::dm_v2::SpecError::Stale))?;
    let mut goals: Vec<ManipulationGoal> = Vec::new();
    for (i, g) in req.goals.iter().enumerate() {
        let bad = || DmError::BadObservable(format!("goal {i}: v1 cannot address {:?}", g.observable));
        let (term, arg_index) = match &g.observable {
            Observable::EmitArg { key, arg, path } if path.is_empty() => {
                let e = spec.baseline().emit(key).ok_or_else(bad)?;
                (*e.chain.chain.first().ok_or_else(bad)?, *arg)
            }
            Observable::Watch { term, at, path } if path.is_empty() => {
                let e = spec.baseline().emit(at).ok_or_else(bad)?;
                e.chain
                    .chain
                    .iter()
                    .find_map(|&c| {
                        let t = program.terms.get(c.0 as usize)?;
                        let off = t.op.arg_offset()?;
                        let k = t.inputs.iter().skip(off).position(|x| x == term)?;
                        Some((c, k))
                    })
                    .ok_or_else(bad)?
            }
            _ => return Err(bad()),
        };
        let new_value = match &g.target {
            GoalTarget::Num(t) => {
                if t.fract() == 0.0 {
                    StaticValue::Int(*t as i64)
                } else {
                    StaticValue::Float(*t)
                }
            }
            GoalTarget::Value(v) => v.clone(),
        };
        goals.push(ManipulationGoal { term, arg_index, new_value });
    }
    let mut policy: HashMap<String, VarPolicy> = HashMap::new();
    for s in req.filter.allow.iter().flatten() {
        if let ParamSelector::Variable(n) = s {
            policy.insert(n.clone(), VarPolicy::Configurable);
        }
    }
    for s in &req.filter.deny {
        if let ParamSelector::Variable(n) = s {
            policy.insert(n.clone(), VarPolicy::Static);
        }
    }
    let per_goal = v1::propose_edits_batch(program, &goals, None, &policy)
        .map_err(|e| DmError::BadObservable(e.message))?;
    let origin = |f: FileId| {
        program.source_map.files.get(f.0 as usize).and_then(|f| f.origin.clone())
    };
    let to_alt = |props: Vec<&v1::EditProposal>| -> Alternative {
        let edits: Vec<FileEdit> = props
            .iter()
            .map(|p| FileEdit { file: p.edit.span.file, path: origin(p.edit.span.file), edit: p.edit.clone() })
            .collect();
        Alternative {
            edits,
            changes: Vec::new(),
            achieved: Vec::new(),
            predicted: Vec::new(),
            max_error: f64::NAN,
            verified: false,
            collateral: Collateral::default(),
            stateful: false,
            config_only: props.iter().all(|p| p.config),
            regime_changed: false,
            rng_order_changed: false,
            rng_difference: None,
            score: 0.0,
            score_parts: ScoreParts::default(),
            kind: SubsetKind::Tuple,
            iterations: 0,
            engine: Engine::V1,
            description: props.iter().map(|p| p.description.clone()).collect::<Vec<_>>().join(", "),
        }
    };
    let alternatives: Vec<Alternative> = if per_goal.len() == 1 {
        per_goal[0].iter().take(req.max_alternatives.max(1)).map(|p| to_alt(vec![p])).collect()
    } else {
        let firsts: Vec<&v1::EditProposal> = per_goal.iter().filter_map(|ps| ps.first()).collect();
        if firsts.is_empty() { Vec::new() } else { vec![to_alt(firsts)] }
    };
    Ok(DmResponse {
        alternatives,
        candidates: Vec::new(),
        diagnostics: DmDiagnostics { engine: Some(Engine::V1), ..Default::default() },
        explain: None,
    })
}
