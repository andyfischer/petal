//! Warn about `state(key)`, the deprecated keyed form of `state`.
//!
//! `state(key) name = init` keys its slot by the key's value and ignores the
//! call path (docs/dev/state-call-paths.md §2.2): every caller, loop
//! iteration and recursion depth that reaches the declaration with an equal
//! key lands on one slot. That is a global under another name, hidden inside
//! a function, and it breaks the rule that stateful code is safe to call
//! twice. Each use is better spelled as one of three things:
//!
//! - `memo(key) name = init` when the slot is never written and the
//!   initializer is a pure function of the key: sharing a pure result between
//!   callers is unobservable (docs/features/memo.md);
//! - a top-level `state var` holding a Map, written with `set m[k] = v`, when
//!   the sharing is the point;
//! - plain `state`, kept per call path, when each caller should own its slot.
//!
//! The feature keeps working; this is a warning. It is silenced like a lint
//! finding, with `// petal-lint-ignore state-key-deprecated` on the line before
//! the declaration or trailing on its line (or `petal-lint-ignore-file`), and
//! `petal migrate-old-code` rewrites to `memo` the never-written ones whose
//! initializer is plainly a pure function of the key ([`init_is_pure_of_key`];
//! `crate::migrate`'s migration of the same name).
//!
//! A first-party bundled package is not warned about: its author is not the
//! reader of the warning ([`super::is_bundled_copy`]).

use crate::ast::{Expr, ExprKind, ExprVisitor, RecordField, Stmt, StmtKind, walk_expr, walk_stmt};
use crate::diagnostic::Diagnostic;
use crate::source_map::SourceSpan;

/// The rule id the warning carries, and the migration's name.
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
    /// The initializer is, by its syntax alone, a pure function of the key:
    /// see [`init_is_pure_of_key`]. Only then is the `memo` rewrite sound.
    pub init_pure_of_key: bool,
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
                init,
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
                    init_pure_of_key: init_is_pure_of_key(init, key),
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

/// Is `init` a pure function of `key`, judged from its syntax alone?
///
/// A `memo` hands one caller's value to every caller with an equal key, and
/// recomputes it after a reload or a run that does not reach it, so the
/// rewrite from `state(key)` is sound only when the initializer cannot give
/// a different answer for an equal key. This accepts the initializers that
/// provably cannot: literals, the values the key holds, operators, and list,
/// record, field, index and interpolation forms over those. Anything else
/// (any call, a `get`, a value the key does not hold, a lambda, a branch or a
/// loop) is rejected, though many such initializers are pure: the full
/// purity check, with the static classifier, is docs/tasks/memo-purity-check.md.
///
/// "The values the key holds" are access paths, not names: the key
/// `[g.code_hash]` holds `g.code_hash` and whatever is read through it, but
/// not `g.uniforms`, and not `g`. And a path counts only where the key keeps
/// its whole value: as the key itself, or as an element of a list or the
/// value of a field the key is built of. Under an operator or an index
/// (`state(x % 2)`) equal keys do not mean equal `x`.
pub(crate) fn init_is_pure_of_key(init: &Expr, key: &Expr) -> bool {
    struct HasLambda(bool);
    impl ExprVisitor for HasLambda {
        fn visit_expr(&mut self, e: &Expr) {
            if matches!(e.kind, ExprKind::Lambda { .. }) {
                self.0 = true;
            }
            walk_expr(self, e);
        }
    }
    let mut lambda = HasLambda(false);
    lambda.visit_expr(key);
    let mut held = Vec::new();
    key_paths(key, &mut held);
    !lambda.0 && pure_over(init, &held)
}

/// `e` as an access path: a name, then the fields read through it
/// (`g.pass.code_hash` is `[g, pass, code_hash]`). `None` for anything else.
fn access_path(e: &Expr) -> Option<Vec<&str>> {
    match &e.kind {
        ExprKind::Ident(n) => Some(vec![n.as_str()]),
        ExprKind::FieldAccess { object, field } => {
            let mut path = access_path(object)?;
            path.push(field.as_str());
            Some(path)
        }
        _ => None,
    }
}

/// The access paths whose whole value `key` holds.
fn key_paths<'a>(key: &'a Expr, out: &mut Vec<Vec<&'a str>>) {
    if let Some(path) = access_path(key) {
        out.push(path);
        return;
    }
    match &key.kind {
        ExprKind::List(items) => items.iter().for_each(|item| key_paths(item, out)),
        ExprKind::Record(fields) => {
            for f in fields {
                if let RecordField::Named(_, v) = f {
                    key_paths(v, out);
                }
            }
        }
        _ => {}
    }
}

fn pure_over(e: &Expr, held: &[Vec<&str>]) -> bool {
    let all = |es: &[Expr]| es.iter().all(|x| pure_over(x, held));
    // A path the key holds, or one read through such a path.
    if let Some(path) = access_path(e) {
        return held.iter().any(|k| path.starts_with(k));
    }
    match &e.kind {
        ExprKind::Literal(_) => true,
        ExprKind::BinaryOp { left, right, .. } => pure_over(left, held) && pure_over(right, held),
        ExprKind::UnaryOp { operand, .. } => pure_over(operand, held),
        ExprKind::List(items) => all(items),
        ExprKind::Record(fields) => fields.iter().all(|f| match f {
            RecordField::Named(_, v) => pure_over(v, held),
            // A computed key is a spread of an internal call; a spread
            // of a value the key holds is pure.
            RecordField::Spread(v) => pure_over(v, held),
        }),
        ExprKind::FieldAccess { object, .. } => pure_over(object, held),
        ExprKind::IndexAccess { object, index } => {
            pure_over(object, held) && pure_over(index, held)
        }
        ExprKind::OptionalAccess(inner) => pure_over(inner, held),
        ExprKind::StringInterp { exprs, .. } => all(exprs),
        _ => false,
    }
}

/// The message for a keyed declaration of `name`. The compile warning adds
/// the hint to run `petal migrate-old-code` and the rule id; a migration
/// finding is printed after it already.
pub(crate) fn message(name: &str) -> String {
    format!(
        "`state(…) {name}` is deprecated: keyed state is a hidden shared global (one slot per \
         key value, whichever caller reaches it). Use `memo(key)` for a cache whose initializer reads \
         only its key, a top-level \
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
        .map(|k| {
            Diagnostic::new(
                k.head,
                format!("{}; {} [{RULE}]", message(&k.name), crate::migrate::RUN_HINT),
            )
        })
        .collect()
}
