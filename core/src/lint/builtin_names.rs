//! Rules about a function that is already there.
//!
//! **`shadows-builtin`** — a `fn` with the name of a builtin, of the standard
//! prelude, or of a bundled package the file imports whole (`import ui`),
//! taking a number of arguments the existing one takes. The file's own
//! declaration wins without a word, so every call in the file reaches it and
//! a reader who knows the builtin is misled. A different argument count is
//! an overload, which is how a library adds a form to a native, and is not
//! reported.
//!
//! **`reimplements-builtin`** — a `fn` that is a builtin under another name:
//!
//! ```text
//! fn gf(rec, key, dflt)                       // field(rec, key, dflt): it
//!   if rec == nil then dflt                   // already answers the fallback
//!   else field(rec, key, dflt) end            // for nil
//! end
//! fn count(xs) len(xs) end                    // len
//! ```
//!
//! Neither has a fix: the callers may be in other files.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::ast::{
    BinOp, ElseBranch, Expr, ExprKind, ExprVisitor, Literal, Param, Stmt, StmtKind, walk_expr,
    walk_stmt,
};

use super::Fix;
use super::util::{bound_names, call_of};

/// The argument counts a known function takes: `(fewest, most)`, `most` being
/// `None` for a variadic one.
type Arity = (usize, Option<usize>);

fn fn_arity(params: &[Param]) -> Arity {
    (crate::ast::required_param_count(params), Some(params.len()))
}

/// The functions every file can call with no import: the core natives and
/// the standard prelude.
fn always_in_scope() -> &'static HashMap<String, Vec<Arity>> {
    static KNOWN: OnceLock<HashMap<String, Vec<Arity>>> = OnceLock::new();
    KNOWN.get_or_init(|| {
        let mut known: HashMap<String, Vec<Arity>> = HashMap::new();
        for (name, decl) in crate::native_decl::core_decls().iter() {
            let forms = known.entry(name.to_string()).or_default();
            for form in &decl.forms {
                forms.push((
                    form.required,
                    (!form.rest).then_some(form.params.len()),
                ));
            }
        }
        if let Ok((_tree, stmts)) = crate::rewrite::parse_ast(crate::env::STD_PRELUDE) {
            collect_exports(&stmts, false, &mut known);
        }
        known
    })
}

/// The functions `stmts` declares at top level (`pub` ones only when
/// `exported_only`), added to `into`.
fn collect_exports(stmts: &[Stmt], exported_only: bool, into: &mut HashMap<String, Vec<Arity>>) {
    for s in stmts {
        if exported_only && !s.exported {
            continue;
        }
        match &s.kind {
            StmtKind::FnDecl {
                name,
                class: None,
                params,
                ..
            } => into.entry(name.clone()).or_default().push(fn_arity(params)),
            StmtKind::NativeFn(decl) => {
                let form = crate::native_decl::NativeForm::from_decl(decl);
                into.entry(decl.name.clone())
                    .or_default()
                    .push((form.required, (!form.rest).then_some(form.params.len())));
            }
            _ => {}
        }
    }
}

/// What a plain import of the bundled package `name` brings into scope.
fn bundled_exports(name: &str) -> Option<&'static HashMap<String, Vec<Arity>>> {
    static PACKAGES: OnceLock<HashMap<&'static str, HashMap<String, Vec<Arity>>>> = OnceLock::new();
    PACKAGES
        .get_or_init(|| {
            let mut all = HashMap::new();
            for package in crate::bundled::BUNDLED_PACKAGES {
                if package.name == crate::bundled::CORE_PACKAGE {
                    continue;
                }
                let mut exports = HashMap::new();
                for (_path, source) in package.sources() {
                    if let Ok((_tree, stmts)) = crate::rewrite::parse_ast(source) {
                        collect_exports(&stmts, true, &mut exports);
                    }
                }
                all.insert(package.name, exports);
            }
            all
        })
        .get(name)
}

fn takes(forms: &[Arity], count: usize) -> bool {
    forms
        .iter()
        .any(|(min, max)| *min <= count && max.is_none_or(|m| count <= m))
}

/// The names of the functions `stmts` declares at top level, sorted: what
/// identifies a file as one of the sources below however it is laid out.
fn declared_fns(stmts: &[Stmt]) -> Vec<String> {
    let mut names: Vec<String> = stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::FnDecl { name, .. } => Some(name.clone()),
            StmtKind::NativeFn(decl) => Some(decl.name.clone()),
            _ => None,
        })
        .collect();
    names.sort();
    names
}

