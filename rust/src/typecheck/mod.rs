//! Optional static type checker (warning-only).
//!
//! See docs/dev/type-declarations-plan.md §7. This pass is SHALLOW, LOCAL, and
//! CONSERVATIVE: a false positive (warning on correct code) is far worse than a
//! false negative. Whenever inference is at all ambiguous we infer [`Type::Any`],
//! which suppresses every check. The checker NEVER errors and NEVER blocks
//! compilation — it only accumulates [`Diagnostic`]s.

use std::collections::HashMap;

use crate::ast::{
    AssignTarget, BinOp, ElseBranch, Expr, ExprKind, JsxChild, Literal, Pattern, RecordField, Stmt,
    StmtKind, TypeAnn, UnaryOp,
};
use crate::diagnostic::Diagnostic;
use crate::source_map::SourceSpan;
use crate::types::{FnSignature, Type};

/// The type knowledge for one bound name. `declared` is the written annotation
/// (if any); `inferred` is what the initializer expression evaluated to. The
/// declared type wins when both are present — see [`VarType::effective`].
struct VarType {
    declared: Option<Type>,
    inferred: Type,
}

impl VarType {
    fn effective(&self) -> Type {
        self.declared.unwrap_or(self.inferred)
    }
}

struct Checker<'a> {
    fn_signatures: &'a HashMap<(String, usize), FnSignature>,
    scopes: Vec<HashMap<String, VarType>>,
    diags: Vec<Diagnostic>,
}

/// Type-check a module's statements against its (globally collected) function
/// signatures, returning any non-fatal warnings. Never fails.
pub fn check_module(
    stmts: &[Stmt],
    fn_signatures: &HashMap<(String, usize), FnSignature>,
) -> Vec<Diagnostic> {
    let mut checker = Checker {
        fn_signatures,
        scopes: vec![HashMap::new()],
        diags: Vec::new(),
    };
    for stmt in stmts {
        checker.check_stmt(stmt);
    }
    checker.diags
}

/// Least-upper-bound used to type a branching expression: identical types keep
/// their type, anything else collapses to `Any` (suppressing further checks).
fn join(a: Type, b: Type) -> Type {
    if a == b {
        a
    } else {
        Type::Any
    }
}

fn is_numeric(t: Type) -> bool {
    matches!(t, Type::Int | Type::Float)
}

impl<'a> Checker<'a> {
    // ── scope management ────────────────────────────────────────────────
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind(&mut self, name: String, declared: Option<Type>, inferred: Type) {
        self.scopes
            .last_mut()
            .expect("at least one scope")
            .insert(name, VarType { declared, inferred });
    }

    fn lookup(&self, name: &str) -> Option<&VarType> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn warn(&mut self, span: SourceSpan, message: String) {
        self.diags.push(Diagnostic { span, message });
    }

    /// Site 1: warn on a written-but-unrecognized type name.
    fn check_type_ann(&mut self, ann: &TypeAnn, span: SourceSpan) {
        if ann.resolved.is_none() {
            self.warn(span, format!("unknown type name `{}`", ann.name));
        }
    }

