//! AST rewrites that run before anything analyses or compiles a module:
//! default parameter values are moved into their function's body (see
//! [`lower_param_defaults`]).
//!
//! This pass used to also lift the `f(@x)` rebind operator into `x = f(x)`.
//! That operator was removed on 2026-11-09 (`@` is kept for annotations,
//! docs/proposals/annotation-syntax.md); the parser now rejects it and
//! `petal migrate-old-code` (`at-rebind`, core/src/migrate/at_rebind.rs)
//! makes the same rewrite in source.

use crate::ast::{
    self, AssignTarget, ElseBranch, Expr, ExprKind, ExprVisitorMut, Param, Stmt, StmtKind,
};

/// Move each default parameter value in `stmts` into its function's body
/// (see [`lower_param_defaults`]).
pub fn desugar(stmts: &mut Vec<Stmt>) {
    let mut lower = LowerDefaults;
    for stmt in stmts.iter_mut() {
        lower.visit_stmt(stmt);
    }
}

/// Move every default parameter value to the head of its function's body.
///
/// `fn f(a, b = a * 2) … end` becomes, in effect,
///
/// ```text
/// fn f(a, b)
///   b = if <b was passed> then b else a * 2 end
///   …
/// end
/// ```
///
/// where `<b was passed>` reads a hidden flag the call fills in
/// ([`ast::arg_given_name`]). That one rewrite is the whole meaning of a
/// default: it is an expression in the callee, run on every call that omits
/// the argument, after the parameters before it are bound. Everything that
/// analyses or compiles a function body — captures, hoisting, `state` paths,
/// the type checker — then sees it as the ordinary code it is.
///
/// The parameter keeps `default_in_body` so its signature still says the
/// argument is optional.
fn lower_param_defaults(params: &mut [Param], body: &mut Vec<Stmt>) {
    if !params.iter().any(|p| p.default.is_some()) {
        return;
    }
    let mut prologue = Vec::new();
    for p in params.iter_mut() {
        let Some(default) = p.default.take() else {
            continue;
        };
        p.default_in_body = true;
        prologue.push(param_default_stmt(&p.name, default));
    }
    // A function with nothing but defaults still returns what an empty body
    // does, not its last default.
    if body.is_empty()
        && let Some(span) = prologue.last().map(|s| s.span)
    {
        prologue.push(Stmt {
            kind: StmtKind::Expr(Expr {
                kind: ExprKind::Literal(ast::Literal::Nil),
                span,
            }),
            span,
            exported: false,
            test: None,
        });
    }
    prologue.append(body);
    *body = prologue;
}

/// `name = if name#given then name else default end`, carrying the default
/// expression's span throughout so an error inside it points at what was
/// written.
fn param_default_stmt(name: &str, default: Expr) -> Stmt {
    let span = default.span;
    let ident = |n: String| Expr {
        kind: ExprKind::Ident(n),
        span,
    };
    let expr_stmt = |e: Expr| Stmt {
        kind: StmtKind::Expr(e),
        span,
        exported: false,
        test: None,
    };
    let value = Expr {
        kind: ExprKind::If {
            condition: Box::new(ident(ast::arg_given_name(name))),
            then_body: vec![expr_stmt(ident(name.to_string()))],
            else_body: Some(ElseBranch::Block(vec![expr_stmt(default)])),
        },
        span,
    };
    Stmt {
        kind: StmtKind::Assign {
            target: AssignTarget::Name(name.to_string()),
            value,
        },
        span,
        exported: false,
        test: None,
    }
}

/// If `stmt` is the statement [`lower_param_defaults`] wrote for a defaulted
/// parameter, the parameter's name and its default expression.
pub fn as_param_default(stmt: &Stmt) -> Option<(&str, &Expr)> {
    let StmtKind::Assign {
        target: AssignTarget::Name(name),
        value,
    } = &stmt.kind
    else {
        return None;
    };
    let ExprKind::If {
        condition,
        else_body: Some(ElseBranch::Block(else_body)),
        ..
    } = &value.kind
    else {
        return None;
    };
    let ExprKind::Ident(flag) = &condition.kind else {
        return None;
    };
    if *flag != ast::arg_given_name(name) {
        return None;
    }
    match else_body.as_slice() {
        [
            Stmt {
                kind: StmtKind::Expr(default),
                ..
            },
        ] => Some((name, default)),
        _ => None,
    }
}

/// Runs [`lower_param_defaults`] on every function and lambda, however deeply
/// nested — including one written inside another's default expression, which
/// the walk reaches as part of the body it was just moved into.
struct LowerDefaults;

impl ExprVisitorMut for LowerDefaults {
    fn visit_expr(&mut self, e: &mut Expr) {
        if let ExprKind::Lambda { params, body } = &mut e.kind {
            lower_param_defaults(params, body);
        }
        ast::walk_expr_mut(self, e);
    }
    fn visit_stmt(&mut self, s: &mut Stmt) {
        if let StmtKind::FnDecl { params, body, .. } = &mut s.kind {
            lower_param_defaults(params, body);
        }
        ast::walk_stmt_mut(self, s);
    }
}
