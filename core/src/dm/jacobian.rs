//! Sensitivity by finite differences: the Jacobian of the observed rows
//! with respect to the screened parameters (docs/dev/direct-manipulation-v2-plan.md
//! §2.3, M2).
//!
//! Each column is measured by real speculative runs with one literal nudged
//! ([`SpecSession::run`]): central differences by default, two runs per
//! column. A column is also *classified* ([`ColumnQuality`]), because a
//! program is only piecewise smooth in its literals:
//!
//! - `Smooth`: the same path both ways and the one-sided slopes agree;
//! - `Kink`: the same path, slopes that disagree (`abs`, `min`, `clamp`);
//! - `BranchFlip`: one side took another branch (or re-initialized a state
//!   slot); the slope is the one-sided one from the other side, after trying
//!   a smaller step;
//! - `Step`: no movement at the base step but movement at a larger one
//!   (`round`, `floor`, int truncation inside a native), or both sides took
//!   other paths; the column is a secant;
//! - `Discrete`: an int literal, probed type-preserving (`n ± 1`, then
//!   `± 2`, `± 4`), so `10 / 4` stays integer division (§2.3.4);
//! - `Zero`: nothing moved at any scale;
//! - `Errored`: a probing run failed.
//!
//! Rows are [`Observable`]s: the goal rows first, then collateral rows
//! ([`collateral_rows`]), the raw native arguments of every other emit, so
//! the solver (M3) can tell what else an edit moves.
//!
//! The float-column rules (the smooth/kink test, the branch-flip retry and
//! step escalation) are written once, over a [`ColumnProbe`]: DM's probe
//! re-runs the program with a literal overridden, and
//! [`column_from_samples`] replays the samples a script measured, which is
//! how the `autodiff` package's `deriv` / `jacobian` classify their columns
//! (the `fd_column` native; docs/features/differentiation.md). A script's
//! `speculate` has no path fingerprint, so there a kink is told from a jump
//! and from curvature by one more probe at a tenth of the step
//! ([`ColumnProbe::sees_paths`]).

use crate::dm::observe::{ObsValue, Observable, PathStep};
use crate::dm::lever::TermLever;
use crate::dm::params::RunBudget;
use crate::dm::spec::{OverrideValue, Overrides, SpecHost, SpecRun, SpecSession};
use crate::env::Env;

/// How a column is differenced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FdMode {
    /// Two runs per column, `p + h` and `p - h`.
    #[default]
    Central,
    /// One run per column, `p + h`, against the base point.
    Forward,
}

/// The finite-difference settings (§6.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FdOptions {
    pub mode: FdMode,
    /// The step relative to `max(|p|, 1)`.
    pub rel_step: f64,
    /// How many times a step may grow ×10 (floats) or double (ints) looking
    /// for movement.
    pub max_escalations: u8,
    /// Also try a float probe of an int literal, and use it when it agrees
    /// with the integer secant (editing would then respell `10` as `10.5`).
    pub allow_respell: bool,
}

impl Default for FdOptions {
    fn default() -> Self {
        FdOptions {
            mode: FdMode::Central,
            rel_step: 1e-3,
            max_escalations: 3,
            allow_respell: false,
        }
    }
}

/// Which side of a parameter kept the base point's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// `p - h`.
    Left,
    /// `p + h`.
    Right,
}

/// What kind of column a parameter has (§2.3.3).
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnQuality {
    Smooth,
    /// One-sided slopes of the first goal row that disagree; the column holds
    /// their average (the solver picks the side of the goal's direction).
    Kink { left: f64, right: f64 },
    /// The other side changed the path; the column is the slope of `side`.
    BranchFlip { side: Side },
    /// The column is a secant over `±secant_h`.
    Step { secant_h: f64 },
    /// An int literal's integer secant.
    Discrete,
    Zero,
    Errored(String),
}

impl ColumnQuality {
    /// Whether the column is usable for solving.
    pub fn usable(&self) -> bool {
        !matches!(self, ColumnQuality::Zero | ColumnQuality::Errored(_))
    }
}

/// The measured Jacobian (§6.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Jacobian {
    /// The columns' parameters, as indices into the param list.
    pub params: Vec<usize>,
    /// Goal rows first, then collateral.
    pub rows: Vec<Observable>,
    pub goal_rows: usize,
    /// Row-major, `rows × params`.
    pub d: Vec<f64>,
    /// The step each column was measured with (an int column's is the
    /// integer offset).
    pub step: Vec<f64>,
    pub quality: Vec<ColumnQuality>,
    /// Runs spent.
    pub runs: usize,
    /// The budget ran out; the columns not measured are `Errored`.
    pub budget_exhausted: bool,
}

