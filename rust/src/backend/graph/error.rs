//! Debug tracing (the `PETAL_TRACE` eprintln path) and the graph engine's
//! adapter to the shared error annotator ([`crate::backend::errors`]).

use crate::backend::errors::{self, TraceFrame};

use super::*;

impl<'a> Evaluator<'a> {
    /// Annotate a runtime error with source position, snippet, provenance, and a
    /// stack trace. Builds the trace-frame list from the graph call frames and
    /// defers formatting to the shared annotator.
    pub(super) fn annotate_error(&self, msg: String, failing: TermId) -> String {
        let frames: Vec<TraceFrame> = self
            .stack
            .frames
            .iter()
            .map(|f| TraceFrame {
                name: f.fn_name.clone(),
                call_site: f.return_term,
            })
            .collect();
        errors::annotate_error(self.program, failing, msg, &frames)
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
