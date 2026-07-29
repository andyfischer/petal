//! A typed load error: *which phase* of the front end rejected the program,
//! plus the individual diagnostics that phase produced.
//!
//! ## Why this exists
//!
//! The CLI used to guess the phase by sniffing the message text
//! (`classify_load_error`), which tagged every compiler diagnostic — `var`/`set`
//! disjointness, cross-function assignment, import binding, overload exports —
//! as `"parse"`. The phase is now decided at the point the error is *raised*
//! and travels with it.
//!
//! ## Scope (deliberate)
//!
//! - **`phase` is typed at birth.** Every internal front-end entry point
//!   (`cst::parse_source_phased`, `module::load_modules`,
//!   `Compiler::compile_modules`, `Env::compile_source`) returns [`LoadError`],
//!   so no caller has to reconstruct the phase from text. That is the whole
//!   ask, and it is fully achieved.
//! - **`span` is structured only for compiler errors.** Compiler diagnostics
//!   already carry a [`SourceSpan`] ([`crate::diagnostic::Diagnostic`]), so
//!   [`LoadError::from_diagnostics`] moves it across intact. Lexer and parser
//!   errors are still `Result<_, String>` with a `" [line N, column M]"` suffix
//!   baked into the message, so [`ErrorItem::from_legacy`] *recovers* the
//!   position by parsing that suffix. Giving `lexer.rs` (~15 raise sites) and
//!   `parse.rs` (~60 `Result<_, String>` methods) real spans is a separate and
//!   much larger job; `from_legacy` is the one remaining string-shape parser,
//!   deliberately isolated in one place so that job can delete it.
//!
//! ## The public facade stays `String`
//!
//! Every public `Env` API keeps `Result<_, String>`; the typed error is an
//! internal channel with typed entry points (`Env::load_program_diag`) added
//! alongside. [`LoadError`]'s [`Display`] therefore has to reproduce the old
//! strings *byte for byte* — that is what keeps every existing caller and test
//! green. See the `display_*` tests below.

use std::fmt;

use crate::diagnostic::Diagnostic;
use crate::source_map::{SourcePosition, SourceSpan};

/// Which stage of the front end rejected the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Tokenizing (`crate::lexer`).
    Lex,
    /// Parsing / CST projection (`crate::parse`, `crate::cst_project`).
    Parse,
    /// Resolving and loading imported modules (`crate::module`).
    Module,
    /// Compiling statements to the term graph (`crate::compiler`).
    Compile,
    /// Lowering the term graph to bytecode (`crate::backend::bytecode`).
    Lower,
}

impl Phase {
    /// The wire name used by `--json` output (`"phase"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Lex => "lex",
            Phase::Parse => "parse",
            Phase::Module => "module",
            Phase::Compile => "compile",
            Phase::Lower => "lower",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One diagnostic within a [`LoadError`].
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorItem {
    /// The message alone: no `" [line N, column M]"` suffix, no file prefix.
    pub message: String,
    /// Where it happened, when known.
    pub span: Option<SourceSpan>,
    /// Display name of the module the error is in; `None` is the entry file.
    pub file: Option<String>,
}

impl ErrorItem {
    /// A message with no position.
    pub fn message(message: impl Into<String>) -> Self {
        ErrorItem {
            message: message.into(),
            span: None,
            file: None,
        }
    }

    /// Recover structure from a legacy `"msg [line N, column M]"` string.
    ///
    /// This is the **only** remaining place that parses an error's string
    /// shape. It exists because the lexer and parser format their positions
    /// into the message instead of carrying a span; when they gain real spans
    /// this function goes away. The offsets in the recovered span are zero —
    /// only line/column survive the round trip.
    pub fn from_legacy(msg: impl Into<String>) -> Self {
        let msg: String = msg.into();
        if let Some(open) = msg.rfind(" [line ")
            && let Some(close) = msg[open..].find(']')
            && let Some((line, column)) =
                msg[open + " [line ".len()..open + close].split_once(", column ")
            && let (Ok(line), Ok(column)) = (line.trim().parse(), column.trim().parse())
            && open + close + 1 == msg.len()
        {
            let pos = SourcePosition {
                line,
                column,
                offset: 0,
            };
            return ErrorItem {
                message: msg[..open].to_string(),
                span: Some(SourceSpan {
                    start: pos,
                    end: pos,
                    file: crate::source_map::ENTRY_FILE,
                }),
                file: None,
            };
        }
        ErrorItem::message(msg)
    }
}

impl fmt::Display for ErrorItem {
    /// `[{file}: ]{message}[ [line N, column M]]` — byte-identical to what the
    /// compiler's join and `module::parse_module`'s old `annotate` closure
    /// produced.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}: ")?;
        }
        f.write_str(&self.message)?;
        if let Some(span) = &self.span {
            write!(
                f,
                " [line {}, column {}]",
                span.start.line, span.start.column
            )?;
        }
        Ok(())
    }
}

