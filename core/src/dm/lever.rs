//! Levers: the numbers a solve may move.
//!
//! The solver ([`super::solve`]) and the finite-difference Jacobian
//! ([`super::jacobian`]) never needed to know *what* a parameter is, only its
//! value, whether it must stay whole, where it may go, and — to run the
//! program with it moved — how to override it. [`Lever`] is that much.
//!
//! Two kinds exist (docs/proposals/declarative-dragging.md, §P2):
//!
//! - [`LiteralLever`], DM v2's [`Param`]: a numeric literal in the source,
//!   overridden by its `Constant` term in a speculative program run
//!   ([`TermLever`]). Its behaviour is exactly the solver's before the
//!   extraction; the DM goldens pin that.
//! - [`PathLever`]: a number at a path inside a model value (`ui/drag`'s
//!   `vary`). It is run by the Petal side — `speculate(view, set_in(model,
//!   path, v))` — because a native cannot call a Petal closure, so the
//!   per-frame loop lives in Petal and hands this crate numbers:
//!   [`lever_step`] turns values, residuals and sensitivities into the next
//!   point, projected onto each lever's [`Bound`].

use crate::dm::params::Param;
use crate::dm::solve::{Bound, GoalRow, goal_step};
use crate::dm::spec::OverrideValue;
use crate::program::TermId;

/// A number a solve may move.
pub trait Lever {
    /// The value the solve starts from (a literal's value as written).
    fn value(&self) -> f64;
    /// Whether the lever holds an int.
    fn is_int(&self) -> bool;
    /// Where the lever may go.
    fn bound(&self, allow_respell: bool) -> Bound;
    /// The override that makes the lever read `v`: an int lever takes an
    /// int when `v` is whole.
    fn override_for(&self, v: f64) -> OverrideValue {
        if self.is_int() && v.fract() == 0.0 && v.abs() < 9.0e15 {
            OverrideValue::Int(v as i64)
        } else {
            OverrideValue::Num(v)
        }
    }
    /// The override that makes the lever read the int `n`.
    fn override_int(&self, n: i64) -> OverrideValue {
        OverrideValue::Int(n)
    }
}

/// A lever that a speculative program run ([`super::spec::SpecSession::run`])
/// sets by overriding one term.
pub trait TermLever: Lever {
    fn term(&self) -> TermId;
}

/// DM v2's lever: a numeric literal of the source.
pub type LiteralLever = Param;

impl Lever for Param {
    fn value(&self) -> f64 {
        self.value
    }
    fn is_int(&self) -> bool {
        self.is_int
    }
    fn bound(&self, allow_respell: bool) -> Bound {
        Bound::of(self, allow_respell)
    }
    fn override_for(&self, v: f64) -> OverrideValue {
        Param::override_for(self, v)
    }
    fn override_int(&self, n: i64) -> OverrideValue {
        Param::override_int(self, n)
    }
}

impl TermLever for Param {
    fn term(&self) -> TermId {
        self.term
    }
}

/// A number at a path inside a model value. The path itself is the Petal
/// side's business (it builds the candidate with `set_in`); this side keeps
/// the value and the bound.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PathLever {
    pub value: f64,
    pub bound: Bound,
}

impl Lever for PathLever {
    fn value(&self) -> f64 {
        self.value
    }
    fn is_int(&self) -> bool {
        self.bound.int
    }
    fn bound(&self, _allow_respell: bool) -> Bound {
        self.bound
    }
}

/// One warm-started Gauss-Newton step for `levers` (their current values),
/// with goal rows `goals`, the goals' current values `rows`, the goal block
/// of the Jacobian `jg` (rows × levers) and per-lever cost weights `cost`:
/// [`goal_step`], scaled by `scale` (a backtrack shortens it), then each
/// lever projected onto its bound. Returns the next values.
pub fn lever_step<L: Lever>(
    levers: &[L],
    goals: &[GoalRow],
    rows: &[Option<f64>],
    jg: &[Vec<f64>],
    cost: &[f64],
    scale: f64,
) -> Vec<f64> {
    let delta = goal_step(goals, rows, jg, &[], cost);
    levers
        .iter()
        .zip(&delta)
        .map(|(l, d)| l.bound(false).project(l.value() + scale * d))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_lever_step_meets_one_goal_and_projects() {
        // y = 2·a: aim y at 10 from a = 1 (y = 2).
        let l = [PathLever { value: 1.0, bound: Bound::default() }];
        let g = [GoalRow::eq(10.0, 0.5, 1.0)];
        let next = lever_step(&l, &g, &[Some(2.0)], &[vec![2.0]], &[1.0], 1.0);
        assert!((next[0] - 5.0).abs() < 1e-9, "{next:?}");
        // Bounded to [0, 3]: projected.
        let l = [PathLever { value: 1.0, bound: Bound { min: Some(0.0), max: Some(3.0), step: None, int: false } }];
        assert_eq!(lever_step(&l, &g, &[Some(2.0)], &[vec![2.0]], &[1.0], 1.0), vec![3.0]);
        // Half a step.
        let l = [PathLever { value: 1.0, bound: Bound::default() }];
        assert!((lever_step(&l, &g, &[Some(2.0)], &[vec![2.0]], &[1.0], 0.5)[0] - 3.0).abs() < 1e-9);
    }

    #[test]
    fn two_goals_one_lever_is_least_squares() {
        // x = cos-like 1·a, y = 1·a; aim (4, 2) from a = 0: least squares a = 3.
        let l = [PathLever::default()];
        let g = [GoalRow::eq(4.0, 0.5, 1.0), GoalRow::eq(2.0, 0.5, 1.0)];
        let next = lever_step(&l, &g, &[Some(0.0), Some(0.0)], &[vec![1.0], vec![1.0]], &[1.0], 1.0);
        assert!((next[0] - 3.0).abs() < 1e-6, "{next:?}");
    }
}
