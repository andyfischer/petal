//! The goal solver (docs/dev/direct-manipulation-v2-plan.md §2.4, M3):
//! from a measured Jacobian to literal values that put the goals where the
//! user wants them, verified by re-running.
//!
//! - [`linear_step`]: one Gauss-Newton step for a subset of parameters. When
//!   the goals can all be met by the subset (the goal block of the Jacobian
//!   has full row rank), they are *constraints*, and the step is the one that
//!   meets them while moving the code least (each parameter weighted by its
//!   cost `d_i / s_i`) and moving the other emits least (the collateral rows).
//!   When they cannot (conflicting goals, fewer parameters than goals), it is
//!   the weighted least-squares compromise.
//! - [`solve_subset`]: the verification loop. Each step is projected onto the
//!   `tunable` ranges and steps (and onto the integers for an int literal
//!   that may not be respelled), run, and kept when the goal error went
//!   down; the Jacobian follows by Broyden updates and is re-measured every
//!   third step or after the control flow changed.
//! - [`round_solution`]: nice literals (§2.4.4). An int stays an int, a float
//!   keeps its decimals style, else the fewest decimals that keep every goal
//!   within tolerance; jointly first, then one parameter at a time. A
//!   solve that stopped short of the goals is rounded too, as far as it
//!   gets no worse than one tolerance.
//!
//! The dense algebra is a small Gaussian elimination with partial pivoting:
//! `k ≤ 32` unknowns, no new dependency.

use crate::dm_v2::jacobian::{FdOptions, Jacobian, jacobian};
use crate::dm_v2::params::{Param, RunBudget};
use crate::dm_v2::spec::{Overrides, RunError, SpecHost, SpecRun, SpecSession};
use crate::dm_v2::hints::Hint;
use crate::env::Env;

// ── dense algebra ──────────────────────────────────────────────────────────