/// Whether the file is itself the standard prelude or a file of a bundled
/// package: the place the existing functions are written, which shadows
/// nothing by declaring them (and a bundled library that replaces a `ui`
/// function with its own does so as its design). Told by the functions it
/// declares, so that it still holds for the file after `--fix` or `fmt`.
fn is_bundled_source(stmts: &[Stmt]) -> bool {
    static SOURCES: OnceLock<HashSet<Vec<String>>> = OnceLock::new();
    let mine = declared_fns(stmts);
    if mine.is_empty() {
        return false;
    }
    SOURCES
        .get_or_init(|| {
            let texts = std::iter::once(crate::env::STD_PRELUDE).chain(
                crate::bundled::BUNDLED_PACKAGES
                    .iter()
                    .flat_map(|p| p.sources().map(|(_, text)| text)),
            );
            texts
                .filter_map(|text| crate::rewrite::parse_ast(text).ok())
                .map(|(_tree, stmts)| declared_fns(&stmts))
                .collect()
        })
        .contains(&mine)
}

pub(super) fn plan_shadow_reports(stmts: &[Stmt]) -> Vec<Fix> {
    if is_bundled_source(stmts) {
        return Vec::new();
    }
    // The bundled packages this file imports whole.
    let imported: Vec<(&str, &HashMap<String, Vec<Arity>>)> = stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::Import(i) if i.names.is_none() && !i.star => {
                bundled_exports(&i.module).map(|e| (i.module.as_str(), e))
            }
            _ => None,
        })
        .collect();
    let mut finder = Shadows {
        imported,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

struct Shadows<'a> {
    imported: Vec<(&'a str, &'static HashMap<String, Vec<Arity>>)>,
    fixes: Vec<Fix>,
}

impl ExprVisitor for Shadows<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        if let StmtKind::FnDecl {
            name,
            class: None,
            params,
            ..
        } = &s.kind
        {
            let (min, max) = fn_arity(params);
            let overlaps =
                |forms: &[Arity]| (min..=max.unwrap_or(min)).any(|count| takes(forms, count));
            let whose = if always_in_scope().get(name).is_some_and(|f| overlaps(f)) {
                Some("the builtin".to_string())
            } else {
                self.imported
                    .iter()
                    .find(|(_, exports)| exports.get(name).is_some_and(|f| overlaps(f)))
                    .map(|(package, _)| format!("`{package}`'s"))
            };
            if let Some(whose) = whose {
                self.fixes.push(Fix {
                    anchor: s.span.start.offset as usize,
                    message: format!(
                        "`fn {name}` takes the place of {whose} `{name}` in this file: every \
                         call here reaches this one. Use the existing function, or give this \
                         one its own name"
                    ),
                    splices: Vec::new(),
                });
            }
        }
        walk_stmt(self, s);
    }
}

