//! Heuristics and ranking (docs/dev/direct-manipulation-v2-plan.md §2.5,
//! M3): which literals a drag should prefer to move, which subsets of them
//! are worth solving, and how the verified alternatives are ordered.
//!
//! - [`param_weight`]: the cost weight `d_i` of §2.5.2 (lower is preferred):
//!   `tunable` and `config let` first, then a literal written in the goal's
//!   own call, then top-level bindings, then function bodies, then other
//!   files; feedback through `var`, `state` or the clock makes a literal
//!   costlier.
//! - [`candidate_subsets`]: the subsets of §2.5.3: each single parameter (one
//!   goal) or tuple (several goals) among the strongest columns that can
//!   reach the goals, the config-only subset, the client's set, and
//!   everything.
//! - [`collateral_norm`] and [`score`]: §2.5.4. What else moved is measured
//!   from the verification run, not guessed from the program's shape.

use crate::dm_v2::hints::Hint;
use crate::dm_v2::jacobian::Jacobian;
use crate::dm_v2::params::{CodeOrigin, Param};
use crate::dm_v2::solve::rank;

/// The cost weight `d_i` of a parameter (§2.5.2).
pub fn param_weight(p: &Param, clock_coupled: bool) -> f64 {
    let mut d = if matches!(p.hint, Some(Hint::Tunable { .. })) {
        0.25
    } else if p.config {
        0.5
    } else if p.in_goal_frame {
        0.75
    } else {
        match p.origin {
            CodeOrigin::Entry if p.function.is_none() => 1.0,
            CodeOrigin::Entry => 1.5,
            _ => 2.0,
        }
    };
    if p.through_var {
        d *= 1.5;
    }
    if p.through_state {
        d *= 4.0;
    }
    if clock_coupled {
        d *= 4.0;
    }
    d
}

/// The `κ` of the score (§2.5.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreWeights {
    /// Collateral.
    pub kc: f64,
    /// Each edit beyond one per goal.
    pub kn: f64,
    /// Each int respelled as a float.
    pub kr: f64,
    /// The control flow changed.
    pub kf: f64,
    /// The random draws would reshuffle.
    pub kg: f64,
    /// The goal error, when not verified.
    pub ke: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        ScoreWeights { kc: 1.0, kn: 0.5, kr: 0.3, kf: 0.5, kg: 0.5, ke: 2.0 }
    }
}

/// One parameter change as the score sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoredChange {
    pub weight: f64,
    pub old: f64,
    pub new: f64,
}

/// What the score of one alternative is made of.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScoreParts {
    /// `Σ d_i |Δp_i| / s_i`: how much code meaning moves.
    pub edit: f64,
    pub collateral: f64,
    pub count: f64,
    pub respelled: f64,
    pub regime: f64,
    pub rng: f64,
    pub error: f64,
}

impl ScoreParts {
    pub fn total(&self) -> f64 {
        self.edit + self.collateral + self.count + self.respelled + self.regime + self.rng + self.error
    }
}

/// The inputs of [`score`] beyond the changes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScoreFacts {
    pub collateral_norm: f64,
    pub goals: usize,
    pub respelled: usize,
    pub regime_changed: bool,
    pub rng_order_changed: bool,
    pub verified: bool,
    /// `max_j |achieved - target| / τ_j`.
    pub max_error: f64,
}

/// The score of §2.5.4 (lower is better). The "∞ when unverified and a
/// verified alternative exists" rule is the sort's, not the score's.
pub fn score(changes: &[ScoredChange], facts: &ScoreFacts, k: &ScoreWeights) -> ScoreParts {
    let edit = changes
        .iter()
        .filter(|c| c.new != c.old)
        .map(|c| c.weight * (c.new - c.old).abs() / c.old.abs().max(1.0))
        .sum();
    let moved = changes.iter().filter(|c| c.new != c.old).count();
    ScoreParts {
        edit,
        collateral: k.kc * facts.collateral_norm,
        count: k.kn * moved.saturating_sub(facts.goals) as f64,
        respelled: k.kr * facts.respelled as f64,
        regime: if facts.regime_changed { k.kf } else { 0.0 },
        rng: if facts.rng_order_changed { k.kg } else { 0.0 },
        error: if facts.verified { 0.0 } else { k.ke * facts.max_error.min(1e6) },
    }
}

