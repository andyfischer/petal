//! Shared runtime-error annotation used by both execution backends.
//!
//! When a term/instruction fails at runtime, both engines dress the raw message
//! with the failing term's source position, a caret snippet, dataflow
//! provenance, and a call stack trace — so the graph `Evaluator` and the
//! bytecode `Vm` produce identical error text. The per-engine part is only
//! *building* the [`TraceFrame`] list (from graph `Frame`s vs `VmFrame`s); the
//! formatting lives here.

use crate::program::{Program, TermId};
use crate::source_map::SourceSpan;

/// One call frame's contribution to a stack trace: the function name and the
/// call-site term in the caller (used to locate the call's source position).
pub struct TraceFrame {
    pub name: Option<String>,
    pub call_site: Option<TermId>,
}

/// Annotate a raw runtime error `msg` (raised while executing `failing`) with
/// source position, a caret snippet, provenance, and a stack trace built from
/// `frames` (in stack order, bottom to top — walked top-first here).
pub fn annotate_error(
    program: &Program,
    failing: TermId,
    msg: String,
    frames: &[TraceFrame],
) -> String {
    let span = program.source_map.get(failing);
    let mut error_msg = match span {
        Some(s) if s.start.line > 0 => {
            format!("{} [line {}, column {}]", msg, s.start.line, s.start.column)
        }
        _ => msg,
    };

    // A 3-line snippet with a caret, when we have both a span and the source.
    if let Some(s) = span
        && !program.source.is_empty()
        && let Some(snippet) = format_source_snippet(&program.source, s)
    {
        error_msg.push('\n');
        error_msg.push_str(&snippet);
    }

    let provenance = format_provenance(program, failing, 5);
    if !provenance.is_empty() {
        error_msg.push_str("\nCaused by:");
        for entry in &provenance {
            error_msg.push_str(&format!("\n  {}", entry));
        }
    }

    let trace = build_stack_trace(program, frames);
    if !trace.is_empty() {
        error_msg.push_str("\nStack trace:");
        for entry in &trace {
            error_msg.push_str(&format!("\n  {}", entry));
        }
    }

    error_msg
}

/// Up to `max` nearest named ancestors of `failing` that carry a source span —
/// the user-visible variables that fed the failure.
fn format_provenance(program: &Program, failing: TermId, max: usize) -> Vec<String> {
    let (ancestors, _edges) = program.trace_provenance(failing);
    let mut out = Vec::new();
    for aid in ancestors {
        if out.len() >= max {
            break;
        }
        let term = program.get_term(aid);
        let Some(name) = term.name.as_deref() else {
            continue;
        };
        let Some(span) = program.source_map.get(aid) else {
            continue;
        };
        if span.start.line == 0 {
            continue;
        }
        out.push(format!(
            "{} [line {}, column {}]",
            name, span.start.line, span.start.column
        ));
    }
    out
}

/// Build a stack trace from call `frames` (bottom-to-top), walked top-first.
/// Only frames with a function name (i.e. function-call frames) are included.
fn build_stack_trace(program: &Program, frames: &[TraceFrame]) -> Vec<String> {
    let mut trace = Vec::new();
    for frame in frames.iter().rev() {
        let Some(ref name) = frame.name else {
            continue;
        };
        let call_site = frame
            .call_site
            .and_then(|tid| program.source_map.get(tid))
            .filter(|span| span.start.line > 0);
        match call_site {
            Some(span) => trace.push(format!(
                "in {}() [line {}, column {}]",
                name, span.start.line, span.start.column
            )),
            None => trace.push(format!("in {}()", name)),
        }
    }
    trace
}

/// Render a 3-line source snippet for a given span: the source line it points
/// at, prefixed with a gutter, followed by a caret line marking the column.
/// Returns `None` if the span is a placeholder or the line is out of range.
/// ASCII-only and zero-dependency; callers append it to error messages.
pub fn format_source_snippet(source: &str, span: &SourceSpan) -> Option<String> {
    if span.start.line == 0 || source.is_empty() {
        return None;
    }
    let line_num = span.start.line as usize;
    let col = span.start.column.max(1) as usize;
    let line = source.lines().nth(line_num - 1)?;
    // Right-align the gutter width on the line number.
    let gutter_width = line_num.to_string().len().max(1);
    let blank_gutter = " ".repeat(gutter_width);
    // Build the caret offset: 1-based column → col-1 spaces before the caret,
    // preserving tab stops in the original line so the caret lines up visually.
    let mut caret_pad = String::new();
    for (i, ch) in line.chars().enumerate() {
        if i + 1 >= col {
            break;
        }
        caret_pad.push(if ch == '\t' { '\t' } else { ' ' });
    }
    // Clamp span length to what fits on this line for a multi-char underline.
    let span_len: usize =
        if span.end.line == span.start.line && span.end.column > span.start.column {
            (span.end.column - span.start.column) as usize
        } else {
            1
        };
    let underline: String = std::iter::repeat_n('^', span_len.max(1)).collect();
    Some(format!(
        "{} |\n{} | {}\n{} | {}{}",
        blank_gutter, line_num, line, blank_gutter, caret_pad, underline,
    ))
}
