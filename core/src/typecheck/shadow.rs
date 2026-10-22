//! Names that silently replace one another.
//!
//! Petal has one namespace for values: a `let`, a `fn`, a class constructor,
//! an enum and a builtin are all just names in scope, and the later binding
//! wins without a word. One of those collisions is almost never what the
//! author meant, and it compiles and then computes the wrong thing:
//!
//! - **A value over a builtin** ([`check_shadowed_builtins`]). `state split =
//!   396` makes every later `split(text, ",")` in that scope a call to the
//!   number.
//!
//! A warning, in the house style of the rest of `typecheck/`.

use std::collections::HashMap;

use crate::ast::{
    self, ElseBranch, Expr, ExprKind, ExprVisitor, Literal, Param, Pattern, Stmt, StmtKind,
};
use crate::diagnostic::Diagnostic;
use crate::source_map::SourceSpan;

/// Report a call to a builtin's name that reaches a `let`/`var`/`state` of
/// that name instead, when the binding plainly holds a value that cannot be
/// called.
///
/// `is_builtin` answers for the natives this compile knows. The walk follows
/// the compiler's scoping: a binding is visible from its declaration to the end
/// of its block, a `fn` above the declaration still sees the builtin, and a
/// parameter, loop variable, pattern variable or nested `fn` of the same name
/// takes the name back. One diagnostic per declaration, at the first such call.
///
/// Only a declaration whose initializer is a literal, a list, a record, an
/// interpolated string or operator arithmetic counts. `let filter =
/// make_filter(cfg)` and `let map = fn(x) … end` are deliberate replacements
/// and say nothing.
pub fn check_shadowed_builtins(stmts: &[Stmt], is_builtin: &dyn Fn(&str) -> bool) -> Vec<Diagnostic> {
    let mut w = ShadowWalker {
        is_builtin,
        scopes: vec![HashMap::new()],
        diags: Vec::new(),
    };
    for stmt in stmts {
        w.walk_stmt(stmt);
    }
    w.diags
}

/// What a name in scope is bound to, as far as this pass cares.
enum Binding {
    /// A `let`/`var`/`state` holding a plain value, under a builtin's name.
    /// `reported` is set once a call through it has been warned about.
    Value {
        keyword: &'static str,
        declared: SourceSpan,
        reported: bool,
    },
    /// Anything else: a parameter, a `fn`, a binding that may be callable.
    Other,
}

struct ShadowWalker<'a> {
    is_builtin: &'a dyn Fn(&str) -> bool,
    scopes: Vec<HashMap<String, Binding>>,
    diags: Vec<Diagnostic>,
}

impl ShadowWalker<'_> {
    fn bind_other(&mut self, name: &str) {
        // Only names that are in play need tracking: a builtin's.
        if (self.is_builtin)(name) {
            self.scopes
                .last_mut()
                .expect("at least one scope")
                .insert(name.to_string(), Binding::Other);
        }
    }

    fn bind_value(&mut self, name: &str, keyword: &'static str, init: &Expr, at: SourceSpan) {
        if !(self.is_builtin)(name) {
            return;
        }
        let binding = if is_plain_value(init) {
            Binding::Value {
                keyword,
                declared: at,
                reported: false,
            }
        } else {
            Binding::Other
        };
        self.scopes
            .last_mut()
            .expect("at least one scope")
            .insert(name.to_string(), binding);
    }

    fn bind_params(&mut self, params: &[Param]) {
        for p in params {
            self.bind_other(&p.name);
        }
    }

    fn bind_pattern(&mut self, pattern: &Pattern) {
        for name in pattern.bound_names() {
            self.bind_other(&name);
        }
    }

    fn scoped(&mut self, f: impl FnOnce(&mut Self)) {
        self.scopes.push(HashMap::new());
        f(self);
        self.scopes.pop();
    }

    fn walk_block(&mut self, stmts: &[Stmt]) {
        self.scoped(|w| stmts.iter().for_each(|s| w.walk_stmt(s)));
    }

    fn walk_fn(&mut self, params: &[Param], body: &[Stmt]) {
        self.scoped(|w| {
            for default in params.iter().filter_map(|p| p.default.as_ref()) {
                w.walk_expr(default);
            }
            w.bind_params(params);
            body.iter().for_each(|s| w.walk_stmt(s));
        });
    }

    fn walk_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let {
                name,
                value,
                is_var,
                is_const,
                ..
            } => {
                self.walk_expr(value);
                let keyword = match (*is_var, *is_const) {
                    (true, _) => "var",
                    (false, true) => "const",
                    (false, false) => "let",
                };
                self.bind_value(name, keyword, value, stmt.span);
            }
            StmtKind::State {
                name, init, key, ..
            } => {
                self.walk_expr(init);
                if let Some(k) = key {
                    self.walk_expr(k);
                }
                self.bind_value(name, "state", init, stmt.span);
            }
            StmtKind::Memo {
                name, key, init, ..
            } => {
                self.walk_expr(key);
                self.walk_expr(init);
                self.bind_value(name, "memo", init, stmt.span);
            }
            StmtKind::FnDecl {
                name, params, body, ..
            } => {
                self.bind_other(name);
                self.walk_fn(params, body);
            }
            StmtKind::For { var, iter, key, body } => {
                self.walk_expr(iter);
                self.scoped(|w| {
                    w.bind_other(var);
                    if let Some(k) = key {
                        w.walk_expr(k);
                    }
                    body.iter().for_each(|s| w.walk_stmt(s));
                });
            }
            StmtKind::While { condition, body } => {
                self.walk_expr(condition);
                self.walk_block(body);
            }
            _ => ast::walk_stmt(self, stmt),
        }
    }

    fn walk_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Call { function, .. } => {
                if let ExprKind::Ident(name) = &function.kind {
                    self.note_call(name, expr.span);
                }
                ast::walk_expr(self, expr);
            }
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.walk_expr(condition);
                self.walk_block(then_body);
                match else_body {
                    Some(ElseBranch::Block(stmts)) => self.walk_block(stmts),
                    Some(ElseBranch::ElseIf(e)) => self.walk_expr(e),
                    None => {}
                }
            }
            ExprKind::Match { subject, arms } => {
                self.walk_expr(subject);
                for arm in arms {
                    self.scoped(|w| {
                        w.bind_pattern(&arm.pattern);
                        if let Some(g) = &arm.guard {
                            w.walk_expr(g);
                        }
                        w.walk_block(&arm.body);
                    });
                }
            }
            ExprKind::For { var, iter, key, body } => {
                self.walk_expr(iter);
                self.scoped(|w| {
                    w.bind_other(var);
                    if let Some(k) = key {
                        w.walk_expr(k);
                    }
                    body.iter().for_each(|s| w.walk_stmt(s));
                });
            }
            ExprKind::Lambda { params, body } => self.walk_fn(params, body),
            _ => ast::walk_expr(self, expr),
        }
    }

    /// A call written `name(…)`: warn if the innermost binding of `name` is a
    /// plain value standing where a builtin was.
    fn note_call(&mut self, name: &str, at: SourceSpan) {
        let Some(binding) = self.scopes.iter_mut().rev().find_map(|s| s.get_mut(name)) else {
            return;
        };
        let Binding::Value {
            keyword,
            declared,
            reported,
        } = binding
        else {
            return;
        };
        if std::mem::replace(reported, true) {
            return;
        }
        let declared = *declared;
        self.diags.push(Diagnostic::citing(
            at,
            format!(
                "`{name}` here is the `{keyword} {name}` declared on line {{line}}, which \
                 shadows the builtin `{name}`: this calls the value, not the builtin. Rename \
                 the `{keyword}`."
            ),
            declared,
        ));
    }
}

