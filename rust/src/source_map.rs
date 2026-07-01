//! SourceMap - Maps terms to source locations for error reporting and live editing.
//!
//! See docs/Architecture.md for the surrounding compiler design.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ir_serialize::{deserialize_termid_map, serialize_termid_map};
use crate::program::TermId;

/// Identifies one source file within a program's file table
/// ([`SourceMap::files`]). File 0 is always the entry file; imported modules
/// get 1..N in load order. Serialized with a default so single-file Schema v0
/// IR (which has no `file` field on spans) still loads. See
/// docs/ir-as-target.md (schema v0.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct FileId(pub u16);

/// The entry file's id — the file a program was loaded from.
pub const ENTRY_FILE: FileId = FileId(0);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SourcePosition {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start: SourcePosition,
    pub end: SourcePosition,
    /// Which file (in the program's file table) line/column refer to.
    /// Each module is lexed independently, so positions are file-local.
    #[serde(default, skip_serializing_if = "is_entry_file")]
    pub file: FileId,
}

fn is_entry_file(f: &FileId) -> bool {
    *f == ENTRY_FILE
}

/// A zero-value span used as a placeholder when no source position is available.
pub const ZERO_SPAN: SourceSpan = SourceSpan {
    start: SourcePosition { line: 0, column: 0, offset: 0 },
    end: SourcePosition { line: 0, column: 0, offset: 0 },
    file: ENTRY_FILE,
};

impl Default for SourceSpan {
    fn default() -> Self {
        ZERO_SPAN
    }
}

/// One source file in a compiled program: the entry file (index 0) or an
/// imported module. `name` is the display name used in diagnostics
/// (module name + `.ptl` for resolved files, the module name for in-memory
/// registrations). `origin` is the filesystem path the source was read from,
/// when there is one — the basis of the module manifest hosts use to drive
/// hot-reload watching (see `Env::module_manifest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFile {
    pub name: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub struct SourceMap {
    #[serde(
        default,
        serialize_with = "serialize_termid_map",
        deserialize_with = "deserialize_termid_map"
    )]
    term_spans: HashMap<TermId, SourceSpan>,
    /// File table: entry file first, then imported modules in load order.
    /// Empty for single-file programs compiled through legacy paths and for
    /// pre-v0.1 IR; treat "missing" as "entry file only".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self {
            term_spans: HashMap::new(),
            files: Vec::new(),
        }
    }

    pub fn add(&mut self, term_id: TermId, span: SourceSpan) {
        self.term_spans.insert(term_id, span);
    }

    pub fn get(&self, term_id: TermId) -> Option<&SourceSpan> {
        self.term_spans.get(&term_id)
    }

    /// The file table entry for a file id, if the table has one.
    pub fn file(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    /// Diagnostic prefix for a span: `None` for entry-file spans (errors keep
    /// today's `[line N, column M]` format), the file's display name for
    /// module spans (`ui.ptl [line N, column M]`).
    pub fn file_name_for_span(&self, span: &SourceSpan) -> Option<&str> {
        if span.file == ENTRY_FILE {
            return None;
        }
        self.file(span.file).map(|f| f.name.as_str())
    }

    /// The source text a span's positions index into: the file-table entry
    /// when present, else `None` (callers fall back to `Program::source`).
    pub fn source_for_span(&self, span: &SourceSpan) -> Option<&str> {
        self.file(span.file).map(|f| f.source.as_str())
    }
}

impl Default for SourceMap {
    fn default() -> Self {
        Self::new()
    }
}
