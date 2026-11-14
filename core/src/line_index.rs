//! Line lookups over source text, by char offset.

use crate::source_map::SourcePosition;

/// Where each line of a text starts, for offset → line lookups.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Char offset at which each line starts; the first is always 0.
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        Self::from_iter(text.chars())
    }

    pub fn from_chars(chars: &[char]) -> Self {
        Self::from_iter(chars.iter().copied())
    }

    fn from_iter(chars: impl Iterator<Item = char>) -> Self {
        let mut starts = vec![0];
        for (i, c) in chars.enumerate() {
            if c == '\n' {
                starts.push(i + 1);
            }
        }
        LineIndex { starts }
    }

    /// The 0-based line `offset` is on.
    pub fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset) - 1
    }

    /// The char offset at which 0-based `line` starts.
    pub fn line_start(&self, line: usize) -> usize {
        self.starts[line]
    }

    /// How many lines the text has (a trailing newline starts an empty one).
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The 1-based line and column of `offset`.
    pub fn position(&self, offset: u32) -> SourcePosition {
        let line = self.line_of(offset as usize);
        SourcePosition {
            line: line as u32 + 1,
            column: offset - self.starts[line] as u32 + 1,
            offset,
        }
    }
}

/// The offset of the start of the line `at` is on.
pub fn line_start(chars: &[char], at: usize) -> usize {
    chars[..at.min(chars.len())]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |i| i + 1)
}

/// The spaces and tabs the line `at` is on starts with.
pub fn line_indent(chars: &[char], at: usize) -> String {
    chars[line_start(chars, at)..]
        .iter()
        .take_while(|&&c| c == ' ' || c == '\t')
        .collect()
}