/// A front-end failure: the phase that produced it and its diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadError {
    pub phase: Phase,
    pub items: Vec<ErrorItem>,
}

impl LoadError {
    /// A single spanless message, e.g. an unresolvable import.
    pub fn message(phase: Phase, message: impl Into<String>) -> Self {
        LoadError {
            phase,
            items: vec![ErrorItem::message(message)],
        }
    }

    /// A single legacy `"msg [line N, column M]"` string, position recovered.
    pub fn legacy(phase: Phase, message: impl Into<String>) -> Self {
        LoadError {
            phase,
            items: vec![ErrorItem::from_legacy(message)],
        }
    }

    /// Every compiler diagnostic, spans intact. The compiler walks the whole
    /// program before aborting, so all of them are reported at once.
    pub fn from_diagnostics(phase: Phase, diagnostics: &[Diagnostic]) -> Self {
        LoadError {
            phase,
            items: diagnostics
                .iter()
                .map(|d| ErrorItem {
                    message: d.message.clone(),
                    span: Some(d.span),
                    file: None,
                })
                .collect(),
        }
    }
}

impl fmt::Display for LoadError {
    /// Items joined by `\n`. This must stay byte-identical to the old
    /// hand-rolled formatting, because every public API renders a `LoadError`
    /// with `.to_string()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{item}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LoadError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_map::ENTRY_FILE;

    fn span(line: u32, column: u32) -> SourceSpan {
        let pos = SourcePosition {
            line,
            column,
            offset: 0,
        };
        SourceSpan {
            start: pos,
            end: pos,
            file: ENTRY_FILE,
        }
    }

    /// The byte-identity contract with `Compiler::compile_modules`' old join:
    /// `format!("{msg} [line {l}, column {c}]")` per diagnostic, `"\n"`-joined.
    #[test]
    fn display_matches_the_old_compiler_join() {
        let diags = vec![
            Diagnostic {
                span: span(4, 5),
                message: "`x` is bound outside this function".to_string(),
            },
            Diagnostic {
                span: span(5, 5),
                message: "`y` is bound outside this function".to_string(),
            },
        ];
        let expected = diags
            .iter()
            .map(|d| {
                format!(
                    "{} [line {}, column {}]",
                    d.message, d.span.start.line, d.span.start.column
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            LoadError::from_diagnostics(Phase::Compile, &diags).to_string(),
            expected
        );
    }

    /// The byte-identity contract with `module::parse_module`'s old `annotate`
    /// closure: `format!("{name}: {e}")` for a module, `e` for the entry file.
    #[test]
    fn display_matches_the_old_module_annotate() {
        let raw = "Expected Assign [line 2, column 7]";
        let mut err = LoadError::legacy(Phase::Parse, raw);
        assert_eq!(err.to_string(), raw, "entry-file errors are unchanged");
        err.items[0].file = Some("helper.ptl".to_string());
        assert_eq!(err.to_string(), format!("helper.ptl: {raw}"));
    }

    #[test]
    fn from_legacy_recovers_the_position_suffix() {
        let item = ErrorItem::from_legacy("Unterminated string [line 1, column 9]");
        assert_eq!(item.message, "Unterminated string");
        assert_eq!(
            item.span.map(|s| (s.start.line, s.start.column)),
            Some((1, 9))
        );
        // Round-trips.
        assert_eq!(item.to_string(), "Unterminated string [line 1, column 9]");
    }

    #[test]
    fn from_legacy_leaves_a_suffixless_message_alone() {
        for raw in [
            "cannot find module 'nope'",
            "too many modules (file table limit is 65535)",
            "a [line 1, column 2] trailing words",
        ] {
            let item = ErrorItem::from_legacy(raw);
            assert_eq!(item.message, raw);
            assert!(item.span.is_none());
            assert_eq!(item.to_string(), raw);
        }
    }

    #[test]
    fn spanless_messages_render_verbatim() {
        assert_eq!(
            LoadError::message(Phase::Module, "import cycle: a -> b -> a").to_string(),
            "import cycle: a -> b -> a"
        );
    }

    #[test]
    fn phase_names_are_the_json_wire_names() {
        assert_eq!(Phase::Lex.as_str(), "lex");
        assert_eq!(Phase::Parse.as_str(), "parse");
        assert_eq!(Phase::Module.as_str(), "module");
        assert_eq!(Phase::Compile.as_str(), "compile");
        assert_eq!(Phase::Lower.as_str(), "lower");
    }
}
