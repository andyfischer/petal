//! `@`-argument desugaring — the in-out call operator.
//!
//! Petal lets a call argument be prefixed with `@` to mean "update this
//! variable in place with the call's result". The canonical form
//!
//! ```text
//! something(@var)
//! ```
//!
//! is equivalent to
//!
//! ```text
//! var = something(var)
//! ```
//!
//! ## Interpretation (v1): hoist the nearest enclosing call
//!
//! `@v` binds to the *nearest enclosing call* — the call it is an argument of.
//! That call is lifted into an assignment placed immediately before the current
//! statement, and the call's site is replaced by a plain reference to `v`:
//!
//! ```text
//! func1(func2(@a))   =>   a = func2(a)
//!                         func1(a)
//! ```
//!
//! Here `@a`'s nearest enclosing call is `func2`, so `func2(a)` (not the whole
//! statement) is what gets written back to `a`.
//!
//! When the lifted call was the entire statement, no reference remains to
//! replace — the statement desugars to exactly the assignment, so the
//! canonical form above compiles to identical IR:
//!
//! ```text
//! something(@var)    =>   var = something(var)
//! ```
//!
//! ## Scope: "nearest statement level"
//!
//! Lifting only happens for expressions evaluated **once, unconditionally, at
//! the current statement level**: `let`/assignment values, expression
//! statements, `return`, `state` initializers, `for` iterables and `if`/`while`
//! *bodies* (each recursed as its own statement scope). Deferred or conditional
//! positions — lambda bodies, `match` arm bodies, `while` conditions — are left
//! untouched in v1; an `@` there survives to the compiler as a deferred error.
//! This keeps the rewrite from silently changing evaluation semantics.
//!
//! ## Limits (v1)
//!
//! At most one `@` per enclosing call. `f(@a, @b)` would need to assign one
//! result to two variables, so both markers are left in place and become
//! compile-time errors.

use crate::ast::{
    AssignTarget, ElseBranch, Expr, ExprKind, JsxChild, RecordField, Stmt, StmtKind,
};

/// Rewrite every `@`-argument in `stmts` in place (see module docs).
pub fn desugar(stmts: &mut Vec<Stmt>) {
    desugar_stmts(stmts);
}

/// Process a statement list: lift each statement's `@`-arguments into
/// assignments spliced in just before it, and recurse into nested blocks.
fn desugar_stmts(stmts: &mut Vec<Stmt>) {
    let taken = std::mem::take(stmts);
    let mut out = Vec::with_capacity(taken.len());
    for mut stmt in taken {
        let mut hoisted = Vec::new();
        lift_stmt(&mut stmt, &mut hoisted);
        // When the lifted call was the *entire* expression statement, the
        // generic rewrite leaves a residual read of the variable behind
        // (`f(@x)` → `x = f(x)` then a bare `x`). That read is load-bearing
        // when the call is a subexpression, but as a statement it is a pure
        // no-op — and its value equals the assignment's, so even a
        // block-result position is unchanged. Drop it: statement-level
        // `f(@x)` desugars to exactly `x = f(x)`.
        let residual = match (&stmt.kind, hoisted.last()) {
            (StmtKind::Expr(e), Some(h)) => matches!(
                (&e.kind, &h.kind),
                (ExprKind::Ident(n), StmtKind::Assign { target: AssignTarget::Name(t), .. })
                    if n == t
            ),
            _ => false,
        };
        out.extend(hoisted);
        if !residual {
            out.push(stmt);
        }
    }
    *stmts = out;
}

