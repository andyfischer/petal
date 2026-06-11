//! Error annotation and debug tracing: source snippets, provenance,
//! stack traces, and the PETAL_TRACE eprintln path.

use crate::source_map::SourceSpan;

use super::*;

impl<'a> Evaluator<'a> {
    /// Annotate a runtime error message with the failing term's source
    /// position, a caret snippet, dataflow provenance, and a stack trace.
    pub(super) fn annotate_error(&self, msg: String, failing: TermId) -> String {
        let program = self.program;
        let span = program.source_map.get(failing);
        let mut error_msg = match span {
            Some(s) if s.start.line > 0 => {
                format!("{} [line {}, column {}]", msg, s.start.line, s.start.column)
            }
            _ => msg,
        };

        // If we have a span AND the program carries its source, append a
        // 3-line snippet with a caret so users can see exactly where the
        // failing term is in their code.
        if let Some(s) = span {
            if !program.source.is_empty() {
                if let Some(snippet) = format_source_snippet(&program.source, &s) {
                    error_msg.push('\n');
                    error_msg.push_str(&snippet);
                }
            }
        }

        // Append provenance: up to 5 nearest named ancestors with spans
        let provenance = self.format_provenance(failing, 5);
        if !provenance.is_empty() {
            error_msg.push_str("\nCaused by:");
            for entry in &provenance {
                error_msg.push_str(&format!("\n  {}", entry));
            }
        }

        // Build stack trace from call frames
        let trace = self.build_stack_trace();
        if !trace.is_empty() {
            error_msg.push_str("\nStack trace:");
            for entry in &trace {
                error_msg.push_str(&format!("\n  {}", entry));
            }
        }

        error_msg
    }

    /// Walk provenance of the failing term and format up to `max` nearest
    /// ancestors that have both a name and a source span. This surfaces the
    /// user-visible variables that fed into the failure so error messages
    /// point at causes, not just the failing operation.
    fn format_provenance(&self, failing: TermId, max: usize) -> Vec<String> {
        let program = self.program;
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

    /// Build a stack trace from the current call frames.
    /// Returns a list of strings like "in foo() [line 5, column 1]".
    fn build_stack_trace(&self) -> Vec<String> {
        let mut trace = Vec::new();

        // Walk frames from top to bottom, collecting call frames with
        // function names
        for frame in self.stack.frames.iter().rev() {
            let Some(ref name) = frame.fn_name else {
                continue;
            };
            // Find the call site: the return_term is the Call term in the parent
            let call_site = frame
                .return_term
                .and_then(|tid| self.program.source_map.get(tid))
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

    /// Emit a one-line trace event to stderr when PETAL_TRACE=1.
    /// Reads the result value from the term's register post-advance.
    pub(super) fn trace_term(&self, term: &Term, inputs: &[Value]) {
        use std::sync::OnceLock;
        static ENABLED: OnceLock<bool> = OnceLock::new();
        let enabled = *ENABLED.get_or_init(|| {
            std::env::var("PETAL_TRACE").is_ok() || std::env::var("PETAL_DEBUG").is_ok()
        });
        if !enabled {
            return;
        }

        // Read result from the current frame's register for this term
        let result = self
            .stack
            .frames
            .last()
            .map(|f| f.get_register(term.register.0 as usize))
            .unwrap_or(Value::Nil);

        let input_strs: Vec<String> = inputs
            .iter()
            .map(|v| value::value_to_display_string(v, self.heap))
            .collect();
        let result_str = value::value_to_display_string(&result, self.heap);

        let span = self.program.source_map.get(term.id);
        let loc = match span {
            Some(s) if s.start.line > 0 => format!("{}:{}", s.start.line, s.start.column),
            _ => "-".to_string(),
        };

        let name = term.name.as_deref().unwrap_or("");
        eprintln!(
            "[trace] t{:<3} {:<4} {:<20} {:?} inputs=[{}] -> {}",
            term.id.0,
            loc,
            name,
            term.op,
            input_strs.join(", "),
            result_str,
        );
    }
}

/// Render a 3-line source snippet for a given span: the source line it
/// points at, prefixed with a gutter, followed by a caret line marking
/// the column. Returns `None` if the span is a placeholder or the line
/// is out of range. Kept ASCII-only and zero-dependency; callers append
/// the snippet to error messages shown on stderr.
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
    let underline: String = std::iter::repeat('^').take(span_len.max(1)).collect();
    Some(format!(
        "{} |\n{} | {}\n{} | {}{}",
        blank_gutter, line_num, line, blank_gutter, caret_pad, underline,
    ))
}
