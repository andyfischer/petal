//! A non-fatal compile-time diagnostic (currently only type-checker warnings).
//! Surfaced alongside the compiled program without ever aborting compilation.
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
}