impl Jacobian {
    /// `∂ row / ∂ param column`.
    pub fn get(&self, row: usize, col: usize) -> f64 {
        self.d[row * self.params.len() + col]
    }

    /// The columns `cols`, every row (rows × cols).
    pub fn local(&self, cols: &[usize]) -> Vec<Vec<f64>> {
        (0..self.rows.len()).map(|r| cols.iter().map(|&c| self.get(r, c)).collect()).collect()
    }

    /// One column, goal rows only.
    pub fn goal_column(&self, col: usize) -> Vec<f64> {
        (0..self.goal_rows).map(|r| self.get(r, col)).collect()
    }

    /// The column of param-list index `param`, if measured.
    pub fn col_of(&self, param: usize) -> Option<usize> {
        self.params.iter().position(|&p| p == param)
    }

    /// Append the columns of `other`, measured over the same rows.
    pub fn append_columns(&mut self, other: Jacobian) {
        debug_assert_eq!(self.rows, other.rows);
        let (a, b) = (self.params.len(), other.params.len());
        let mut d = Vec::with_capacity(self.rows.len() * (a + b));
        for r in 0..self.rows.len() {
            d.extend_from_slice(&self.d[r * a..(r + 1) * a]);
            d.extend_from_slice(&other.d[r * b..(r + 1) * b]);
        }
        self.d = d;
        self.params.extend(other.params);
        self.step.extend(other.step);
        self.quality.extend(other.quality);
        self.runs += other.runs;
        self.budget_exhausted |= other.budget_exhausted;
    }

    /// Drop the collateral rows that are a goal under another name, given
    /// every row's baseline value `base`: a goal on an emit's value or on a
    /// watched term reads the same number as one of that emit's raw native
    /// arguments (`push_output(s, v)`'s `v`), and that row is the goal, not
    /// something else the edit moves. A row is dropped when it is on the
    /// goal's emit, has the goal's baseline value and the goal's derivative
    /// in every column; `base` is filtered alongside.
    pub fn drop_goal_aliases(&mut self, base: &mut Vec<Option<f64>>) {
        let n = self.params.len();
        let same = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1e-12);
        let key_of = |o: &Observable| match o {
            Observable::EmitArg { .. } => None,
            Observable::EmitValue { key, .. } => Some(*key),
            Observable::Watch { at, .. } => Some(*at),
        };
        let keep: Vec<bool> = (0..self.rows.len())
            .map(|r| {
                if r < self.goal_rows {
                    return true;
                }
                let Observable::EmitArg { key, .. } = &self.rows[r] else { return true };
                !(0..self.goal_rows).any(|g| {
                    key_of(&self.rows[g]) == Some(*key)
                        && matches!((base.get(r), base.get(g)), (Some(Some(a)), Some(Some(b))) if same(*a, *b))
                        && (0..n).all(|c| same(self.get(r, c), self.get(g, c)))
                })
            })
            .collect();
        if keep.iter().all(|k| *k) {
            return;
        }
        let mut d = Vec::with_capacity(self.d.len());
        for (r, row) in self.d.chunks(n.max(1)).enumerate().take(self.rows.len()) {
            if keep[r] {
                d.extend_from_slice(&row[..n]);
            }
        }
        let mut k = keep.iter();
        self.rows.retain(|_| *k.next().unwrap());
        let mut k = keep.iter();
        base.retain(|_| *k.next().unwrap_or(&true));
        self.d = d;
    }
}

/// The most numbers one raw native argument contributes to the collateral
/// rows: a list of records (a row of icons, every platform of a level) is
/// read through, but a long buffer does not crowd out the other emits.
pub const COLLATERAL_LEAVES_PER_ARG: usize = 256;

/// Push a row for every number inside `v` (at `path`), depth first, until
/// `left` runs out: numbers, the fields of records (maps), the items of lists
/// (vec2/vec3, `f64_array`) and a variant's payload, at any depth the
/// recorded value kept.
fn numeric_leaves(v: &ObsValue, path: &mut Vec<PathStep>, left: &mut usize, out: &mut Vec<Vec<PathStep>>) {
    if *left == 0 {
        return;
    }
    match v {
        ObsValue::Num(_) | ObsValue::Int(_) => {
            *left -= 1;
            out.push(path.clone());
        }
        ObsValue::Record(fields) => {
            for (name, item) in fields {
                path.push(PathStep::Field(name.clone()));
                numeric_leaves(item, path, left, out);
                path.pop();
            }
        }
        ObsValue::List(items) | ObsValue::Variant { data: items, .. } => {
            for (i, item) in items.iter().enumerate() {
                path.push(PathStep::Index(i));
                numeric_leaves(item, path, left, out);
                path.pop();
            }
        }
        ObsValue::Bool(_) | ObsValue::Str(_) | ObsValue::Nil | ObsValue::Opaque => {}
    }
}

