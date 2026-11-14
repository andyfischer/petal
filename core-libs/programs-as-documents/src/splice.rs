//! Editing one literal where it is written, whatever surrounds it.
//!
//! Petal's path edits ([`petal::literal_edit`]) address a value from a
//! top-level binding. A literal the output map points at can be anywhere: an
//! argument, a helper's body, an element nested in a list. [`edit_span`]
//! lifts the literal's text into a scratch source at its own indentation,
//! applies the path edit there, and splices the result back, so only the
//! literal's text changes and the rest of the file stays byte for byte.

use petal::literal_edit::{remove_path, set_path};
use petal::source_map::SourceSpan;
use petal::static_value::StaticValue;

/// The binding a literal is lifted into.
const SCRATCH: &str = "__programs_as_documents_literal";

/// A change to a literal, at a path inside it (`""` for the literal itself,
/// `.x` for an attribute or field, `.style.fill`, `[2]`).
#[derive(Debug, Clone, Copy)]
pub enum LiteralEdit<'a> {
    Set { path: &'a str, value: &'a StaticValue },
    Remove { path: &'a str },
}

/// The byte offset of character offset `c` (petal's spans count characters).
pub fn byte_at(source: &str, c: u32) -> Option<usize> {
    let c = c as usize;
    if source.is_ascii() {
        return (c <= source.len()).then_some(c);
    }
    source.char_indices().map(|(b, _)| b).chain([source.len()]).nth(c)
}

/// The byte range `span` covers in `source`.
pub fn byte_range(source: &str, span: &SourceSpan) -> Option<std::ops::Range<usize>> {
    let (a, b) = (byte_at(source, span.start.offset)?, byte_at(source, span.end.offset)?);
    (a <= b).then_some(a..b)
}

/// Apply `edit` to the literal written at `span` of `source` (the span's file
/// must be `source`), changing only that literal's text.
pub fn edit_span(source: &str, span: &SourceSpan, edit: LiteralEdit<'_>) -> Result<String, String> {
    lifted(source, span, |scratch, root| {
        match edit {
            LiteralEdit::Set { path, value } => set_path(scratch, &format!("{root}{path}"), value),
            LiteralEdit::Remove { path } => remove_path(scratch, &format!("{root}{path}")),
        }
        .map_err(|e| e.message)
    })
}

/// Edit the literal written at `span` of `source` as the value of a scratch
/// binding, and splice the edited text back, so only that literal's text
/// changes.
///
/// `edit` gets the scratch source and the path text of the literal in it
/// (`__programs_as_documents_literal[0]`): it applies any path edits of
/// [`petal::literal_edit`] under that root and returns the edited scratch.
/// The literal is lifted at its own column, so the lines of a multi-line
/// literal keep their indentation.
pub fn lifted(
    source: &str,
    span: &SourceSpan,
    edit: impl FnOnce(&str, &str) -> Result<String, String>,
) -> Result<String, String> {
    let range = byte_range(source, span).ok_or("the literal's position is outside the source")?;
    let text = &source[range.clone()];
    let indent = column_indent(source, range.start);
    let head = format!("let {SCRATCH} = [\n{indent}");
    let tail = ",\n]\n";
    let scratch = format!("{head}{text}{tail}");
    let edited = edit(&scratch, &format!("{SCRATCH}[0]"))?;
    let new_text = edited
        .strip_prefix(&head)
        .and_then(|t| t.strip_suffix(tail))
        .ok_or("the edit changed more than the literal")?;
    let mut out = String::with_capacity(source.len() + new_text.len());
    out.push_str(&source[..range.start]);
    out.push_str(new_text);
    out.push_str(&source[range.end..]);
    Ok(out)
}

/// The text that puts a literal starting at byte `at` at its column: the
/// line's own indentation when only whitespace comes before it on its line,
/// else as many spaces as there are characters before it.
pub fn column_indent(source: &str, at: usize) -> String {
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let before = &source[line_start..at];
    if before.chars().all(char::is_whitespace) { before.to_string() } else { " ".repeat(before.chars().count()) }
}

/// The leading whitespace of the line byte `at` is on.
pub fn line_indent(source: &str, at: usize) -> String {
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    source[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// `text` (a value's source, whose first line started at a line indented by
/// `indent`) with its later lines made relative to its first: `indent`
/// stripped from each.
pub fn dedent(text: &str, indent: &str) -> String {
    let mut lines = text.split('\n');
    let mut out = lines.next().unwrap_or("").to_string();
    for line in lines {
        out.push('\n');
        out.push_str(line.strip_prefix(indent).unwrap_or(line.trim_start()));
    }
    out
}

/// [`dedent`]ed `text` placed at a line indented by `indent`: each later line
/// gets `indent` in front (blank lines stay blank).
pub fn reindent(text: &str, indent: &str) -> String {
    let mut lines = text.split('\n');
    let mut out = lines.next().unwrap_or("").to_string();
    for line in lines {
        out.push('\n');
        if !line.trim().is_empty() {
            out.push_str(indent);
        }
        out.push_str(line);
    }
    out
}
