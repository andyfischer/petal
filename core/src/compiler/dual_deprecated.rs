//! Warn about the dual-number functions of `autodiff`, which are deprecated
//! (docs/tasks/differentiation-rewrite.md, Phase 2).
//!
//! `dual(v, d)`, `value_of(x)` and `deriv_of(x)` go away with the dual value
//! kind. `deriv(f, x)`, `jacobian`, `solve_step` and `solve`, in the same
//! package, replace them by measuring `f` with its input nudged. Until the
//! removal they keep working, and a file that reaches one of them through
//! `import autodiff` is warned once per name, at its first use. It is
//! silenced like a lint finding, with `// petal-lint-ignore dual-deprecated`
//! (or `petal-lint-ignore-file`). `petal lint --fix` rewrites the common
//! shape, `deriv_of(f(dual(x, 1.0)))` for a named `f`, to
//! `deriv(f, x).value` (the lint rule of the same name, `crate::lint`).
//!
//! A first-party bundled package is not warned about: its author is not the
//! reader of the warning ([`super::is_bundled_copy`]).

use std::collections::HashSet;

use crate::ast::{Expr, ExprKind, ExprVisitor, ImportDecl, Stmt, StmtKind, walk_expr, walk_stmt};
use crate::diagnostic::Diagnostic;
use crate::source_map::SourceSpan;

/// The rule id the warning carries, and the lint rule's name.
pub(crate) const RULE: &str = "dual-deprecated";

/// The package the deprecated functions live in.
pub(crate) const MODULE: &str = "autodiff";

/// The deprecated functions.
pub(crate) const NAMES: [&str; 3] = ["dual", "value_of", "deriv_of"];

/// The warning for a use of `name`.
pub(crate) fn message(name: &str) -> String {
    format!(
        "`{name}` is deprecated: dual numbers are being removed from `autodiff`. Use \
         `deriv(f, x)` for a derivative, or `solve_step(f, xs, target)` to move inputs toward \
         a goal (docs/features/differentiation.md); `petal lint --fix` rewrites \
         `deriv_of(f(dual(x, 1.0)))` [{RULE}]"
    )
}

/// How a file reaches `autodiff`'s functions: bare, and through which
/// qualifiers. `None` when it does not import the package.
pub(crate) struct Reach {
    /// The deprecated names an import binds bare.
    pub bare: HashSet<&'static str>,
    /// The qualifiers (`autodiff`, or an `as` alias) the package is reachable
    /// through.
    pub qualifiers: HashSet<String>,
    /// A plain import binds the package's `deriv` bare, and the file does
    /// not declare a `deriv` of its own.
    pub deriv_bare: bool,
}

impl Reach {
    /// From a parsed file, whose imports are among its statements.
    pub(crate) fn of(stmts: &[Stmt]) -> Option<Reach> {
        let imports = stmts.iter().filter_map(|s| match &s.kind {
            StmtKind::Import(import) => Some(import),
            _ => None,
        });
        Reach::from_imports(imports, stmts)
    }

    /// From a file's imports and the rest of its statements (as the loader
    /// hands a module to the compiler, imports split out).
    pub(crate) fn from_imports<'a>(
        imports: impl Iterator<Item = &'a ImportDecl>,
        stmts: &[Stmt],
    ) -> Option<Reach> {
        let mut reach = Reach {
            bare: HashSet::new(),
            qualifiers: HashSet::new(),
            deriv_bare: false,
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
                    reach.deriv_bare = true;
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
            if name == "deriv" {
                reach.deriv_bare = false;
            }
            reach.bare.remove(name.as_str());
            reach.qualifiers.remove(&name);
        }
        Some(reach)
    }

    /// The deprecated function `e` names, if it names one: a bare name or
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

/// One warning per deprecated name the file uses, at its first use, unless
/// an ignore comment covers that line.
pub(super) fn deprecated_dual_uses(
    module: &crate::module::LoadedModule,
    stmts: &[Stmt],
) -> Vec<Diagnostic> {
    let source = module.source.as_str();
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
    if finder.uses.is_empty() {
        return Vec::new();
    }
    finder.uses.sort_by_key(|(_, span)| span.start.offset);
    // A file whose comments do not scan is reported in full.
    let ignores = crate::lint::Ignores::scan(source).ok();
    let mut warned = HashSet::new();
    finder
        .uses
        .into_iter()
        .filter(|(name, span)| {
            let line = (span.start.line as usize).saturating_sub(1);
            !ignores.as_ref().is_some_and(|i| i.suppresses(RULE, line)) && warned.insert(*name)
        })
        .map(|(name, span)| Diagnostic::new(span, message(name)))
        .collect()
}
