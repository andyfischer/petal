//! `native fn` declarations in the compiler: the two source-level passes that
//! run before a module is compiled.
//!
//! Binding a declared name to its native term, and recording what the
//! program needs registered, live with the rest of the prescan
//! (`Compiler::bind_native_decl`, `Compiler::record_native_decls`).

use std::collections::{HashMap, HashSet};

use crate::ast::{
    self, Expr, ExprKind, ExprVisitor, NativeFnDecl, Param, Stmt, StmtKind, TypeAnn,
};
use crate::diagnostic::Diagnostic;

/// The hidden module-scope name the native half of a mixed overload set is
/// bound under. `#` cannot appear in a source identifier, so no program can
/// name, shadow or capture it by accident.
pub(crate) fn native_alias(name: &str) -> String {
    format!("{name}#native")
}

/// Make a `native fn` that shares its name with Petal `fn`s an ordinary
/// member of that overload set.
///
/// ```petal ignore
/// native fn clear(r: num, g: num, b: num)
/// fn clear(c) clear(c.r, c.g, c.b) end
/// ```
///
/// Petal overloads by arity, and an overload set is made of closures; a
/// native is not one. So each such declaration is rewritten in place into the
/// `fn`s that forward to it — one per argument count the form takes — while
/// the declaration itself moves to a hidden name (`clear#native`) that only
/// those bodies call:
///
/// ```petal ignore
/// native fn clear#native(r: num, g: num, b: num) = "clear"
/// fn clear(r: num, g: num, b: num) clear#native(r, g, b) end
/// fn clear(c) clear(c.r, c.g, c.b) end
/// ```
///
/// The forwarding call compiles to a static `BuiltinCall`, the parameter
/// types and result type carry over as annotations, and from here on the set
/// is all `fn`s — hoisting, exports, the checker and the VM's overload
/// resolution need no native case.
///
/// Returns the errors found: a native form whose argument count a Petal `fn`
/// of the set already takes (the same arity twice has no meaning), and a
/// variadic native in a set (no arity to dispatch on).
pub(crate) fn lower_native_overloads(stmts: &mut Vec<Stmt>) -> Vec<Diagnostic> {
    // Names with at least one free Petal `fn`, and every arity those take.
    let mut fn_arities: HashMap<String, HashSet<usize>> = HashMap::new();
    for stmt in stmts.iter() {
        if let StmtKind::FnDecl {
            name,
            class: None,
            params,
            ..
        } = &stmt.kind
        {
            let required = ast::required_param_count(params);
            fn_arities
                .entry(name.clone())
                .or_default()
                .extend(required..=params.len());
        }
    }
    if !stmts.iter().any(|s| {
        matches!(&s.kind, StmtKind::NativeFn(d) if d.class.is_none() && fn_arities.contains_key(&d.name))
    }) {
        return Vec::new();
    }

    let mut errors = Vec::new();
    let mut out: Vec<Stmt> = Vec::with_capacity(stmts.len() + 4);
    // Arities the natives of each set have already produced a `fn` for: two
    // forms of one native may overlap, and the set needs each arity once.
    let mut made: HashMap<String, HashSet<usize>> = HashMap::new();
    for stmt in stmts.drain(..) {
        let StmtKind::NativeFn(decl) = &stmt.kind else {
            out.push(stmt);
            continue;
        };
        let Some(taken) = fn_arities.get(&decl.name).filter(|_| decl.class.is_none()) else {
            out.push(stmt);
            continue;
        };
        if decl.rest.is_some() {
            errors.push(Diagnostic::error(
                stmt.span,
                format!(
                    "native fn '{}' takes a rest parameter and so cannot share its name with \
                     a Petal fn: an overload is chosen by argument count",
                    decl.name
                ),
            ));
            continue;
        }
        let alias = native_alias(&decl.name);
        let host = decl.host_name().to_string();
        for arity in decl.required()..=decl.params.len() {
            if taken.contains(&arity) {
                errors.push(Diagnostic::error(
                    stmt.span,
                    format!(
                        "native fn '{}' takes {arity} argument{}, and so does a fn of the same \
                         name: an overload is chosen by argument count, so each count can be \
                         declared once",
                        decl.name,
                        if arity == 1 { "" } else { "s" }
                    ),
                ));
                continue;
            }
            if !made.entry(decl.name.clone()).or_default().insert(arity) {
                continue;
            }
            out.push(forwarding_fn(decl, &alias, arity, &stmt));
        }
        out.push(Stmt {
            kind: StmtKind::NativeFn(NativeFnDecl {
                name: alias,
                host: Some(host),
                ..decl.clone()
            }),
            span: stmt.span,
            // The hidden name is private; the `fn`s above carry the `pub`.
            exported: false,
            test: None,
        });
    }
    *stmts = out;
    errors
}

/// `fn name(p0, …, p{arity-1}) name#native(p0, …) end`, with the native
/// form's annotations.
fn forwarding_fn(decl: &NativeFnDecl, alias: &str, arity: usize, stmt: &Stmt) -> Stmt {
    let span = stmt.span;
    let params: Vec<Param> = decl.params[..arity]
        .iter()
        .map(|p| Param {
            name: p.name.clone(),
            ty: p.ty.clone(),
            default: None,
            default_in_body: false,
            optional: false,
        })
        .collect();
    let args: Vec<Expr> = params
        .iter()
        .map(|p| Expr {
            kind: ExprKind::Ident(p.name.clone()),
            span,
        })
        .collect();
    let call = Expr {
        kind: ExprKind::Call {
            function: Box::new(Expr {
                kind: ExprKind::Ident(alias.to_string()),
                span,
            }),
            args,
            arg_names: Vec::new(),
        },
        span,
    };
    Stmt {
        kind: StmtKind::FnDecl {
            name: decl.name.clone(),
            class: None,
            params,
            ret: decl.ret.as_ref().map(|t| TypeAnn::at(t.name.clone(), t.span)),
            body: vec![Stmt {
                kind: StmtKind::Expr(call),
                span,
                exported: false,
                test: None,
            }],
        },
        span,
        exported: stmt.exported,
        test: None,
    }
}

/// A `native fn` anywhere but the top level of a file: inside a function, a
/// block, a loop. A declaration binds a module-scoped name, so there is
/// nowhere else for it to be.
pub(crate) fn misplaced_native_decls(stmts: &[Stmt]) -> Vec<Diagnostic> {
    struct Finder {
        depth: usize,
        out: Vec<Diagnostic>,
    }
    impl ExprVisitor for Finder {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::NativeFn(decl) = &s.kind
                && self.depth > 0
            {
                self.out.push(Diagnostic::error(
                    s.span,
                    format!(
                        "native fn '{}' must be declared at the top level of a file",
                        decl.name
                    ),
                ));
            }
            self.depth += 1;
            ast::walk_stmt(self, s);
            self.depth -= 1;
        }
    }
    let mut finder = Finder {
        depth: 0,
        out: Vec::new(),
    };
    for stmt in stmts {
        finder.visit_stmt(stmt);
    }
    finder.out
}

/// Two declarations of one name that bind different host natives.
pub(crate) fn conflicting_native_decls(stmts: &[Stmt]) -> Vec<Diagnostic> {
    let mut decls = crate::native_decl::NativeDecls::new();
    let mut out = Vec::new();
    for stmt in stmts {
        if let StmtKind::NativeFn(decl) = &stmt.kind
            && let Err(message) = decls.add(decl)
        {
            out.push(Diagnostic::error(stmt.span, message));
        }
    }
    out
}
