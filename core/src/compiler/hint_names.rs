//! `tunable` and `rigid` are reserved: Petal code cannot bind either name
//! (docs/proposals/remove-config-keyword.md §2).
//!
//! Tools read both hints by name, without running the program: a top-level
//! `let X = tunable(v, …)` is a setting that hosts list and set
//! (`crate::static_value::is_setting`), and direct manipulation reads the
//! marks from the IR. A file that declared its own `fn tunable` would make
//! every such reading wrong, so a binding of either name is a compile error
//! rather than a silent shadow. A host that registers a native under either
//! name is still reported where the hints are read (`dm-hint-disabled`).

use crate::ast::{self, Expr, ExprKind, ExprVisitor, Stmt, StmtKind};
use crate::diagnostic::Diagnostic;
use crate::source_map::SourceSpan;

/// The names no Petal code may bind.
pub(crate) const RESERVED_HINT_NAMES: [&str; 2] = ["tunable", "rigid"];

fn is_reserved(name: &str) -> bool {
    RESERVED_HINT_NAMES.contains(&name)
}

fn message(name: &str) -> String {
    format!(
        "`{name}` is a builtin direct-manipulation hint and cannot be redefined; \
         choose another name"
    )
}

/// An error for every binding of a reserved hint name in `stmts`, at any
/// depth: a `let`/`var`/`const`, `state`, `memo`, `fn`, a parameter, a loop
/// variable, a match binding, an enum or class, or an import alias. A
/// `native fn` declaration is left alone (the core's own declarations of the
/// two hints are `native fn`s).
pub(crate) fn reserved_hint_bindings(stmts: &[Stmt]) -> Vec<Diagnostic> {
    struct Finder(Vec<Diagnostic>);
    impl Finder {
        fn check(&mut self, name: &str, span: SourceSpan) {
            if is_reserved(name) {
                self.0.push(Diagnostic::error(span, message(name)));
            }
        }
    }
    impl ExprVisitor for Finder {
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Let { name, .. }
                | StmtKind::State { name, .. }
                | StmtKind::Memo { name, .. }
                | StmtKind::EnumDecl { name, .. }
                | StmtKind::ClassDecl { name, .. } => self.check(name, s.span),
                StmtKind::For { var, .. } => self.check(var, s.span),
                StmtKind::FnDecl { name, params, .. } => {
                    self.check(name, s.span);
                    for p in params {
                        self.check(&p.name, s.span);
                    }
                }
                StmtKind::Import(import) => {
                    if let Some(alias) = &import.alias {
                        self.check(alias, s.span);
                    }
                }
                _ => {}
            }
            ast::walk_stmt(self, s);
        }
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Lambda { params, .. } => {
                    for p in params {
                        self.check(&p.name, e.span);
                    }
                }
                ExprKind::For { var, .. } => self.check(var, e.span),
                ExprKind::Match { arms, .. } => {
                    for arm in arms {
                        for name in arm.pattern.bound_names() {
                            self.check(&name, e.span);
                        }
                    }
                }
                _ => {}
            }
            ast::walk_expr(self, e);
        }
    }
    let mut finder = Finder(Vec::new());
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(src: &str) -> Vec<String> {
        let (_, stmts) = crate::rewrite::parse_ast(src).expect("parses");
        reserved_hint_bindings(&stmts).into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn every_way_of_binding_a_hint_name_is_an_error() {
        for src in [
            "fn tunable(v) v end\n",
            "let rigid = 1\n",
            "var tunable = 1\n",
            "fn f(tunable) tunable end\n",
            "let g = fn(rigid) rigid end\n",
            "for tunable in [1] do print(tunable) end\n",
            "fn f() let tunable = 2\n tunable end\n",
        ] {
            let errs = errors(src);
            assert_eq!(errs.len(), 1, "{src:?}: {errs:?}");
            assert!(errs[0].contains("cannot be redefined"), "{errs:?}");
        }
    }

    #[test]
    fn calling_the_hints_and_declaring_them_native_is_fine() {
        assert!(errors("let x = tunable(4, 0, 10)\nlet y = rigid(2)\n").is_empty());
        assert!(errors("pub native fn tunable(value)\n").is_empty());
    }
}
