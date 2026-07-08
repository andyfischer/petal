//! Pass 2 — rebind (`x = f(x)` → `f(@x)`).

use crate::ast::{self, AssignTarget, Expr, ExprKind, ExprVisitor, Stmt, StmtKind};

/// One rebind rewrite, as two minimal char-offset edits: delete the
/// `x = ` prefix `[prefix_start, prefix_end)` and insert `@` at `at_pos`
/// (the start of the matching argument). No reprinting, so comments and
/// layout inside the call survive.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rebind {
    prefix_start: usize,
    prefix_end: usize,
    at_pos: usize,
}

/// Detect rebind candidates. A statement `x = f(a, …)` qualifies when:
/// - the target is a plain name and the value is a call whose callee is a
///   plain identifier (not `x` itself, and not a field/method chain),
/// - exactly one argument is exactly `Ident(x)`,
/// - `x` appears nowhere else in the whole value (no `x = g(x, x)` ambiguity —
///   this also rejects `x` captured in nested lambdas/blocks, conservatively),
/// - and the value contains no `@` already (a second marker on the same call
///   would stop the desugarer from lifting either).
///
/// The walk mirrors [`crate::desugar`]'s recursion *exactly*: candidates are
/// only collected in statement scopes the desugarer lifts `@` from (top level,
/// fn/for/while/if/block/lambda bodies) — never inside match arms or `while`
/// conditions, where an `@` would survive to the compiler as an error.
///
/// The desugarer rewrites statement-level `f(@x)` back to exactly `x = f(x)`,
/// so the rewrite is semantics-preserving by construction; [`crate::lint::lint_source`]
/// verifies it against the compiled IR anyway.
pub(super) fn find_rebinds(stmts: &[Stmt], chars: &[char]) -> Vec<Rebind> {
    let mut out = Vec::new();
    let mut finder = Rebinder { chars, out: &mut out };
    for stmt in stmts {
        finder.visit_stmt(stmt);
    }
    out
}

/// Walks the statement scopes the desugarer lifts `@` from, collecting rebind
/// candidates. Its traversal mirrors [`crate::desugar`] exactly by overriding
/// the two nodes where the desugarer stops short of a total walk: a `while`
/// condition (re-evaluated each iteration) and `match` arms (conditional
/// bodies) are never lifted into, so no candidates are collected there.
struct Rebinder<'a> {
    chars: &'a [char],
    out: &'a mut Vec<Rebind>,
}