/// The collateral rows of a baseline (§2.3): every number in every raw
/// native argument of every emit, except the goals. Containers are read
/// through: a record's fields, a list's items, a list of records, a record
/// in a record (each argument contributes at most
/// [`COLLATERAL_LEAVES_PER_ARG`] numbers). Past `cap` rows, only emits on the
/// goals' channels are kept, then the list is cut at `cap`.
pub fn collateral_rows(baseline: &SpecRun, goals: &[Observable], cap: usize) -> Vec<Observable> {
    collateral_rows_keeping(baseline, goals, cap, &[])
}

/// [`collateral_rows`], also keeping the emits on `keep` past `cap` (the
/// channels a request weighs, §9 Canopy S3), ahead of the goals' channels.
pub fn collateral_rows_keeping(
    baseline: &SpecRun,
    goals: &[Observable],
    cap: usize,
    keep: &[crate::symbol::SymbolId],
) -> Vec<Observable> {
    let rows_of = |only: Option<&[crate::symbol::SymbolId]>| {
        let mut rows = Vec::new();
        let mut leaves = Vec::new();
        for e in &baseline.emits {
            if only.is_some_and(|c| !c.contains(&e.channel)) {
                continue;
            }
            for (arg, v) in e.native_args.iter().enumerate() {
                leaves.clear();
                let mut left = COLLATERAL_LEAVES_PER_ARG;
                numeric_leaves(v, &mut Vec::new(), &mut left, &mut leaves);
                rows.extend(leaves.drain(..).map(|path| Observable::EmitArg { key: e.key, arg, path }));
            }
        }
    rows.retain(|r| !goals.contains(r));
        rows
    };
    let all = rows_of(None);
    if all.len() <= cap {
        return all;
    }
    let channels: Vec<crate::symbol::SymbolId> = goals
        .iter()
        .filter_map(|g| match g {
            Observable::EmitArg { key, .. } | Observable::EmitValue { key, .. } => {
                baseline.emit(key).map(|e| e.channel)
            }
            Observable::Watch { at, .. } => baseline.emit(at).map(|e| e.channel),
        })
        .collect();
    let mut kept = if keep.is_empty() { Vec::new() } else { rows_of(Some(keep)) };
    let rest: Vec<crate::symbol::SymbolId> = channels.into_iter().filter(|c| !keep.contains(c)).collect();
    kept.extend(rows_of(Some(&rest)));
    kept.truncate(cap);
    kept
}

/// One probing run, read.
#[derive(Debug, Clone)]
pub struct Sample {
    error: Option<String>,
    fingerprint: u64,
    values: Vec<Option<f64>>,
}

impl Sample {
    /// A sample a script measured: one value per row, `None` where the row
    /// was not a finite number. Script samples carry no path fingerprint.
    pub fn measured(values: Vec<Option<f64>>) -> Sample {
        Sample { error: None, fingerprint: 0, values }
    }

    /// A probe that failed with `why`, over `rows` rows.
    pub fn failed(rows: usize, why: impl Into<String>) -> Sample {
        Sample { error: Some(why.into()), fingerprint: 0, values: vec![None; rows] }
    }

    fn of(run: &SpecRun, rows: &[Observable]) -> Sample {
        Sample {
            error: run.error.as_ref().map(|e| e.to_string()),
            fingerprint: run.fingerprint,
            values: run.read_all(rows),
        }
    }

    /// Every goal row was there.
    fn readable(&self, goal_rows: usize) -> bool {
        self.error.is_none() && self.values[..goal_rows].iter().all(|v| v.is_some_and(f64::is_finite))
    }

    /// Readable and on the base point's path.
    fn valid(&self, base: &Sample, goal_rows: usize) -> bool {
        self.readable(goal_rows) && self.fingerprint == base.fingerprint
    }
}

/// Why a probe was not made.
enum NoRun {
    Budget,
    /// A replayed probe ([`column_from_samples`]) has no sample at this
    /// point yet: the script has to measure it.
    Need(f64),
}