/// Solve `a x = b` (`a` is `n × n`, row-major) by Gaussian elimination with
/// partial pivoting. `None` when a pivot is negligible against the matrix's
/// largest entry (the system is singular to working precision).
pub fn solve_dense(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    debug_assert_eq!(a.len(), n * n);
    let scale = a.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    if scale == 0.0 || !scale.is_finite() {
        return None;
    }
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i * n + col].abs().total_cmp(&a[j * n + col].abs()))?;
        if a[piv * n + col].abs() <= 1e-13 * scale {
            return None;
        }
        if piv != col {
            for k in 0..n {
                a.swap(piv * n + k, col * n + k);
            }
            b.swap(piv, col);
        }
        let p = a[col * n + col];
        for row in col + 1..n {
            let f = a[row * n + col] / p;
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                a[row * n + k] -= f * a[col * n + k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let mut s = b[row];
        for k in row + 1..n {
            s -= a[row * n + k] * x[k];
        }
        x[row] = s / a[row * n + row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The numerical rank of the rows `m` (each of the same length), by
/// elimination with a tolerance relative to the largest row norm.
pub fn rank(m: &[Vec<f64>]) -> usize {
    let mut rows: Vec<Vec<f64>> = m.to_vec();
    let norm = rows
        .iter()
        .map(|r| r.iter().map(|x| x * x).sum::<f64>().sqrt())
        .fold(0.0f64, f64::max);
    if norm == 0.0 {
        return 0;
    }
    let tol = 1e-9 * norm;
    let cols = rows.first().map_or(0, Vec::len);
    let mut rank = 0;
    for col in 0..cols {
        let Some(piv) = (rank..rows.len()).max_by(|&i, &j| rows[i][col].abs().total_cmp(&rows[j][col].abs()))
        else {
            break;
        };
        if rows[piv][col].abs() <= tol {
            continue;
        }
        rows.swap(piv, rank);
        for r in rank + 1..rows.len() {
            let f = rows[r][col] / rows[rank][col];
            for c in col..cols {
                rows[r][c] -= f * rows[rank][c];
            }
        }
        rank += 1;
        if rank == rows.len() {
            break;
        }
    }
    rank
}

/// One linear step for `k` parameters (§2.4.1, as revised in §9 M3).
///
/// - `jg`: the goal rows (`m`, each `k` long), `r`: the goal residuals
///   (target minus current), `wt`: each goal's weight over its tolerance;
/// - `jc`: the collateral rows;
/// - `cost`: each parameter's `d_i / s_i`.
///
/// With full-row-rank `jg` and `m ≤ k`, the step solves
/// `min Σ (cost_i Δ_i)² + ‖jc Δ‖² / ‖r‖²` subject to `jg Δ = r` (the KKT
/// system). Otherwise it is `min Σ (wt_j (jg Δ - r)_j)²`, regularized by a
/// vanishing multiple of the same objective so that it is unique.
pub fn linear_step(jg: &[Vec<f64>], jc: &[Vec<f64>], r: &[f64], wt: &[f64], cost: &[f64]) -> Vec<f64> {
    let m = jg.len();
    let k = cost.len();
    if k == 0 {
        return Vec::new();
    }
    let rn2: f64 = r.iter().map(|x| x * x).sum::<f64>();
    if rn2 == 0.0 {
        return vec![0.0; k];
    }
    // H = diag(cost²) + jcᵀ jc / ‖r‖².
    let mut h = vec![0.0; k * k];
    for i in 0..k {
        h[i * k + i] = cost[i].max(1e-6).powi(2);
    }
    for row in jc {
        for i in 0..k {
            if row[i] == 0.0 {
                continue;
            }
            for j in 0..k {
                h[i * k + j] += row[i] * row[j] / rn2;
            }
        }
    }
    if m <= k && rank(jg) == m {
        let n = k + m;
        let mut a = vec![0.0; n * n];
        let mut b = vec![0.0; n];
        for i in 0..k {
            for j in 0..k {
                a[i * n + j] = h[i * k + j];
            }
        }
        for (g, row) in jg.iter().enumerate() {
            for i in 0..k {
                a[i * n + k + g] = row[i];
                a[(k + g) * n + i] = row[i];
            }
            b[k + g] = r[g];
        }
        if let Some(x) = solve_dense(a, b, n) {
            return x[..k].to_vec();
        }
    }
    // The least-squares compromise.
    let mut a = vec![0.0; k * k];
    let mut b = vec![0.0; k];
    for (g, row) in jg.iter().enumerate() {
        let w2 = wt[g] * wt[g];
        for i in 0..k {
            b[i] += w2 * row[i] * r[g];
            for j in 0..k {
                a[i * k + j] += w2 * row[i] * row[j];
            }
        }
    }
    let tr_a: f64 = (0..k).map(|i| a[i * k + i]).sum();
    let tr_h: f64 = (0..k).map(|i| h[i * k + i]).sum();
    if tr_a == 0.0 {
        return vec![0.0; k];
    }
    let eps = 1e-9 * tr_a / tr_h.max(1e-300);
    for i in 0..k * k {
        a[i] += eps * h[i];
    }
    solve_dense(a, b, k).unwrap_or_else(|| vec![0.0; k])
}

// ── projection ─────────────────────────────────────────────────────────────

/// Where one parameter may go (§2.4.2, `tunable` ranges; §2.3.4 ints).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Bound {
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Values are `min + n · step` (or `n · step` with no `min`).
    pub step: Option<f64>,
    /// An int literal that may not be respelled stays whole.
    pub int: bool,
}

impl Bound {
    /// The bound of `p`: its `tunable` range and step, and whether it must
    /// stay an int (an int literal, unless respelling is allowed).
    pub fn of(p: &Param, allow_respell: bool) -> Bound {
        let (min, max, step) = match p.hint {
            Some(Hint::Tunable { min, max, step }) => (min, max, step),
            _ => (None, None, None),
        };
        Bound { min, max, step, int: p.is_int && !allow_respell }
    }

    fn clamp(&self, mut v: f64) -> f64 {
        if let Some(lo) = self.min {
            v = v.max(lo);
        }
        if let Some(hi) = self.max {
            v = v.min(hi);
        }
        v
    }

    /// The nearest allowed value to `v`.
    pub fn project(&self, v: f64) -> f64 {
        let mut v = self.clamp(v);
        if let Some(step) = self.step.filter(|s| *s > 0.0 && s.is_finite()) {
            let base = self.min.unwrap_or(0.0);
            let mut n = ((v - base) / step).round();
            if self.max.is_some_and(|hi| base + n * step > hi + 1e-12 * hi.abs().max(1.0)) {
                n -= 1.0;
            }
            v = base + n * step;
            // `0.1 * 3` is not `0.3`: clean the float up to the step's decimals.
            v = round_to(v, decimals_of_f64(step).max(decimals_of_f64(base)));
        }
        if self.int {
            v = self.clamp(v.round());
            if self.min.is_some_and(|lo| v < lo) {
                v += 1.0;
            }
            if self.max.is_some_and(|hi| v > hi) {
                v -= 1.0;
            }
        }
        v
    }

    /// Whether `v` sits at an end of the range.
    pub fn at_limit(&self, v: f64) -> bool {
        self.min.is_some_and(|lo| v <= lo) || self.max.is_some_and(|hi| v >= hi)
    }
}

/// `v` rounded to `d` decimals.
pub fn round_to(v: f64, d: u32) -> f64 {
    if d > 15 {
        return v;
    }
    let f = 10f64.powi(d as i32);
    let r = (v * f).round() / f;
    // Through the text, so `2.675` rounded to 2 decimals reads back as written.
    format!("{r:.*}", d as usize).parse().unwrap_or(r)
}

/// How many decimals the shortest spelling of `v` has.
fn decimals_of_f64(v: f64) -> u32 {
    let s = format!("{v:?}");
    match s.split_once('.') {
        Some((_, d)) if !d.contains('e') => d.trim_end_matches('0').len() as u32,
        _ => 0,
    }
}

// ── goals ──────────────────────────────────────────────────────────────────

/// One goal as the solver sees it: a row of the Jacobian, where it should
/// be, how close is close enough, and how much it counts against the others.
///
/// An *equality* (`range: None`) wants the value at `target`, within
/// `tolerance`. A *range* (`--at-most`, `--at-least`, `--within`) wants it
/// inside `[lo, hi]` (an open end is infinite): it holds anywhere inside and
/// nowhere outside, so `tolerance` is not slack there but the margin a
/// violated range is solved to, one tolerance inside the bound it crossed
/// (rounding then cannot push it back out). A range that holds asks the step
/// for nothing ([`goal_step`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalRow {
    /// The equality's target; for a range, the finite bound (the middle of
    /// a band): only [`aim`](Self::aim) reads it there.
    pub target: f64,
    pub tolerance: f64,
    pub weight: f64,
    pub range: Option<(f64, f64)>,
}

impl GoalRow {
    /// The equality "at `target`, within `tolerance`".
    pub fn eq(target: f64, tolerance: f64, weight: f64) -> GoalRow {
        GoalRow { target, tolerance, weight, range: None }
    }

    /// The range "inside `[lo, hi]`"; `lo` may be `-inf` and `hi` `inf`.
    pub fn range(lo: f64, hi: f64, tolerance: f64, weight: f64) -> GoalRow {
        let target = match (lo.is_finite(), hi.is_finite()) {
            (true, true) => 0.5 * (lo + hi),
            (true, false) => lo,
            _ => hi,
        };
        GoalRow { target, tolerance, weight, range: Some((lo, hi)) }
    }

    /// The goal holds at `v`: within tolerance of an equality's target, or
    /// inside a range.
    pub fn holds(&self, v: f64) -> bool {
        self.error(v) <= 1.0
    }

    /// How far `v` is from the goal, in the goal's own units: `|v - t|`, or
    /// how far outside the range it is (0 inside).
    pub fn miss(&self, v: f64) -> f64 {
        match self.range {
            None => (v - self.target).abs(),
            Some((lo, hi)) => (lo - v).max(v - hi).max(0.0),
        }
    }

    /// The error in tolerance units, `≤ 1` when the goal holds:
    /// `|v - t| / τ` for an equality; for a range 0 inside and
    /// `1 + miss / τ` outside, so that a value past the bound by any amount
    /// does not hold.
    pub fn error(&self, v: f64) -> f64 {
        if !v.is_finite() {
            return f64::INFINITY;
        }
        match self.range {
            None => self.miss(v) / self.tolerance,
            Some(_) => {
                let m = self.miss(v);
                if m == 0.0 { 0.0 } else { 1.0 + m / self.tolerance }
            }
        }
    }

    /// Where a step from `v` aims: an equality's target; for a range, `v`
    /// itself inside it, else the bound `v` is past moved one tolerance
    /// inside (the middle of a band narrower than two tolerances).
    pub fn aim(&self, v: f64) -> f64 {
        let Some((lo, hi)) = self.range else { return self.target };
        let margin = if lo.is_finite() && hi.is_finite() { self.tolerance.min(0.5 * (hi - lo)) } else { self.tolerance };
        if v > hi {
            hi - margin
        } else if v < lo {
            lo + margin
        } else {
            v
        }
    }

    /// What the step should move the goal by from `v` (`aim - v`); 0 for a
    /// missing value.
    pub fn residual(&self, v: Option<f64>) -> f64 {
        match v {
            Some(v) if v.is_finite() => self.aim(v) - v,
            _ => 0.0,
        }
    }

    /// The row asks a step for something at `v`: an equality always (a
    /// held one asks to stay), a range only when it does not hold.
    pub fn active(&self, v: Option<f64>) -> bool {
        match (self.range, v) {
            (None, _) => true,
            (Some(_), Some(v)) => !self.holds(v),
            (Some(_), None) => false,
        }
    }
}

/// `max_j error_j(g_j)` ([`GoalRow::error`]); `inf` when a goal is missing.
pub fn max_error(goals: &[GoalRow], values: &[Option<f64>]) -> f64 {
    goals
        .iter()
        .zip(values)
        .map(|(g, v)| match v {
            Some(v) if v.is_finite() => g.error(*v),
            _ => f64::INFINITY,
        })
        .fold(0.0, f64::max)
}

/// `Σ_j (w_j · error_j)²`: what a step must make smaller.
fn sq_error(goals: &[GoalRow], values: &[Option<f64>]) -> f64 {
    goals
        .iter()
        .zip(values)
        .map(|(g, v)| match v {
            Some(v) if v.is_finite() => (g.weight * g.error(*v)).powi(2),
            _ => f64::INFINITY,
        })
        .sum()
}

/// The goal rows that ask a step for something at `values`
/// ([`GoalRow::active`]).
pub fn active_goals(goals: &[GoalRow], values: &[Option<f64>]) -> Vec<bool> {
    goals.iter().zip(values).map(|(g, v)| g.active(*v)).collect()
}

/// One step for the goals at `values` ([`linear_step`] over the rows that
/// ask for something), with the inequalities as an active set: a range
/// that holds is left out, unless the step would push it out, in which
/// case it joins, aimed one tolerance inside the bound it would cross, and
/// the step is solved again (at most once per goal).
pub fn goal_step(goals: &[GoalRow], values: &[Option<f64>], jg: &[Vec<f64>], jc: &[Vec<f64>], cost: &[f64]) -> Vec<f64> {
    let k = cost.len();
    let mut active = active_goals(goals, values);
    let mut aim: Vec<f64> = goals.iter().zip(values).map(|(g, v)| g.residual(*v)).collect();
    let mut delta = vec![0.0; k];
    for _ in 0..=goals.len() {
        let rows: Vec<usize> = (0..goals.len()).filter(|&g| active[g]).collect();
        if rows.is_empty() {
            return vec![0.0; k];
        }
        let jga: Vec<Vec<f64>> = rows.iter().map(|&g| jg[g].clone()).collect();
        let r: Vec<f64> = rows.iter().map(|&g| aim[g]).collect();
        let wt: Vec<f64> = rows.iter().map(|&g| goals[g].weight / goals[g].tolerance).collect();
        delta = linear_step(&jga, jc, &r, &wt, cost);
        let mut joined = false;
        for g in 0..goals.len() {
            let (false, Some(v)) = (active[g], values[g]) else { continue };
            let moved = v + jg[g].iter().zip(&delta).map(|(j, d)| j * d).sum::<f64>();
            if !goals[g].holds(moved) {
                active[g] = true;
                aim[g] = goals[g].aim(moved) - v;
                joined = true;
            }
        }
        if !joined {
            break;
        }
    }
    delta
}

// ── the verification loop ──────────────────────────────────────────────────

/// The solver's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveOptions {
    /// Verification steps per subset (§2.4.2).
    pub max_iters: usize,
    pub allow_respell: bool,
    pub fd: FdOptions,
}

