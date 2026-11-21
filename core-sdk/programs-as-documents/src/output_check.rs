//! A static check that a document makes its output call once.
//!
//! The runner reads the arguments of the *last* call to an [`OutputCall`]'s
//! native, so a document that calls `canvas(doc, layers)` twice would run,
//! and silently show only the second. [`check_output_calls`] reads the
//! source before it runs and refuses one where the call can run more than
//! once: two calls on one path through the script, or a call inside a loop.
//!
//! The check is about the path the script can take, not the text: calls in
//! the two branches of an `if` (or the arms of a `match`) are one call,
//! since only one branch runs. Calls inside a function or lambda the
//! document declares are not counted (how often it is called is not known
//! here), and nor are calls when the document binds the call's name itself.

use petal::ast::{ElseBranch, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_expr, walk_stmt};
use petal::rewrite::parse_ast;
use petal::source_map::SourceSpan;

use crate::run::OutputCall;

/// Refuse `source` when one of `calls` can run more than once: the error
/// names the call and the lines it is made on. A source that does not
/// parse passes (loading it reports that).
pub fn check_output_calls(source: &str, calls: &[OutputCall]) -> Result<(), String> {
    if calls.is_empty() {
        return Ok(());
    }
    let Ok((_, stmts)) = parse_ast(source) else { return Ok(()) };
    for c in calls {
        let name = function_name(c);
        if binds(&stmts, name) {
            continue;
        }
        let mut v = Paths { names: [name, c.native], path: Path::default() };
        for s in &stmts {
            v.visit_stmt(s);
        }
        let path = v.path;
        if let Some(span) = path.looped {
            return Err(format!(
                "{} is called inside a loop (line {}), so it runs more than once: a document makes its output call once",
                c.call, span.start.line
            ));
        }
        if let [first, second, ..] = path.calls.as_slice() {
            let times = if path.calls.len() == 2 { "twice".to_string() } else { format!("{} times", path.calls.len()) };
            return Err(format!(
                "{} is called {times}, on line {} and again on line {}: a document makes its output call once",
                c.call, first.start.line, second.start.line
            ));
        }
    }
    Ok(())
}

/// The function a document calls: `canvas` in `"canvas(doc, layers)"`.
fn function_name(c: &OutputCall) -> &'static str {
    c.call.split('(').next().unwrap_or(c.call).trim()
}

/// Whether the document's top level binds `name` itself (its own `fn` or
/// `let`), so a call to it is not the output call.
fn binds(stmts: &[Stmt], name: &str) -> bool {
    stmts.iter().any(|s| match &s.kind {
        StmtKind::Let { name: n, .. } | StmtKind::FnDecl { name: n, .. } => n == name,
        _ => false,
    })
}

/// The output calls along the path through the script that makes the most.
#[derive(Default)]
struct Path {
    calls: Vec<SourceSpan>,
    /// A call inside a loop body: it runs once per iteration.
    looped: Option<SourceSpan>,
}

impl Path {
    fn then(&mut self, next: Path) {
        self.calls.extend(next.calls);
        self.looped = self.looped.or(next.looped);
    }

    fn weight(&self) -> usize {
        if self.looped.is_some() { usize::MAX } else { self.calls.len() }
    }

    /// The heaviest of alternative branches, only one of which runs (the
    /// first of equals).
    fn heaviest(branches: Vec<Path>) -> Path {
        branches.into_iter().rev().max_by_key(Path::weight).unwrap_or_default()
    }

    /// `body` run once per iteration of a loop.
    fn looped(body: Path) -> Path {
        let looped = body.looped.or(body.calls.first().copied());
        Path { calls: body.calls, looped }
    }
}

/// Walks the script in run order, adding each output call to the path.
struct Paths {
    /// The call's function name and its native's.
    names: [&'static str; 2],
    path: Path,
}

impl Paths {
    fn sub(&self) -> Paths {
        Paths { names: self.names, path: Path::default() }
    }

    fn of_expr(&self, e: &Expr) -> Path {
        let mut v = self.sub();
        v.visit_expr(e);
        v.path
    }

    fn of_stmts(&self, stmts: &[Stmt]) -> Path {
        let mut v = self.sub();
        for s in stmts {
            v.visit_stmt(s);
        }
        v.path
    }

    fn is_output_call(&self, function: &Expr) -> bool {
        matches!(&function.kind, ExprKind::Ident(n) if self.names.contains(&n.as_str()))
    }

    fn else_path(&self, else_body: &Option<ElseBranch>) -> Path {
        match else_body {
            Some(ElseBranch::Block(stmts)) => self.of_stmts(stmts),
            Some(ElseBranch::ElseIf(e)) => self.of_expr(e),
            None => Path::default(),
        }
    }

    fn a_loop(&mut self, head: &[&Expr], body: &[Stmt]) {
        for e in head {
            let p = self.of_expr(e);
            self.path.then(p);
        }
        let body = self.of_stmts(body);
        self.path.then(Path::looped(body));
    }
}

impl ExprVisitor for Paths {
    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Call { function, .. } if self.is_output_call(function) => {
                // Its arguments run first.
                walk_expr(self, e);
                self.path.calls.push(e.span);
            }
            ExprKind::If { condition, then_body, else_body } => {
                let cond = self.of_expr(condition);
                self.path.then(cond);
                let branch = Path::heaviest(vec![self.of_stmts(then_body), self.else_path(else_body)]);
                self.path.then(branch);
            }
            ExprKind::Match { subject, arms } => {
                let subject = self.of_expr(subject);
                self.path.then(subject);
                let arms = arms.iter().map(|arm| {
                    let mut p = arm.guard.as_ref().map(|g| self.of_expr(g)).unwrap_or_default();
                    p.then(self.of_stmts(&arm.body));
                    p
                });
                let branch = Path::heaviest(arms.collect::<Vec<_>>());
                self.path.then(branch);
            }
            ExprKind::For { iter, key, body, .. } => {
                let head: Vec<&Expr> = std::iter::once(&**iter).chain(key.as_deref()).collect();
                self.a_loop(&head, body);
            }
            // How often a lambda runs is not known here.
            ExprKind::Lambda { .. } => {}
            _ => walk_expr(self, e),
        }
    }

    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            // Nor how often a declared function is called.
            StmtKind::FnDecl { .. } => {}
            StmtKind::For { iter, key, body, .. } => {
                let head: Vec<&Expr> = std::iter::once(iter).chain(key.as_deref()).collect();
                self.a_loop(&head, body);
            }
            StmtKind::While { condition, body } => self.a_loop(&[condition], body),
            _ => walk_stmt(self, s),
        }
    }
}