/// Where a float column's probes come from.
trait ColumnProbe {
    /// The sample with the parameter at `x`.
    fn probe(&mut self, x: f64) -> Result<Sample, NoRun>;
    /// Whether samples carry a path fingerprint, so that a side that took
    /// another branch shows as invalid. Without one (a script's samples), a
    /// kink is checked once more at a tenth of the step, which tells a jump
    /// (the difference does not shrink) and curvature (the slopes' gap
    /// shrinks with the step) from a true kink.
    fn sees_paths(&self) -> bool;
}

/// DM's probe: a run with the lever `p` set to `x`, as a float.
struct LeverProbe<'p, 'a, L> {
    pr: &'p mut Prober<'a>,
    p: &'p L,
}

impl<L: TermLever> ColumnProbe for LeverProbe<'_, '_, L> {
    fn probe(&mut self, x: f64) -> Result<Sample, NoRun> {
        // Floats are probed as floats even when written as ints elsewhere.
        let v = match self.p.override_for(x) {
            OverrideValue::Int(i) => OverrideValue::Num(i as f64),
            other => other,
        };
        self.pr.probe(self.p, v)
    }
    fn sees_paths(&self) -> bool {
        true
    }
}

/// A script's probe: the samples it has measured so far, by point.
struct Replay<'a> {
    samples: &'a [(f64, Sample)],
}

impl ColumnProbe for Replay<'_> {
    fn probe(&mut self, x: f64) -> Result<Sample, NoRun> {
        self.samples
            .iter()
            .find(|(at, _)| at.to_bits() == x.to_bits())
            .map(|(_, s)| s.clone())
            .ok_or(NoRun::Need(x))
    }
    fn sees_paths(&self) -> bool {
        false
    }
}

/// The measuring context of one [`jacobian`] call.
struct Prober<'a> {
    session: &'a mut SpecSession,
    env: &'a mut Env,
    host: &'a mut dyn SpecHost,
    at: &'a Overrides,
    rows: &'a [Observable],
    budget: &'a mut RunBudget,
    runs: usize,
}

impl Prober<'_> {
    /// Run with `param` set to `value` on top of the base point.
    fn probe<L: TermLever>(&mut self, param: &L, value: OverrideValue) -> Result<Sample, NoRun> {
        if !self.budget.take() {
            return Err(NoRun::Budget);
        }
        let mut ov = Overrides(self.at.0.iter().filter(|(t, _)| *t != param.term()).copied().collect());
        ov.0.push((param.term(), value));
        let run = self.session.run(self.env, self.host, &ov);
        self.runs += 1;
        Ok(Sample::of(&run, self.rows))
    }
}

/// `(a - b) / div` per row, 0 where a row is missing on either side.
fn diff(a: &Sample, b: &Sample, div: f64) -> Vec<f64> {
    a.values
        .iter()
        .zip(&b.values)
        .map(|(x, y)| match (x, y) {
            (Some(x), Some(y)) => (x - y) / div,
            _ => 0.0,
        })
        .collect()
}

/// Whether the goal rows of `a` and `b` are the same within float noise.
fn still(a: &Sample, b: &Sample, goal_rows: usize) -> bool {
    a.values[..goal_rows].iter().zip(&b.values[..goal_rows]).all(|(x, y)| match (x, y) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-12 * x.abs().max(y.abs()).max(1.0),
        _ => false,
    })
}

/// The measured column and its quality.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// One derivative per row.
    pub values: Vec<f64>,
    /// The step the column was measured with, after any escalation.
    pub step: f64,
    pub quality: ColumnQuality,
}

impl Column {
    fn errored(n: usize, why: impl Into<String>) -> Column {
        Column { values: vec![0.0; n], step: 0.0, quality: ColumnQuality::Errored(why.into()) }
    }
}

