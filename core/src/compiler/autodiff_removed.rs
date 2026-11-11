//! The functions `autodiff` no longer has: `dual(v, d)`, `value_of(x)` and
//! `deriv_of(x)`, removed with the value kind they worked on
//! (docs/tasks/differentiation-rewrite.md, Phase 5). `deriv(f, x)`,
//! `jacobian`, `solve_step` and `solve`, in the same package, replace them by
//! measuring `f` with its input nudged.
//!
//! A file that reaches one of the removed names through `import autodiff`
//! (bare, or qualified as `autodiff.name`) is a compile error at each use,
//! pointing at the replacement. A file that declares a function of that name
//! itself, or does not import the package, is unaffected.

use std::collections::HashSet;

use crate::ast::{Expr, ExprKind, ExprVisitor, ImportDecl, Stmt, StmtKind, walk_expr, walk_stmt};
use crate::diagnostic::Diagnostic;
use crate::source_map::SourceSpan;

/// The package the removed functions lived in.
pub(crate) const MODULE: &str = "autodiff";

/// The removed functions.
pub(crate) const NAMES: [&str; 3] = ["dual", "value_of", "deriv_of"];

/// The error for a use of `name`.
pub(crate) fn message(name: &str) -> String {
    format!(
        "`{name}` was removed from `autodiff` (dual numbers are gone).\n  Use `deriv(f, x)` \
         for a derivative, or `solve_step(f, xs, target)` to move\n  inputs toward a goal. \
         See docs/features/differentiation.md."
    )
}

/// How a file reaches `autodiff`'s functions: bare, and through which
/// qualifiers. `None` when it does not import the package.
pub(crate) struct Reach {
    /// The removed names an import binds bare.
    pub bare: HashSet<&'static str>,
    /// The qualifiers (`autodiff`, or an `as` alias) the package is reachable
    /// through.
    pub qualifiers: HashSet<String>,
}

impl Reach {
    /// From a file's imports and the rest of its statements (as the loader
    /// hands a module to the compiler, imports split out).
    pub(crate) fn from_imports<'a>(
        imports: impl Iterator<Item = &'a ImportDecl>,
        stmts: &[Stmt],
    ) -> Option<Reach> {
        let mut reach = Reach {
            bare: HashSet::new(),
            qualifiers: HashSet::new(),
        };
        let mut found = false;
        for import in imports {
            if import.module != MODULE {
                continue;
            }
            found = true;
            match &import.names {
                Some(names) => {
                    reach
                        .bare
                        .extend(NAMES.iter().filter(|n| names.iter().any(|m| m == *n)));
                }
                None => {
                    reach.bare.extend(NAMES);
                    reach
                        .qualifiers
                        .insert(import.alias.clone().unwrap_or_else(|| MODULE.to_string()));
                }
            }
        }
        if !found {
            return None;
        }
        // A name the file declares itself is its own, not the package's.
        for name in declared_names(stmts) {
            reach.bare.remove(name.as_str());
            reach.qualifiers.remove(&name);
        }
        Some(reach)
    }

    /// The removed function `e` names, if it names one: a bare name or
    /// `autodiff.name`.
    pub(crate) fn names(&self, e: &Expr) -> Option<&'static str> {
        match &e.kind {
            ExprKind::Ident(n) => NAMES
                .iter()
                .find(|d| **d == n.as_str() && self.bare.contains(*d))
                .copied(),
            ExprKind::FieldAccess { object, field } => match &object.kind {
                ExprKind::Ident(q) if self.qualifiers.contains(q) => {
                    NAMES.iter().find(|d| **d == field.as_str()).copied()
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// Every name the file declares with `fn`, `let`, `var` or a parameter.
fn declared_names(stmts: &[Stmt]) -> HashSet<String> {
    struct Decls(HashSet<String>);
    impl ExprVisitor for Decls {
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Let { name, .. } | StmtKind::State { name, .. } => {
                    self.0.insert(name.clone());
                }
                StmtKind::FnDecl { name, params, .. } => {
                    self.0.insert(name.clone());
                    self.0.extend(params.iter().map(|p| p.name.clone()));
                }
                _ => {}
            }
            walk_stmt(self, s);
        }
        fn visit_expr(&mut self, e: &Expr) {
            if let ExprKind::Lambda { params, .. } = &e.kind {
                self.0.extend(params.iter().map(|p| p.name.clone()));
            }
            walk_expr(self, e);
        }
    }
    let mut d = Decls(HashSet::new());
    for s in stmts {
        d.visit_stmt(s);
    }
    d.0
}

/// One error per use of a removed name.
pub(super) fn removed_dual_uses(
    module: &crate::module::LoadedModule,
    stmts: &[Stmt],
) -> Vec<Diagnostic> {
    let imports = module.imports.iter().filter(|i| !i.implicit).map(|i| &i.decl);
    let Some(reach) = Reach::from_imports(imports, stmts) else {
        return Vec::new();
    };
    struct Finder<'a> {
        reach: &'a Reach,
        uses: Vec<(&'static str, SourceSpan)>,
    }
    impl ExprVisitor for Finder<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            if let Some(name) = self.reach.names(e) {
                self.uses.push((name, e.span));
            }
            walk_expr(self, e);
        }
    }
    let mut finder = Finder {
        reach: &reach,
        uses: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.uses.sort_by_key(|(_, span)| span.start.offset);
    finder
        .uses
        .into_iter()
        .map(|(name, span)| Diagnostic::error(span, message(name)))
        .collect()
}
