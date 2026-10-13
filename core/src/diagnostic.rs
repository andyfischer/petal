//! A non-fatal compile-time diagnostic: a type-checker or compiler finding,
//! surfaced alongside the compiled program without ever aborting compilation.
//!
//! # Message house style
//!
//! The two layers that talk to a user quote differently, and the quoting is
//! itself information — it tells the reader which phase spoke:
//!
//! - **Compile time** (`typecheck/`, the newer `compiler/` checks): backticks
//!   around every piece of source text, and the callee named bare —
//!   ``argument 1 to `double` ``, `` `f` expects 2 arguments, got 1 ``.
//! - **Runtime** (`backend/`): `'single quotes'`, and the callee named with
//!   parens the way a stack frame names it — `No field 'x' on class Rect`,
//!   `add() expects 2 arguments, got 3`.
//!
//! The one deliberate exception is the named-argument pair
//! (`has no parameter named`, `got multiple values for parameter`), which the
//! checker and the VM both report on the same call. There the checker adopts
//! the *runtime* style so the two lines read as one diagnosis followed by its
//! failure rather than the same complaint in two dialects; the checker's line
//! carries the extra detail. See `typecheck::Checker::check_named_args`.
use crate::source_map::SourceSpan;

/// Two places whose *layout* a compile-time check compared: whether one is on
/// a later line than the other, and whether it is indented deeper.
///
/// A check that looks at layout makes the compiler's output depend on
/// whitespace, which a hot reload otherwise treats as changing nothing
/// (`Env::reload_program`). So every such check records what it compared, the
/// program keeps the list ([`Program::layout_deps`](crate::program::Program::layout_deps)),
/// and a layout-only reload re-reads each pair at its new position: if any
/// reading changed, the edit may have added or removed a warning, and the
/// program is recompiled instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutDep {
    pub prev: SourceSpan,
    pub next: SourceSpan,
}

impl LayoutDep {
    /// The layout facts a check may read: `next` does not start on a later
    /// line than `prev`, and `next` starts in a later column.
    pub fn reading(&self) -> (bool, bool) {
        (
            self.next.start.line <= self.prev.start.line,
            self.next.start.column > self.prev.start.column,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub span: SourceSpan,
    pub message: String,
    pub severity: Severity,
    /// A second place in the source the message names by its line number
    /// (`... written on line 12 ...`), when it names one. Kept so the message
    /// can be re-rendered when the source's layout changes without a
    /// recompile ([`Diagnostic::relocate`]); see [`Diagnostic::citing`].
    pub cited: Option<CitedLine>,
}

/// The lines a diagnostic's message cites, and the message with a
/// placeholder where each number goes: `{line}` for the first cited place,
/// `{line:1}`, `{line:2}`, … for the ones after it.
#[derive(Debug, Clone, PartialEq)]
pub struct CitedLine {
    /// The first cited place.
    pub span: SourceSpan,
    /// Every cited place after the first, in placeholder order.
    more: Vec<SourceSpan>,
    template: String,
}

impl CitedLine {
    /// Every place the message cites.
    pub fn spans(&self) -> impl Iterator<Item = &SourceSpan> {
        std::iter::once(&self.span).chain(&self.more)
    }

    fn render(&self) -> String {
        let mut out = self.template.replace("{line}", &self.span.start.line.to_string());
        for (i, span) in self.more.iter().enumerate() {
            out = out.replace(&format!("{{line:{}}}", i + 1), &span.start.line.to_string());
        }
        out
    }
}

/// How sure the checker is that a diagnostic describes a failure.
///
/// Neither severity stops compilation: the program still compiles and runs, and
/// the line may never execute. The difference is what `petal check` does with
/// it. A [`Severity::Warning`] is advice (a type mismatch against an advisory
/// annotation, a discarded pure call). A [`Severity::Error`] is a line that
/// cannot succeed when it runs (a call to a name nothing defines, a call no
/// overload accepts), so `check` prints it as `error:` and exits non-zero
/// without needing `--strict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    #[default]
    Warning,
    Error,
}

impl Severity {
    /// The label a rendered diagnostic starts with: `warning` or `error`.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

impl Diagnostic {
    /// The message with the line numbers it cites left as placeholders
    /// (`{line}`): the same text however the source around it has moved.
    /// The message itself when it cites nothing.
    pub fn stable_message(&self) -> &str {
        match &self.cited {
            Some(cited) => &cited.template,
            None => &self.message,
        }
    }
}

impl Diagnostic {
    /// A warning: advice that does not by itself mean the line will fail.
    pub fn new(span: SourceSpan, message: String) -> Diagnostic {
        Diagnostic {
            span,
            message,
            severity: Severity::Warning,
            cited: None,
        }
    }

    /// A warning whose message names the line of another place in the source.
    /// `template` is the message with `{line}` standing for that line number;
    /// it is rendered with `cited`'s line. A diagnostic that quotes a line any
    /// other way goes stale when a layout-only edit is hot-reloaded without a
    /// recompile.
    pub fn citing(span: SourceSpan, template: String, cited: SourceSpan) -> Diagnostic {
        let cited = CitedLine {
            span: cited,
            more: Vec::new(),
            template,
        };
        Diagnostic {
            span,
            message: cited.render(),
            severity: Severity::Warning,
            cited: Some(cited),
        }
    }

    /// An error whose message names the lines of other places in the source:
    /// `{line}` in `template` stands for the line of `cited[0]`, `{line:1}`
    /// for that of `cited[1]`, and so on. With nothing cited the template is
    /// the message. See [`Diagnostic::citing`] for why the lines are kept.
    pub fn error_citing(span: SourceSpan, template: String, cited: Vec<SourceSpan>) -> Diagnostic {
        let mut cited = cited.into_iter();
        let Some(first) = cited.next() else {
            return Diagnostic::error(span, template);
        };
        let cited = CitedLine {
            span: first,
            more: cited.collect(),
            template,
        };
        Diagnostic {
            span,
            message: cited.render(),
            severity: Severity::Error,
            cited: Some(cited),
        }
    }

    /// Move the diagnostic to where its source went: `map` gives the new
    /// position of an old span, or `None` when it has none. Re-renders a
    /// cited line number. Returns `None` (and changes nothing) when either
    /// position cannot be mapped.
    pub fn relocated(
        &self,
        map: impl Fn(SourceSpan) -> Option<SourceSpan>,
    ) -> Option<Diagnostic> {
        let mut out = self.clone();
        out.span = map(self.span)?;
        if let Some(cited) = &mut out.cited {
            cited.span = map(cited.span)?;
            for span in &mut cited.more {
                *span = map(*span)?;
            }
            out.message = cited.render();
        }
        Some(out)
    }

    /// An error: the line fails whenever it runs. See [`Severity::Error`].
    pub fn error(span: SourceSpan, message: String) -> Diagnostic {
        Diagnostic {
            span,
            message,
            severity: Severity::Error,
            cited: None,
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// The candidate nearest `word` by [`edit_distance`], when it is within
/// `budget` edits: near enough to be a slip rather than a different word.
pub fn closest<'a>(
    word: &str,
    candidates: impl IntoIterator<Item = &'a str>,
    budget: usize,
) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|c| (edit_distance(word, c), c))
        .filter(|(d, _)| *d <= budget)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance over chars — the measure behind every "did you
/// mean" in the crate.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (diag + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diag = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}