impl Default for SolveOptions {
    fn default() -> Self {
        SolveOptions { max_iters: 6, allow_respell: false, fd: FdOptions::default() }
    }
}

/// What a request's solves share: the session, the parameters, the
/// Jacobian measured at the baseline and the goals.
pub struct SolveCtx<'a> {
    pub session: &'a mut SpecSession,
    pub env: &'a mut Env,
    pub host: &'a mut dyn SpecHost,
    pub params: &'a [Param],
    pub jac: &'a Jacobian,
    /// The baseline value of every Jacobian row.
    pub base_rows: Vec<Option<f64>>,
    pub goals: &'a [GoalRow],
    /// Each Jacobian column's cost weight `d_i` (§2.5.2).
    pub weights: &'a [f64],
    pub bounds: Vec<Bound>,
    pub budget: &'a mut RunBudget,
    pub opts: SolveOptions,
    /// Runs made by this context.
    pub runs: usize,
    pub budget_exhausted: bool,
    /// Every run of this context that raised an error, in order: the values
    /// it tried were rejected (an `assert` is how a program states a
    /// constraint between literals).
    pub failed: Vec<FailedRun>,
    pub log: Option<&'a mut Vec<String>>,
}

/// A run that raised an error, and the values it was tried at.
#[derive(Debug, Clone, PartialEq)]
pub struct FailedRun {
    /// Jacobian columns.
    pub cols: Vec<usize>,
    /// Each column's value, as written.
    pub values: Vec<f64>,
    pub error: RunError,
}

