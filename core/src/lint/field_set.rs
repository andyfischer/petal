//! Rule — `set X = {...get X, f: v}` is `set X.f = v`.
//!
//! ```text
//! set C = {...get C, yaw: a}              =>   set C.yaw = a
//! set T = {...get T, rev: get T.rev + 1}  =>   set T.rev = get T.rev + 1
//! cam = {...cam, x: 1, y: 2}              =>   cam.x = 1
//!                                              cam.y = 2
//! ```
//!
//! The spread copies every field of the record to change one; the field
//! write changes it in place. The two differ when `X` is not a plain record:
//! spreading `nil` gives a record where a field write to `nil` fails, and
//! spreading a class instance gives a plain record. So the fix is applied
//! only where the file shows what `X` holds: it is declared once, with a
//! record literal, and every whole assignment to it is a record literal too.
//! Anywhere else the rule reports and leaves the line.
//!
//! Several fields become several writes, one per line, which is only the same
//! program when no value can observe the writes before it: each value is
//! free of calls and does not mention `X`, and the statement has its line to
//! itself. A single field needs neither: its value is computed before the
//! one write, as it was before the one assignment.

use std::collections::{HashMap, HashSet};

use crate::ast::{
    AssignTarget, Expr, ExprKind, ExprVisitor, RecordField, Stmt, StmtKind, walk_expr, walk_stmt,
};

use super::Fix;
use super::to_match::Splice;
use super::util::{
    bound_names, ends_its_line, indent_of, is_pure, mentions, span_text, squash, starts_its_line,
    text,
};

pub(super) fn plan_field_set_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let mut shapes = Shapes::default();
    for s in stmts {
        shapes.visit_stmt(s);
    }
    let mut finder = Finder {
        chars,
        bound: bound_names(stmts),
        shapes,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

/// What the file shows each name to hold.
#[derive(Default)]
struct Shapes {
    /// How many times each name is bound, by any kind of binding.
    bindings: HashMap<String, usize>,
    /// Names declared (`let`/`var`/`state`) with a record literal.
    record_decl: HashSet<String>,
    /// Names some whole assignment gives something other than a record
    /// literal.
    other_value: HashSet<String>,
}

impl Shapes {
    fn bind(&mut self, name: &str) {
        *self.bindings.entry(name.to_string()).or_default() += 1;
    }

    /// The file shows `name` to be a plain record wherever it is assigned.
    fn is_record(&self, name: &str) -> bool {
        self.bindings.get(name) == Some(&1)
            && self.record_decl.contains(name)
            && !self.other_value.contains(name)
    }
}

impl ExprVisitor for Shapes {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, value, .. } | StmtKind::State { name, init: value, .. } => {
                self.bind(name);
                if matches!(value.kind, ExprKind::Record(_)) {
                    self.record_decl.insert(name.clone());
                }
            }
            StmtKind::For { var, .. } => self.bind(var),
            StmtKind::FnDecl { name, params, .. } => {
                self.bind(name);
                for p in params {
                    self.bind(&p.name);
                }
            }
            StmtKind::Assign {
                target: AssignTarget::Name(name),
                value,
            }
            | StmtKind::Set {
                target: AssignTarget::Name(name),
                value,
            } => {
                if !matches!(value.kind, ExprKind::Record(_)) {
                    self.other_value.insert(name.clone());
                }
            }
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lambda { params, .. } => {
                for p in params {
                    self.bind(&p.name);
                }
            }
            ExprKind::For { var, .. } => self.bind(var),
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    for n in arm.pattern.bound_names() {
                        self.bind(&n);
                    }
                }
            }
            // `f(@x)` assigns `x` whatever `f` returns.
            ExprKind::AtVar(name) => {
                self.other_value.insert(name.clone());
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}

struct Finder<'a> {
    chars: &'a [char],
    bound: HashSet<String>,
    shapes: Shapes,
    fixes: Vec<Fix>,
}

impl ExprVisitor for Finder<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        let planned = match &s.kind {
            StmtKind::Assign {
                target: AssignTarget::Name(name),
                value,
            } => self.plan(s, name, value, false),
            StmtKind::Set {
                target: AssignTarget::Name(name),
                value,
            } => self.plan(s, name, value, true),
            _ => None,
        };
        self.fixes.extend(planned);
        walk_stmt(self, s);
    }
}

