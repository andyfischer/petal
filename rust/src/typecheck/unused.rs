//! Unused-result lint: warn when a **side-effect-free** call's return value is
//! discarded, so the call does nothing.
//!
//! The motivating case is the value-semantics migration footgun: `push(xs, x)`
//! and `append(xs, x)` return a *new* list and mutate nothing, so the
//! statement form `push(xs, x)` (result thrown away) silently accumulates
//! nothing — a list-building loop stays empty with no error. This pass turns
//! that into a compile-time warning pointing at the exact call.
//!
//! Precision over recall, by construction:
//! - Only calls to a fixed set of **known-pure builtins**
//!   ([`crate::builtins::is_pure_builtin`]) warn. Effectful natives (`print`,
//!   `draw_*`, `random`, `assert`, host input readers) and higher-order
//!   builtins that run a user closure (`map`, `filter`, `reduce`, `forEach`)
//!   are never in the set, so they never warn.
//! - A name shadowed by a local binding, or defined as a user `fn`, is treated
//!   as user code of unknown effect and never warns.
//! - Only *discarded* positions warn: a non-tail statement, or a block tail
//!   whose block value is itself discarded (`for`/`while` bodies, a discarded
//!   `if`/`match`/block). A value that flows into a `let`, an argument, a
//!   `return`, or a used block tail is left alone.

use std::collections::HashSet;

use crate::ast::{self, AssignTarget, ElseBranch, Expr, ExprKind, ExprVisitor, Stmt, StmtKind};
use crate::builtins::{is_pure_builtin, looks_mutating};
use crate::diagnostic::Diagnostic;

/// Walk a module's statements and report each discarded pure-builtin call.
pub fn check_unused(stmts: &[Stmt]) -> Vec<Diagnostic> {
    let mut w = Walker {
        user_fns: HashSet::new(),
        scopes: vec![HashSet::new()],
        diags: Vec::new(),
    };
    collect_fn_names(stmts, &mut w.user_fns);
    // The top-level program's final expression is its result value — treat the
    // module block's value as used so a script's trailing expression is fine.
    w.walk_block(stmts, true);
    w.diags
}

struct Walker {
    /// Every `fn` name declared anywhere in the module. A call to one of these
    /// is user code of unknown effect, so it never warns even if it collides
    /// with a builtin name.
    user_fns: HashSet<String>,
    /// Locally bound names (let / params / loop var / state), innermost last.
    /// A bound name shadows a builtin of the same name.
    scopes: Vec<HashSet<String>>,
    diags: Vec<Diagnostic>,
}

impl Walker {
    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind(&mut self, name: &str) {
        self.scopes
            .last_mut()
            .expect("at least one scope")
            .insert(name.to_string());
    }

