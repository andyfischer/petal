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
//!
//! v1 (`crate::direct_manipulation`) is unchanged, except that its provenance
//! looks through `tunable(x)`.

pub mod hints;
pub mod jacobian;
pub mod observe;
pub mod params;
pub mod spec;

pub use hints::{Hint, HintKind, Hints, read_hints};
pub use jacobian::{ColumnQuality, FdMode, FdOptions, Jacobian, Side, collateral_rows, jacobian};
pub use params::{
    Anchor, CodeOrigin, Discovered, Eligibility, EligibilityContext, Param, ParamFilter,
    ParamSelector, RunBudget, ScreenResult, anchors_for, discover_params, eligibility,
    screen_params,
};

pub use observe::{EmitKey, LiveObservation, ObsValue, Observable, PathStep};
pub use spec::{
    CaptureSpec, NoHost, OverrideStrategy, OverrideValue, Overrides, RngMode, RunError, SpecConfig,
    SpecError, SpecHost, SpecRun, SpecSession, StateMode,
};

/// An error from the direct-manipulation v2 pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum DmError {
    /// The speculative runner could not open a session.
    Spec(spec::SpecError),
}

impl std::fmt::Display for DmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DmError::Spec(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DmError {}

impl From<spec::SpecError> for DmError {
    fn from(e: spec::SpecError) -> Self {
        DmError::Spec(e)
    }
}
