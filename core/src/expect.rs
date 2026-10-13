//! Recorded expectations: what `expect`, `expect_eq` and `expect_near` leave
//! behind when a check does not hold.
//!
//! `assert` raises, which ends the run at the first failure. An expectation
//! records the failure — where it was written, what was checked, both values
//! — and the program carries on, so one run reports everything that is wrong
//! with it. The record lives on the [`ExecutionContext`] the call ran in
//! ([`ExpectLog`]); a host reads it with
//! [`Env::take_expect_failures`](crate::env::Env::take_expect_failures), and
//! the test runner ([`Env::test_begin`](crate::env::Env::test_begin)) hands
//! each test the failures recorded while it ran.
//!
//! [`ExecutionContext`]: crate::execution_context::ExecutionContext

use serde::Serialize;

/// The most failures an [`ExpectLog`] holds. An expectation at the top level
/// of a script a host runs every frame fails every frame; past this many the
/// log only counts ([`ExpectLog::failed`]).
pub const MAX_RECORDED_FAILURES: usize = 1000;

/// One expectation that did not hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpectFailure {
    /// The check that failed: `expect`, `expect_eq`, `expect_near`, or the
    /// name a host native gave ([`PetalCxt::fail_expectation`]).
    ///
    /// [`PetalCxt::fail_expectation`]: crate::native_fn::PetalCxt::fail_expectation
    pub check: String,
    /// The file the check is written in, as diagnostics name it (`sim.ptl`;
    /// `<entry>` for an entry with no path). Empty when the call had no
    /// source position (a program loaded from IR).
    pub file: String,
    /// 1-based line and column of the call; 0 when unknown.
    pub line: u32,
    pub column: u32,
    /// The whole failure in one line, without the position:
    /// `expected 3, got 4 (the score after one hit)`.
    pub message: String,
    /// The value the check was given, as `str` would not print it: a string
    /// is quoted, so `"1"` and `1` read differently. `None` for a plain
    /// `expect`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// The value it was held to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// `expect_near`'s tolerance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<String>,
    /// The message argument the call passed, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ExpectFailure {
    /// `file:line: message`, or just the message when the position is
    /// unknown.
    pub fn render(&self) -> String {
        if self.line == 0 {
            self.message.clone()
        } else if self.file.is_empty() {
            format!("line {}: {}", self.line, self.message)
        } else {
            format!("{}:{}: {}", self.file, self.line, self.message)
        }
    }
}

impl std::fmt::Display for ExpectFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

/// The expectations of one execution context: the failures recorded since
/// the host last took them, and the counts.
#[derive(Debug, Clone, Default)]
pub struct ExpectLog {
    /// The failures, oldest first, at most [`MAX_RECORDED_FAILURES`].
    pub failures: Vec<ExpectFailure>,
    /// How many expectations have been evaluated, held or not.
    pub checked: u64,
    /// How many did not hold. More than `failures.len()` once the log is
    /// full.
    pub failed: u64,
    /// Whether a native may ask the running call to suspend
    /// ([`PetalCxt::suspend`](crate::native_fn::PetalCxt::suspend)): true
    /// only while a test is being driven by
    /// [`Env::test_resume`](crate::env::Env::test_resume).
    pub(crate) suspendable: bool,
    /// Set by a native that asked (which also raises
    /// [`Heap::interrupt`](crate::heap::Heap::interrupt), so the VM stops
    /// after the instruction that called it); the driver then parks the
    /// test's frames.
    pub(crate) suspend_requested: bool,
}

impl ExpectLog {
    /// Note an expectation that held.
    #[inline]
    pub fn pass(&mut self) {
        self.checked += 1;
    }

    /// Record an expectation that did not hold.
    pub fn fail(&mut self, failure: ExpectFailure) {
        self.checked += 1;
        self.failed += 1;
        if self.failures.len() < MAX_RECORDED_FAILURES {
            self.failures.push(failure);
        }
    }

    /// Take the recorded failures, leaving the counts.
    pub fn take(&mut self) -> Vec<ExpectFailure> {
        std::mem::take(&mut self.failures)
    }
}

/// Where a native call is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallLocation {
    /// The file's display name (see [`ExpectFailure::file`]).
    pub file: String,
    /// 1-based.
    pub line: u32,
    pub column: u32,
}