    fn is_locally_bound(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    /// Would a call to `name` be a discarded pure builtin (not shadowed, not a
    /// user function)?
    fn is_discardable_call(&self, name: &str) -> bool {
        is_pure_builtin(name) && !self.user_fns.contains(name) && !self.is_locally_bound(name)
    }

    /// Walk a block whose overall value is used (`block_used`) or discarded.
    /// The last statement's expression inherits `block_used`; every earlier
    /// statement is in discarded position.
    fn walk_block(&mut self, stmts: &[Stmt], block_used: bool) {
        self.push_scope();
        let last = stmts.len().wrapping_sub(1);
        for (i, stmt) in stmts.iter().enumerate() {
            match &stmt.kind {
                StmtKind::Expr(e) => {
                    let used = i == last && block_used;
                    self.walk_expr(e, used);
                }
                _ => self.walk_stmt(stmt),
            }
        }
        self.pop_scope();
    }

    fn walk_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, value, .. } => {
                self.walk_expr(value, true);
                self.bind(name);
            }
            StmtKind::State { name, init, key, .. } => {
                self.walk_expr(init, true);
                if let Some(k) = key {
                    self.walk_expr(k, true);
                }
                self.bind(name);
            }
            StmtKind::Assign { target, value } => {
                match target {
                    AssignTarget::Name(_) => {}
                    AssignTarget::Field(obj, _) => self.walk_expr(obj, true),
                    AssignTarget::Index(obj, idx) => {
                        self.walk_expr(obj, true);
                        self.walk_expr(idx, true);
                    }
                }
                self.walk_expr(value, true);
            }
            StmtKind::Expr(e) => self.walk_expr(e, false),
            StmtKind::FnDecl { params, body, .. } => {
                self.push_scope();
                for p in params {
                    self.bind(&p.name);
                }
                // A function body's tail is its return value — used.
                self.walk_block(body, true);
                self.pop_scope();
            }
            StmtKind::For { var, iter, body } => {
                self.walk_expr(iter, true);
                self.push_scope();
                self.bind(var);
                // Statement-form loop: the body runs for side effects and
                // collects nothing, so its tail value is discarded.
                self.walk_block(body, false);
                self.pop_scope();
            }
            StmtKind::While { condition, body } => {
                self.walk_expr(condition, true);
                self.walk_block(body, false);
            }
            StmtKind::Return(value) => {
                if let Some(e) = value {
                    self.walk_expr(e, true);
                }
            }
            StmtKind::EnumDecl { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import(_) => {}
        }
    }

    /// Walk an expression whose value is used (`used`) or discarded. When
    /// discarded and the expression is a pure-builtin call, warn.
    fn walk_expr(&mut self, expr: &Expr, used: bool) {
        if !used {
            if let ExprKind::Call { function, args: _ } = &expr.kind {
                if let ExprKind::Ident(name) = &function.kind {
                    if self.is_discardable_call(name) {
                        self.warn_discarded(expr, name);
                    }
                }
            }
        }
        // Descend into children, tracking used-ness so nested discarded pure
        // calls are caught too. Only the nodes that propagate used-ness or open
        // a scope need custom handling; everything else evaluates its children
        // in value position, which is exactly the default total walk.
        match &expr.kind {
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.walk_expr(condition, true);
                self.walk_block(then_body, used);
                match else_body {
                    Some(ElseBranch::Block(stmts)) => self.walk_block(stmts, used),
                    Some(ElseBranch::ElseIf(e)) => self.walk_expr(e, used),
                    None => {}
                }
            }
            ExprKind::Match { subject, arms } => {
                self.walk_expr(subject, true);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.walk_expr(g, true);
                    }
                    self.walk_expr(&arm.body, used);
                }
            }
            ExprKind::For { var, iter, body } => {
                self.walk_expr(iter, true);
                self.push_scope();
                self.bind(var);
                // Value-form loop: each iteration's tail becomes a list element,
                // so the body tail is used iff the loop's own value is used.
                self.walk_block(body, used);
                self.pop_scope();
            }
            ExprKind::Block(stmts) => self.walk_block(stmts, used),
            ExprKind::Lambda { params, body } => {
                self.push_scope();
                for p in params {
                    self.bind(&p.name);
                }
                self.walk_block(body, true);
                self.pop_scope();
            }
            _ => ast::walk_expr(self, expr),
        }
    }

    fn warn_discarded(&mut self, call: &Expr, name: &str) {
        let message = if looks_mutating(name) {
            format!(
                "result of `{name}` is discarded, so this call does nothing — \
                 `{name}` returns a new value and never mutates its argument. \
                 Capture it, e.g. `xs = {name}(xs, …)`."
            )
        } else {
            format!("result of `{name}` is discarded, so this call has no effect.")
        };
        self.diags.push(Diagnostic {
            span: call.span,
            message,
        });
    }
}

/// The delegated half of [`Walker::walk_expr`]: nodes with no used-ness policy
/// of their own evaluate every child in value position, so the default walk
/// visits them all as used.
impl ExprVisitor for Walker {
    fn visit_expr(&mut self, e: &Expr) {
        self.walk_expr(e, true);
    }

    fn visit_stmt(&mut self, s: &Stmt) {
        self.walk_stmt(s);
    }
}