/// The first line of a (possibly annotated, multi-line) error message.
pub fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// A solve of one subset, before rounding.
#[derive(Debug, Clone)]
pub struct Solution {
    /// Jacobian columns.
    pub cols: Vec<usize>,
    /// Each column's value, as written.
    pub values: Vec<f64>,
    /// The run at `values` (`None`: never moved from the baseline).
    pub run: Option<SpecRun>,
    /// Every row's value at `values`.
    pub rows: Vec<Option<f64>>,
    pub max_error: f64,
    pub iterations: usize,
    /// The local Jacobian at the end (rows × cols), after Broyden updates.
    pub local: Vec<Vec<f64>>,
}

impl SolveCtx<'_> {
    fn log(&mut self, line: impl FnOnce() -> String) {
        if let Some(log) = self.log.as_deref_mut() {
            log.push(line());
        }
    }

    /// The written value of column `c` at the baseline.
    pub fn base_value(&self, c: usize) -> f64 {
        self.params[self.jac.params[c]].value
    }

    /// The overrides that set `cols` to `values` (as written).
    pub fn overrides(&self, cols: &[usize], values: &[f64]) -> Overrides {
        let mut ov = Overrides::none();
        for (&c, &v) in cols.iter().zip(values) {
            let p = &self.params[self.jac.params[c]];
            if v != p.value || !p.is_int {
                ov = ov.with(p.term, p.override_for(v));
            }
        }
        ov
    }

    /// One run at `values`, if the budget allows.
    pub fn run_at(&mut self, cols: &[usize], values: &[f64]) -> Option<SpecRun> {
        if !self.budget.take() {
            self.budget_exhausted = true;
            return None;
        }
        let ov = self.overrides(cols, values);
        self.runs += 1;
        let run = self.session.run(self.env, self.host, &ov);
        if let Some(error) = &run.error {
            self.failed.push(FailedRun { cols: cols.to_vec(), values: values.to_vec(), error: error.clone() });
        }
        Some(run)
    }

    /// Every row of `run`.
    pub fn read_rows(&self, run: &SpecRun) -> Vec<Option<f64>> {
        run.read_all(&self.jac.rows)
    }

    pub fn goal_values<'v>(&self, rows: &'v [Option<f64>]) -> &'v [Option<f64>] {
        &rows[..self.jac.goal_rows]
    }

    /// The baseline Jacobian restricted to `cols`, every row.
    pub fn local_jacobian(&self, cols: &[usize]) -> Vec<Vec<f64>> {
        (0..self.jac.rows.len()).map(|r| cols.iter().map(|&c| self.jac.get(r, c)).collect()).collect()
    }

    /// The step for `cols` from `rows` with Jacobian `local` ([`goal_step`]).
    pub fn step(&self, cols: &[usize], rows: &[Option<f64>], local: &[Vec<f64>]) -> Vec<f64> {
        let gr = self.jac.goal_rows;
        let cost: Vec<f64> = cols
            .iter()
            .map(|&c| self.weights[c] / self.base_value(c).abs().max(1.0))
            .collect();
        goal_step(self.goals, &rows[..gr], &local[..gr], &local[gr..], &cost)
    }

    /// `values + Δ` projected onto the bounds.
    pub fn project(&self, cols: &[usize], values: &[f64], delta: &[f64], scale: f64) -> Vec<f64> {
        cols.iter()
            .zip(values.iter().zip(delta))
            .map(|(&c, (&v, &d))| self.bounds[c].project(v + scale * d))
            .collect()
    }

    /// Solve for `cols` from the baseline (§2.4.2): project, run, keep the
    /// step when the goal error went down, otherwise shorten it.
    pub fn solve_subset(&mut self, cols: &[usize]) -> Solution {
        let k = cols.len();
        let mut values: Vec<f64> = cols.iter().map(|&c| self.base_value(c)).collect();
        let mut rows = self.base_rows.clone();
        let mut local = self.local_jacobian(cols);
        let mut err = sq_error(self.goals, self.goal_values(&rows));
        let mut fingerprint = self.session.baseline().fingerprint;
        let mut run: Option<SpecRun> = None;
        let mut scale = 1.0f64;
        let mut iterations = 0;
        let mut since_measure = 0;
        while iterations < self.opts.max_iters {
            if max_error(self.goals, self.goal_values(&rows)) <= 1.0 {
                break;
            }
            let delta = self.step(cols, &rows, &local);
            let next = self.project(cols, &values, &delta, scale);
            if next == values {
                if scale < 1.0 {
                    // A shortened step that rounds to nothing: try a full one.
                    scale = 1.0;
                    let full = self.project(cols, &values, &delta, 1.0);
                    if full == values {
                        break;
                    }
                    continue;
                }
                break;
            }
            iterations += 1;
            let Some(r) = self.run_at(cols, &next) else { break };
            let new_rows = self.read_rows(&r);
            let new_err = if r.ok() { sq_error(self.goals, self.goal_values(&new_rows)) } else { f64::INFINITY };
            self.log(|| {
                format!(
                    "  step {iterations}: {:?} → error {:.3e} (was {:.3e}){}{}",
                    next,
                    new_err,
                    err,
                    if r.fingerprint != fingerprint { ", other branch" } else { "" },
                    r.error.as_ref().map(|e| format!(", rejected: {}", first_line(&e.to_string()))).unwrap_or_default()
                )
            });
            if new_err < err {
                // Broyden: J += (Δy - J Δp) Δpᵀ / (Δpᵀ Δp), with Δp scaled
                // per parameter so that differently sized literals mix.
                let dp: Vec<f64> = next.iter().zip(&values).map(|(a, b)| a - b).collect();
                let sc: Vec<f64> = cols.iter().map(|&c| self.base_value(c).abs().max(1.0)).collect();
                let dps: Vec<f64> = dp.iter().zip(&sc).map(|(d, s)| d / s).collect();
                let nn: f64 = dps.iter().map(|x| x * x).sum();
                let regime = r.fingerprint != fingerprint;
                if nn > 0.0 {
                    for (row, (new, old)) in local.iter_mut().zip(new_rows.iter().zip(&rows)) {
                        let (Some(new), Some(old)) = (new, old) else { continue };
                        let pred: f64 = row.iter().zip(&dp).map(|(j, d)| j * d).sum();
                        let resid = (new - old) - pred;
                        for i in 0..k {
                            row[i] += resid * dps[i] / nn / sc[i];
                        }
                    }
                }
                values = next;
                rows = new_rows;
                err = new_err;
                fingerprint = r.fingerprint;
                run = Some(r);
                scale = (scale * 2.0).min(1.0);
                since_measure += 1;
                if (regime || since_measure >= 3)
                    && k <= 3
                    && max_error(self.goals, self.goal_values(&rows)) > 1.0
                    && let Some(j) = self.remeasure(cols, &values)
                {
                    local = j;
                    since_measure = 0;
                }
            } else {
                scale /= 4.0;
                if scale < 1e-3 {
                    break;
                }
            }
        }
        let max_err = max_error(self.goals, self.goal_values(&rows));
        Solution { cols: cols.to_vec(), values, run, rows, max_error: max_err, iterations, local }
    }

    /// Measure the columns of `cols` again at `values` (the point a step
    /// reached), when the budget has room for it.
    fn remeasure(&mut self, cols: &[usize], values: &[f64]) -> Option<Vec<Vec<f64>>> {
        let need = 1 + 2 * cols.len();
        if self.budget.max_runs.saturating_sub(self.budget.used) < need + 2 {
            return None;
        }
        let at = self.overrides(cols, values);
        let keep: Vec<usize> = cols.iter().map(|&c| self.jac.params[c]).collect();
        let j = jacobian(
            self.session,
            self.env,
            self.host,
            self.params,
            &keep,
            self.jac.rows.clone(),
            self.jac.goal_rows,
            &at,
            &self.opts.fd,
            self.budget,
        );
        self.runs += j.runs;
        if j.budget_exhausted {
            self.budget_exhausted = true;
        }
        if !j.quality.iter().all(|q| q.usable()) {
            return None;
        }
        self.log(|| format!("  re-measured {} column(s) at {values:?}", cols.len()));
        Some((0..j.rows.len()).map(|r| (0..cols.len()).map(|c| j.get(r, c)).collect()).collect())
    }
}