impl Finder<'_> {
    fn plan(&self, stmt: &Stmt, name: &str, value: &Expr, is_set: bool) -> Option<Fix> {
        let ExprKind::Record(fields) = &value.kind else {
            return None;
        };
        let (first, rest) = fields.split_first()?;
        // `{...X, …}` / `{...get X, …}`, then named fields only.
        let RecordField::Spread(base) = first else {
            return None;
        };
        let base_text = squash(&span_text(self.chars, base.span)?);
        let base_ok = match &base.kind {
            ExprKind::Ident(n) => n == name && base_text == name,
            ExprKind::CellGet(n) => n == name && base_text == format!("get{name}"),
            _ => false,
        };
        if !base_ok || rest.is_empty() {
            return None;
        }
        let mut named: Vec<(&str, &Expr)> = Vec::new();
        for f in rest {
            let RecordField::Named(key, v) = f else {
                return None;
            };
            if named.iter().any(|(k, _)| k == key) {
                return None;
            }
            named.push((key, v));
        }
        let start = stmt.span.start.offset as usize;
        let end = stmt.span.end.offset as usize;
        let kw = if is_set { "set " } else { "" };
        let example = format!("`{kw}{name}.{} = …`", named[0].0);
        let report = |why: &str| {
            Some(Fix {
                anchor: start,
                message: format!(
                    "`{kw}{name} = {{...{name}, {}: …}}` copies the record to change a field; \
                     {why}write {example}",
                    named[0].0
                ),
                splices: Vec::new(),
            })
        };

        // The text must be exactly the shape, so that each value can be
        // lifted out verbatim: `set X = { ...get X, k: ` v `, k2: ` v2 ` }`.
        let get = if matches!(base.kind, ExprKind::CellGet(_)) { "get" } else { "" };
        let mut cursor = start;
        let mut pieces: Vec<(usize, usize)> = Vec::new();
        for (i, (key, v)) in named.iter().enumerate() {
            let v_start = v.span.start.offset as usize;
            let v_end = v.span.end.offset as usize;
            let glue = squash(&text(self.chars, cursor, v_start)?);
            let want = if i == 0 {
                format!("{}{name}={{...{get}{name},{key}:", kw.trim())
            } else {
                format!(",{key}:")
            };
            if glue != want {
                return None;
            }
            pieces.push((v_start, v_end));
            cursor = v_end;
        }
        let tail = squash(&text(self.chars, cursor, end)?);
        if tail != "}" && tail != ",}" {
            return None;
        }

        if !self.shapes.is_record(name) {
            return report(&format!("when `{name}` holds a record, "));
        }
        let single = named.len() == 1;
        if !single {
            let independent = named
                .iter()
                .all(|(_, v)| is_pure(v, &self.bound) && !mentions(v, name));
            if !independent || !starts_its_line(self.chars, start) || !ends_its_line(self.chars, end)
            {
                return report("");
            }
        }
        let indent = indent_of(self.chars, start);
        let mut splices = Vec::new();
        let mut from = start;
        for (i, ((key, _), (v_start, v_end))) in named.iter().zip(&pieces).enumerate() {
            let lead = if i == 0 {
                String::new()
            } else {
                format!("\n{indent}")
            };
            splices.push(Splice {
                start: from,
                end: *v_start,
                text: format!("{lead}{kw}{name}.{key} = "),
            });
            from = *v_end;
        }
        splices.push(Splice {
            start: from,
            end,
            text: String::new(),
        });
        Some(Fix {
            anchor: start,
            message: format!(
                "`{kw}{name} = {{...{name}, {}: …}}` copies the record to change a field; \
                 write {example}",
                named[0].0
            ),
            splices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::to_match::apply_match_edits;
    use super::*;

    fn plan(src: &str) -> (String, usize) {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        let fixes = plan_field_set_fixes(&stmts, &chars);
        let n = fixes.len();
        (apply_match_edits(&chars, &super::super::flatten(fixes)), n)
    }

    #[test]
    fn one_field_becomes_a_field_write() {
        let src = "state var C = {yaw: 0.0, rev: 0}\n\
                   fn turn(a)\n  set C = {...get C, yaw: a}\n  set C = { ...get C, rev: get C.rev + 1 }\nend\n\
                   let cam = {x: 0}\ncam = {...cam, x: cam.x + 1}\n";
        assert_eq!(
            plan(src).0,
            "state var C = {yaw: 0.0, rev: 0}\n\
             fn turn(a)\n  set C.yaw = a\n  set C.rev = get C.rev + 1\nend\n\
             let cam = {x: 0}\ncam.x = cam.x + 1\n"
        );
    }

    #[test]
    fn several_independent_fields_become_several_writes() {
        let src = "var D = {open: true, aside: false, n: 0}\n\
                   fn close(k)\n  set D = {...get D, open: false, aside: true, n: k + 1}\nend\n";
        assert_eq!(
            plan(src).0,
            "var D = {open: true, aside: false, n: 0}\n\
             fn close(k)\n  set D.open = false\n  set D.aside = true\n  set D.n = k + 1\nend\n"
        );
    }

    #[test]
    fn reports_without_fixing_what_it_cannot_show_to_be_a_record() {
        for src in [
            // Starts as nil: a field write to nil fails where the spread does not.
            "state var T = nil\nfn bump() set T = {...get T, rev: 1} end\n",
            // Assigned a call's result somewhere.
            "fn blank() {rev: 0} end\nvar T = {rev: 0}\nfn reset() set T = blank() end\n\
             fn bump() set T = {...get T, rev: 1} end\n",
            // A parameter.
            "fn f(r)\n  r = {...r, a: 1}\n  r\nend\n",
            // Several fields, one reading the record.
            "var D = {a: 1, b: 2}\nfn f()\n  set D = {...get D, a: 2, b: get D.a}\nend\n",
            // Several fields, one calling out.
            "fn g() 1 end\nvar D = {a: 1, b: 2}\nfn f()\n  set D = {...get D, a: 2, b: g()}\nend\n",
            // Several fields on a line shared with other code.
            "var D = {a: 1, b: 2}\nfn f(c)\n  if c then set D = {...get D, a: 2, b: 3} end\nend\n",
        ] {
            let (out, n) = plan(src);
            assert_eq!(out, src);
            assert_eq!(n, 1, "{src}");
        }
    }

    #[test]
    fn leaves_other_spreads_alone() {
        for src in [
            "var D = {a: 1}\nvar E = {a: 1}\nfn f() set D = {...get E, a: 2} end\n",
            "var D = {a: 1}\nfn f() set D = {a: 2, ...get D} end\n",
            "var D = {a: 1}\nfn f(o) set D = {...get D, ...o, a: 2} end\n",
            "var D = {a: 1}\nfn f() set D = {...get D} end\n",
            "var D = {a: 1}\nfn f() set D = {...get D, a: 1, a: 2} end\n",
            "var D = {a: 1}\nlet e = {...D, a: 2}\n",
        ] {
            assert_eq!(plan(src).1, 0, "{src}");
        }
    }
}
