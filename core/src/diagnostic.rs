//! A non-fatal compile-time diagnostic: a type-checker or compiler finding,
//! surfaced alongside the compiled program without ever aborting compilation.
//!
//! # Message house style
//!
//! The two layers that talk to a user quote differently, and the quoting is
//! itself information — it tells the reader which phase spoke:
//!
//! - **Compile time** (`typecheck/`, the newer `compiler/` checks): backticks
//!   around every piece of source text, and the callee named bare —
//!   ``argument 1 to `double` ``, `` `f` expects 2 arguments, got 1 ``.
//! - **Runtime** (`backend/`): `'single quotes'`, and the callee named with
//!   parens the way a stack frame names it — `No field 'x' on class Rect`,
//!   `add() expects 2 arguments, got 3`.
//!
//! The one deliberate exception is the named-argument pair
//! (`has no parameter named`, `got multiple values for parameter`), which the
//! checker and the VM both report on the same call. There the checker adopts
//! the *runtime* style so the two lines read as one diagnosis followed by its
//! failure rather than the same complaint in two dialects; the checker's line
//! carries the extra detail. See `typecheck::Checker::check_named_args`.
use crate::source_map::SourceSpan;

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub span: SourceSpan,
    pub message: String,
    pub severity: Severity,
}

/// How sure the checker is that a diagnostic describes a failure.
///
/// Neither severity stops compilation: the program still compiles and runs, and
/// the line may never execute. The difference is what `petal check` does with
/// it. A [`Severity::Warning`] is advice (a type mismatch against an advisory
/// annotation, a discarded pure call). A [`Severity::Error`] is a line that
/// cannot succeed when it runs (a call to a name nothing defines, a call no
/// overload accepts), so `check` prints it as `error:` and exits non-zero
/// without needing `--strict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    #[default]
    Warning,
    Error,
}

impl Severity {
    /// The label a rendered diagnostic starts with: `warning` or `error`.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

impl Diagnostic {
    /// A warning: advice that does not by itself mean the line will fail.
    pub fn new(span: SourceSpan, message: String) -> Diagnostic {
        Diagnostic {
            span,
            message,
            severity: Severity::Warning,
        }
    }

    /// An error: the line fails whenever it runs. See [`Severity::Error`].
    pub fn error(span: SourceSpan, message: String) -> Diagnostic {
        Diagnostic {
            span,
            message,
            severity: Severity::Error,
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}