impl ExprVisitor for Rebinder<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let StmtKind::Assign { target: AssignTarget::Name(x), value } = &stmt.kind
            && let Some(rebind) = rebind_candidate(stmt, x, value, self.chars)
        {
            self.out.push(rebind);
        }
        match &stmt.kind {
            StmtKind::Let { value, .. }
            | StmtKind::Expr(value)
            | StmtKind::Return(Some(value)) => self.visit_expr(value),
            StmtKind::Assign { target, value } => {
                self.visit_expr(value);
                match target {
                    AssignTarget::Name(_) => {}
                    AssignTarget::Field(obj, _) => self.visit_expr(obj),
                    AssignTarget::Index(obj, idx) => {
                        self.visit_expr(obj);
                        self.visit_expr(idx);
                    }
                }
            }
            StmtKind::State { init, key, .. } => {
                self.visit_expr(init);
                if let Some(k) = key {
                    self.visit_expr(k);
                }
            }
            StmtKind::FnDecl { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            // The desugarer never lifts from a `while` condition (it
            // re-evaluates each iteration), so recurse the body only.
            StmtKind::While { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::Return(None)
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::EnumDecl { .. }
            | StmtKind::Import(_) => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        // The desugarer lifts only the subject of a `match`, not the arms.
        if let ExprKind::Match { subject, .. } = &expr.kind {
            self.visit_expr(subject);
            return;
        }
        ast::walk_expr(self, expr);
    }
}

/// Check one `x = value` assignment against the rebind rule, with textual
/// sanity checks so a surprising span can only ever make us *skip*, never
/// produce a wrong edit.
fn rebind_candidate(stmt: &Stmt, x: &str, value: &Expr, chars: &[char]) -> Option<Rebind> {
    let ExprKind::Call { function, args } = &value.kind else { return None };
    // Callee must be a plain identifier other than `x` — a field/method chain
    // could evaluate `x` on its own, which the occurrence count below would
    // catch, but a plain-ident callee is also what the `@` sugar reads best on.
    let ExprKind::Ident(callee) = &function.kind else { return None };
    if callee == x {
        return None;
    }
    if count_ident(value, x) != 1 {
        return None;
    }
    // An `@` already present anywhere in the value could end up as a second
    // marker on the same call, which the desugarer refuses to lift.
    if contains_atvar(value) {
        return None;
    }
    let matching: Vec<&Expr> = args
        .iter()
        .filter(|a| matches!(&a.kind, ExprKind::Ident(n) if n == x))
        .collect();
    let [arg] = matching[..] else { return None };

    let prefix_start = stmt.span.start.offset as usize;
    let prefix_end = value.span.start.offset as usize;
    let at_pos = arg.span.start.offset as usize;
    if prefix_start >= prefix_end || at_pos < prefix_end || at_pos >= chars.len() {
        return None;
    }
    // The deleted prefix must be exactly `x`, `=`, and whitespace — same line.
    let prefix: String = chars[prefix_start..prefix_end].iter().collect();
    let rest = prefix.strip_prefix(x)?;
    if rest.trim() != "=" || prefix.contains('\n') {
        return None;
    }
    // The insertion point must sit on the identifier itself (`f((x))` gives
    // the inner ident's span, but be safe against span drift).
    if !chars[at_pos..].starts_with(&x.chars().collect::<Vec<_>>()[..]) {
        return None;
    }
    Some(Rebind { prefix_start, prefix_end, at_pos })
}

/// Whether any `@var` marker appears anywhere in `e` (nested bodies included).
fn contains_atvar(e: &Expr) -> bool {
    let mut found = false;
    ast::for_each_expr(e, &mut |e| found |= matches!(e.kind, ExprKind::AtVar(_)));
    found
}

/// Occurrences of `name` as an identifier or `@name` anywhere in `e`, nested
/// statement bodies included. (Nested statements *assigning* `name` don't
/// matter: they are identical text on both sides of the rewrite.)
fn count_ident(e: &Expr, name: &str) -> usize {
    let mut n = 0;
    ast::for_each_expr(e, &mut |e| match &e.kind {
        ExprKind::Ident(id) | ExprKind::AtVar(id) if id == name => n += 1,
        _ => {}
    });
    n
}


/// Apply rebind edits to the source, highest offset first so earlier
/// positions stay valid.
pub(super) fn apply_rebinds(chars: &[char], rebinds: &[Rebind]) -> String {
    let mut edits: Vec<(usize, usize, &str)> = Vec::with_capacity(rebinds.len() * 2);
    for r in rebinds {
        edits.push((r.at_pos, r.at_pos, "@"));
        edits.push((r.prefix_start, r.prefix_end, ""));
    }
    edits.sort_by_key(|&(start, _, _)| std::cmp::Reverse(start));
    let mut out: Vec<char> = chars.to_vec();
    for (start, end, text) in edits {
        out.splice(start..end, text.chars());
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use crate::lint::{lint_source, LintOptions, LintOutcome};

    fn lint(src: &str) -> LintOutcome {
        lint_source(src, &LintOptions::default()).expect("lint_source")
    }

    fn rebound(src: &str) -> String {
        lint(src).output
    }

    #[test]
    fn rebind_flagship_examples() {
        assert_eq!(
            rebound("let x = 1\nx = double(x)\n"),
            "let x = 1\ndouble(@x)\n"
        );
        assert_eq!(
            rebound("let nums = [1, 2, 3]\nnums = append(nums, 4)\nprint(nums)\n"),
            "let nums = [1, 2, 3]\nappend(@nums, 4)\nprint(nums)\n"
        );
    }

    #[test]
    fn rebind_keeps_trailing_comment() {
        assert_eq!(
            rebound("let n = [1]\nn = append(n, 2) // grow\n"),
            "let n = [1]\nappend(@n, 2) // grow\n"
        );
    }

    #[test]
    fn rebind_applies_inside_fn_bodies() {
        let out = rebound("fn f()\nlet a = [1]\na = append(a, 2)\na\nend\nf()\n");
        assert_eq!(out, "fn f()\n  let a = [1]\n  append(@a, 2)\n  a\nend\nf()\n");
    }

    #[test]
    fn rebind_skips_ambiguous_and_non_candidates() {
        // x used twice in the call.
        let src = "let x = 1\nx = add(x, x)\n";
        assert_eq!(rebound(src), src);
        // `let` introduces a new binding; RHS x is a different variable.
        let src = "let x = 1\nlet y = double(x)\nprint(y)\n";
        assert_eq!(rebound(src), src);
        // x also used outside the arg position.
        let src = "let x = 1\nx = add(x, x + 1)\n";
        assert_eq!(rebound(src), src);
        // Callee is x itself.
        let src = "let x = double\nx = x(1)\n";
        assert_eq!(rebound(src), src);
        // RHS is not a call.
        let src = "let x = 1\nx = x + 1\n";
        assert_eq!(rebound(src), src);
    }

    #[test]
    fn lint_is_idempotent_on_its_own_output() {
        let src = "fn f()\nlet a = [1]\na = append(a, 2)\nend\n";
        let once = rebound(src);
        assert_eq!(rebound(&once), once);
    }
}
