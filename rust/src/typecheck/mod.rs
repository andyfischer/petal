//! Optional static type checker (warning-only).
//!
//! See docs/dev/type-declarations-plan.md §7. This pass is SHALLOW, LOCAL, and
//! CONSERVATIVE: a false positive (warning on correct code) is far worse than a
//! false negative. Whenever inference is at all ambiguous we infer [`Type::Any`],
//! which suppresses every check. The checker NEVER errors and NEVER blocks
//! compilation — it only accumulates [`Diagnostic`]s.

use std::collections::HashMap;

pub mod unused;

use crate::ast::{
    AssignTarget, BinOp, ElseBranch, Expr, ExprKind, JsxChild, Literal, Param, Pattern, RecordField,
    Stmt, StmtKind, TypeAnn, UnaryOp,
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
    /// The declared return type of each enclosing function-like scope, innermost
    /// last. `Some((ty, name))` when the nearest `fn` declared a resolved return
    /// type; `None` for a lambda or an un-annotated `fn`. `return <expr>` is
    /// checked against the top entry — `return` is function-local at runtime, so
    /// a `return` inside a lambda is unchecked (its `None` frame).
    ret_stack: Vec<Option<(Type, String)>>,
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
        ret_stack: Vec::new(),
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

    /// Warn when `actual` can't be assigned into the slot `name` declared as
    /// `declared`. Shared by `let` initializers and re-assignments. `Any` on
    /// either side is trusted (no warning), matching the conservative policy.
    fn check_assignment(
        &mut self,
        span: SourceSpan,
        name: &str,
        declared: Option<Type>,
        actual: Type,
    ) {
        let Some(dt) = declared else { return };
        if actual != Type::Any && dt != Type::Any && !actual.is_assignable_to(&dt) {
            self.warn(
                span,
                format!(
                    "type mismatch: `{}` declared `{}` but assigned `{}`",
                    name,
                    dt.name(),
                    actual.name()
                ),
            );
        }
    }

    /// Warn when `actual` can't satisfy the enclosing function's declared return
    /// type. Shared by the body's tail expression and every explicit `return`.
    /// A no-op when there's no declared return type in scope, or when either
    /// side is `Any` (trusted).
    fn check_return_type(&mut self, actual: Type, span: SourceSpan) {
        let Some(Some((rt, name))) = self.ret_stack.last() else {
            return;
        };
        let (rt, name) = (*rt, name.clone());
        if actual != Type::Any && rt != Type::Any && !actual.is_assignable_to(&rt) {
            self.warn(
                span,
                format!(
                    "return type mismatch: `{}` declares `{}` but returns `{}`",
                    name,
                    rt.name(),
                    actual.name()
                ),
            );
        }
    }

    /// Warn on any unrecognized parameter annotations, then bind every parameter
    /// into the current scope (its resolved type when present, else `Any`).
    /// Shared by named functions and lambdas; the caller pushes the scope and
    /// supplies the span used for annotation warnings.
    fn check_and_bind_params(&mut self, params: &[Param], span: SourceSpan) {
        for p in params {
            if let Some(ann) = &p.ty {
                self.check_type_ann(ann, span);
            }
        }
        for p in params {
            let declared = p.ty.as_ref().and_then(|t| t.resolved);
            self.bind(p.name.clone(), declared, declared.unwrap_or(Type::Any));
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
                self.check_assignment(value.span, name, declared, inferred);
                self.bind(name.clone(), declared, inferred);
            }
            StmtKind::Assign { target, value } => match target {
                AssignTarget::Name(n) => {
                    let vt = self.check_expr(value);
                    let declared = self.lookup(n).and_then(|v| v.declared);
                    self.check_assignment(value.span, n, declared, vt);
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
                self.push_scope();
                // Site 1 for params + return.
                self.check_and_bind_params(params, stmt.span);
                if let Some(ann) = ret {
                    self.check_type_ann(ann, stmt.span);
                }
                // Record the declared return type so both the tail expression
                // and every explicit `return` in the body are checked against it.
                let ctx = ret
                    .as_ref()
                    .and_then(|ann| ann.resolved)
                    .map(|rt| (rt, name.clone()));
                self.ret_stack.push(ctx);
                let (tail_ty, tail_span) = self.check_block_body(body);
                self.check_return_type(tail_ty, tail_span.unwrap_or(stmt.span));
                self.ret_stack.pop();
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
                    let ty = self.check_expr(e);
                    // Check the returned value against the enclosing fn's
                    // declared return type (bare `return` → nil is left
                    // unchecked, to avoid warning on early-exit patterns).
                    self.check_return_type(ty, e.span);
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
                self.push_scope();
                self.check_and_bind_params(params, expr.span);
                // Lambdas have no declared return type, and `return` is
                // lambda-local at runtime — push a `None` frame so any `return`
                // in the body is not checked against an outer fn's return type.
                self.ret_stack.push(None);
                self.check_block_body(body);
                self.ret_stack.pop();
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
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Type::Bool,
        // `&&`/`||` are value-returning, not strict boolean: at runtime the
        // result type depends on the operands' truthiness (`5 && 10` → `10`,
        // `0 || 42` → `42`), so it isn't statically knowable. Infer `Any` to
        // avoid false positives on idiomatic default/guard code like
        // `let name: string = arg || "default"`.
        BinOp::And | BinOp::Or => Type::Any,
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

    // ── Fix #1: `&&`/`||` are value-returning, not strictly boolean ──────────
    #[test]
    fn logical_or_default_does_not_warn() {
        // `arg || "default"` yields the operand at runtime, not a bool.
        assert!(warns("let name: string = \"\" || \"default\"").is_empty());
    }

    #[test]
    fn logical_and_guard_does_not_warn() {
        assert!(warns("let x: int = 5 && 10").is_empty());
    }

    // ── Fix #2: explicit `return` is checked against the declared return ─────
    #[test]
    fn early_return_mismatch_warns() {
        let w = warns("fn f() -> int\n  return \"nope\"\n  0\nend");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("return type mismatch"), "{w:?}");
    }

    #[test]
    fn early_return_match_no_warning() {
        assert!(warns("fn f() -> int\n  return 5\nend").is_empty());
    }

    #[test]
    fn early_return_int_promotes_to_float() {
        assert!(warns("fn f() -> float\n  return 5\nend").is_empty());
    }

    #[test]
    fn return_inside_lambda_not_checked_against_outer_fn() {
        // `return` is lambda-local; it must not be checked against `f`'s `-> int`.
        assert!(warns("fn f() -> int\n  let g = fn(x)\n    return \"s\"\n  end\n  0\nend").is_empty());
    }

    #[test]
    fn nested_fn_return_checked_against_own_signature() {
        let w = warns("fn a() -> int\n  fn b() -> string\n    return 7\n  end\n  0\nend");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`b`"), "{w:?}");
    }

    // ── Fix #3: `nil`/`enum` type names parse (checked via full pipeline) ────
    #[test]
    fn nil_type_annotation_parses_and_checks() {
        assert!(warns("let x: nil = nil").is_empty());
        let w = warns("let x: nil = 5");
        assert_eq!(w.len(), 1, "{w:?}");
    }
}
