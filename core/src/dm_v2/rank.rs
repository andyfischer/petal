//! Heuristics and ranking (docs/dev/direct-manipulation-v2-plan.md §2.5,
//! M3): which literals a drag should prefer to move, which subsets of them
//! are worth solving, and how the verified alternatives are ordered.
//!
//! - [`param_weight`]: the cost weight `d_i` of §2.5.2 (lower is preferred):
//!   knobs (`config let` or `tunable`, one tier) first, then a literal written in the goal's
//!   own call, then top-level bindings, then function bodies, then other
//!   files; feedback through `var`, `state` or the clock makes a literal
//!   costlier.
//! - [`candidate_subsets`]: the subsets of §2.5.3: each single parameter (one
//!   goal) or tuple (several goals) among the strongest columns that can
//!   reach the goals that move (pins do not count), a few single literals
//!   with several goals, the config-only subset, the client's set, and
//!   everything.
//! - [`collateral_norm`] and [`score`]: §2.5.4. What else moved is measured
//!   from the verification run, not guessed from the program's shape.

use crate::dm_v2::jacobian::Jacobian;
use crate::dm_v2::params::{CodeOrigin, Param};
use crate::dm_v2::solve::rank;

/// The weight of a knob (§2.5.2): a literal marked by `config let`, by
/// `tunable(...)`, or by both. One tier, so marking a literal twice changes
/// nothing and `config let x = tunable(v)` ranks as `config let x = v` does.
pub const KNOB_WEIGHT: f64 = 0.5;

/// The cost weight `d_i` of a parameter (§2.5.2).
pub fn param_weight(p: &Param, clock_coupled: bool) -> f64 {
    let mut d = if p.is_knob() {
        KNOB_WEIGHT
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
    /// The share of the drag this change carries, `‖J_g[:, i] Δ_i‖₁ / ‖r‖₁`
    /// (§9 M5, revision 1); `None` charges the relative change instead.
    pub share: Option<f64>,
}

/// The weight of the relative change in [`edit_cost`].
pub const REL_WEIGHT: f64 = 0.1;

/// What one change costs (§2.5.4 as revised in §9 M5, revision 1):
/// `d_i · (share_i + 0.1 · min(|Δ_i| / max(|p_i|, 1), 10))`. The share is how
/// much of the drag the literal does, in output units, so every literal
/// that does the whole drag alone costs its weight `d_i` and the weights and
/// the measured collateral decide between them; the small relative term
/// breaks ties against implausible values (2 → 200). Charging the
/// relative change alone (M3) made a large coordinate (`150` → `130`)
/// cheaper than the offset or gap a drag means (`0` → `-20`, `8` → `16`),
/// and a rate or angle (whose change is small in its own units) cheaper
/// than either.
pub fn edit_cost(c: &ScoredChange) -> f64 {
    let rel = (c.new - c.old).abs() / c.old.abs().max(1.0);
    match c.share {
        Some(share) => c.weight * (share.min(4.0) + REL_WEIGHT * rel.min(10.0)),
        None => c.weight * rel,
    }
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
    /// What the client's channel weights above 1 add, uncompressed
    /// ([`weighted_collateral`]).
    pub collateral_excess: f64,
    /// The edits that are free of the count penalty: the goal count. The
    /// final ranking charges again against the fewest edits any verified
    /// alternative needed (§9 M5, revision 2).
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
        .map(edit_cost)
        .sum();
    let moved = changes.iter().filter(|c| c.new != c.old).count();
    ScoreParts {
        edit,
        collateral: k.kc * (compress_collateral(facts.collateral_norm) + facts.collateral_excess.max(0.0)),
        count: k.kn * moved.saturating_sub(facts.goals) as f64,
        respelled: k.kr * facts.respelled as f64,
        regime: if facts.regime_changed { k.kf } else { 0.0 },
        rng: if facts.rng_order_changed { k.kg } else { 0.0 },
        error: if facts.verified { 0.0 } else { k.ke * facts.max_error.min(1e6) },
    }
}

/// The collateral term of the score (§9 M5, revision 6), over
/// [`weighted_collateral`]: linear up to 1 (one other shape moved as far as
/// the drag), logarithmic beyond, so that once a change moves other shapes,
/// moving *more* of them costs less than another edit does. Without it, in
/// a scene where every literal moves everything (a recursive tree), a
/// spread of three edits that partly cancel each other outranks every
/// single-literal edit.
pub fn compress_collateral(norm: f64) -> f64 {
    if norm <= 1.0 { norm.max(0.0) } else { 1.0 + norm.ln() }
}

/// [`weighted_collateral`]'s result: the collateral count under the
/// client's channel weights (§9, Canopy S3).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WeightedCollateral {
    /// `Σ_e min(w_e, 1) · share_e`: the count the score compresses
    /// ([`compress_collateral`]).
    pub shapes: f64,
    /// `Σ_e max(w_e - 1, 0) · share_e`: what weights above 1 add, charged as
    /// it is. A shape on a channel of weight 10 that moves as far as the drag
    /// costs 1 in the compressed count plus 9 here, so weight 100 costs ten
    /// times what weight 10 does instead of `ln 10` more.
    pub excess: f64,
}

