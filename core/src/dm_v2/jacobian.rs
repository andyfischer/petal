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

use crate::dm_v2::observe::{ObsValue, Observable, PathStep};
use crate::dm_v2::params::{Param, RunBudget};
use crate::dm_v2::spec::{OverrideValue, Overrides, SpecHost, SpecRun, SpecSession};
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

    /// One column, every row.
    pub fn column(&self, col: usize) -> Vec<f64> {
        (0..self.rows.len()).map(|r| self.get(r, col)).collect()
    }

    /// One column, goal rows only.
    pub fn goal_column(&self, col: usize) -> Vec<f64> {
        (0..self.goal_rows).map(|r| self.get(r, col)).collect()
    }

    /// The column of param-list index `param`, if measured.
    pub fn col_of(&self, param: usize) -> Option<usize> {
        self.params.iter().position(|&p| p == param)
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
    let mut kept = rows_of(Some(&channels));
    kept.truncate(cap);
    kept
}

/// One probing run, read.
struct Sample {
    error: Option<String>,
    fingerprint: u64,
    values: Vec<Option<f64>>,
}

impl Sample {
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
    fn probe(&mut self, param: &Param, value: OverrideValue) -> Result<Sample, NoRun> {
        if !self.budget.take() {
            return Err(NoRun::Budget);
        }
        let mut ov = Overrides(self.at.0.iter().filter(|(t, _)| *t != param.term).copied().collect());
        ov.0.push((param.term, value));
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
struct Column {
    values: Vec<f64>,
    step: f64,
    quality: ColumnQuality,
}

impl Column {
    fn errored(n: usize, why: impl Into<String>) -> Column {
        Column { values: vec![0.0; n], step: 0.0, quality: ColumnQuality::Errored(why.into()) }
    }
}

/// The Jacobian at the point `at` (empty: the baseline) of `rows` (the first
/// `goal_rows` of which are goals) with respect to `params[keep[..]]`, which
/// must be parameters of `session`.
#[allow(clippy::too_many_arguments)]
pub fn jacobian(
    session: &mut SpecSession,
    env: &mut Env,
    host: &mut dyn SpecHost,
    params: &[Param],
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
            .find(|(t, _)| *t == p.term)
            .map(|(_, v)| match v {
                OverrideValue::Num(f) => *f,
                OverrideValue::Int(i) => *i as f64,
                OverrideValue::Dual(f, _) => *f,
            })
            .unwrap_or(p.value);
        let col = if base.error.is_some() || !base.readable(goal_rows) {
            Ok(Column::errored(n, "a goal is not readable at the base point"))
        } else if p.is_int && cur.fract() == 0.0 {
            int_column(&mut prober, p, cur as i64, &base, goal_rows, opts)
        } else {
            float_column(&mut prober, p, cur, &base, goal_rows, opts)
        };
        let col = match col {
            Ok(col) => col,
            Err(NoRun::Budget) => {
                jac.budget_exhausted = true;
                Column::errored(n, "run budget exhausted")
            }
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
fn int_column(
    pr: &mut Prober,
    p: &Param,
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
fn respell(
    pr: &mut Prober,
    p: &Param,
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

/// A float literal's column (§2.3.2, §2.3.3).
fn float_column(
    pr: &mut Prober,
    p: &Param,
    cur: f64,
    base: &Sample,
    goal_rows: usize,
    opts: &FdOptions,
) -> Result<Column, NoRun> {
    let rows = base.values.len();
    let scale = cur.abs().max(1.0);
    let cap = 0.1 * scale * (1.0 + 1e-9);
    let set = |v: f64| p.override_for(v);
    // Floats are probed as floats even when written as ints elsewhere.
    let setf = |v: f64| match set(v) {
        OverrideValue::Int(i) => OverrideValue::Num(i as f64),
        other => other,
    };
    let mut h = opts.rel_step * scale;
    let plus = pr.probe(p, setf(cur + h))?;
    if let Some(e) = &plus.error {
        return Ok(Column::errored(rows, e.clone()));
    }
    let minus = match opts.mode {
        FdMode::Central => {
            let m = pr.probe(p, setf(cur - h))?;
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
            let m = pr.probe(p, setf(cur - h))?;
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
                return escalate(pr, p, cur, base, goal_rows, opts, h, cap, setf);
            }
            Ok(smooth_or_kink(&plus, base, &minus, h, goal_rows))
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
                let pp = pr.probe(p, setf(cur + h))?;
                let mm = pr.probe(p, setf(cur - h))?;
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
#[allow(clippy::too_many_arguments)]
fn escalate(
    pr: &mut Prober,
    p: &Param,
    cur: f64,
    base: &Sample,
    goal_rows: usize,
    opts: &FdOptions,
    mut h: f64,
    cap: f64,
    setf: impl Fn(f64) -> OverrideValue,
) -> Result<Column, NoRun> {
    let rows = base.values.len();
    let mut found: Option<(Vec<f64>, f64)> = None;
    for _ in 0..opts.max_escalations {
        h *= 10.0;
        if h > cap {
            break;
        }
        let pp = pr.probe(p, setf(cur + h))?;
        let mm = pr.probe(p, setf(cur - h))?;
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
