//! Rules — the migration off dual numbers (docs/tasks/differentiation-rewrite.md,
//! Phase 2).
//!
//! **`dual-deprecated`**: the one-input derivative written with duals, in its
//! common shape, becomes a call of `deriv`:
//!
//! ```text
//! deriv_of(f(dual(x, 1.0)))     =>     deriv(f, x).value
//! ```
//!
//! `f` must be a name (a `fn`, a builtin like `atan`, or a variable holding a
//! function) called with the one dual argument, and the seed must be the
//! literal `1.0` (or `1`). The rewrite is not the same program: `deriv`
//! measures a central difference, so the number it gives is a close
//! approximation of the exact derivative the dual gave. It is the migration
//! the deprecation asks for, so `--fix` applies it; every other use of `dual`,
//! `value_of` or `deriv_of` is left to the compiler's warning of the same
//! rule id ([`crate::compiler::dual_deprecated`]) and a hand edit.
//!
//! **`vary-sensitivity`**: `ui/drag`'s `vary` lost its `sensitivity` option
//! (forward differences are the only mode; the key is ignored for one
//! release). The fix deletes the key from the options record, and the whole
//! options argument when that was its only field. That is the same program.

use crate::ast::{Expr, ExprKind, ExprVisitor, Literal, RecordField, Stmt, StmtKind, walk_expr};
use crate::compiler::dual_deprecated::Reach;

use super::Fix;
use super::to_match::Splice;
use super::util::{span_text, text};

pub(super) fn plan_dual_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let Some(reach) = Reach::of(stmts) else {
        return Vec::new();
    };
    struct Finder<'a> {
        reach: &'a Reach,
        chars: &'a [char],
        fixes: Vec<Fix>,
    }
    impl ExprVisitor for Finder<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            if let Some(fix) = self.one(e) {
                // The outer call is rewritten whole; what is inside it is not
                // visited again.
                self.fixes.push(fix);
                return;
            }
            walk_expr(self, e);
        }
    }
    impl Finder<'_> {
        fn one(&self, e: &Expr) -> Option<Fix> {
            let (outer_fn, [inner]) = positional_call(e)? else {
                return None;
            };
            if self.reach.names(outer_fn)? != "deriv_of" {
                return None;
            }
            let (f, [seeded]) = positional_call(inner)? else {
                return None;
            };
            let ExprKind::Ident(f_name) = &f.kind else {
                return None;
            };
            if crate::compiler::dual_deprecated::NAMES.contains(&f_name.as_str()) {
                return None;
            }
            let (dual_fn, [x, seed]) = positional_call(seeded)? else {
                return None;
            };
            if self.reach.names(dual_fn)? != "dual" || !is_one(seed) {
                return None;
            }
            // `deriv` is reached the way `deriv_of` was: bare, or through the
            // same qualifier.
            // A bare `deriv` must be the package's: bound by a plain import,
            // and not a name the file declares itself.
            let head = match &outer_fn.kind {
                ExprKind::Ident(_) if self.reach.deriv_bare => "deriv".to_string(),
                ExprKind::FieldAccess { object, .. } => {
                    format!("{}.deriv", span_text(self.chars, object.span)?)
                }
                _ => return None,
            };
            let x_text = span_text(self.chars, x.span)?;
            let start = e.span.start.offset as usize;
            let end = e.span.end.offset as usize;
            // The span must cover exactly the call as written.
            let written = text(self.chars, start, end)?;
            if !written.starts_with(span_text(self.chars, outer_fn.span)?.as_str())
                || !written.ends_with(')')
            {
                return None;
            }
            Some(Fix {
                anchor: start,
                message: format!(
                    "dual numbers are deprecated: `deriv_of({f_name}(dual(x, 1.0)))` is \
                     `deriv({f_name}, x).value`, a central difference"
                ),
                splices: vec![Splice {
                    start,
                    end,
                    text: format!("{head}({f_name}, {x_text}).value"),
                }],
            })
        }
    }
    let mut finder = Finder {
        reach: &reach,
        chars,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

/// `e` as a call with positional arguments only: its callee and arguments.
fn positional_call(e: &Expr) -> Option<(&Expr, &[Expr])> {
    match &e.kind {
        ExprKind::Call {
            function,
            args,
            arg_names,
        } if arg_names.iter().all(Option::is_none) => Some((function, args)),
        _ => None,
    }
}

fn is_one(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Literal(Literal::Float(v)) if *v == 1.0)
        || matches!(&e.kind, ExprKind::Literal(Literal::Int(1)))
}