    /// Bind every variable a pattern introduces as `Any`, so pattern names
    /// shadow any outer typed binding (never a false positive from an arm body).
    fn bind_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard | Pattern::Literal(_) => {}
            Pattern::Variable(n) => self.bind(n.clone(), None, Type::Any),
            Pattern::Variant { fields, .. } => {
                for f in fields {
                    self.bind_pattern(f);
                }
            }
            Pattern::List { elements, rest } => {
                for e in elements {
                    self.bind_pattern(e);
                }
                if let Some(r) = rest {
                    self.bind(r.clone(), None, Type::Any);
                }
            }
            Pattern::Record(fields) => {
                for (_, p) in fields {
                    self.bind_pattern(p);
                }
            }
        }
    }

    // ── statement walk ──────────────────────────────────────────────────
    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, ty, value } => {
                if let Some(ann) = ty {
                    self.check_type_ann(ann, stmt.span);
                }
                let inferred = self.check_expr(value);
                let declared = ty.as_ref().and_then(|t| t.resolved);
                if let Some(dt) = declared {
                    if inferred != Type::Any && dt != Type::Any && !inferred.is_assignable_to(&dt) {
                        self.warn(
                            value.span,
                            format!(
                                "type mismatch: `{}` declared `{}` but assigned `{}`",
                                name,
                                dt.name(),
                                inferred.name()
                            ),
                        );
                    }
                }
                self.bind(name.clone(), declared, inferred);
            }
            StmtKind::Assign { target, value } => match target {
                AssignTarget::Name(n) => {
                    let vt = self.check_expr(value);
                    let declared = self.lookup(n).and_then(|v| v.declared);
                    if let Some(dt) = declared {
                        if vt != Type::Any && dt != Type::Any && !vt.is_assignable_to(&dt) {
                            self.warn(
                                value.span,
                                format!(
                                    "type mismatch: `{}` declared `{}` but assigned `{}`",
                                    n,
                                    dt.name(),
                                    vt.name()
                                ),
                            );
                        }
                    }
                }
                AssignTarget::Field(object, _) => {
                    self.check_expr(object);
                    self.check_expr(value);
                }
                AssignTarget::Index(object, index) => {
                    self.check_expr(object);
                    self.check_expr(index);
                    self.check_expr(value);
                }
            },
            StmtKind::Expr(e) => {
                self.check_expr(e);
            }
            StmtKind::FnDecl {
                name,
                params,
                ret,
                body,
            } => {
                // Site 1 for params + return.
                for p in params {
                    if let Some(ann) = &p.ty {
                        self.check_type_ann(ann, stmt.span);
                    }
                }
                if let Some(ann) = ret {
                    self.check_type_ann(ann, stmt.span);
                }
                self.push_scope();
                for p in params {
                    let declared = p.ty.as_ref().and_then(|t| t.resolved);
                    let inferred = declared.unwrap_or(Type::Any);
                    self.bind(p.name.clone(), declared, inferred);
                }
                let (tail_ty, tail_span) = self.check_block_body(body);
                if let Some(ann) = ret {
                    if let Some(rt) = ann.resolved {
                        if tail_ty != Type::Any
                            && rt != Type::Any
                            && !tail_ty.is_assignable_to(&rt)
                        {
                            let span = tail_span.unwrap_or(stmt.span);
                            self.warn(
                                span,
                                format!(
                                    "return type mismatch: `{}` declares `{}` but returns `{}`",
                                    name,
                                    rt.name(),
                                    tail_ty.name()
                                ),
                            );
                        }
                    }
                }
                self.pop_scope();
            }
            StmtKind::EnumDecl { .. } => {}
            StmtKind::For { var, iter, body } => {
                self.check_expr(iter);
                self.push_scope();
                self.bind(var.clone(), None, Type::Any);
                self.check_block_body(body);
                self.pop_scope();
            }
            StmtKind::While { condition, body } => {
                self.check_expr(condition);
                self.push_scope();
                self.check_block_body(body);
                self.pop_scope();
            }
            StmtKind::Return(value) => {
                if let Some(e) = value {
                    self.check_expr(e);
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::State { name, init, key, .. } => {
                self.check_expr(init);
                if let Some(k) = key {
                    self.check_expr(k);
                }
                // Reactive binding: infer nothing (Any) and shadow any outer name.
                self.bind(name.clone(), None, Type::Any);
            }
            StmtKind::Import(_) => {}
        }
    }

    /// Walk a block's statements in a scope the caller already entered, returning
    /// the block's tail-expression type and span: the last statement's expression
    /// when it is a bare `Expr`, else `Any`/none.
    fn check_block_body(&mut self, stmts: &[Stmt]) -> (Type, Option<SourceSpan>) {
        let mut tail = (Type::Any, None);
        let last = stmts.len().wrapping_sub(1);
        for (i, stmt) in stmts.iter().enumerate() {
            if i == last {
                if let StmtKind::Expr(e) = &stmt.kind {
                    let t = self.check_expr(e);
                    tail = (t, Some(e.span));
                    continue;
                }
            }
            self.check_stmt(stmt);
        }
        tail
    }

    /// Push a fresh scope, walk a nested block, pop, and yield its tail type.
    fn check_block_scoped(&mut self, stmts: &[Stmt]) -> Type {
        self.push_scope();
        let (ty, _) = self.check_block_body(stmts);
        self.pop_scope();
        ty
    }

    // ── expression walk + inference (folded) ────────────────────────────
    /// Walk an expression (emitting nested diagnostics, incl. call-arg checks)
    /// and return its conservatively inferred [`Type`].
    fn check_expr(&mut self, expr: &Expr) -> Type {
        match &expr.kind {
            ExprKind::Literal(lit) => match lit {
                Literal::Nil => Type::Nil,
                Literal::Bool(_) => Type::Bool,
                Literal::Int(_) => Type::Int,
                Literal::Float(_) => Type::Float,
                Literal::String(_) => Type::String,
            },
            ExprKind::Ident(name) => self
                .lookup(name)
                .map(|v| v.effective())
                .unwrap_or(Type::Any),
            ExprKind::AtVar(_) => Type::Any,
            ExprKind::BinaryOp { op, left, right } => {
                let l = self.check_expr(left);
                let r = self.check_expr(right);
                binary_type(*op, l, r)
            }
            ExprKind::UnaryOp { op, operand } => {
                let t = self.check_expr(operand);
                match op {
                    UnaryOp::Not => Type::Bool,
                    UnaryOp::Neg => match t {
                        Type::Int => Type::Int,
                        Type::Float => Type::Float,
                        _ => Type::Any,
                    },
                }
            }
            ExprKind::Call { function, args } => {
                self.check_expr(function);
                let arg_types: Vec<Type> = args.iter().map(|a| self.check_expr(a)).collect();
                self.check_call(function, args, &arg_types)
            }
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.check_expr(condition);
                let then_ty = self.check_block_scoped(then_body);
                match else_body {
                    Some(ElseBranch::Block(stmts)) => {
                        let else_ty = self.check_block_scoped(stmts);
                        join(then_ty, else_ty)
                    }
                    Some(ElseBranch::ElseIf(e)) => {
                        self.check_expr(e);
                        Type::Any
                    }
                    None => Type::Any,
                }
            }
            ExprKind::Match { subject, arms } => {
                self.check_expr(subject);
                let mut result: Option<Type> = None;
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.check_expr(g);
                    }
                    let t = self.check_expr(&arm.body);
                    self.pop_scope();
                    result = Some(match result {
                        None => t,
                        Some(prev) => join(prev, t),
                    });
                }
                result.unwrap_or(Type::Any)
            }
            ExprKind::For { var, iter, body } => {
                self.check_expr(iter);
                self.push_scope();
                self.bind(var.clone(), None, Type::Any);
                self.check_block_body(body);
                self.pop_scope();
                Type::List
            }
            ExprKind::List(items) => {
                for it in items {
                    self.check_expr(it);
                }
                Type::List
            }
            ExprKind::Record(fields) => {
                for f in fields {
                    match f {
                        RecordField::Named(_, e) | RecordField::Spread(e) => {
                            self.check_expr(e);
                        }
                    }
                }
                Type::Record
            }
            ExprKind::FieldAccess { object, .. } => {
                self.check_expr(object);
                Type::Any
            }
            ExprKind::IndexAccess { object, index } => {
                self.check_expr(object);
                self.check_expr(index);
                Type::Any
            }
            ExprKind::Block(stmts) => self.check_block_scoped(stmts),
            ExprKind::Lambda { params, body } => {
                for p in params {
                    if let Some(ann) = &p.ty {
                        self.check_type_ann(ann, expr.span);
                    }
                }
                self.push_scope();
                for p in params {
                    let declared = p.ty.as_ref().and_then(|t| t.resolved);
                    let inferred = declared.unwrap_or(Type::Any);
                    self.bind(p.name.clone(), declared, inferred);
                }
                self.check_block_body(body);
                self.pop_scope();
                Type::Function
            }
            ExprKind::StringInterp { exprs, .. } => {
                for e in exprs {
                    self.check_expr(e);
                }
                Type::String
            }
            ExprKind::Element {
                props, children, ..
            } => {
                for (_, e) in props {
                    self.check_expr(e);
                }
                for c in children {
                    if let JsxChild::Expr(e) = c {
                        self.check_expr(e);
                    }
                }
                Type::Element
            }
        }
    }

    /// Resolve a call's result type and, for a statically-known named function,
    /// check each argument against its declared parameter type (site 5). Assumes
    /// the args were already visited; `arg_types` are their inferred types.
    fn check_call(&mut self, function: &Expr, args: &[Expr], arg_types: &[Type]) -> Type {
        let ExprKind::Ident(f) = &function.kind else {
            return Type::Any;
        };
        // A local binding shadows the function/builtin name.
        if self.lookup(f).is_some() {
            return Type::Any;
        }
        // Sanctioned cast builtins produce a concrete type.
        match f.as_str() {
            "int" => return Type::Int,
            "float" => return Type::Float,
            "str" => return Type::String,
            _ => {}
        }
        let Some(sig) = self.fn_signatures.get(&(f.clone(), args.len())).cloned() else {
            return Type::Any;
        };
        for (i, pt) in sig.params.iter().enumerate() {
            let Some(pt) = pt else { continue };
            if *pt == Type::Any {
                continue;
            }
            let at = arg_types[i];
            if at != Type::Any && !at.is_assignable_to(pt) {
                self.warn(
                    args[i].span,
                    format!(
                        "argument {} to `{}`: expected `{}`, found `{}`",
                        i + 1,
                        f,
                        pt.name(),
                        at.name()
                    ),
                );
            }
        }
        sig.ret.unwrap_or(Type::Any)
    }
}

