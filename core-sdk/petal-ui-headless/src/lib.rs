//! petal-ui-headless — run a petal-ui script with no renderer attached.
//!
//! Everything here is built on `petal-ui`'s host contract and is tooling
//! rather than something a host links:
//!
//! - [`harness`]: [`harness::Headless`], a test driver over
//!   [`petal_ui::frame_core::FrameCore`] (the frame core Garden's panels run
//!   on) with a deterministic fixed-`dt` clock, so widget logic is testable
//!   with no renderer attached.
//! - [`scenario`]: declarative, replayable input scripts for that driver,
//!   hand-written or generated from a seed.
//! - [`drag_trace`]: reading a `ui/drag` drag in flight out of the state dump.
//! - [`run_cli`] and [`propose_edit_cli`]: the `petal-ui-run` trace driver and
//!   `petal-ui-propose-edit`, as library entry points, so a host can ship the
//!   same CLIs with its own natives registered (Garden's `garden-panel-run`
//!   and `garden-panel-propose-edit` do). The binaries of this crate are thin
//!   `main`s over them with no host natives at all.
//!
//! This crate names no host. A script written for a host with natives of its
//! own (a Garden panel that does `import garden`) runs here with that host's
//! stand-ins registered through a setup hook:
//! [`harness::Headless::from_file_with_setup`], or [`CliHost::setup`] for the
//! CLIs.

pub mod drag_trace;
pub mod harness;
pub mod propose_edit_cli;
pub mod run_cli;
pub mod scenario;

use petal::env::Env;

/// What a host adds to [`run_cli`] and [`propose_edit_cli`]: natives
/// registered before the script loads, and flags of its own. The default is
/// the generic CLI — no host natives, no extra flags.
pub trait CliHost {
    /// Register the host's natives and interface packages on `env`, after
    /// petal-ui's and before the script loads (loading links every native the
    /// script names).
    fn setup(&self, env: &mut Env) {
        let _ = env;
    }

    /// Handle a flag the generic CLI does not know. Return `Ok(true)` when
    /// `flag` is the host's; `value` reads the flag's argument.
    fn flag(
        &mut self,
        flag: &str,
        value: &mut dyn FnMut() -> Result<String, String>,
    ) -> Result<bool, String> {
        let _ = (flag, value);
        Ok(false)
    }

    /// Usage text for the flags [`flag`](Self::flag) takes, appended to the
    /// generic usage line (`[--query-fixtures q.json]`).
    fn usage(&self) -> &str {
        ""
    }
}

/// The generic host: petal-ui's natives and nothing else.
pub struct NoHost;

impl CliHost for NoHost {}