/// Whether the file imports `ui/drag`, where `vary` lives.
fn imports_drag(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| {
        matches!(&s.kind, StmtKind::Import(i) if i.module == "ui/drag" || i.module == "ui")
    })
}

pub(super) fn plan_sensitivity_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    if !imports_drag(stmts) {
        return Vec::new();
    }
    struct Finder<'a> {
        chars: &'a [char],
        fixes: Vec<Fix>,
    }
    impl ExprVisitor for Finder<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            if let Some(fix) = self.one(e) {
                self.fixes.push(fix);
            }
            walk_expr(self, e);
        }
    }
    impl Finder<'_> {
        fn one(&self, e: &Expr) -> Option<Fix> {
            let (f, args) = positional_call(e)?;
            let is_vary = match &f.kind {
                ExprKind::Ident(n) => n == "vary",
                ExprKind::FieldAccess { field, .. } => field == "vary",
                _ => false,
            };
            if !is_vary || args.len() != 3 {
                return None;
            }
            let ExprKind::Record(fields) = &args[2].kind else {
                return None;
            };
            let at = fields.iter().position(
                |fl| matches!(fl, RecordField::Named(k, _) if k == "sensitivity"),
            )?;
            let message = "`vary`'s `sensitivity` option was removed: `ui/drag` always uses \
                           forward differences, and ignores the key"
                .to_string();
            let value_of = |i: usize| match &fields[i] {
                RecordField::Named(_, v) | RecordField::Spread(v) => v,
            };
            let key_start = self.key_start(value_of(at), "sensitivity")?;
            let anchor = key_start;
            let splices = if fields.len() == 1 {
                // The only field: the options argument goes, with the comma
                // before it.
                let from = args[1].span.end.offset as usize;
                let to = args[2].span.end.offset as usize;
                vec![self.gap_splice(from, to, &[',', '{', '}', ':'], "sensitivity", value_of(at))?]
            } else if at > 0 {
                // From the end of the field before to the end of this one.
                let from = value_of(at - 1).span.end.offset as usize;
                let to = value_of(at).span.end.offset as usize;
                vec![self.gap_splice(from, to, &[',', ':'], "sensitivity", value_of(at))?]
            } else {
                // The first of several: from its key to the next field's start.
                let next_start = match &fields[1] {
                    RecordField::Named(k, v) => self.key_start(v, k)?,
                    RecordField::Spread(v) => {
                        let s = v.span.start.offset as usize;
                        if s < 3 || text(self.chars, s - 3, s)? != "..." {
                            return None;
                        }
                        s - 3
                    }
                };
                let from = key_start;
                let to = next_start;
                let between = text(self.chars, value_of(0).span.end.offset as usize, to)?;
                if between.trim() != "," {
                    return None;
                }
                vec![Splice {
                    start: from,
                    end: to,
                    text: String::new(),
                }]
            };
            Some(Fix {
                anchor,
                message,
                splices,
            })
        }

        /// Where the key `name` of the field whose value is `v` starts: the
        /// text before `v` must read `name:` (spaces allowed).
        fn key_start(&self, v: &Expr, name: &str) -> Option<usize> {
            let vs = v.span.start.offset as usize;
            let mut i = vs;
            while i > 0 && self.chars[i - 1].is_whitespace() {
                i -= 1;
            }
            if i == 0 || self.chars[i - 1] != ':' {
                return None;
            }
            i -= 1;
            while i > 0 && self.chars[i - 1].is_whitespace() {
                i -= 1;
            }
            let n = name.chars().count();
            if i < n || text(self.chars, i - n, i)? != name {
                return None;
            }
            Some(i - n)
        }

        /// A splice deleting `from..to`, accepted only when that text is
        /// the removed field and punctuation: the value as written, the key,
        /// and nothing else but `allowed` characters and whitespace (no
        /// comment is lost).
        fn gap_splice(
            &self,
            from: usize,
            to: usize,
            allowed: &[char],
            key: &str,
            value: &Expr,
        ) -> Option<Splice> {
            let all = text(self.chars, from, to)?;
            let value_text = span_text(self.chars, value.span)?;
            let at = all.rfind(value_text.as_str())?;
            let rest = format!("{}{}", &all[..at], &all[at + value_text.len()..]);
            let rest = rest.replacen(key, "", 1);
            if !rest.chars().all(|c| c.is_whitespace() || allowed.contains(&c)) {
                return None;
            }
            Some(Splice {
                start: from,
                end: to,
                text: String::new(),
            })
        }
    }
    let mut finder = Finder {
        chars,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

#[cfg(test)]
mod tests {
    use super::super::{LintOptions, lint_source};

    fn fixed(src: &str) -> (String, Vec<&'static str>) {
        let out = lint_source(src, &LintOptions::default()).expect("lints");
        (out.output, out.findings.iter().map(|f| f.rule).collect())
    }

    #[test]
    fn deriv_of_a_seeded_call_becomes_deriv() {
        let src = "import autodiff\n\nfn f(x)\n  x * x\nend\n\nprint(deriv_of(f(dual(3.0, 1.0))))\nprint(autodiff.deriv_of(atan(autodiff.dual(1.0, 1))))\n";
        let (out, rules) = fixed(src);
        assert!(out.contains("print(deriv(f, 3.0).value)"), "{out}");
        assert!(out.contains("print(autodiff.deriv(atan, 1.0).value)"), "{out}");
        assert_eq!(rules.iter().filter(|r| **r == "dual-deprecated").count(), 2);
    }

    #[test]
    fn other_dual_shapes_are_left_alone() {
        // A seed other than 1, a lambda callee, a two-argument f, and
        // value_of are not the mechanical shape.
        let src = "import autodiff\n\nfn g(a, b)\n  a * b\nend\n\nlet y = dual(2.0, 0.5)\nprint(deriv_of(sin(dual(1.0, 2.0))))\nprint(deriv_of(g(dual(1.0, 1.0), 2.0)))\nprint(value_of(y))\n";
        let (out, rules) = fixed(src);
        assert!(!out.contains("deriv("), "{out}");
        assert!(!rules.contains(&"dual-deprecated"), "{rules:?}");
    }

    #[test]
    fn a_selective_import_or_an_own_deriv_keeps_the_qualified_form_only() {
        let src = "import autodiff: dual, deriv_of\n\nprint(deriv_of(sin(dual(1.0, 1.0))))\n";
        let (out, _) = fixed(src);
        assert!(!out.contains("deriv("), "{out}");
    }

    #[test]
    fn without_the_import_nothing_is_reported() {
        let src = "fn deriv_of(x)\n  x\nend\n\nfn dual(a, b)\n  a\nend\n\nprint(deriv_of(sin(dual(1.0, 1.0))))\n";
        let (_, rules) = fixed(src);
        assert!(!rules.contains(&"dual-deprecated"), "{rules:?}");
    }

    #[test]
    fn the_sensitivity_key_is_deleted() {
        let src = "import ui/drag\n\nstate m = {v: 0.0}\n\nlet a = fn() vary(m, [\"v\"], {sensitivity: \"dual\"}) end\nlet b = fn() vary(m, \"v\", {bounds: [0, 1], sensitivity: \"fd\"}) end\nlet c = fn() vary(m, \"v\", {sensitivity: \"dual\", bounds: [0, 1]}) end\n";
        let (out, rules) = fixed(src);
        assert!(out.contains("vary(m, [\"v\"])"), "{out}");
        assert!(out.contains("vary(m, \"v\", {bounds: [0, 1]})"), "{out}");
        assert_eq!(out.matches("{bounds: [0, 1]}").count(), 2, "{out}");
        assert!(!out.contains("sensitivity"), "{out}");
        assert_eq!(rules.iter().filter(|r| **r == "vary-sensitivity").count(), 3);
    }
}
