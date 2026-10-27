//! The request: goals on observed outputs and what may move to meet them
//! (§6.3).

use std::time::Duration;

use crate::dm_v2::jacobian::FdOptions;
use crate::dm_v2::observe::{LiveObservation, Observable};
use crate::dm_v2::params::ParamFilter;
use crate::dm_v2::spec::SpecConfig;
use crate::static_value::StaticValue;

/// What a goal wants its observable to be.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalTarget {
    Num(f64),
    /// A non-numeric value (a string, a bool): no derivative, so the request
    /// is answered by v1.
    Value(StaticValue),
    /// Anywhere in `[lo, hi]` (an end left out is open): `--at-most`,
    /// `--at-least`, `--within`. It holds anywhere inside, so one that holds
    /// at the baseline asks for no edit, and one that does not is solved to
    /// one tolerance inside the bound it is past ([`GoalRow`](crate::dm_v2::solve::GoalRow)). A drag's
    /// `targets` entry for it is ignored.
    Range { lo: Option<f64>, hi: Option<f64> },
}

impl GoalTarget {
    /// The range's ends, open ends infinite; `None` for a point target.
    pub fn bounds(&self) -> Option<(f64, f64)> {
        match self {
            GoalTarget::Range { lo, hi } => Some((lo.unwrap_or(f64::NEG_INFINITY), hi.unwrap_or(f64::INFINITY))),
            _ => None,
        }
    }

    /// `≤ 3`, `≥ 1`, `in 0..1280`.
    pub fn range_text(&self) -> Option<String> {
        match self {
            GoalTarget::Range { lo: None, hi: Some(h) } => Some(format!("≤ {}", short(*h))),
            GoalTarget::Range { lo: Some(l), hi: None } => Some(format!("≥ {}", short(*l))),
            GoalTarget::Range { lo: Some(l), hi: Some(h) } => Some(format!("in {}..{}", short(*l), short(*h))),
            _ => None,
        }
    }
}

/// `v` for a sentence: at most 4 decimals, trailing zeros dropped.
pub(crate) fn short(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
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

    /// The goal "`observable` at most `hi`".
    pub fn at_most(observable: Observable, hi: f64) -> DmGoal {
        DmGoal { observable, target: GoalTarget::Range { lo: None, hi: Some(hi) }, tolerance: None, weight: 1.0 }
    }

    /// The goal "`observable` at least `lo`".
    pub fn at_least(observable: Observable, lo: f64) -> DmGoal {
        DmGoal { observable, target: GoalTarget::Range { lo: Some(lo), hi: None }, tolerance: None, weight: 1.0 }
    }

    /// The goal "`observable` between `lo` and `hi`" (inclusive).
    pub fn between(observable: Observable, lo: f64, hi: f64) -> DmGoal {
        DmGoal { observable, target: GoalTarget::Range { lo: Some(lo), hi: Some(hi) }, tolerance: None, weight: 1.0 }
    }

    /// The number a default tolerance scales with (§2.4.3): the target, or
    /// the larger end of a range; `t` is the drag's target for a point goal.
    pub(super) fn tolerance_scale(&self, t: f64) -> f64 {
        match &self.target {
            GoalTarget::Range { lo, hi } => {
                lo.iter().chain(hi.iter()).fold(0.0f64, |m, x| m.max(x.abs()))
            }
            GoalTarget::Value(_) => 0.0,
            GoalTarget::Num(_) => t,
        }
    }

    /// The same goal with tolerance `tol`.
    pub fn within(mut self, tol: f64) -> DmGoal {
        self.tolerance = Some(tol);
        self
    }
}

/// How strongly knobs (`config let` and `tunable`, one tier) are preferred
/// (§2.5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Prefer {
    /// Knobs cost less, and alternatives made only of them are listed first
    /// when they verify; the others are still listed.
    #[default]
    Soft,
    /// v1's rule: once the program declares a `config let` (or a `tunable`
    /// that marks a literal), every other named binding is pinned, and when a
    /// knob can move the goal, only knobs are used.
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
    /// Per goal: it holds its value (`--by 0`, a pinned shape) rather than
    /// asking for a change. Screening resolves what moves the other goals
    /// first (§2.2 stage 5). Empty (or short): a goal holds when its numeric
    /// target is its baseline value within its tolerance.
    pub holds: Vec<bool>,
    /// The client's collateral weight per output channel (§9, Canopy S3;
    /// `--collateral solid=10,coin=1`); a channel not named weighs 1. A
    /// shape on a channel of weight `W > 1` that moves as far as the drag
    /// adds `W - 1` to the score on top of its share of the compressed count
    /// ([`crate::dm_v2::rank::WeightedCollateral`]), and the solver's step
    /// scales that channel's rows by `W`; a weight below 1 discounts the
    /// channel, 0 ignores it.
    pub collateral_weights: Vec<(String, f64)>,
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
            holds: Vec::new(),
            collateral_weights: Vec::new(),
        }
    }
}

impl DmRequest {
    /// A request for `goals` with the defaults.
    pub fn new(goals: Vec<DmGoal>) -> DmRequest {
        DmRequest { goals, ..Default::default() }
    }

    /// The finite-difference settings this request asks for.
    pub(super) fn fd(&self) -> FdOptions {
        FdOptions { allow_respell: self.allow_respell, ..FdOptions::default() }
    }
}
