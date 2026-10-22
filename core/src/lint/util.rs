//! What several rules need: source text by span, the names a file binds, and
//! a test for an expression that can be evaluated again, or not at all,
//! without anyone noticing.

use std::collections::HashSet;

use crate::ast::{Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_expr, walk_stmt};
use crate::source_map::SourceSpan;

pub(super) fn text(chars: &[char], start: usize, end: usize) -> Option<String> {
    (start <= end && end <= chars.len()).then(|| chars[start..end].iter().collect())
}

pub(super) fn span_text(chars: &[char], span: SourceSpan) -> Option<String> {
    text(chars, span.start.offset as usize, span.end.offset as usize)
}

/// `s` with every whitespace character removed: what two spellings of one
/// token sequence have in common.
pub(super) fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Whether nothing but spaces and tabs stands between the start of the line
/// and `pos`.
pub(super) fn starts_its_line(chars: &[char], pos: usize) -> bool {
    chars[..pos.min(chars.len())]
        .iter()
        .rev()
        .take_while(|c| **c != '\n')
        .all(|c| *c == ' ' || *c == '\t')
}

/// Whether nothing but spaces and tabs stands between `pos` and the end of
/// its line.
pub(super) fn ends_its_line(chars: &[char], pos: usize) -> bool {
    chars[pos.min(chars.len())..]
        .iter()
        .take_while(|c| **c != '\n')
        .all(|c| *c == ' ' || *c == '\t' || *c == '\r')
}

/// The spaces and tabs the line holding `pos` starts with.
pub(super) fn indent_of(chars: &[char], pos: usize) -> String {
    let mut start = pos.min(chars.len());
    while start > 0 && chars[start - 1] != '\n' {
        start -= 1;
    }
    chars[start..]
        .iter()
        .take_while(|c| **c == ' ' || **c == '\t')
        .collect()
}

/// Every name the file binds anywhere: declarations, parameters, loop and
/// pattern variables, and the names and qualifiers its imports bring in.
/// A builtin whose name is in this set may be something else somewhere in
/// the file, and a rule that rewrites a call to it leaves the file alone.
pub(super) fn bound_names(stmts: &[Stmt]) -> HashSet<String> {
    struct Binder(HashSet<String>);
    impl ExprVisitor for Binder {
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Let { name, .. }
                | StmtKind::State { name, .. }
                | StmtKind::Memo { name, .. }
                | StmtKind::EnumDecl { name, .. }
                | StmtKind::ClassDecl { name, .. } => {
                    self.0.insert(name.clone());
                }
                StmtKind::For { var, .. } => {
                    self.0.insert(var.clone());
                }
                StmtKind::FnDecl { name, params, .. } => {
                    self.0.insert(name.clone());
                    self.0.extend(params.iter().map(|p| p.name.clone()));
                }
                StmtKind::NativeFn(decl) => {
                    self.0.insert(decl.name.clone());
                }
                StmtKind::Import(import) => {
                    if let Some(names) = &import.names {
                        self.0.extend(names.iter().cloned());
                    }
                    if let Some(alias) = &import.alias {
                        self.0.insert(alias.clone());
                    }
                    self.0
                        .insert(crate::ast::module_local_name(&import.module).to_string());
                }
                _ => {}
            }
            walk_stmt(self, s);
        }
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Lambda { params, .. } => {
                    self.0.extend(params.iter().map(|p| p.name.clone()));
                }
                ExprKind::For { var, .. } => {
                    self.0.insert(var.clone());
                }
                ExprKind::Match { arms, .. } => {
                    for arm in arms {
                        self.0.extend(arm.pattern.bound_names());
                    }
                }
                _ => {}
            }
            walk_expr(self, e);
        }
    }
    let mut b = Binder(HashSet::new());
    for s in stmts {
        b.visit_stmt(s);
    }
    b.0
}

/// Builtins that compute a value from their arguments and do nothing else.
/// A call to one of these (when the file binds no such name) may be
/// repeated or skipped.
const PURE_BUILTINS: &[&str] = &[
    "len", "abs", "min", "max", "clamp", "floor", "ceil", "round", "sqrt", "int", "float", "str",
    "type", "contains", "starts_with", "ends_with", "lower", "upper", "trim", "field",
    "keys", "values", "sin", "cos", "index_of",
];

/// Whether evaluating `e` can do nothing but produce a value: names, literals,
/// field and index reads, operators, and calls to [`PURE_BUILTINS`] that the
/// file (`bound`) does not rebind. An index or field read can still fail;
/// that is an error either way and not what this asks.
pub(super) fn is_pure(e: &Expr, bound: &HashSet<String>) -> bool {
    match &e.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::CellGet(_) => true,
        ExprKind::BinaryOp { left, right, .. } => is_pure(left, bound) && is_pure(right, bound),
        ExprKind::UnaryOp { operand, .. } => is_pure(operand, bound),
        ExprKind::FieldAccess { object, .. } => is_pure(object, bound),
        ExprKind::IndexAccess { object, index } => is_pure(object, bound) && is_pure(index, bound),
        ExprKind::OptionalAccess(inner) => is_pure(inner, bound),
        ExprKind::List(items) => items.iter().all(|i| is_pure(i, bound)),
        ExprKind::Record(fields) => fields.iter().all(|f| match f {
            crate::ast::RecordField::Named(_, v) | crate::ast::RecordField::Spread(v) => {
                is_pure(v, bound)
            }
        }),
        ExprKind::StringInterp { exprs, .. } => exprs.iter().all(|x| is_pure(x, bound)),
        ExprKind::Call {
            function,
            args,
            arg_names,
        } => {
            let ExprKind::Ident(name) = &function.kind else {
                return false;
            };
            arg_names.is_empty()
                && PURE_BUILTINS.contains(&name.as_str())
                && !bound.contains(name)
                && args.iter().all(|a| is_pure(a, bound))
        }
        _ => false,
    }
}

/// Whether `e` mentions the name `name` anywhere, as a read.
pub(super) fn mentions(e: &Expr, name: &str) -> bool {
    let mut found = false;
    crate::ast::for_each_expr(e, &mut |x| {
        if let ExprKind::Ident(n) | ExprKind::CellGet(n) | ExprKind::AtVar(n) = &x.kind
            && n == name
        {
            found = true;
        }
    });
    found
}

/// A call `name(args…)` of a bare name with positional arguments only.
pub(super) fn call_of<'e>(e: &'e Expr, name: &str) -> Option<&'e [Expr]> {
    let ExprKind::Call {
        function,
        args,
        arg_names,
    } = &e.kind
    else {
        return None;
    };
    let ExprKind::Ident(f) = &function.kind else {
        return None;
    };
    (f == name && arg_names.is_empty()).then_some(args.as_slice())
}