/// Lift `@`-arguments from the expressions a statement evaluates at its own
/// level, and recurse `desugar_stmts` into any nested blocks it owns.
fn lift_stmt(stmt: &mut Stmt, hoisted: &mut Vec<Stmt>) {
    match &mut stmt.kind {
        StmtKind::Let { value, .. } => lift_expr(value, hoisted),
        StmtKind::Assign { target, value } => {
            lift_expr(value, hoisted);
            match target {
                AssignTarget::Name(_) => {}
                AssignTarget::Field(object, _) => lift_expr(object, hoisted),
                AssignTarget::Index(object, index) => {
                    lift_expr(object, hoisted);
                    lift_expr(index, hoisted);
                }
            }
        }
        StmtKind::Expr(e) => lift_expr(e, hoisted),
        StmtKind::Return(Some(e)) => lift_expr(e, hoisted),
        StmtKind::Return(None) => {}
        StmtKind::State { init, key, .. } => {
            lift_expr(init, hoisted);
            if let Some(k) = key {
                lift_expr(k, hoisted);
            }
        }
        StmtKind::FnDecl { body, .. } => desugar_stmts(body),
        StmtKind::For { iter, body, .. } => {
            // The iterable is evaluated once, before the loop — safe to lift.
            lift_expr(iter, hoisted);
            desugar_stmts(body);
        }
        StmtKind::While { body, .. } => {
            // The condition re-evaluates each iteration, so hoisting an `@` out
            // of it would be wrong; leave it (→ deferred error) and only
            // recurse into the body.
            desugar_stmts(body);
        }
        StmtKind::EnumDecl { .. } | StmtKind::Break | StmtKind::Continue
        | StmtKind::Import(_) => {}
    }
}

/// Walk an expression evaluated at the current statement level, lifting the
/// nearest-enclosing-call `@`-arguments into `hoisted`. Recurses into nested
/// blocks (`if`/block/lambda bodies) as their own statement scopes rather than
/// lifting across them.
fn lift_expr(expr: &mut Expr, hoisted: &mut Vec<Stmt>) {
    match &mut expr.kind {
        // Same-frame sub-expressions: recurse, lifting into the same buffer.
        ExprKind::Call { function, args } => {
            lift_expr(function, hoisted);
            for a in args.iter_mut() {
                lift_expr(a, hoisted);
            }
        }
        ExprKind::BinaryOp { left, right, .. } => {
            lift_expr(left, hoisted);
            lift_expr(right, hoisted);
        }
        ExprKind::UnaryOp { operand, .. } => lift_expr(operand, hoisted),
        ExprKind::List(xs) => {
            for x in xs {
                lift_expr(x, hoisted);
            }
        }
        ExprKind::Record(fields) => {
            for f in fields {
                match f {
                    RecordField::Named(_, e) | RecordField::Spread(e) => lift_expr(e, hoisted),
                }
            }
        }
        ExprKind::FieldAccess { object, .. } => lift_expr(object, hoisted),
        ExprKind::IndexAccess { object, index } => {
            lift_expr(object, hoisted);
            lift_expr(index, hoisted);
        }
        ExprKind::StringInterp { exprs, .. } => {
            for e in exprs {
                lift_expr(e, hoisted);
            }
        }
        ExprKind::Element { props, children, .. } => {
            for (_, e) in props {
                lift_expr(e, hoisted);
            }
            for c in children {
                if let JsxChild::Expr(e) = c {
                    lift_expr(e, hoisted);
                }
            }
        }

        // Block boundaries: the condition/subject evaluates at this level, but
        // each nested body is its own statement scope.
        ExprKind::If {
            condition,
            then_body,
            else_body,
        } => {
            lift_expr(condition, hoisted);
            desugar_stmts(then_body);
            match else_body {
                Some(ElseBranch::Block(stmts)) => desugar_stmts(stmts),
                Some(ElseBranch::ElseIf(e)) => lift_expr(e, hoisted),
                None => {}
            }
        }
        ExprKind::Block(stmts) => desugar_stmts(stmts),
        ExprKind::Lambda { body, .. } => desugar_stmts(body),
        ExprKind::Match { subject, arms } => {
            lift_expr(subject, hoisted);
            // Arm bodies are conditional expressions; v1 does not lift into them.
            let _ = arms;
        }

        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::AtVar(_) => {}
    }

    // Post-order: this call's own `@`-arguments (nested calls already handled
    // theirs) are now visible. Exactly one → lift it.
    if let ExprKind::Call { .. } = &expr.kind {
        if count_call_atvars(expr) == 1 {
            let name = replace_one_call_atvar(expr).expect("count == 1");
            let span = expr.span;
            let call = std::mem::replace(
                expr,
                Expr {
                    kind: ExprKind::Ident(name.clone()),
                    span,
                },
            );
            hoisted.push(Stmt {
                kind: StmtKind::Assign {
                    target: AssignTarget::Name(name),
                    value: call,
                },
                span,
            });
        }
    }
}