/// The Jacobian at the point `at` (empty: the baseline) of `rows` (the first
/// `goal_rows` of which are goals) with respect to `params[keep[..]]`, which
/// must be parameters of `session`. Generic over the lever ([`TermLever`]);
/// DM v2's is the source literal, [`Param`](crate::dm::params::Param).
#[allow(clippy::too_many_arguments)]
pub fn jacobian<L: TermLever>(
    session: &mut SpecSession,
    env: &mut Env,
    host: &mut dyn SpecHost,
    params: &[L],
    keep: &[usize],
    rows: Vec<Observable>,
    goal_rows: usize,
    at: &Overrides,
    opts: &FdOptions,
    budget: &mut RunBudget,
) -> Jacobian {
    let goal_rows = goal_rows.min(rows.len());
    let mut jac = Jacobian {
        params: keep.to_vec(),
        rows: Vec::new(),
        goal_rows,
        d: vec![0.0; rows.len() * keep.len()],
        step: vec![0.0; keep.len()],
        quality: Vec::with_capacity(keep.len()),
        runs: 0,
        budget_exhausted: false,
    };
    let base = if at.0.is_empty() {
        Some(Sample::of(session.baseline(), &rows))
    } else if budget.take() {
        jac.runs += 1;
        Some(Sample::of(&session.run(env, host, at), &rows))
    } else {
        None
    };
    let Some(base) = base else {
        jac.budget_exhausted = true;
        jac.quality = vec![ColumnQuality::Errored("run budget exhausted".into()); keep.len()];
        jac.rows = rows;
        return jac;
    };
    let mut prober = Prober { session, env, host, at, rows: &rows, budget, runs: 0 };
    let n = rows.len();
    let ncols = keep.len();
    for (c, &pi) in keep.iter().enumerate() {
        let p = &params[pi];
        // The parameter's value at the point, as written.
        let cur = at
            .0
            .iter()
            .find(|(t, _)| *t == p.term())
            .map(|(_, v)| match v {
                OverrideValue::Num(f) => *f,
                OverrideValue::Int(i) => *i as f64,
                OverrideValue::Dual(f, _) => *f,
            })
            .unwrap_or(p.value());
        let col = if base.error.is_some() || !base.readable(goal_rows) {
            Ok(Column::errored(n, "a goal is not readable at the base point"))
        } else if p.is_int() && cur.fract() == 0.0 {
            int_column(&mut prober, p, cur as i64, &base, goal_rows, opts)
        } else {
            let h = opts.rel_step * cur.abs().max(1.0);
            float_column(&mut LeverProbe { pr: &mut prober, p }, cur, h, &base, goal_rows, opts)
        };
        let col = match col {
            Ok(col) => col,
            Err(NoRun::Budget) => {
                jac.budget_exhausted = true;
                Column::errored(n, "run budget exhausted")
            }
            Err(NoRun::Need(_)) => unreachable!("a run probe never replays"),
        };
        for (r, v) in col.values.iter().enumerate() {
            jac.d[r * ncols + c] = *v;
        }
        jac.step[c] = col.step;
        jac.quality.push(col.quality);
    }
    jac.runs += prober.runs;
    jac.rows = rows;
    jac
}

/// An int literal's column: the integer secant, `n ± k` for `k` = 1, 2, 4, …
fn int_column<L: TermLever>(
    pr: &mut Prober,
    p: &L,
    n: i64,
    base: &Sample,
    goal_rows: usize,
    opts: &FdOptions,
) -> Result<Column, NoRun> {
    let rows = base.values.len();
    let mut k: i64 = 1;
    for _ in 0..=opts.max_escalations {
        let plus = pr.probe(p, p.override_int(n + k))?;
        let minus = match opts.mode {
            FdMode::Central => Some(pr.probe(p, p.override_int(n - k))?),
            FdMode::Forward => None,
        };
        for s in std::iter::once(&plus).chain(minus.iter()) {
            if let Some(e) = &s.error {
                return Ok(Column::errored(rows, e.clone()));
            }
            if !s.readable(goal_rows) {
                return Ok(Column::errored(rows, "a goal is not there when this literal changes"));
            }
        }
        let (values, moved) = match &minus {
            Some(m) => (diff(&plus, m, 2.0 * k as f64), !still(&plus, m, goal_rows)),
            None => (diff(&plus, base, k as f64), !still(&plus, base, goal_rows)),
        };
        if moved {
            let mut col = Column { values, step: k as f64, quality: ColumnQuality::Discrete };
            if opts.allow_respell {
                respell(pr, p, n as f64, base, goal_rows, opts, &mut col)?;
            }
            return Ok(col);
        }
        k *= 2;
    }
    Ok(Column { values: vec![0.0; rows], step: k as f64, quality: ColumnQuality::Zero })
}

/// Under `allow_respell`: a float probe of an int literal, kept when it
/// agrees with the integer secant within 25% (§2.3.4).
fn respell<L: TermLever>(
    pr: &mut Prober,
    p: &L,
    cur: f64,
    base: &Sample,
    goal_rows: usize,
    opts: &FdOptions,
    col: &mut Column,
) -> Result<(), NoRun> {
    let h = opts.rel_step * cur.abs().max(1.0);
    let plus = pr.probe(p, OverrideValue::Num(cur + h))?;
    let minus = pr.probe(p, OverrideValue::Num(cur - h))?;
    if !plus.valid(base, goal_rows) || !minus.valid(base, goal_rows) {
        return Ok(());
    }
    let float = diff(&plus, &minus, 2.0 * h);
    let agrees = (0..goal_rows).all(|r| {
        let (a, b) = (float[r], col.values[r]);
        (a - b).abs() <= 0.25 * a.abs().max(b.abs()) || (a.abs() < 1e-12 && b.abs() < 1e-12)
    });
    if agrees {
        *col = Column { values: float, step: h, quality: ColumnQuality::Smooth };
    }
    Ok(())
}

