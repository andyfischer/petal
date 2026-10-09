//! SourceMap - Maps terms to source locations for error reporting and live editing.
//!
//! See docs/Architecture.md for the surrounding compiler design.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ir_serialize::{deserialize_termid_map, serialize_termid_map};
use crate::program::TermId;

/// Identifies one source file within a program's file table
/// ([`SourceMap::files`]). File 0 is always the entry file; imported modules
/// get 1..N in load order. Serialized with a default so single-file spans
/// (which omit the file index) still load. See docs/dev/ir-as-target.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct FileId(pub u16);

/// The entry file's id — the file a program was loaded from.
pub const ENTRY_FILE: FileId = FileId(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourcePosition {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    pub start: SourcePosition,
    pub end: SourcePosition,
    /// Which file (in the program's file table) line/column refer to.
    /// Each module is lexed independently, so positions are file-local.
    pub file: FileId,
}

/// Compact wire encoding (schema v0.2, shared by the IR `source_map` and the
/// AST JSON): a lossless array
/// `[startLine, startCol, startOffset, endLine, endCol, endOffset]`, with a
/// seventh element — the file index — appended only when it is nonzero
/// (non-entry file). The verbose v0 object form
/// (`{"start": {line, column, offset}, "end": {...}, "file"?}`) is still
/// accepted on input; see `Deserialize` below.
impl Serialize for SourceSpan {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let n = if self.file == ENTRY_FILE { 6 } else { 7 };
        let mut seq = serializer.serialize_seq(Some(n))?;
        seq.serialize_element(&self.start.line)?;
        seq.serialize_element(&self.start.column)?;
        seq.serialize_element(&self.start.offset)?;
        seq.serialize_element(&self.end.line)?;
        seq.serialize_element(&self.end.column)?;
        seq.serialize_element(&self.end.offset)?;
        if self.file != ENTRY_FILE {
            seq.serialize_element(&self.file.0)?;
        }
        seq.end()
    }
}

/// Mirror of the pre-v0.2 object encoding, kept so legacy IR/AST JSON still
/// deserializes. Only used inside [`SourceSpanVisitor::visit_map`].
#[derive(Deserialize)]
struct VerboseSpan {
    start: SourcePosition,
    end: SourcePosition,
    #[serde(default)]
    file: FileId,
}

struct SourceSpanVisitor;

impl<'de> Visitor<'de> for SourceSpanVisitor {
    type Value = SourceSpan;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(
            "a span array [startLine, startCol, startOffset, endLine, endCol, endOffset, file?] \
             or a {start, end, file?} object",
        )
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<SourceSpan, A::Error> {
        let mut next = |what: &str| -> Result<u32, A::Error> {
            seq.next_element::<u32>()?
                .ok_or_else(|| de::Error::custom(format!("span array missing {what}")))
        };
        let span = SourceSpan {
            start: SourcePosition {
                line: next("start line")?,
                column: next("start column")?,
                offset: next("start offset")?,
            },
            end: SourcePosition {
                line: next("end line")?,
                column: next("end column")?,
                offset: next("end offset")?,
            },
            file: FileId(seq.next_element::<u16>()?.unwrap_or(0)),
        };
        // Reject trailing elements so a malformed span fails loudly.
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(de::Error::custom("span array has more than 7 elements"));
        }
        Ok(span)
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<SourceSpan, A::Error> {
        let v = VerboseSpan::deserialize(de::value::MapAccessDeserializer::new(map))?;
        Ok(SourceSpan {
            start: v.start,
            end: v.end,
            file: v.file,
        })
    }
}

impl<'de> Deserialize<'de> for SourceSpan {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<SourceSpan, D::Error> {
        deserializer.deserialize_any(SourceSpanVisitor)
    }
}

/// A zero-value span used as a placeholder when no source position is available.
pub const ZERO_SPAN: SourceSpan = SourceSpan {
    start: SourcePosition {
        line: 0,
        column: 0,
        offset: 0,
    },
    end: SourcePosition {
        line: 0,
        column: 0,
        offset: 0,
    },
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

    /// True when the map carries nothing worth serializing — no term spans and
    /// no file table. Lets the Program serializer omit `source_map` entirely
    /// for span-less imports (schema v0.2 omit-defaults rule).
    pub fn is_empty(&self) -> bool {
        self.term_spans.is_empty() && self.files.is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn span(file: u16) -> SourceSpan {
        SourceSpan {
            start: SourcePosition {
                line: 1,
                column: 2,
                offset: 3,
            },
            end: SourcePosition {
                line: 4,
                column: 5,
                offset: 6,
            },
            file: FileId(file),
        }
    }

    #[test]
    fn span_serializes_compact() {
        assert_eq!(
            serde_json::to_string(&span(0)).unwrap(),
            "[1,2,3,4,5,6]",
            "entry-file spans omit the file index"
        );
        assert_eq!(serde_json::to_string(&span(2)).unwrap(), "[1,2,3,4,5,6,2]");
    }

    #[test]
    fn span_deserializes_both_forms() {
        // Compact array, with and without the file index.
        assert_eq!(
            serde_json::from_str::<SourceSpan>("[1,2,3,4,5,6]").unwrap(),
            span(0)
        );
        assert_eq!(
            serde_json::from_str::<SourceSpan>("[1,2,3,4,5,6,2]").unwrap(),
            span(2)
        );
        // Legacy v0 object form.
        let verbose = r#"{"start":{"line":1,"column":2,"offset":3},
                          "end":{"line":4,"column":5,"offset":6}}"#;
        assert_eq!(
            serde_json::from_str::<SourceSpan>(verbose).unwrap(),
            span(0)
        );
        let verbose_file = r#"{"start":{"line":1,"column":2,"offset":3},
                               "end":{"line":4,"column":5,"offset":6},"file":2}"#;
        assert_eq!(
            serde_json::from_str::<SourceSpan>(verbose_file).unwrap(),
            span(2)
        );
    }

    #[test]
    fn span_rejects_short_and_long_arrays() {
        assert!(serde_json::from_str::<SourceSpan>("[1,2,3]").is_err());
        assert!(serde_json::from_str::<SourceSpan>("[1,2,3,4,5,6,7,8]").is_err());
    }
}
