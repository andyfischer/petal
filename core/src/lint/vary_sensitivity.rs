//! Rule `vary-sensitivity`: `ui/drag`'s `vary` lost its `sensitivity`
//! option (forward differences are the only mode; the key is ignored for one
//! release, docs/tasks/differentiation-rewrite.md). The fix deletes the key
//! from the options record, and the whole options argument when that was its
//! only field. That is the same program.

use crate::ast::{Expr, ExprKind, ExprVisitor, RecordField, Stmt, StmtKind, walk_expr};

use super::Fix;
use super::to_match::Splice;
use super::util::{span_text, text};

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
    fn the_sensitivity_key_is_deleted() {
        let src = "import ui/drag\n\nstate m = {v: 0.0}\n\nlet a = fn() vary(m, [\"v\"], {sensitivity: \"exact\"}) end\nlet b = fn() vary(m, \"v\", {bounds: [0, 1], sensitivity: \"fd\"}) end\nlet c = fn() vary(m, \"v\", {sensitivity: \"exact\", bounds: [0, 1]}) end\n";
        let (out, rules) = fixed(src);
        assert!(out.contains("vary(m, [\"v\"])"), "{out}");
        assert!(out.contains("vary(m, \"v\", {bounds: [0, 1]})"), "{out}");
        assert_eq!(out.matches("{bounds: [0, 1]}").count(), 2, "{out}");
        assert!(!out.contains("sensitivity"), "{out}");
        assert_eq!(rules.iter().filter(|r| **r == "vary-sensitivity").count(), 3);
    }
}