/// Collect every `fn` name declared anywhere in `stmts` — lambdas declare no
/// name, so only `FnDecl` contributes. The traversal is total, so a `fn` nested
/// anywhere a statement can appear (a lambda passed as a call argument, a block
/// inside a list element, …) is still found.
fn collect_fn_names(stmts: &[Stmt], out: &mut HashSet<String>) {
    struct Collector<'a> {
        names: &'a mut HashSet<String>,
    }
    impl ExprVisitor for Collector<'_> {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::FnDecl { name, .. } = &s.kind {
                self.names.insert(name.clone());
            }
            ast::walk_stmt(self, s);
        }
    }
    let mut c = Collector { names: out };
    for stmt in stmts {
        c.visit_stmt(stmt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parse::Parser;

    fn messages(src: &str) -> Vec<String> {
        let mut lexer = Lexer::new(src);
        lexer.tokenize().expect("tokenize");
        let mut parser = Parser::new(lexer.tokens.clone(), lexer.token_spans.clone());
        let stmts = parser.parse_program().expect("parse");
        check_unused(&stmts)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn statement_form_push_warns_with_capture_hint() {
        let m = messages("state xs = []\nfor i in range(0, 3) do\n  push(xs, i)\nend");
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("`push`"));
        assert!(m[0].contains("xs = push"));
    }

    #[test]
    fn captured_append_is_silent() {
        assert!(messages("let a = [1]\na = append(a, 2)\nprint(len(a))").is_empty());
    }

    #[test]
    fn discarded_append_in_loop_warns() {
        let m = messages("let a = []\nfor i in range(0, 3) do\n  append(a, i)\nend");
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("`append`"));
    }

    #[test]
    fn effectful_calls_are_silent() {
        // print and random advance observable state — never flagged.
        assert!(messages("print(\"hi\")\nlet r = random(0.0, 1.0)\nr").is_empty());
    }

    #[test]
    fn user_fn_shadowing_a_builtin_is_silent() {
        assert!(messages("fn push(a, b)\n  print(\"fx\")\n  a\nend\npush([1], 2)").is_empty());
    }

    #[test]
    fn user_fn_declared_under_a_list_element_is_silent() {
        // `push` is declared inside an `if` that is a list element — a spot the
        // fn-name collection only reaches with a total walk. Missing it would
        // make the later `push` call look like the builtin and warn.
        let m = messages(
            "let xs = [if true then\n  fn push(a, b)\n    print(\"fx\")\n    a\n  end\n  0\nend]\n\
             push([1], 2)\nprint(len(xs))",
        );
        assert!(m.is_empty(), "unexpected warnings: {m:?}");
    }

    #[test]
    fn local_shadowing_a_builtin_is_silent() {
        // `len` bound to a value here is not the builtin.
        assert!(messages("let len = 3\nlen").is_empty());
    }

    #[test]
    fn pure_builtin_as_program_tail_is_silent() {
        assert!(messages("let a = [1]\nappend(a, 3)").is_empty());
    }

    #[test]
    fn value_position_for_collecting_is_silent() {
        assert!(
            messages("let ys = for i in range(0, 3) do\n  append([], i)\nend\nprint(len(ys))")
                .is_empty()
        );
    }

    #[test]
    fn discarded_pure_call_in_if_branch_warns() {
        // A non-tail `if` is in statement position (value discarded), so its
        // branch tail is discarded too. The trailing print keeps the `if` off
        // the program tail (whose value would count as used).
        let m = messages("let a = [1]\nif true then\n  append(a, 2)\nend\nprint(\"done\")");
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("`append`"));
    }

    #[test]
    fn pure_call_in_used_if_branch_is_silent() {
        // Here the `if` value flows into a `let`, so both branch tails are used.
        let m = messages(
            "let a = [1]\nlet b = if true then\n  append(a, 2)\nelse\n  a\nend\nprint(len(b))",
        );
        assert!(m.is_empty(), "unexpected warnings: {m:?}");
    }

    #[test]
    fn non_mutating_pure_builtin_uses_plain_message() {
        let m = messages("let x = 4.0\nsqrt(x)\nx");
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("`sqrt`"));
        assert!(m[0].contains("no effect"));
        assert!(!m[0].contains("Capture"));
    }
}
