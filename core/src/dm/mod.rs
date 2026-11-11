//! Direct manipulation v2: backpropagating a desired change of an output
//! (a dragged shape, an emitted value) into the source literals that produce
//! it, through function calls, by re-running the program with literals
//! nudged and measuring how the output moves.
//!
//! The plan is docs/dev/direct-manipulation-v2-plan.md. This module grows by
//! milestone:
//!
//! - [`spec`] (M1): the speculative runner. Runs a program again with literal
//!   overrides, with no side effects and frozen inputs, and records what it
//!   emitted.
//! - [`observe`] (M1): what a run records, independent of any heap, and
//!   [`Observable`](observe::Observable), the address of one number in it.
//! - [`hints`] (M2): the `tunable` / `rigid` declarations, read from the IR.
//! - [`params`] (M2): which literals may move a goal (static discovery,
//!   eligibility, empirical screening).
//! - [`jacobian`] (M2): finite-difference sensitivities, with each column
//!   classified (smooth, kink, branch flip, step, discrete).
//! - [`lever`]: what a solve moves — the [`Lever`](lever::Lever) trait, with
//!   the source literal (`Param`) and `ui/drag`'s path lever.
//! - [`solve`] (M3): the constrained Gauss-Newton step, the verification
//!   loop and rounding to nice literals.
//! - [`rank`] (M3): parameter weights, candidate subsets, measured
//!   collateral and the score.
//! - [`api`] (M3): [`propose_edits_v2`], [`DmSession`] and
//!   [`apply_edits_multi`].
//!
//! v1 (`crate::direct_manipulation`) is unchanged, except that its provenance
//! looks through `tunable(x)`. v2 falls back to it for non-numeric goals.

pub mod api;
pub mod hints;
pub mod jacobian;
pub mod lever;
pub mod observe;
pub mod params;
pub mod rank;
pub mod solve;
pub mod spec;

pub use api::{
    Alternative, CandidateReport, Collateral, CollateralItem, DmDiagnostics, DmGoal, DmRequest, DmResponse,
    DmSession, Engine, ExplainLog, FileEdit, GoalTarget, ParamChange, Policy, Prefer, Rejection, TriedLiteral,
    apply_edits_multi, program_files,
    propose_edits_v2,
};
pub use rank::SubsetKind;
pub use lever::{Lever, LiteralLever, PathLever, TermLever, lever_step};

pub use hints::{Hint, HintKind, Hints, read_hints};
pub use jacobian::{ColumnQuality, FdMode, FdOptions, Jacobian, Side, collateral_rows, jacobian};
pub use params::{
    Anchor, BudgetLimit, CodeOrigin, Discovered, Eligibility, EligibilityContext, Param, ParamFilter,
    ParamSelector, RunBudget, ScreenResult, anchors_for, discover_params, eligibility,
    screen_order, screen_params, screen_params_for,
};

pub use observe::{EmitKey, LiveObservation, ObsValue, Observable, PathStep, path_text};
pub use spec::{
    CaptureSpec, NoHost, OverrideStrategy, OverrideValue, Overrides, RngMode, RunError, SpecConfig,
    SpecError, SpecHost, SpecRun, SpecSession, StateMode,
};

/// An error from the direct-manipulation v2 pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum DmError {
    /// The speculative runner could not open a session.
    Spec(spec::SpecError),
    /// The request has no goals.
    NoGoals,
    /// A goal does not address a number of the baseline run.
    BadObservable(String),
    /// No literal may move the goals; `why` says what the candidates were.
    NothingToMove { why: String },
    /// Edits could not be applied.
    Edit(String),
    /// Under [`Policy::BestEffort`], no verified edit reaches the goals;
    /// `why` says how close the best try came, and what rejected it.
    Unreachable { why: String },
}

impl std::fmt::Display for DmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DmError::Spec(e) => write!(f, "{e}"),
            DmError::NoGoals => write!(f, "the request has no goals"),
            DmError::BadObservable(e) => write!(f, "{e}"),
            DmError::NothingToMove { why } => write!(f, "nothing can move the goals: {why}"),
            DmError::Edit(e) => write!(f, "{e}"),
            DmError::Unreachable { why } => write!(f, "no edit reaches the goals: {why}"),
        }
    }
}

impl std::error::Error for DmError {}

impl From<spec::SpecError> for DmError {
    fn from(e: spec::SpecError) -> Self {
        DmError::Spec(e)
    }
}
