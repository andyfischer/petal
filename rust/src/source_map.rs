//! SourceMap - Maps terms to source locations for error reporting and live editing.
//!
//! See docs/Architecture.md for the surrounding compiler design.

use std::collections::HashMap;

use serde::Serialize;

use crate::ir_serialize::serialize_termid_map;
use crate::program::TermId;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SourcePosition {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SourceSpan {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

/// A zero-value span used as a placeholder when no source position is available.
pub const ZERO_SPAN: SourceSpan = SourceSpan {
    start: SourcePosition { line: 0, column: 0, offset: 0 },
    end: SourcePosition { line: 0, column: 0, offset: 0 },
};

impl Default for SourceSpan {
    fn default() -> Self {
        ZERO_SPAN
    }
}

#[derive(Serialize)]
pub struct SourceMap {
    #[serde(serialize_with = "serialize_termid_map")]
    term_spans: HashMap<TermId, SourceSpan>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self {
            term_spans: HashMap::new(),
        }
    }

    pub fn add(&mut self, term_id: TermId, span: SourceSpan) {
        self.term_spans.insert(term_id, span);
    }

    pub fn get(&self, term_id: TermId) -> Option<&SourceSpan> {
        self.term_spans.get(&term_id)
    }
}

impl Default for SourceMap {
    fn default() -> Self {
        Self::new()
    }
}