// ── rounding (§2.4.4) ──────────────────────────────────────────────────────

/// How many decimals a literal is written with (`3.50` → 2, `40` → 0).
pub fn written_decimals(text: &str) -> Option<u32> {
    let t = text.trim().trim_start_matches('-');
    let (_, d) = t.split_once('.')?;
    d.chars().all(|c| c.is_ascii_digit()).then_some(d.len() as u32)
}

/// The values a parameter may be rounded to, nicest first (§2.4.4): an int
/// if it was one, then its own decimals, then 0 to 6 decimals, then the
/// value itself (to 9 decimals).
pub fn ladder(v: f64, p: &Param, text: &str, bound: &Bound, allow_respell: bool) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    let mut push = |x: f64| {
        let x = bound.project(x);
        if x.is_finite() && !out.contains(&x) {
            out.push(x);
        }
    };
    if p.is_int {
        push(v.round());
        if !allow_respell || bound.int {
            return out;
        }
    }
    if let Some(d) = written_decimals(text) {
        push(round_to(v, d));
    }
    for d in 0..=6 {
        push(round_to(v, d));
    }
    push(round_to(v, 9));
    out
}

/// A solution after rounding.
#[derive(Debug, Clone)]
pub struct Rounded {
    pub values: Vec<f64>,
    pub run: Option<SpecRun>,
    pub rows: Vec<Option<f64>>,
    pub max_error: f64,
}