pub(super) fn plan_reimplement_reports(stmts: &[Stmt]) -> Vec<Fix> {
    let mut finder = Wrappers {
        bound: bound_names(stmts),
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

struct Wrappers {
    bound: HashSet<String>,
    fixes: Vec<Fix>,
}

impl ExprVisitor for Wrappers {
    fn visit_stmt(&mut self, s: &Stmt) {
        if let StmtKind::FnDecl {
            name,
            class: None,
            params,
            body,
            ..
        } = &s.kind
            && let Some(message) = self.wrapper(name, params, body)
        {
            self.fixes.push(Fix {
                anchor: s.span.start.offset as usize,
                message,
                splices: Vec::new(),
            });
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
}

impl Wrappers {
    /// The builtin `e` calls with exactly `params`, in order.
    fn forwards<'e>(&self, e: &'e Expr, params: &[Param]) -> Option<&'e str> {
        let ExprKind::Call {
            function,
            args,
            arg_names,
        } = &e.kind
        else {
            return None;
        };
        let ExprKind::Ident(callee) = &function.kind else {
            return None;
        };
        let known = always_in_scope().get(callee)?;
        if !arg_names.is_empty()
            || self.bound.contains(callee)
            || args.len() != params.len()
            || !takes(known, args.len())
        {
            return None;
        }
        args.iter()
            .zip(params)
            .all(|(a, p)| matches!(&a.kind, ExprKind::Ident(n) if *n == p.name))
            .then_some(callee.as_str())
    }

    fn wrapper(&self, name: &str, params: &[Param], body: &[Stmt]) -> Option<String> {
        if params.is_empty() || params.iter().any(|p| p.has_default() || p.ty.is_some()) {
            return None;
        }
        let [
            Stmt {
                kind: StmtKind::Expr(e),
                ..
            },
        ] = body
        else {
            return None;
        };
        let list = params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(callee) = self.forwards(e, params) {
            return Some(format!(
                "`fn {name}({list})` only calls `{callee}({list})`: call `{callee}`"
            ));
        }
        // `if r == nil then d else field(r, k, d) end`.
        let [r, _k, d] = params else { return None };
        let ExprKind::If {
            condition,
            then_body,
            else_body: Some(ElseBranch::Block(else_body)),
        } = &e.kind
        else {
            return None;
        };
        let ExprKind::BinaryOp {
            op: BinOp::Eq,
            left,
            right,
        } = &condition.kind
        else {
            return None;
        };
        let is_name = |x: &Expr, n: &str| matches!(&x.kind, ExprKind::Ident(i) if i == n);
        let is_nil = |x: &Expr| matches!(x.kind, ExprKind::Literal(Literal::Nil));
        if !((is_name(left, &r.name) && is_nil(right)) || (is_nil(left) && is_name(right, &r.name)))
        {
            return None;
        }
        let [
            Stmt {
                kind: StmtKind::Expr(then_e),
                ..
            },
        ] = then_body.as_slice()
        else {
            return None;
        };
        let [
            Stmt {
                kind: StmtKind::Expr(else_e),
                ..
            },
        ] = else_body.as_slice()
        else {
            return None;
        };
        if !is_name(then_e, &d.name) || self.forwards(else_e, params) != Some("field") {
            return None;
        }
        let _ = call_of;
        Some(format!(
            "`fn {name}({list})` is `field({list})`: `field` already answers the fallback for \
             `nil` (and `r?.k ?? d` reads a field named in the source)"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shadows(src: &str) -> Vec<String> {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        plan_shadow_reports(&stmts)
            .into_iter()
            .map(|f| f.message)
            .collect()
    }

    fn wrappers(src: &str) -> Vec<String> {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        plan_reimplement_reports(&stmts)
            .into_iter()
            .map(|f| f.message)
            .collect()
    }

    #[test]
    fn a_fn_named_for_a_builtin_is_reported() {
        let got = shadows("fn sum(xs)\n  var t = 0\n  for x in xs do set t += x end\n  t\nend\n");
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("the builtin `sum`"), "{got:?}");
        // The standard prelude counts, and so does a nested declaration.
        assert_eq!(shadows("fn f()\n  fn find(xs, p) nil end\n  find([], f)\nend\n").len(), 1);
    }

    #[test]
    fn an_overload_by_argument_count_is_not_a_shadow() {
        assert_eq!(shadows("fn len(a, b, c, d, e) 0 end\n"), Vec::<String>::new());
        assert_eq!(shadows("fn mine(xs) 0 end\n"), Vec::<String>::new());
        // A method is called through its class.
        assert_eq!(
            shadows("class P\n  x: int\nend\nfn P.len(self) 1 end\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_bundled_package_counts_only_where_it_is_imported_whole() {
        let body = "fn color_lerp(a, b, t) a end\n";
        assert_eq!(shadows(body), Vec::<String>::new());
        let got = shadows(&format!("import color\n{body}"));
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("`color`'s `color_lerp`"), "{got:?}");
        assert_eq!(
            shadows(&format!("import color: rgb\n{body}")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_prelude_and_the_bundled_packages_do_not_shadow_themselves() {
        assert_eq!(shadows(crate::env::STD_PRELUDE), Vec::<String>::new());
        for package in crate::bundled::BUNDLED_PACKAGES {
            for (path, source) in package.sources() {
                assert_eq!(shadows(source), Vec::<String>::new(), "{}/{path}", package.name);
            }
        }
    }

    #[test]
    fn a_wrapper_of_a_builtin_is_reported() {
        let got = wrappers(
            "fn gf(rec, key, dflt)\n  if rec == nil then dflt else field(rec, key, dflt) end\nend\n",
        );
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("is `field(rec, key, dflt)`"), "{got:?}");
        let got = wrappers("fn count(xs) len(xs) end\n");
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("only calls `len(xs)`"), "{got:?}");
    }

    #[test]
    fn a_function_that_does_more_is_not_a_wrapper() {
        for src in [
            "fn gf(rec, key, dflt)\n  if rec == nil then 0 else field(rec, key, dflt) end\nend\n",
            "fn count(xs) len(xs) + 1 end\n",
            "fn swap(a, b) max(b, a) end\n",
            "fn half(xs) slice(xs, 0) end\n",
            "fn typed(xs: list) len(xs) end\n",
            // `len` is the file's own here.
            "fn len(a, b, c) 0 end\nfn count(a, b, c) len(a, b, c) end\n",
            "fn mine(x) x end\nfn other(x) mine(x) end\n",
        ] {
            assert_eq!(wrappers(src), Vec::<String>::new(), "{src}");
        }
    }
}
