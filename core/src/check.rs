//! The check: "will this program run?", answered without running it.
//!
//! One function, [`Env::check_program`](crate::env::Env::check_program), for
//! everything that asks: `petal check` reports its answer, `petal run`
//! refuses to start on it, and a host that reloads a script while it is
//! being edited prints it and runs anyway (a half-typed line must not take
//! the frame down).
//!
//! The check has three parts, and only the last is done here:
//!
//! 1. **The compile.** The type checker and the compiler's own lints leave
//!    their findings on the program
//!    ([`Program::warnings`](crate::program::Program::warnings)), each a
//!    warning or an error ([`Severity`](crate::diagnostic::Severity)).
//! 2. **The name scan** ([`crate::typecheck::globals`]): a call to, or read
//!    of, a name nothing defines compiles to a deferred error that fires when
//!    its line runs. Every compile an `Env` does ends with the scan, so those
//!    are on the program as errors too — for the LSP and for a host that only
//!    ever reads `warnings`, not just for a caller of this module.
//! 3. **Lowering.** A program can compile and still fail to lower to
//!    bytecode.
//!
//! Linking is not part of it. Whether the host natives a program declares are
//! registered is a question about one `Env`, not about the program: `petal
//! check` passes a script written for a host it is not, and
//! [`Env::link`](crate::env::Env::link) is the separate step a run takes
//! after the check.

use crate::diagnostic::Diagnostic;
use crate::program::Program;
use crate::source_map::SourceSpan;

/// What [`Env::check_program`](crate::env::Env::check_program) found.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckReport {
    /// Every finding, errors and warnings, in the order the compile reported
    /// them. The same list as the program's `warnings`.
    pub diagnostics: Vec<Diagnostic>,
    /// Why the program could not be lowered to bytecode, when it could not.
    pub lower_error: Option<String>,
}

impl CheckReport {
    /// The findings that fail the check: each is a line that fails whenever
    /// it runs.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.is_error())
    }

    pub fn error_count(&self) -> usize {
        self.errors().count()
    }

    /// The findings that do not fail the check.
    pub fn warning_count(&self) -> usize {
        self.diagnostics.len() - self.error_count()
    }

    /// Whether the program passed: no error, and it lowers. Warnings do not
    /// fail a check.
    pub fn passed(&self) -> bool {
        self.lower_error.is_none() && self.error_count() == 0
    }

    /// The findings as text, one block each; see [`render_diagnostics`].
    pub fn render(&self, program: &Program) -> String {
        render_diagnostics(program, &self.diagnostics, false)
    }

    /// The closing line of a failed check (`2 errors found by check`), or
    /// `None` when no finding is an error.
    pub fn summary(&self) -> Option<String> {
        match self.error_count() {
            0 => None,
            1 => Some("1 error found by check".to_string()),
            n => Some(format!("{n} errors found by check")),
        }
    }
}

/// The `[line N, column M]` (or `[file line N, column M]`) position tag for a
/// diagnostic's span — mirrors `backend::errors::format_position`.
fn position(program: &Program, span: &SourceSpan) -> String {
    match program.source_map.file_name_for_span(span) {
        Some(file) => format!(
            "[{} line {}, column {}]",
            file, span.start.line, span.start.column
        ),
        None => format!("[line {}, column {}]", span.start.line, span.start.column),
    }
}

/// Render diagnostics as human-readable text. Each becomes a `warning:` or
/// `error:` line (its [`Severity`](crate::diagnostic::Severity)), a
/// ` --> <position>` line, and (when a real span and its source exist) a
/// caret snippet. Empty when there are none.
///
/// `bare` drops the position line and the snippet: they encode where the
/// diagnostic sits in the file, so a re-indenting refactor would change them
/// without changing anything about the program. Only the message survives.
pub fn render_diagnostics(program: &Program, diagnostics: &[Diagnostic], bare: bool) -> String {
    let mut out = String::new();
    for d in diagnostics {
        out.push_str(&format!("{}: {}\n", d.severity.label(), d.message));
        if bare {
            continue;
        }
        out.push_str(&format!(" --> {}\n", position(program, &d.span)));
        let src = program
            .source_map
            .source_for_span(&d.span)
            .unwrap_or(&program.source);
        if let Some(snippet) = crate::backend::errors::format_source_snippet(src, &d.span) {
            out.push_str(&snippet);
            out.push('\n');
        }
    }
    out
}

