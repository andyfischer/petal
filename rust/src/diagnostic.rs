//! A non-fatal compile-time diagnostic (currently only type-checker warnings).
//! Surfaced alongside the compiled program without ever aborting compilation.
use crate::source_map::SourceSpan;

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub span: SourceSpan,
    pub message: String,
}
