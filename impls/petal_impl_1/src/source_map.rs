//! Source map for mapping terms to source locations

use std::collections::HashMap;

use crate::program::TermId;

/// A position in source code
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourcePosition {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}

/// A span in source code
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

impl SourceSpan {
    pub fn new(start: SourcePosition, end: SourcePosition) -> Self {
        Self { start, end }
    }

    pub fn from_offsets(start_offset: u32, end_offset: u32, source: &str) -> Self {
        Self {
            start: position_from_offset(start_offset, source),
            end: position_from_offset(end_offset, source),
        }
    }
}

fn position_from_offset(offset: u32, source: &str) -> SourcePosition {
    let mut line = 1;
    let mut column = 1;
    for (i, c) in source.chars().enumerate() {
        if i as u32 >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    SourcePosition { line, column, offset }
}

/// Maps terms to source locations
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    term_spans: HashMap<TermId, SourceSpan>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, term_id: TermId, span: SourceSpan) {
        self.term_spans.insert(term_id, span);
    }

    pub fn get(&self, term_id: TermId) -> Option<SourceSpan> {
        self.term_spans.get(&term_id).copied()
    }
}