impl ExprVisitor for ShadowWalker<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        self.walk_expr(e);
    }

    fn visit_stmt(&mut self, s: &Stmt) {
        self.walk_stmt(s);
    }
}

/// Is this initializer plainly a value nobody can call? Anything that might
/// yield a function (a lambda, another name, a call, a field read, a branch)
/// answers no.
fn is_plain_value(init: &Expr) -> bool {
    match &init.kind {
        ExprKind::Literal(lit) => !matches!(lit, Literal::Nil),
        ExprKind::List(_) | ExprKind::Record(_) | ExprKind::StringInterp { .. } => true,
        ExprKind::UnaryOp { .. } => true,
        ExprKind::BinaryOp { op, .. } => !matches!(
            op,
            ast::BinOp::And | ast::BinOp::Or | ast::BinOp::Coalesce
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<Stmt> {
        let (_, mut stmts) = crate::rewrite::parse_ast(src).expect("parse");
        crate::desugar::desugar(&mut stmts);
        stmts
    }

    fn shadow_msgs(src: &str) -> Vec<String> {
        let builtin = |n: &str| matches!(n, "split" | "len" | "map" | "print");
        check_shadowed_builtins(&parse(src), &builtin)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn calling_a_builtin_under_a_value_binding_warns_once() {
        let m = shadow_msgs(
            "state split = 396\nfn parts(t)\n  split(t, \",\")\nend\nprint(split(\"a\", \"b\"))",
        );
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("`state split` declared on line 1"), "{m:?}");
        assert!(m[0].contains("shadows the builtin `split`"), "{m:?}");
        assert_eq!(shadow_msgs("fn f(t)\n  let len = 3\n  len(t)\nend").len(), 1);
    }

    #[test]
    fn shadowing_that_is_not_called_or_not_in_scope_is_silent() {
        // Never called.
        assert!(shadow_msgs("let len = 3\nprint(len)").is_empty());
        // A `fn` above the declaration still sees the builtin.
        assert!(shadow_msgs("fn f(t)\n  split(t, \",\")\nend\nlet split = 1").is_empty());
        // Out of scope once its block ends.
        assert!(
            shadow_msgs("fn f(t)\n  if true then\n    let split = 1\n  end\n  split(t, \",\")\nend")
                .is_empty()
        );
        // A parameter or loop variable takes the name back.
        assert!(shadow_msgs("let map = 3\nfn f(map, x)\n  map(x)\nend").is_empty());
        // Deliberate replacements: something that may be callable.
        assert!(shadow_msgs("let map = fn(x)\n  x\nend\nmap(1)").is_empty());
        assert!(shadow_msgs("fn mk()\n  print\nend\nlet map = mk()\nmap(1)").is_empty());
        // Not a builtin's name at all.
        assert!(shadow_msgs("let total = 3\ntotal(1)").is_empty());
    }
}