impl SolveCtx<'_> {
    /// Round `sol` to nice literals (§2.4.4): pick, per parameter, the
    /// nicest ladder value the linear model says keeps every goal within
    /// tolerance, and run them jointly; when that misses, round one
    /// parameter at a time, largest change first, each checked by a run.
    /// `texts` are the columns' literal texts as written.
    pub fn round_solution(&mut self, sol: &Solution, texts: &[String]) -> Rounded {
        let cols = &sol.cols;
        let gr = self.jac.goal_rows;
        let ladders: Vec<Vec<f64>> = cols
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let p = &self.params[self.jac.params[c]];
                if sol.values[i] == p.value {
                    vec![p.value]
                } else {
                    ladder(sol.values[i], p, &texts[i], &self.bounds[c], self.opts.allow_respell)
                }
            })
            .collect();
        // How far each goal may end up from its target. A verified solution
        // keeps every goal within tolerance; one that stopped short (an
        // `assert` in the way, the budget, a goal out of reach) may get
        // worse by at most one tolerance, so that where it stopped prints
        // as a clean number too (`3.55`, not `3.5546874999999756`).
        let verified = sol.max_error <= 1.0;
        let goals: &[GoalRow] = self.goals;
        // In each goal's own units ([`GoalRow::miss`]); a verified solution
        // must keep every goal holding ([`GoalRow::holds`]: a range inside
        // its bounds).
        let slack: Vec<f64> = (0..gr)
            .map(|g| match sol.rows[g] {
                _ if verified => f64::NAN,
                Some(v) if v.is_finite() => goals[g].miss(v) + goals[g].tolerance,
                _ => f64::NAN,
            })
            .collect();
        let within = |g: usize, v: Option<f64>| -> bool {
            v.is_some_and(|v| if verified { goals[g].holds(v) } else { goals[g].miss(v) <= slack[g] })
        };
        // The linear prediction of the goals with column i moved by `d`.
        let predict_ok =
            |i: usize, d: f64| -> bool { (0..gr).all(|g| within(g, sol.rows[g].map(|v| v + sol.local[g][i] * d))) };
        let accept = |run: &SpecRun, rows: &[Option<f64>]| -> bool { run.ok() && (0..gr).all(|g| within(g, rows[g])) };
        let picks: Vec<usize> = ladders
            .iter()
            .enumerate()
            .map(|(i, l)| l.iter().position(|&x| predict_ok(i, x - sol.values[i])).unwrap_or(l.len() - 1))
            .collect();
        let joint: Vec<f64> = picks.iter().zip(&ladders).map(|(&p, l)| l[p]).collect();
        let finish = |values: Vec<f64>, run: Option<SpecRun>, rows: Vec<Option<f64>>, goals: &[GoalRow]| {
            let max_error = max_error(goals, &rows[..gr]);
            Rounded { values, run, rows, max_error }
        };
        if joint == sol.values {
            return finish(joint, sol.run.clone(), sol.rows.clone(), self.goals);
        }
        let Some(run) = self.run_at(cols, &joint) else {
            return finish(sol.values.clone(), sol.run.clone(), sol.rows.clone(), self.goals);
        };
        let rows = self.read_rows(&run);
        if accept(&run, &rows) {
            return finish(joint, Some(run), rows, self.goals);
        }
        let joint_err = if run.ok() { max_error(self.goals, &rows[..gr]) } else { f64::INFINITY };
        self.log(|| format!("  joint rounding {joint:?} missed (error {joint_err:.3}); one at a time"));
        // One at a time, largest change first.
        let mut order: Vec<usize> = (0..cols.len()).collect();
        order.sort_by(|&a, &b| {
            let da = (sol.values[a] - self.base_value(cols[a])).abs() / self.base_value(cols[a]).abs().max(1.0);
            let db = (sol.values[b] - self.base_value(cols[b])).abs() / self.base_value(cols[b]).abs().max(1.0);
            db.total_cmp(&da)
        });
        let mut cur = sol.values.clone();
        let mut best: (Option<SpecRun>, Vec<Option<f64>>) = (sol.run.clone(), sol.rows.clone());
        for i in order {
            let l = &ladders[i];
            if l.len() == 1 && l[0] == cur[i] {
                continue;
            }
            for &x in l.iter().skip(picks[i]).take(4) {
                if x == cur[i] {
                    break;
                }
                let mut test = cur.clone();
                test[i] = x;
                let Some(run) = self.run_at(cols, &test) else { break };
                let rows = self.read_rows(&run);
                if accept(&run, &rows) {
                    cur = test;
                    best = (Some(run), rows);
                    break;
                }
            }
        }
        finish(cur, best.0, best.1, self.goals)
    }
}

