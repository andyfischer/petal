//! Warn about `state(key)`, the deprecated keyed form of `state`.
//!
//! `state(key) name = init` keys its slot by the key's value and ignores the
//! call path (docs/dev/state-call-paths.md §2.2): every caller, loop
//! iteration and recursion depth that reaches the declaration with an equal
//! key lands on one slot. That is a global under another name, hidden inside
//! a function, and it breaks the rule that stateful code is safe to call
//! twice. Each use is better spelled as one of three things:
//!
//! - `memo(key) name = init` when the slot is never written: sharing a pure
//!   result between callers is unobservable (docs/features/memo.md);
//! - a top-level `state var` holding a Map, written with `set m[k] = v`, when
//!   the sharing is the point;
//! - plain `state`, kept per call path, when each caller should own its slot.
//!
//! The feature keeps working; this is a warning. It is silenced like a lint
//! finding, with `// petal-lint-ignore state-key-deprecated` on the line before
//! the declaration or trailing on its line (or `petal-lint-ignore-file`), and
//! `petal lint --fix` rewrites the never-written ones to `memo`
//! (`crate::lint`'s rule of the same name).
//!
//! A first-party bundled package is not warned about: its author is not the
//! reader of the warning ([`super::is_bundled_copy`]).

use crate::ast::{ExprVisitor, Stmt, StmtKind, walk_stmt};
use crate::diagnostic::Diagnostic;
use crate::source_map::SourceSpan;

/// The rule id the warning carries, and the lint rule's name.
pub(crate) const RULE: &str = "state-key-deprecated";

/// One `state(key)` declaration, wherever it is written.
pub(crate) struct KeyedState {
    pub name: String,
    pub is_var: bool,
    /// Written `pub state(…)`.
    pub exported: bool,
    /// The whole statement.
    pub span: SourceSpan,
    /// From its first word to the end of its key: where the caret goes, under
    /// `state(key)` rather than a long initializer.
    pub head: SourceSpan,
}

/// Every `state(key)` declaration in `stmts`, nested ones included, in source
/// order.
pub(crate) fn keyed_states(stmts: &[Stmt]) -> Vec<KeyedState> {
    struct Finder(Vec<KeyedState>);
    impl ExprVisitor for Finder {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::State {
                name,
                key: Some(key),
                is_var,
                ..
            } = &s.kind
            {
                let head = if key.span.file == s.span.file {
                    SourceSpan {
                        start: s.span.start,
                        end: key.span.end,
                        file: s.span.file,
                    }
                } else {
                    s.span
                };
                self.0.push(KeyedState {
                    name: name.clone(),
                    is_var: *is_var,
                    exported: s.exported,
                    span: s.span,
                    head,
                });
            }
            walk_stmt(self, s);
        }
    }
    let mut finder = Finder(Vec::new());
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.0.sort_by_key(|k| k.span.start.offset);
    finder.0
}

/// The message for a keyed declaration of `name`. The compile warning adds
/// the rule id; a lint finding is printed after it already.
pub(crate) fn message(name: &str) -> String {
    format!(
        "`state(…) {name}` is deprecated: keyed state is a hidden shared global (one slot per \
         key value, whichever caller reaches it). Use `memo(key)` for a pure cache, a top-level \
         `state var` holding a Map for state shared on purpose, or plain `state` (kept per call \
         path) for per-instance state"
    )
}

/// The deprecation warning for each `state(key)` in a module's `stmts`,
/// parsed from `source`, less the ones a `petal-lint-ignore` comment
/// silences.
pub(super) fn deprecated_keyed_states(stmts: &[Stmt], source: &str) -> Vec<Diagnostic> {
    let found = keyed_states(stmts);
    if found.is_empty() {
        return Vec::new();
    }
    // A file whose comments do not scan is reported in full.
    let ignores = crate::lint::Ignores::scan(source).ok();
    found
        .into_iter()
        .filter(|k| {
            let line = (k.span.start.line as usize).saturating_sub(1);
            !ignores.as_ref().is_some_and(|i| i.suppresses(RULE, line))
        })
        .map(|k| Diagnostic::new(k.head, format!("{} [{RULE}]", message(&k.name))))
        .collect()
}