/// Conservative result type of a binary operator given operand types.
fn binary_type(op: BinOp, l: Type, r: Type) -> Type {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
            if l == Type::Int && r == Type::Int {
                Type::Int
            } else if is_numeric(l) && is_numeric(r) {
                // At least one is Float here (both-Int handled above).
                Type::Float
            } else {
                Type::Any
            }
        }
        BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => Type::Bool,
        BinOp::Concat => {
            if l == Type::String && r == Type::String {
                Type::String
            } else {
                Type::Any
            }
        }
        BinOp::Coalesce => Type::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::check_module;

    fn warns(src: &str) -> Vec<String> {
        let (_, mut stmts) = crate::rewrite::parse_ast(src).expect("parse");
        crate::desugar::desugar(&mut stmts);
        let sigs = crate::compiler::collect_fn_signatures(&stmts);
        check_module(&stmts, &sigs)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn let_matching_type_no_warning() {
        assert!(warns("let x: int = 5").is_empty());
    }

    #[test]
    fn let_type_mismatch_warns() {
        let w = warns("let x: int = \"hi\"");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains('x') && w[0].contains("int") && w[0].contains("string"));
    }

    #[test]
    fn int_promotes_to_float() {
        assert!(warns("let x: float = 3").is_empty());
    }

    #[test]
    fn float_not_assignable_to_int() {
        let w = warns("let x: int = 3.5");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn cast_builtin_fixes_mismatch() {
        assert!(warns("let x: int = int(\"5\")").is_empty());
    }

    #[test]
    fn any_suppresses() {
        assert!(warns("let x: any = \"hi\"").is_empty());
    }

    #[test]
    fn unknown_rhs_infers_any() {
        assert!(warns("let x: int = y").is_empty());
    }

    #[test]
    fn unknown_type_name_warns() {
        let w = warns("let x: banana = 5");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("banana"));
    }

    #[test]
    fn fn_return_mismatch_warns() {
        let w = warns("fn f() -> int\n  \"no\"\nend");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn fn_return_match_no_warning() {
        assert!(warns("fn f() -> int\n  5\nend").is_empty());
    }

    #[test]
    fn fn_return_int_promotes_to_float() {
        assert!(warns("fn f() -> float\n  5\nend").is_empty());
    }

    #[test]
    fn call_arg_mismatch_warns() {
        let w = warns("fn area(r: float) -> float\n  r\nend\narea(\"x\")");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("argument 1"));
    }

    #[test]
    fn call_arg_int_promotes() {
        assert!(warns("fn area(r: float) -> float\n  r\nend\narea(2)").is_empty());
    }

    #[test]
    fn call_arg_float_ok() {
        assert!(warns("fn area(r: float) -> float\n  r\nend\narea(2.0)").is_empty());
    }

    #[test]
    fn param_unknown_type_warns() {
        let w = warns("fn f(a: banana)\n  a\nend");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("banana"));
    }

    #[test]
    fn reassignment_conflict_warns() {
        let w = warns("let x: int = 1\nx = \"s\"");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn reassignment_ok_no_warning() {
        assert!(warns("let x: int = 1\nx = 2").is_empty());
    }

    #[test]
    fn unannotated_program_is_silent() {
        assert!(warns("let a = 1\nlet b = a + 2\nprint(b)").is_empty());
    }
}