/// Count `@`-arguments belonging directly to `call` — those reachable from its
/// callee and arguments without crossing another call or block boundary.
fn count_call_atvars(call: &Expr) -> usize {
    if let ExprKind::Call { function, args } = &call.kind {
        count_at(function) + args.iter().map(count_at).sum::<usize>()
    } else {
        0
    }
}

/// Count `AtVar`s in `e`, stopping at any call or block boundary (those own
/// their own markers).
fn count_at(e: &Expr) -> usize {
    match &e.kind {
        ExprKind::AtVar(_) => 1,
        ExprKind::Call { .. }
        | ExprKind::If { .. }
        | ExprKind::Block(_)
        | ExprKind::Lambda { .. }
        | ExprKind::Match { .. }
        | ExprKind::Ident(_)
        | ExprKind::Literal(_) => 0,
        ExprKind::BinaryOp { left, right, .. } => count_at(left) + count_at(right),
        ExprKind::UnaryOp { operand, .. } => count_at(operand),
        ExprKind::List(xs) => xs.iter().map(count_at).sum(),
        ExprKind::Record(fields) => fields
            .iter()
            .map(|f| match f {
                RecordField::Named(_, e) | RecordField::Spread(e) => count_at(e),
            })
            .sum(),
        ExprKind::FieldAccess { object, .. } => count_at(object),
        ExprKind::IndexAccess { object, index } => count_at(object) + count_at(index),
        ExprKind::StringInterp { exprs, .. } => exprs.iter().map(count_at).sum(),
        ExprKind::Element { props, children, .. } => {
            props.iter().map(|(_, e)| count_at(e)).sum::<usize>()
                + children
                    .iter()
                    .map(|c| match c {
                        JsxChild::Expr(e) => count_at(e),
                        JsxChild::Text(_) => 0,
                    })
                    .sum::<usize>()
        }
    }
}

/// Replace the single `@`-argument belonging to `call` with a plain reference,
/// returning its name.
fn replace_one_call_atvar(call: &mut Expr) -> Option<String> {
    if let ExprKind::Call { function, args } = &mut call.kind {
        if let Some(n) = replace_one_at(function) {
            return Some(n);
        }
        for a in args.iter_mut() {
            if let Some(n) = replace_one_at(a) {
                return Some(n);
            }
        }
    }
    None
}