/// A float parameter's column at `cur`, first probed at `cur ± h`
/// (§2.3.2, §2.3.3).
fn float_column(
    pr: &mut impl ColumnProbe,
    cur: f64,
    mut h: f64,
    base: &Sample,
    goal_rows: usize,
    opts: &FdOptions,
) -> Result<Column, NoRun> {
    let rows = base.values.len();
    let scale = cur.abs().max(1.0);
    let cap = 0.1 * scale * (1.0 + 1e-9);
    let plus = pr.probe(cur + h)?;
    if let Some(e) = &plus.error {
        return Ok(Column::errored(rows, e.clone()));
    }
    let minus = match opts.mode {
        FdMode::Central => {
            let m = pr.probe(cur - h)?;
            if let Some(e) = &m.error {
                return Ok(Column::errored(rows, e.clone()));
            }
            m
        }
        FdMode::Forward => {
            if plus.valid(base, goal_rows) {
                let values = diff(&plus, base, h);
                let quality = if still(&plus, base, goal_rows) {
                    ColumnQuality::Zero
                } else {
                    ColumnQuality::Smooth
                };
                return Ok(Column { values, step: h, quality });
            }
            // The forward side changed the path: look back instead.
            let m = pr.probe(cur - h)?;
            if m.valid(base, goal_rows) {
                return Ok(Column {
                    values: diff(base, &m, h),
                    step: h,
                    quality: ColumnQuality::BranchFlip { side: Side::Left },
                });
            }
            m
        }
    };
    match (plus.valid(base, goal_rows), minus.valid(base, goal_rows)) {
        (true, true) => {
            if still(&plus, base, goal_rows) && still(base, &minus, goal_rows) {
                // Nothing moved at this step: a step function, or nothing.
                return escalate(pr, cur, base, goal_rows, opts, h, cap);
            }
            let col = smooth_or_kink(&plus, base, &minus, h, goal_rows);
            if pr.sees_paths() || col.quality == ColumnQuality::Smooth {
                return Ok(col);
            }
            blind_kink(pr, cur, h, base, &plus, &minus, goal_rows, col)
        }
        (false, false) => {
            if plus.readable(goal_rows) && minus.readable(goal_rows) {
                Ok(Column {
                    values: diff(&plus, &minus, 2.0 * h),
                    step: h,
                    quality: ColumnQuality::Step { secant_h: h },
                })
            } else {
                Ok(Column::errored(rows, "a goal is not there when this literal changes"))
            }
        }
        (plus_ok, _) => {
            // One side took another path. A smaller step may stay on it.
            let mut good = if plus_ok { (plus, h) } else { (minus, h) };
            let side = if plus_ok { Side::Right } else { Side::Left };
            for _ in 0..2 {
                h /= 10.0;
                let pp = pr.probe(cur + h)?;
                let mm = pr.probe(cur - h)?;
                match (pp.valid(base, goal_rows), mm.valid(base, goal_rows)) {
                    (true, true) => {
                        if still(&pp, base, goal_rows) && still(base, &mm, goal_rows) {
                            break;
                        }
                        return Ok(smooth_or_kink(&pp, base, &mm, h, goal_rows));
                    }
                    (true, false) if side == Side::Right => good = (pp, h),
                    (false, true) if side == Side::Left => good = (mm, h),
                    _ => {}
                }
            }
            let (s, hh) = good;
            let values = match side {
                Side::Right => diff(&s, base, hh),
                Side::Left => diff(base, &s, hh),
            };
            Ok(Column { values, step: hh, quality: ColumnQuality::BranchFlip { side } })
        }
    }
}

/// Both sides on the base path: `Smooth` when the one-sided slopes agree
/// within 10% on every goal row, `Kink` otherwise.
fn smooth_or_kink(plus: &Sample, base: &Sample, minus: &Sample, h: f64, goal_rows: usize) -> Column {
    let fwd = diff(plus, base, h);
    let bwd = diff(base, minus, h);
    let central = diff(plus, minus, 2.0 * h);
    let tiny = |x: f64, r: usize| {
        let v = base.values[r].unwrap_or(0.0).abs().max(1.0);
        (x * h).abs() <= 1e-12 * v
    };
    let kink_row = (0..goal_rows).find(|&r| {
        let (a, b) = (fwd[r], bwd[r]);
        !(tiny(a, r) && tiny(b, r)) && (a - b).abs() > 0.1 * a.abs().max(b.abs())
    });
    let quality = match kink_row {
        None => ColumnQuality::Smooth,
        Some(r) => ColumnQuality::Kink { left: bwd[r], right: fwd[r] },
    };
    Column { values: central, step: h, quality }
}