/// How many other shapes moved, each counted by how far it moved relative to
/// the goals (§9 M5, revision 6): `Σ_e min(1, max_move_e / ‖Δ goals‖)` over
/// the emits `e` of the collateral rows (`keys[k]` is row `k`'s emit). A
/// shape that moves as far as the drag counts 1 however many of its numbers
/// change, so a row of squares that all resize (`size`) counts the squares,
/// not their widths and heights twice over; a shape that is gone counts 1.
/// This, not the root-sum-square norm, is what the score charges.
///
/// With a weight per collateral row (`weights[k]`, row `k`'s channel
/// weight; missing rows weigh 1), each emit's share is
/// split by [`WeightedCollateral`]. A weight below 1 discounts the emit in
/// the compressed count, 0 leaves it out.
pub fn weighted_collateral<K: Eq + std::hash::Hash + Copy>(
    keys: &[Option<K>],
    moves: &[Option<f64>],
    goal_moves: &[f64],
    weights: &[f64],
) -> WeightedCollateral {
    let g = goal_moves.iter().map(|x| x * x).sum::<f64>().sqrt();
    // In first-seen order, so the sum (and a tie between two alternatives)
    // does not depend on hash order.
    let mut index: std::collections::HashMap<K, usize> = std::collections::HashMap::new();
    let mut per: Vec<(f64, f64)> = Vec::new();
    for (row, (k, m)) in keys.iter().zip(moves).enumerate() {
        let Some(k) = *k else { continue };
        let mag = match m {
            Some(x) if x.is_finite() => x.abs(),
            Some(_) | None => f64::INFINITY,
        };
        if mag <= 1e-9 {
            continue;
        }
        let share = if g <= 1e-12 { 1.0 } else { (mag / g).min(1.0) };
        let w = weights.get(row).copied().unwrap_or(1.0);
        let i = *index.entry(k).or_insert_with(|| {
            per.push((0.0, 1.0));
            per.len() - 1
        });
        per[i].0 = per[i].0.max(share);
        // Rows of one emit share its channel's weight.
        per[i].1 = w;
    }
    let mut out = WeightedCollateral::default();
    for (share, w) in per {
        out.shapes += w.clamp(0.0, 1.0) * share;
        out.excess += (w - 1.0).max(0.0) * share;
    }
    out
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

/// How many of the strongest columns are also tried alone when there are
/// several goals (at any goal count, pins included).
pub const SINGLES_FOR_SEVERAL_GOALS: usize = 4;

/// How many of the strongest config columns config-only tuples are drawn
/// from (every config-only subset is verified, so this stays small).
pub const CONFIG_TUPLE_COLUMNS: usize = 6;

/// Every `m`-subset of `items`, in order.
fn combinations(items: &[usize], m: usize) -> Vec<Vec<usize>> {
    let n = items.len();
    let mut out = Vec::new();
    if m == 0 || n < m {
        return out;
    }
    let mut idx: Vec<usize> = (0..m).collect();
    loop {
        out.push(idx.iter().map(|&i| items[i]).collect());
        let mut i = m;
        while i > 0 && idx[i - 1] == n - m + i - 1 {
            i -= 1;
        }
        if i == 0 {
            return out;
        }
        idx[i - 1] += 1;
        for j in i..m {
            idx[j] = idx[j - 1] + 1;
        }
    }
}

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
///
/// `active` are the goal rows that ask for something: every equality that
/// does not hold at the baseline, and each range goal that does not hold (a
/// range that holds needs no literal). A goal that holds (a `--by 0` pin,
/// or one already at its target) does not ask (§9, Canopy S6). Tuple sizes,
/// the rank check and the strength order count only the active rows, so
/// adding a pin to a request keeps the single-literal and tuple answers it
/// had; the pins stay goal rows of every solve, and the verification run
/// judges whether a subset kept them. Empty: every goal row.
pub fn candidate_subsets(
    jac: &Jacobian,
    weights: &[f64],
    scales: &[f64],
    usable: &[bool],
    config: &[bool],
    client: bool,
    active: &[usize],
) -> Vec<Subset> {
    let rows: Vec<usize> = if active.is_empty() { (0..jac.goal_rows).collect() } else { active.to_vec() };
    let m = rows.len();
    let cols: Vec<usize> = (0..jac.params.len()).filter(|&c| usable[c]).collect();
    // How strongly a column moves the moving goals: a literal that only
    // moves a pin is no candidate for a tuple.
    let pull = |c: usize| -> f64 {
        let n: f64 = rows.iter().map(|&r| jac.get(r, c).powi(2)).sum::<f64>().sqrt();
        n * scales[c] / weights[c]
    };
    let full_rank = |sub: &[usize]| -> bool {
        let jg: Vec<Vec<f64>> = rows.iter().map(|&r| sub.iter().map(|&c| jac.get(r, c)).collect()).collect();
        rank(&jg) == m
    };
    // Whether a column moves any goal, pins included: the config-only set
    // and the full spread keep what can hold a pin in place.
    let moves_any = |c: usize| strength(jac, c, scales[c], weights[c]) > 0.0;
    let mut top: Vec<usize> = cols.iter().copied().filter(|&c| pull(c) > 0.0).collect();
    top.sort_by(|&a, &b| pull(b).total_cmp(&pull(a)).then(a.cmp(&b)));
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
    // 1. Tuples of size m (for up to three moving goals; beyond that the
    // full set).
    if (1..=3).contains(&m) {
        for sub in combinations(&top, m) {
            if full_rank(&sub) {
                push(sub, SubsetKind::Tuple, &mut out);
            }
        }
    }
    // 1b. With several goals, single literals too (§9 M5, revision 3; §9,
    // Canopy S6): one literal can move a point along a curve through both
    // targets (an hour hand's angle), which no linear prediction from the
    // baseline sees, but the verified solve can; and one literal that does
    // every moving goal while the pins hold is the answer a pin must not
    // hide.
    // At any goal count, at most [`SINGLES_FOR_SEVERAL_GOALS`] of them.
    if jac.goal_rows >= 2 {
        for &c in top.iter().take(SINGLES_FOR_SEVERAL_GOALS) {
            push(vec![c], SubsetKind::Tuple, &mut out);
        }
    }
    // 2. Config only: tuples of size m among the strongest config columns
    // (§9 M5, revision 3: a single knob before the spread over all of
    // them), then every config column together.
    let cfg: Vec<usize> = cols.iter().copied().filter(|&c| config[c] && moves_any(c)).collect();
    if (1..=3).contains(&m) && cfg.len() > m {
        let mut strong = cfg.clone();
        strong.retain(|&c| pull(c) > 0.0);
        strong.sort_by(|&a, &b| pull(b).total_cmp(&pull(a)).then(a.cmp(&b)));
        strong.truncate(CONFIG_TUPLE_COLUMNS);
        for sub in combinations(&strong, m) {
            if full_rank(&sub) {
                push(sub, SubsetKind::ConfigOnly, &mut out);
            }
        }
    }
    if !cfg.is_empty() {
        push(cfg, SubsetKind::ConfigOnly, &mut out);
    }
    // 3 and 4. The client's set, or everything.
    let all: Vec<usize> = cols.iter().copied().filter(|&c| moves_any(c)).collect();
    push(all, if client { SubsetKind::Client } else { SubsetKind::Full }, &mut out);
    out
}
