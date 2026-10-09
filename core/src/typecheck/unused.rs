//! Unused-result lint: warn when a **side-effect-free** call's return value is
//! discarded, so the call does nothing.
//!
//! The motivating case is the value-semantics migration footgun: `push(xs, x)`
//! and `append(xs, x)` return a *new* list and mutate nothing, so the
//! statement form `push(xs, x)` (result thrown away) silently accumulates
//! nothing — a list-building loop stays empty with no error. This pass turns
//! that into a compile-time warning pointing at the exact call.
//!
//! Two more findings ride on the same used/discarded walk, both about a line
//! whose value silently goes nowhere:
//! - a **bare computation** in statement position (`n + 1`, `x == 5`), see
//!   [`is_bare_computation`];
//! - a line that **starts with `-`** under a line it was meant to continue, see
//!   [`is_broken_continuation`]. A leading `-` starts a new statement, so a sum
//!   written down the page returns only its last line.
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
//!   whose block value is itself discarded (a side-effect `for`/`while` body, a
//!   discarded `if`/`match`/block). A value that flows into a `let`, an
//!   argument, a `return`, or a used block tail is left alone.
//! - A **collecting** `for` uses its body's tail, so that tail never warns.
//!   This holds for the expression form (`let ys = for … end`) and equally for
//!   the statement form in tail position (`fn f() for x in xs do g(x) end end`),
//!   which the compiler also collects. The rule tracks
//!   `Compiler::compile_stmts`'s `value_used`, including its module-level
//!   exception: a trailing `for` at the top level of a file does not collect.

use std::collections::HashSet;

use crate::ast::{self, AssignTarget, ElseBranch, Expr, ExprKind, ExprVisitor, Stmt, StmtKind};
use crate::builtins::{is_pure_builtin, looks_mutating};
use crate::diagnostic::{Diagnostic, LayoutDep};

/// Walk a module's statements and report each discarded pure-builtin call.
pub fn check_unused(stmts: &[Stmt]) -> Vec<Diagnostic> {
    check_unused_with_layout(stmts).0
}