/// No movement at the base step: grow it ×10 (up to `cap`) until something
/// moves. A secant found that way is a `Step`, taken over one more ×10 when
/// that still fits, so that it averages over at least one jump.
fn escalate(
    pr: &mut impl ColumnProbe,
    cur: f64,
    base: &Sample,
    goal_rows: usize,
    opts: &FdOptions,
    mut h: f64,
    cap: f64,
) -> Result<Column, NoRun> {
    let rows = base.values.len();
    let mut found: Option<(Vec<f64>, f64)> = None;
    for _ in 0..opts.max_escalations {
        h *= 10.0;
        if h > cap {
            break;
        }
        let pp = pr.probe(cur + h)?;
        let mm = pr.probe(cur - h)?;
        if !pp.readable(goal_rows) || !mm.readable(goal_rows) {
            break;
        }
        if still(&pp, &mm, goal_rows) {
            if found.is_some() {
                break;
            }
            continue;
        }
        let secant = diff(&pp, &mm, 2.0 * h);
        let first = found.is_none();
        found = Some((secant, h));
        if !first {
            break;
        }
    }
    Ok(match found {
        Some((values, secant_h)) => Column { values, step: secant_h, quality: ColumnQuality::Step { secant_h } },
        None => Column { values: vec![0.0; rows], step: h, quality: ColumnQuality::Zero },
    })
}

/// A kink found without path fingerprints ([`ColumnProbe::sees_paths`]),
/// checked at `h / 10`: a side whose difference does not shrink with the
/// step jumped (another branch), so the column is that of the other side
/// (`BranchFlip`), or a secant when both did (`Step`); slopes whose gap
/// shrinks with the step are curvature (`Smooth`, as at the bottom of
/// `x * x`); otherwise the kink stands.
#[allow(clippy::too_many_arguments)]
fn blind_kink(
    pr: &mut impl ColumnProbe,
    cur: f64,
    h: f64,
    base: &Sample,
    plus: &Sample,
    minus: &Sample,
    goal_rows: usize,
    kink: Column,
) -> Result<Column, NoRun> {
    let rows = base.values.len();
    let hs = h / 10.0;
    let pp = pr.probe(cur + hs)?;
    let mm = pr.probe(cur - hs)?;
    for s in [&pp, &mm] {
        if let Some(e) = &s.error {
            return Ok(Column::errored(rows, e.clone()));
        }
    }
    let at = |s: &Sample, r: usize| s.values[r].unwrap_or(f64::NAN);
    let b = |r: usize| at(base, r);
    let tiny = |d: f64, r: usize| d.abs() <= 1e-12 * b(r).abs().max(1.0);
    // The side's difference from the base did not shrink with the step.
    let jumped = |big: &Sample, small: &Sample| {
        !small.readable(goal_rows)
            || (0..goal_rows).any(|r| {
                let (db, ds) = (at(big, r) - b(r), at(small, r) - b(r));
                !tiny(db, r) && ds.abs() > 0.5 * db.abs()
            })
    };
    Ok(match (jumped(plus, &pp), jumped(minus, &mm)) {
        (false, false) => {
            // The one-sided slopes' gap: `h · f''` for a smooth row, a
            // constant at a kink.
            let gap = |p: &Sample, m: &Sample, h: f64, r: usize| ((at(p, r) - b(r)) - (b(r) - at(m, r))) / h;
            let curvature = (0..goal_rows).all(|r| {
                let (big, small) = (gap(plus, minus, h, r), gap(&pp, &mm, hs, r));
                tiny(big * h, r) || small.abs() <= 0.5 * big.abs()
            });
            if curvature { Column { quality: ColumnQuality::Smooth, ..kink } } else { kink }
        }
        (true, false) => Column {
            values: diff(base, &mm, hs),
            step: hs,
            quality: ColumnQuality::BranchFlip { side: Side::Left },
        },
        (false, true) => Column {
            values: diff(&pp, base, hs),
            step: hs,
            quality: ColumnQuality::BranchFlip { side: Side::Right },
        },
        (true, true) => Column {
            values: diff(plus, minus, 2.0 * h),
            step: h,
            quality: ColumnQuality::Step { secant_h: h },
        },
    })
}