/// What identifies a finding across an edit: its file, its severity, its
/// message (with the line numbers it cites left as placeholders), and the
/// text of the line it points at. Its position is not part of it, since an
/// edit above moves every line below.
fn diagnostic_key(program: &Program, d: &Diagnostic) -> (String, bool, String, String) {
    let file = program
        .source_map
        .file(d.span.file)
        .map(|f| f.name.clone())
        .unwrap_or_default();
    let line = program
        .source_map
        .source_for_span(&d.span)
        .unwrap_or(&program.source)
        .lines()
        .nth((d.span.start.line as usize).saturating_sub(1))
        .unwrap_or("")
        .trim()
        .to_string();
    (file, d.is_error(), d.stable_message().to_string(), line)
}

/// The findings of `new` that `old` did not have: what an edit introduced.
///
/// A finding is the same one before and after when it says the same thing
/// about the same line of text in the same file, wherever that line now is.
/// So a finding on a line the edit moved is not new, one on a line the edit
/// wrote or rewrote is, and of several identical findings on identical lines
/// only as many as were added are reported (the last of them). In the order
/// `new` lists them.
pub fn new_diagnostics(old: &Program, new: &Program) -> Vec<Diagnostic> {
    let mut had: std::collections::HashMap<(String, bool, String, String), usize> =
        std::collections::HashMap::new();
    for d in &old.warnings {
        *had.entry(diagnostic_key(old, d)).or_default() += 1;
    }
    new.warnings
        .iter()
        .filter(|d| match had.get_mut(&diagnostic_key(new, d)) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::env::Env;

    fn check(src: &str) -> (Env, crate::program::ProgramId, super::CheckReport) {
        let mut env = Env::new();
        env.set_check_only(true);
        let pid = env.load_program(src).expect("compiles");
        let report = env.check_program(pid);
        (env, pid, report)
    }

    #[test]
    fn a_clean_program_passes() {
        let (_, _, report) = check("fn f(a, b) a + b end\nprint(f(1, 2))");
        assert!(report.passed());
        assert_eq!(report.diagnostics, vec![]);
        assert_eq!(report.summary(), None);
    }

    #[test]
    fn a_warning_does_not_fail_the_check() {
        let (_, _, report) = check("let x: int = \"s\"");
        assert!(report.passed());
        assert_eq!((report.error_count(), report.warning_count()), (0, 1));
    }

    #[test]
    fn every_kind_of_error_fails_it_reached_or_not() {
        // The compile's own (an argument count), and the name scan's (an
        // unknown name, a named argument a builtin does not have).
        for src in [
            "fn f(a, b) a + b end\nif false then f(1) end",
            "if false then print(nope) end",
            "if false then frob() end",
            "if false then print(clamp(5, low: 0, hi: 3)) end",
            "if false then print(1, sep: 2) end",
        ] {
            let (_, _, report) = check(src);
            assert!(!report.passed(), "{src}");
            assert_eq!(report.error_count(), 1, "{src}: {:?}", report.diagnostics);
        }
    }

    #[test]
    fn the_scan_is_part_of_every_compile() {
        // A caller that only reads the program's `warnings` — the LSP, a
        // host — sees what `check_program` sees.
        let mut env = Env::new();
        let pid = env.load_program("if false then print(nope) end").unwrap();
        let program = env.get_program(pid).unwrap();
        assert_eq!(program.warnings.len(), 1);
        assert!(program.warnings[0].is_error());
        let recompiled = env.compile_program(pid, "if false then frob() end").unwrap();
        assert_eq!(recompiled.warnings.len(), 1);
    }

    #[test]
    fn the_lsp_shows_what_check_reports() {
        use crate::lsp::document::analyze;
        use crate::lsp::lsp_types::DiagnosticSeverity;

        let analysis = analyze("let a = 1\nif false then print(nope) end\n");
        let [d] = analysis.diagnostics.as_slice() else {
            panic!("one diagnostic: {:?}", analysis.diagnostics);
        };
        assert_eq!(d.message, "undefined variable `nope`");
        assert!(matches!(d.severity, Some(DiagnosticSeverity::Error)));
        assert_eq!(d.range.start.line, 1);
    }

    #[test]
    fn the_report_renders_like_petal_check() {
        let (env, pid, report) = check("print(nope)");
        let text = report.render(env.get_program(pid).unwrap());
        assert_eq!(
            text,
            "error: undefined variable `nope`\n --> [line 1, column 7]\n  |\n1 | print(nope)\n  |       ^^^^\n"
        );
        assert_eq!(report.summary().as_deref(), Some("1 error found by check"));
        assert_eq!(env.diagnostics_text(pid), text);
    }

    #[test]
    fn a_check_does_not_link_and_link_is_its_own_step() {
        let src = "native fn host_only()\nhost_only()";
        let (env, pid, report) = check(src);
        assert!(report.passed(), "{:?}", report.diagnostics);
        let err = env.link(pid).unwrap_err();
        assert_eq!(err.phase, crate::error::Phase::Link);
        assert!(err.to_string().contains("host_only"), "{err}");
    }
}