/// [`check_unused`], plus every pair of statements whose layout the
/// broken-continuation rule compared (see [`LayoutDep`]): the one check here
/// whose answer depends on whitespace.
pub fn check_unused_with_layout(stmts: &[Stmt]) -> (Vec<Diagnostic>, Vec<LayoutDep>) {
    let mut w = Walker {
        user_fns: HashSet::new(),
        scopes: vec![HashSet::new()],
        diags: Vec::new(),
        layout: Vec::new(),
    };
    collect_fn_names(stmts, &mut w.user_fns);
    // The top-level program's final expression is its result value — treat the
    // module block's value as used so a script's trailing expression is fine.
    // A trailing `for` is the exception: the compiler only makes a tail loop
    // collect inside a function body, a used branch or a collecting loop, never
    // at module level, so the module's tail loop still runs for side effects.
    w.walk_block_tail(stmts, true, false);
    (w.diags, w.layout)
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
    /// Statement pairs [`is_broken_continuation`] read the layout of.
    layout: Vec<LayoutDep>,
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
        self.walk_block_tail(stmts, block_used, block_used);
    }

    /// [`Self::walk_block`], with the tail-`for` rule split out from the
    /// tail-expression rule. They agree everywhere except the module block,
    /// whose trailing expression is its result but whose trailing `for` does
    /// not collect — mirroring `Compiler::compile_stmts`'s `value_used`.
    fn walk_block_tail(&mut self, stmts: &[Stmt], block_used: bool, for_collects: bool) {
        self.push_scope();
        let last = stmts.len().wrapping_sub(1);
        let mut reported = false;
        for (i, stmt) in stmts.iter().enumerate() {
            let tail = i == last;
            // The `-` line itself was reported on the previous turn.
            let was_reported = std::mem::take(&mut reported);
            // A line that starts with `-` under an unfinished-looking line:
            // report the `-` line once, and say nothing more about the line
            // above it (its discarded value is the same mistake).
            let continued = stmts.get(i + 1).is_some_and(|next| {
                // The rule compares the two statements' lines and columns
                // exactly when the second starts with a negation.
                if matches!(&next.kind, StmtKind::Expr(e) if starts_with_negation(e)) {
                    self.layout.push(LayoutDep {
                        prev: stmt.span,
                        next: next.span,
                    });
                }
                is_broken_continuation(stmt, next)
            });
            if continued {
                self.warn_broken_continuation(&stmts[i + 1]);
                reported = true;
            }
            let quiet = continued || was_reported;
            match &stmt.kind {
                StmtKind::Expr(e) => self.walk_expr(e, quiet || (tail && block_used)),
                // A `for` in the tail of a collecting block collects, exactly
                // as the expression form does: the loop parses as a statement
                // (`fn f() for x in xs do g(x) end end`), but tail position
                // makes its value the block's value, so each iteration's tail
                // becomes a list element rather than being thrown away.
                StmtKind::For { var, iter, body } if tail && for_collects => {
                    self.walk_expr(iter, true);
                    self.push_scope();
                    self.bind(var);
                    self.walk_block(body, true);
                    self.pop_scope();
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
            StmtKind::State {
                name, init, key, ..
            } => {
                self.walk_expr(init, true);
                if let Some(k) = key {
                    self.walk_expr(k, true);
                }
                self.bind(name);
            }
            StmtKind::Assign { target, value } | StmtKind::Set { target, value } => {
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
                // A default value that has not been moved into the body yet
                // (this pass also runs on an un-desugared tree).
                for default in params.iter().filter_map(|p| p.default.as_ref()) {
                    self.walk_expr(default, true);
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
            | StmtKind::ClassDecl { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import(_) => {}
        }
    }

    /// Walk an expression whose value is used (`used`) or discarded. When
    /// discarded and the expression is a pure-builtin call, warn.
    fn walk_expr(&mut self, expr: &Expr, used: bool) {
        if !used {
            if let ExprKind::Call { function, .. } = &expr.kind {
                if let ExprKind::Ident(name) = &function.kind {
                    if self.is_discardable_call(name) {
                        self.warn_discarded(expr, name);
                    }
                }
            }
            if is_bare_computation(expr) {
                self.diags.push(Diagnostic::new(
                    expr.span,
                    "the value of this expression is discarded, so the line has no effect. \
                     Bind it (`let x = …`), or remove the line."
                        .to_string(),
                ));
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
                for default in params.iter().filter_map(|p| p.default.as_ref()) {
                    self.walk_expr(default, true);
                }
                self.walk_block(body, true);
                self.pop_scope();
            }
            _ => ast::walk_expr(self, expr),
        }
    }

    fn warn_broken_continuation(&mut self, stmt: &Stmt) {
        self.diags.push(Diagnostic::new(
            stmt.span,
            "a line that starts with `-` is a new statement (a negation), not a continuation \
             of the line above, so the two are not subtracted. To continue the expression, end \
             the line above with the `-`."
                .to_string(),
        ));
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
        self.diags.push(Diagnostic::new(call.span, message));
    }
}

/// An operator expression with nothing in it that could do work: arithmetic,
/// comparison, `++` or a negation over operands that are themselves bare
/// (names, literals, field and index reads). In statement position its value
/// goes nowhere, so the line is dead — `x == 5` meant as `x = 5`, `n + 1` meant
/// as `n += 1`.
///
/// The short-circuit operators are left out because `ok and report()` is
/// control flow, and anything containing a call is left out because the call
/// may be the point of the line.
fn is_bare_computation(expr: &Expr) -> bool {
    fn bare(e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::CellGet(_) => true,
            ExprKind::FieldAccess { object, .. } => bare(object),
            ExprKind::IndexAccess { object, index } => bare(object) && bare(index),
            ExprKind::UnaryOp { operand, .. } => bare(operand),
            ExprKind::BinaryOp { op, left, right } => {
                !is_short_circuit(*op) && bare(left) && bare(right)
            }
            _ => false,
        }
    }
    match &expr.kind {
        ExprKind::BinaryOp { .. } | ExprKind::UnaryOp { .. } => bare(expr),
        _ => false,
    }
}

fn is_short_circuit(op: ast::BinOp) -> bool {
    matches!(op, ast::BinOp::And | ast::BinOp::Or | ast::BinOp::Coalesce)
}

/// Does the first token of `expr` negate? True for `-x`, and for anything whose
/// leftmost operand is one: `-c * 4`, `-v.x`, `-f(x) + 1`.
fn starts_with_negation(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::UnaryOp {
            op: ast::UnaryOp::Neg,
            ..
        } => true,
        ExprKind::BinaryOp { left, .. } => starts_with_negation(left),
        ExprKind::Call { function, .. } => starts_with_negation(function),
        ExprKind::FieldAccess { object, .. } | ExprKind::IndexAccess { object, .. } => {
            starts_with_negation(object)
        }
        ExprKind::OptionalAccess(inner) => starts_with_negation(inner),
        _ => false,
    }
}

/// Could a `-` on the next line have been meant to continue `expr`? Not when
/// it closes with its own `end` (or is a lambda or element): nobody subtracts
/// from an `if … end` by accident.
fn can_be_continued(expr: &Expr) -> bool {
    !matches!(
        expr.kind,
        ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::For { .. }
            | ExprKind::Block(_)
            | ExprKind::Lambda { .. }
            | ExprKind::Element { .. }
    )
}

/// Is `next` a line that starts with `-` where the author almost certainly
/// meant to go on subtracting from `prev`?
///
/// Every other binary operator continues the line above when it leads a line;
/// `-` cannot, because `-x` is also a whole expression, so the parser starts a
/// new statement and the sum written down the page silently loses its top.
/// Two shapes give the mistake away:
///
/// - the line above is an expression statement that does nothing by itself (a
///   name, a product, a field read): its value is thrown away, and only a
///   continuation would have used it;
/// - the `-` line is indented deeper than the statement above it, which is how
///   a continuation is laid out and never how a new statement is. This is the
///   only evidence accepted under a `let`, an assignment, a `return` or a call,
///   where `let d = a - b` followed by `-d` is an ordinary function body.
fn is_broken_continuation(prev: &Stmt, next: &Stmt) -> bool {
    let StmtKind::Expr(e) = &next.kind else {
        return false;
    };
    if !starts_with_negation(e) || next.span.start.line <= prev.span.start.line {
        return false;
    }
    let indented = next.span.start.column > prev.span.start.column;
    match &prev.kind {
        StmtKind::Expr(p) => {
            can_be_continued(p) && (indented || !matches!(p.kind, ExprKind::Call { .. }))
        }
        StmtKind::Let { value, .. }
        | StmtKind::State { init: value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Set { value, .. }
        | StmtKind::Return(Some(value)) => indented && can_be_continued(value),
        _ => false,
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
    fn leading_minus_continuation_warns_once_on_the_minus_line() {
        let m = messages("fn score(a, b, c)\n  a * 2\n    + b * 3\n    - c * 4\nend\nprint(score(1, 2, 3))");
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("starts with `-`"), "{m:?}");
        // Under a bare name, with no extra indentation.
        let m = messages("fn f(a, b)\n  a\n  - b\nend\nprint(f(1, 2))");
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("starts with `-`"), "{m:?}");
    }

    #[test]
    fn leading_minus_under_a_let_or_call_needs_deeper_indentation() {
        let w = "a line that starts with `-`";
        let m = messages("fn f(a, b)\n  let x = a\n    - b\n  x\nend\nprint(f(1, 2))");
        assert!(m.len() == 1 && m[0].contains(w), "{m:?}");
        let m = messages("fn f(a, b)\n  return a\n    - b\nend\nprint(f(1, 2))");
        assert!(m.len() == 1 && m[0].contains(w), "{m:?}");
        let m = messages("fn f(a, b)\n  g(a)\n    - g(b)\nend\nfn g(x)\n  x\nend\nprint(f(1, 2))");
        assert!(m.len() == 1 && m[0].contains(w), "{m:?}");
        // The ordinary shapes: a negated tail after a `let`, a call, a block.
        assert!(messages("fn f(a, b)\n  let d = a - b\n  -d\nend\nprint(f(1, 2))").is_empty());
        assert!(messages("fn f(a)\n  print(a)\n  -a\nend\nprint(f(1))").is_empty());
        assert!(
            messages("fn f(a)\n  if a > 0 then\n    print(a)\n  end\n  -a\nend\nprint(f(1))")
                .is_empty()
        );
    }

    #[test]
    fn discarded_bare_computation_warns() {
        let m = messages("let n = 1\nn + 1\nprint(n)");
        assert!(m.len() == 1 && m[0].contains("value of this expression is discarded"), "{m:?}");
        assert_eq!(messages("fn f(x)\n  x == 5\n  x\nend\nprint(f(1))").len(), 1);
        // A value that is used, a short-circuit, and a line with a call in it.
        assert!(messages("let n = 1\nn + 1").is_empty());
        assert!(messages("fn f(x)\n  x + 1\nend\nprint(f(1))").is_empty());
        assert!(messages("let ok = true\nok and print(1)\nprint(2)").is_empty());
        assert!(messages("fn g()\n  1\nend\ng() + 1\nprint(2)").is_empty());
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
    fn tail_for_in_fn_body_collects_and_is_silent() {
        // Regression: the statement-form `for` in a function body's tail is a
        // *collecting* loop — `mirror([[1, 2]])` really is `[[2, 1]]` — so its
        // body tail is used and must not warn. The lint used to treat every
        // `StmtKind::For` body as discarded and flagged this.
        let m = messages(
            "fn mirror(g)\n  for row in g do\n    reverse(row)\n  end\nend\nprint(mirror([[1, 2]]))",
        );
        assert!(m.is_empty(), "unexpected warnings: {m:?}");
    }

    #[test]
    fn tail_for_in_used_if_branch_is_silent() {
        // The `if` value flows into a `let`, so its branch tail collects too.
        let m = messages(
            "fn rows(g, on)\n  if on then\n    for row in g do\n      reverse(row)\n    end\n  else\n    []\n  end\nend\nprint(rows([[1, 2]], true))",
        );
        assert!(m.is_empty(), "unexpected warnings: {m:?}");
    }

    #[test]
    fn tail_for_in_match_arm_is_silent() {
        let m = messages(
            "fn rows(g, n)\n  match n\n    when 0 -> []\n    when _ do\n      for row in g do\n        reverse(row)\n      end\n    end\n  end\nend\nprint(rows([[1, 2]], 1))",
        );
        assert!(m.is_empty(), "unexpected warnings: {m:?}");
    }

    #[test]
    fn nested_tail_for_is_silent() {
        // The inner loop is the outer collecting loop's body tail, so it
        // collects as well — one list per outer iteration.
        let m = messages(
            "fn grid(g)\n  for row in g do\n    for cell in row do\n      reverse(cell)\n    end\n  end\nend\nprint(grid([[[1, 2]]]))",
        );
        assert!(m.is_empty(), "unexpected warnings: {m:?}");
    }

    #[test]
    fn non_tail_for_in_fn_body_still_warns() {
        // The `nil` puts the loop off the tail, so it is a side-effect loop
        // again and the discarded `append` is a real bug.
        let m = messages(
            "fn f(xs)\n  let a = []\n  for i in xs do\n    append(a, i)\n  end\n  nil\nend\nprint(f([1]))",
        );
        assert_eq!(m.len(), 1, "expected exactly one warning, got {m:?}");
        assert!(m[0].contains("`append`"));
    }

    #[test]
    fn trailing_for_at_module_level_still_warns() {
        // Module scope is the one place a tail `for` does *not* collect (see
        // `Compiler::compile_stmts`), so the top-level loop stays a
        // side-effect loop and the discarded `append` must still be reported.
        let m = messages("let a = []\nfor i in range(0, 3) do\n  append(a, i)\nend");
        assert_eq!(m.len(), 1, "expected exactly one warning, got {m:?}");
        assert!(m[0].contains("`append`"));
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