/// A float column measured by a script: the rules DM classifies its
/// columns by ([`float_column`]), replayed over the samples measured so far.
/// `base` is the function at `cur`, `samples` its value at other points,
/// and `h` the first step. Returns the column, or `Err(x)`: the point to
/// measure next (add it to `samples` and ask again). Every row is a goal
/// row. The samples carry no path, so a kink is checked at a tenth of the
/// step (see [`ColumnProbe::sees_paths`]).
pub fn column_from_samples(
    cur: f64,
    h: f64,
    base: &Sample,
    samples: &[(f64, Sample)],
    opts: &FdOptions,
) -> Result<Column, f64> {
    let rows = base.values.len();
    if let Some(e) = &base.error {
        return Ok(Column::errored(rows, e.clone()));
    }
    if !base.readable(rows) {
        return Ok(Column::errored(rows, "the function is not a finite number at the point"));
    }
    match float_column(&mut Replay { samples }, cur, h, base, rows, opts) {
        Ok(col) => Ok(col),
        Err(NoRun::Need(x)) => Err(x),
        Err(NoRun::Budget) => unreachable!("a replayed probe has no budget"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive [`column_from_samples`] the way the `autodiff` package does:
    /// measure each point it asks for until it answers. Returns the column
    /// and how many points were measured.
    fn drive(f: impl Fn(f64) -> f64, x: f64, h: f64, mode: FdMode) -> (Column, usize) {
        let opts = FdOptions { mode, ..FdOptions::default() };
        let base = Sample::measured(vec![Some(f(x)).filter(|v| v.is_finite())]);
        let mut samples = Vec::new();
        loop {
            match column_from_samples(x, h, &base, &samples, &opts) {
                Ok(col) => return (col, samples.len()),
                Err(at) => {
                    assert!(samples.len() < 32, "no answer after {} samples", samples.len());
                    samples.push((at, Sample::measured(vec![Some(f(at)).filter(|v| v.is_finite())])));
                }
            }
        }
    }

    #[test]
    fn a_smooth_function_takes_two_samples_central_one_forward() {
        let (col, n) = drive(|x| x * x * x, 2.0, 1e-4, FdMode::Central);
        assert_eq!(col.quality, ColumnQuality::Smooth);
        assert!((col.values[0] - 12.0).abs() < 1e-6, "{col:?}");
        assert_eq!(n, 2);
        let (col, n) = drive(|x| 3.0 * x, 2.0, 1e-4, FdMode::Forward);
        assert_eq!(col.quality, ColumnQuality::Smooth);
        assert_eq!(n, 1);
    }

    #[test]
    fn without_paths_curvature_kinks_and_jumps_are_told_apart() {
        // The bottom of a parabola: the one-sided slopes disagree, but their
        // gap shrinks with the step.
        let (col, _) = drive(|x| x * x, 0.0, 1e-4, FdMode::Central);
        assert_eq!(col.quality, ColumnQuality::Smooth);
        // `abs` at 0: the gap stays.
        let (col, _) = drive(f64::abs, 0.0, 1e-4, FdMode::Central);
        assert!(matches!(col.quality, ColumnQuality::Kink { .. }), "{col:?}");
        // A jump on the left: the right side's slope.
        let (col, _) = drive(|x| if x < 1.0 { x } else { x + 5.0 }, 1.0, 1e-4, FdMode::Central);
        assert_eq!(col.quality, ColumnQuality::BranchFlip { side: Side::Right });
        assert!((col.values[0] - 1.0).abs() < 1e-6, "{col:?}");
        // `floor` between jumps: escalated to a secant.
        let (col, _) = drive(f64::floor, 0.97, 1e-4, FdMode::Central);
        assert!(matches!(col.quality, ColumnQuality::Step { .. }), "{col:?}");
        // A constant: nothing at any scale.
        let (col, _) = drive(|_| 7.0, 2.0, 1e-4, FdMode::Central);
        assert_eq!(col.quality, ColumnQuality::Zero);
        // `sqrt` at 0: the left side is not a number, so the right side's.
        let (col, _) = drive(f64::sqrt, 0.0, 1e-4, FdMode::Central);
        assert_eq!(col.quality, ColumnQuality::BranchFlip { side: Side::Right });
    }

    #[test]
    fn a_failed_base_is_errored_without_samples() {
        let base = Sample::failed(1, "boom");
        let col = column_from_samples(1.0, 1e-4, &base, &[], &FdOptions::default()).unwrap();
        assert_eq!(col.quality, ColumnQuality::Errored("boom".into()));
    }
}