/// How much the non-goal rows moved, relative to how much the goals moved:
/// `‖Δ collateral‖ / ‖Δ goals‖` (root-sum-square, §9 M3). A row that is gone
/// counts as moving as far as the goals did.
pub fn collateral_norm(goal_moves: &[f64], collateral_moves: &[Option<f64>]) -> f64 {
    let g = goal_moves.iter().map(|x| x * x).sum::<f64>().sqrt();
    let (mut sq, mut gone) = (0.0, 0usize);
    for m in collateral_moves {
        match m {
            Some(m) if m.is_finite() => sq += m * m,
            _ => gone += 1,
        }
    }
    if g <= 1e-12 {
        return if sq > 0.0 || gone > 0 { 1.0 } else { 0.0 };
    }
    (sq.sqrt() / g) + gone as f64
}

/// Why a subset is solved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubsetKind {
    /// One parameter per goal, among the strongest columns.
    Tuple,
    /// Every config and tunable parameter that moves a goal.
    ConfigOnly,
    /// What the client allowed.
    Client,
    /// Every screened parameter (the minimum-norm spread).
    Full,
}

/// A subset of Jacobian columns to solve for.
#[derive(Debug, Clone, PartialEq)]
pub struct Subset {
    pub cols: Vec<usize>,
    pub kind: SubsetKind,
}

/// How many of the strongest columns tuples are drawn from (§2.5.3).
pub const TOP_COLUMNS: usize = 12;

/// How strongly column `c` moves the goals per unit of cost: `‖J_g[:, c]‖ ·
/// s_c / d_c`, the goal movement of a relative change of the literal,
/// over its weight.
pub fn strength(jac: &Jacobian, c: usize, scale: f64, weight: f64) -> f64 {
    let n: f64 = (0..jac.goal_rows).map(|r| jac.get(r, c).powi(2)).sum::<f64>().sqrt();
    n * scale / weight
}

/// The candidate subsets of §2.5.3 over the usable columns of `jac`.
/// `scales[c]` is `max(|p|, 1)` of column `c`'s literal, `config[c]` says it
/// is a config or tunable parameter; `client` says the request named the
/// parameters (every usable column is then the client's set).
pub fn candidate_subsets(
    jac: &Jacobian,
    weights: &[f64],
    scales: &[f64],
    usable: &[bool],
    config: &[bool],
    client: bool,
) -> Vec<Subset> {
    let m = jac.goal_rows;
    let cols: Vec<usize> = (0..jac.params.len()).filter(|&c| usable[c]).collect();
    let strength = |c: usize| -> f64 { strength(jac, c, scales[c], weights[c]) };
    let mut top: Vec<usize> = cols.iter().copied().filter(|&c| strength(c) > 0.0).collect();
    top.sort_by(|&a, &b| strength(b).total_cmp(&strength(a)).then(a.cmp(&b)));
    top.truncate(TOP_COLUMNS);
    let mut out: Vec<Subset> = Vec::new();
    let push = |cols: Vec<usize>, kind: SubsetKind, out: &mut Vec<Subset>| {
        let mut cols = cols;
        cols.sort_unstable();
        if cols.is_empty() || out.iter().any(|s| s.cols == cols) {
            return;
        }
        out.push(Subset { cols, kind });
    };
    // 1. Tuples of size m (for up to three goals; beyond that the full set).
    if (1..=3).contains(&m) {
        let mut idx: Vec<usize> = (0..m).collect();
        let n = top.len();
        if n >= m {
            loop {
                let sub: Vec<usize> = idx.iter().map(|&i| top[i]).collect();
                let jg: Vec<Vec<f64>> = (0..m).map(|r| sub.iter().map(|&c| jac.get(r, c)).collect()).collect();
                if rank(&jg) == m {
                    push(sub, SubsetKind::Tuple, &mut out);
                }
                // Next combination.
                let mut i = m;
                while i > 0 && idx[i - 1] == n - m + i - 1 {
                    i -= 1;
                }
                if i == 0 {
                    break;
                }
                idx[i - 1] += 1;
                for j in i..m {
                    idx[j] = idx[j - 1] + 1;
                }
            }
        }
    }
    // 2. Config only.
    let cfg: Vec<usize> = cols.iter().copied().filter(|&c| config[c] && strength(c) > 0.0).collect();
    if !cfg.is_empty() {
        push(cfg, SubsetKind::ConfigOnly, &mut out);
    }
    // 3 and 4. The client's set, or everything.
    let all: Vec<usize> = cols.iter().copied().filter(|&c| strength(c) > 0.0).collect();
    push(all, if client { SubsetKind::Client } else { SubsetKind::Full }, &mut out);
    out
}