/// Find the first `AtVar` in `e` (not crossing a call or block boundary),
/// rewrite it to a plain `Ident`, and return its name.
fn replace_one_at(e: &mut Expr) -> Option<String> {
    match &mut e.kind {
        ExprKind::AtVar(name) => {
            let name = std::mem::take(name);
            e.kind = ExprKind::Ident(name.clone());
            Some(name)
        }
        ExprKind::Call { .. }
        | ExprKind::If { .. }
        | ExprKind::Block(_)
        | ExprKind::Lambda { .. }
        | ExprKind::Match { .. }
        | ExprKind::Ident(_)
        | ExprKind::Literal(_) => None,
        ExprKind::BinaryOp { left, right, .. } => {
            replace_one_at(left).or_else(|| replace_one_at(right))
        }
        ExprKind::UnaryOp { operand, .. } => replace_one_at(operand),
        ExprKind::List(xs) => xs.iter_mut().find_map(replace_one_at),
        ExprKind::Record(fields) => fields.iter_mut().find_map(|f| match f {
            RecordField::Named(_, e) | RecordField::Spread(e) => replace_one_at(e),
        }),
        ExprKind::FieldAccess { object, .. } => replace_one_at(object),
        ExprKind::IndexAccess { object, index } => {
            replace_one_at(object).or_else(|| replace_one_at(index))
        }
        ExprKind::StringInterp { exprs, .. } => exprs.iter_mut().find_map(replace_one_at),
        ExprKind::Element { props, children, .. } => props
            .iter_mut()
            .find_map(|(_, e)| replace_one_at(e))
            .or_else(|| {
                children.iter_mut().find_map(|c| match c {
                    JsxChild::Expr(e) => replace_one_at(e),
                    JsxChild::Text(_) => None,
                })
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rewrite::parse_ast;

    fn desugared(src: &str) -> Vec<Stmt> {
        let (_, mut stmts) = parse_ast(src).expect("parse");
        desugar(&mut stmts);
        stmts
    }

    /// `f(@x)` at statement level → exactly `x = f(x)`: the residual read the
    /// generic rewrite leaves at the call site is dropped for a statement
    /// that was entirely the lifted call.
    #[test]
    fn flat_at_arg_becomes_exactly_the_assignment() {
        let stmts = desugared("double(@a)\n");
        assert_eq!(stmts.len(), 1);
        match &stmts[0].kind {
            StmtKind::Assign { target: AssignTarget::Name(n), value } => {
                assert_eq!(n, "a");
                // RHS is the call `double(a)` with the `@` stripped.
                assert!(matches!(&value.kind, ExprKind::Call { .. }));
            }
            other => panic!("expected `a = double(a)`, got {other:?}"),
        }
    }

    /// A user-written bare identifier statement is *not* a residual and must
    /// survive desugaring, even right after a statement that hoists onto the
    /// same variable.
    #[test]
    fn user_written_bare_ident_statement_survives() {
        let stmts = desugared("let a = 1\na\n");
        assert_eq!(stmts.len(), 2);
        assert!(matches!(
            &stmts[1].kind,
            StmtKind::Expr(e) if matches!(&e.kind, ExprKind::Ident(n) if n == "a")
        ));
    }

    /// `@` binds to the nearest enclosing call: `inc(double(@b))` hoists only
    /// `b = double(b)`, leaving `inc(b)` in place.
    #[test]
    fn nested_at_arg_binds_to_nearest_call() {
        let stmts = desugared("let r = inc(double(@b))\n");
        assert_eq!(stmts.len(), 2);
        match &stmts[0].kind {
            StmtKind::Assign { target: AssignTarget::Name(n), value } => {
                assert_eq!(n, "b");
                // The hoisted call is `double(...)`, not `inc(...)`.
                let ExprKind::Call { function, .. } = &value.kind else {
                    panic!("expected a call");
                };
                assert!(matches!(&function.kind, ExprKind::Ident(f) if f == "double"));
            }
            other => panic!("expected `b = double(b)`, got {other:?}"),
        }
        // `let r = inc(b)` — no `@` markers survive.
        assert!(matches!(&stmts[1].kind, StmtKind::Let { name, .. } if name == "r"));
        assert_eq!(count_atvars_in_stmt(&stmts[1]), 0);
    }

    /// Two `@`s in one call can't both receive the single result — the markers
    /// are left in place so compilation reports the error.
    #[test]
    fn multiple_at_args_in_one_call_are_left_unlifted() {
        let stmts = desugared("add(@a, @b)\n");
        assert_eq!(stmts.len(), 1);
        assert_eq!(count_atvars_in_stmt(&stmts[0]), 2);
    }

    /// An `@` outside any call has nothing to lift and is left in place.
    #[test]
    fn stray_at_var_is_left_unlifted() {
        let stmts = desugared("let b = @a + 1\n");
        assert_eq!(stmts.len(), 1);
        assert_eq!(count_atvars_in_stmt(&stmts[0]), 1);
    }

    /// `@` inside an `if` body hoists within that branch, not out of the `if`.
    #[test]
    fn at_arg_in_if_body_hoists_within_branch() {
        let stmts = desugared("if true then\n  double(@a)\nend\n");
        assert_eq!(stmts.len(), 1);
        let StmtKind::Expr(e) = &stmts[0].kind else {
            panic!("expected the `if` as an expression statement");
        };
        let ExprKind::If { then_body, .. } = &e.kind else {
            panic!("expected an `if`");
        };
        // The branch body became the hoisted assignment `a = double(a)`.
        assert_eq!(then_body.len(), 1);
        assert!(matches!(
            &then_body[0].kind,
            StmtKind::Assign { target: AssignTarget::Name(n), .. } if n == "a"
        ));
    }

    /// Test-only recursive count of surviving `AtVar` markers in a statement.
    fn count_atvars_in_stmt(stmt: &Stmt) -> usize {
        fn in_expr(e: &Expr) -> usize {
            match &e.kind {
                ExprKind::AtVar(_) => 1,
                ExprKind::Ident(_) | ExprKind::Literal(_) => 0,
                ExprKind::BinaryOp { left, right, .. } => in_expr(left) + in_expr(right),
                ExprKind::UnaryOp { operand, .. } => in_expr(operand),
                ExprKind::Call { function, args } => {
                    in_expr(function) + args.iter().map(in_expr).sum::<usize>()
                }
                ExprKind::List(xs) => xs.iter().map(in_expr).sum(),
                ExprKind::Record(fields) => fields
                    .iter()
                    .map(|f| match f {
                        RecordField::Named(_, e) | RecordField::Spread(e) => in_expr(e),
                    })
                    .sum(),
                ExprKind::FieldAccess { object, .. } => in_expr(object),
                ExprKind::IndexAccess { object, index } => in_expr(object) + in_expr(index),
                ExprKind::StringInterp { exprs, .. } => exprs.iter().map(in_expr).sum(),
                ExprKind::If { condition, then_body, else_body } => {
                    in_expr(condition)
                        + then_body.iter().map(in_stmt).sum::<usize>()
                        + match else_body {
                            Some(ElseBranch::Block(s)) => s.iter().map(in_stmt).sum(),
                            Some(ElseBranch::ElseIf(e)) => in_expr(e),
                            None => 0,
                        }
                }
                ExprKind::Block(s) => s.iter().map(in_stmt).sum(),
                ExprKind::Lambda { body, .. } => body.iter().map(in_stmt).sum(),
                ExprKind::Match { subject, .. } => in_expr(subject),
                ExprKind::Element { props, children, .. } => {
                    props.iter().map(|(_, e)| in_expr(e)).sum::<usize>()
                        + children
                            .iter()
                            .map(|c| match c {
                                JsxChild::Expr(e) => in_expr(e),
                                JsxChild::Text(_) => 0,
                            })
                            .sum::<usize>()
                }
            }
        }
        fn in_stmt(s: &Stmt) -> usize {
            match &s.kind {
                StmtKind::Let { value, .. } => in_expr(value),
                StmtKind::Assign { value, .. } => in_expr(value),
                StmtKind::Expr(e) => in_expr(e),
                StmtKind::Return(Some(e)) => in_expr(e),
                StmtKind::State { init, .. } => in_expr(init),
                StmtKind::For { iter, body, .. } => {
                    in_expr(iter) + body.iter().map(in_stmt).sum::<usize>()
                }
                StmtKind::While { condition, body } => {
                    in_expr(condition) + body.iter().map(in_stmt).sum::<usize>()
                }
                StmtKind::FnDecl { body, .. } => body.iter().map(in_stmt).sum(),
                _ => 0,
            }
        }
        in_stmt(stmt)
    }
}
