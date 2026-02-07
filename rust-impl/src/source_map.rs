//! SourceMap - Maps terms to source locations for error reporting and live editing.
//!
//! See docs/tech_outline/data_structures/SourceMap.md

use crate::program::TermId;

#[derive(Debug, Clone)]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

pub struct SourceMap {
    entries: Vec<(TermId, SourceRange)>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, term_id: TermId, range: SourceRange) {
        self.entries.push((term_id, range));
    }

    pub fn get(&self, term_id: TermId) -> Option<&SourceRange> {
        self.entries
            .iter()
            .find(|(id, _)| *id == term_id)
            .map(|(_, range)| range)
    }
}

impl Default for SourceMap {
    fn default() -> Self {
        Self::new()
    }
}
