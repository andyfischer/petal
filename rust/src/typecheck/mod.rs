//! Optional static type checker (warning-only).
//!
//! See docs/dev/type-declarations-plan.md §7. This pass is SHALLOW, LOCAL, and
//! CONSERVATIVE: a false positive (warning on correct code) is far worse than a
//! false negative. Whenever inference is at all ambiguous we infer [`Type::Any`],
//! which suppresses every check. The checker NEVER errors and NEVER blocks
//! compilation — it only accumulates [`Diagnostic`]s.

use std::collections::HashMap;

pub mod builtin_types;
pub mod unused;

use crate::ast::{
    AssignTarget, BinOp, ElseBranch, Expr, ExprKind, JsxChild, Literal, Param, Pattern,
    RecordField, Stmt, StmtKind, TypeAnn, UnaryOp,
};
use crate::classes::ClassTable;
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

/// A call to a cast builtin whose argument is *already* that type, so the cast
/// is the identity — `int(n)` on an `int`, `float(f)` on a `float`, `str(s)` on
/// a `string`. Reported with the spans [`crate::lint`] needs to delete the cast
/// without reprinting the argument.
#[derive(Debug, Clone, Copy)]
pub struct RedundantCast {
    /// The whole `int(...)` call expression.
    pub call: SourceSpan,
    /// Just the argument expression, which is what remains after the fix.
    pub arg: SourceSpan,
    /// The cast builtin's name, for the report line.
    pub name: &'static str,
    /// Whether the argument is a single term, so it stands alone with no
    /// parentheses at all. False for operator expressions and block forms.
    pub arg_is_atomic: bool,
    /// Where the cast sits, which decides what has to replace its parentheses.
    pub slot: CastSlot,
}

/// The syntactic position of a cast call, as far as removing it is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastSlot {
    /// The expression fills a slot with unambiguous edges of its own: an
    /// assignment's right-hand side, a `return` value, a statement, the lone
    /// argument of a call. `let e = int(a + 1)` becomes `let e = a + 1`.
    Delimited,
    /// An operand of a larger expression. Parentheses must replace the call's:
    /// `2 * int(a + b)` becomes `2 * (a + b)`, never `2 * a + b`.
    Operand,
    /// One element of a comma-separated list — a multi-argument call, a list
    /// literal, a record value. Commas are required between elements, so the
    /// slot is always bounded by a real separator and the parens can simply go:
    /// `f(int(a + 1), int(b + 1))` → `f(a + 1, b + 1)`, with no risk of a
    /// neighbour binding across the boundary.
    ListElement,
}

struct Checker<'a> {
    fn_signatures: &'a HashMap<(String, usize), FnSignature>,
    /// Classes in scope for this module — the built-ins plus every `class`
    /// the compiler's prescan found. Resolves class names in type position,
    /// types field reads, and answers "does this class have that method?".
    classes: &'a ClassTable,
    scopes: Vec<HashMap<String, VarType>>,
    /// The declared return type of each enclosing function-like scope, innermost
    /// last. `Some((ty, name))` when the nearest `fn` declared a resolved return
    /// type; `None` for a lambda or an un-annotated `fn`. `return <expr>` is
    /// checked against the top entry — `return` is function-local at runtime, so
    /// a `return` inside a lambda is unchecked (its `None` frame).
    ret_stack: Vec<Option<(Type, String)>>,
    diags: Vec<Diagnostic>,
    casts: Vec<RedundantCast>,
    /// The slot the next expression walked will occupy. Set immediately before
    /// the `check_expr` call that enters it, and read (and reset to the
    /// [`CastSlot::Operand`] default) on entry, so it describes that one
    /// expression and never leaks into its subexpressions.
    slot: CastSlot,
}

/// Walk a module once, returning both products of the pass: the warnings and
/// the identity casts found along the way.
fn run(
    stmts: &[Stmt],
    fn_signatures: &HashMap<(String, usize), FnSignature>,
    classes: &ClassTable,
) -> (Vec<Diagnostic>, Vec<RedundantCast>) {
    let mut checker = Checker {
        fn_signatures,
        classes,
        scopes: vec![HashMap::new()],
        ret_stack: Vec::new(),
        diags: Vec::new(),
        casts: Vec::new(),
        slot: CastSlot::Operand,
    };
    for stmt in stmts {
        checker.check_stmt(stmt);
    }
    (checker.diags, checker.casts)
}

/// Type-check a module's statements against its (globally collected) function
/// signatures, returning any non-fatal warnings. Never fails.
pub fn check_module(
    stmts: &[Stmt],
    fn_signatures: &HashMap<(String, usize), FnSignature>,
    classes: &ClassTable,
) -> Vec<Diagnostic> {
    run(stmts, fn_signatures, classes).0
}

/// Every `int`/`float`/`str` call whose argument the checker already proved to
/// be that type. Same inference as [`check_module`], so it inherits the pass's
/// conservatism: anything ambiguous is `Any` and yields no result here.
pub fn find_redundant_casts(
    stmts: &[Stmt],
    fn_signatures: &HashMap<(String, usize), FnSignature>,
    classes: &ClassTable,
) -> Vec<RedundantCast> {
    run(stmts, fn_signatures, classes).1
}

/// Least-upper-bound used to type a branching expression: identical types keep
/// their type, anything else collapses to `Any` (suppressing further checks).
fn join(a: Type, b: Type) -> Type {
    if a == b { a } else { Type::Any }
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

    /// The type an annotation denotes here. The parser resolves the built-in
    /// vocabulary without context; a class name can only be resolved against
    /// this module's [`ClassTable`], which is what this adds.
    fn resolve_ann(&self, ann: &TypeAnn) -> Option<Type> {
        ann.resolved
            .or_else(|| self.classes.lookup(&ann.name).map(Type::Class))
    }

    /// How a type is spelled in a diagnostic — the class's own name for a
    /// class, [`Type::name`] otherwise.
    fn spell(&self, ty: Type) -> String {
        ty.display(self.classes).into_owned()
    }

    /// Site 1: warn on a written-but-unrecognized type name. A class declared
    /// anywhere in the module counts as recognized, wherever it is written.
    fn check_type_ann(&mut self, ann: &TypeAnn, span: SourceSpan) {
        if self.resolve_ann(ann).is_none() {
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
                    self.spell(dt),
                    self.spell(actual)
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
                    self.spell(rt),
                    self.spell(actual)
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
            let declared = p.ty.as_ref().and_then(|t| self.resolve_ann(t));
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
            StmtKind::Let {
                name,
                ty,
                value,
                is_var,
            } => {
                if let Some(ann) = ty {
                    self.check_type_ann(ann, stmt.span);
                }
                self.slot = CastSlot::Delimited;
                let inferred = self.check_expr(value);
                let declared = ty.as_ref().and_then(|t| self.resolve_ann(t));
                self.check_assignment(value.span, name, declared, inferred);
                // A `var` is a cell, and `set` reaches it from inside functions,
                // closures and conditionals that this linear walk cannot
                // correlate with the declaration
                // (docs/dev/var-next-steps.md, Cells). So the initializer
                // says nothing about what a later *read* observes, and trusting
                // it produces warnings on correct code — the one outcome this
                // pass is built to avoid. Only a written annotation constrains a
                // cell, and it earns that by constraining every `set` too.
                self.bind(
                    name.clone(),
                    declared,
                    if *is_var { Type::Any } else { inferred },
                );
            }
            // A `set` writes the same value into the same target shape as `=`;
            // the declared type of a `var` constrains its writes identically.
            StmtKind::Assign { target, value } | StmtKind::Set { target, value } => match target {
                AssignTarget::Name(n) => {
                    self.slot = CastSlot::Delimited;
                    let vt = self.check_expr(value);
                    let declared = self.lookup(n).and_then(|v| v.declared);
                    self.check_assignment(value.span, n, declared, vt);
                }
                // Field and index writes are walked for nested diagnostics but
                // the written value itself is unchecked, `var` or not: `Type` is
                // unparameterized, so `record`/`list` carry no field or element
                // type for it to conflict with. Blocked on parameterized types,
                // a locked non-goal (type-declarations-plan.md §1) — not a
                // `var`-specific hole.
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
                self.slot = CastSlot::Delimited;
                self.check_expr(e);
            }
            StmtKind::FnDecl {
                name,
                params,
                ret,
                body,
                ..
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
                    .and_then(|ann| self.resolve_ann(ann))
                    .map(|rt| (rt, name.clone()));
                self.ret_stack.push(ctx);
                let (tail_ty, tail_span) = self.check_block_body(body);
                self.check_return_type(tail_ty, tail_span.unwrap_or(stmt.span));
                self.ret_stack.pop();
                self.pop_scope();
            }
            StmtKind::EnumDecl { .. } => {}
            // A class body declares no code — only field annotations, which
            // get the same unknown-name check as a parameter's. Everything
            // structural (duplicate fields, the class's own name) is the
            // compiler prescan's job, and it errors rather than warning.
            StmtKind::ClassDecl { fields, .. } => {
                for f in fields {
                    if let Some(ann) = &f.ty {
                        self.check_type_ann(ann, stmt.span);
                    }
                }
            }
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
                    self.slot = CastSlot::Delimited;
                    let ty = self.check_expr(e);
                    // Check the returned value against the enclosing fn's
                    // declared return type (bare `return` → nil is left
                    // unchecked, to avoid warning on early-exit patterns).
                    self.check_return_type(ty, e.span);
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::State {
                name,
                ty,
                init,
                key,
                ..
            } => {
                if let Some(ann) = ty {
                    self.check_type_ann(ann, stmt.span);
                }
                self.slot = CastSlot::Delimited;
                let inferred = self.check_expr(init);
                if let Some(k) = key {
                    self.slot = CastSlot::Delimited;
                    self.check_expr(k);
                }
                let declared = ty.as_ref().and_then(|t| self.resolve_ann(t));
                self.check_assignment(init.span, name, declared, inferred);
                // A reactive binding infers *nothing* (`Any`), and shadows any
                // outer name: the next frame re-runs the initializer against a
                // persisted value, and a `set` on a `state var` can replace it
                // from anywhere — so the initializer describes at most the first
                // read. Only a written annotation constrains a state name, and
                // it earns that by constraining every write to it, exactly as a
                // `var` cell does (docs/dev/var-next-steps.md, Cells).
                self.bind(name.clone(), declared, Type::Any);
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
                    self.slot = CastSlot::Delimited;
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
        // Consume the caller's "this slot is already delimited" hint: it
        // describes *this* expression, and every subexpression is nested inside
        // something and so is not delimited.
        let slot = std::mem::replace(&mut self.slot, CastSlot::Operand);
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
                // A lone argument fills the call's parens on its own; with two
                // or more, each is an element of a comma-separated list. Since
                // commas are required, both slots are bounded by real
                // delimiters and neither needs parentheses kept — the two kinds
                // are still distinguished so the rewriter can report which slot
                // an edit came from.
                let arg_slot = if args.len() == 1 {
                    CastSlot::Delimited
                } else {
                    CastSlot::ListElement
                };
                let arg_types: Vec<Type> = args
                    .iter()
                    .map(|a| {
                        self.slot = arg_slot;
                        self.check_expr(a)
                    })
                    .collect();
                self.note_redundant_cast(expr, function, args, &arg_types, slot);
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
                    self.slot = CastSlot::ListElement;
                    self.check_expr(it);
                }
                Type::List
            }
            ExprKind::Record(fields) => {
                for f in fields {
                    match f {
                        RecordField::Named(_, e) | RecordField::Spread(e) => {
                            self.slot = CastSlot::ListElement;
                            self.check_expr(e);
                        }
                    }
                }
                Type::Record
            }
            ExprKind::FieldAccess { object, field } => {
                let obj = self.check_expr(object);
                // A class instance has declared field types; a plain record
                // does not (`Type` is unparameterized), so everything else
                // stays `Any`.
                match obj {
                    Type::Class(id) => self
                        .classes
                        .get(id)
                        .field(field)
                        .and_then(|f| f.ty)
                        .unwrap_or(Type::Any),
                    _ => Type::Any,
                }
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
                    self.slot = CastSlot::ListElement;
                    self.check_expr(e);
                }
                for c in children {
                    if let JsxChild::Expr(e) = c {
                        self.slot = CastSlot::ListElement;
                        self.check_expr(e);
                    }
                }
                Type::Element
            }
        }
    }

    /// Record `int(n)` / `float(f)` / `str(s)` calls whose argument already has
    /// the cast's own type. Only the builtin counts: a local binding or a
    /// module `fn` of the same name shadows it and could do anything.
    fn note_redundant_cast(
        &mut self,
        call: &Expr,
        function: &Expr,
        args: &[Expr],
        arg_types: &[Type],
        slot: CastSlot,
    ) {
        let ExprKind::Ident(f) = &function.kind else {
            return;
        };
        let [arg] = args else { return };
        if self.lookup(f).is_some() || self.fn_signatures.contains_key(&(f.clone(), 1)) {
            return;
        }
        let name = match (f.as_str(), arg_types[0]) {
            ("int", Type::Int) => "int",
            ("float", Type::Float) => "float",
            ("str", Type::String) => "str",
            _ => return,
        };
        self.casts.push(RedundantCast {
            call: call.span,
            arg: arg.span,
            name,
            arg_is_atomic: is_atomic(&arg.kind),
            slot,
        });
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
        // A class name is its constructor: `Point(1, 2)` builds an instance,
        // and the declared field types check the arguments positionally — the
        // same rule as a function's parameters, because that is what they are.
        if let Some(id) = self.classes.lookup(f) {
            let fields = self.classes.get(id).fields.clone();
            for (i, fd) in fields.iter().enumerate() {
                let (Some(ft), Some(at)) = (fd.ty, arg_types.get(i).copied()) else {
                    continue;
                };
                if ft != Type::Any && at != Type::Any && !at.is_assignable_to(&ft) {
                    self.warn(
                        args[i].span,
                        format!(
                            "argument {} to `{}`: field `{}` expects `{}`, found `{}`",
                            i + 1,
                            f,
                            fd.name,
                            self.spell(ft),
                            self.spell(at)
                        ),
                    );
                }
            }
            return Type::Class(id);
        }
        let Some(sig) = self.fn_signatures.get(&(f.clone(), args.len())).cloned() else {
            // Not a module function: fall back to the builtin table. It knows
            // only result types (builtins declare no parameter types), so
            // there is nothing to check the arguments against.
            return builtin_types::builtin_return_type(f, arg_types).unwrap_or(Type::Any);
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
                        self.spell(*pt),
                        self.spell(at)
                    ),
                );
            }
        }
        sig.ret.unwrap_or(Type::Any)
    }
}

/// Whether an expression keeps its meaning with no parentheses around it, so a
/// wrapping call's parens can simply be deleted. Anything with an operator or a
/// block form keeps them.
fn is_atomic(kind: &ExprKind) -> bool {
    matches!(
        kind,
        ExprKind::Literal(_)
            | ExprKind::Ident(_)
            | ExprKind::AtVar(_)
            | ExprKind::Call { .. }
            | ExprKind::FieldAccess { .. }
            | ExprKind::IndexAccess { .. }
            | ExprKind::List(_)
            | ExprKind::Record(_)
            | ExprKind::StringInterp { .. }
            | ExprKind::Element { .. }
    )
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
        let mut classes = crate::classes::ClassTable::new();
        crate::compiler::collect_classes(&mut classes, &stmts);
        let sigs = crate::compiler::collect_fn_signatures(&stmts, &classes);
        check_module(&stmts, &sigs, &classes)
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
        assert!(
            warns("fn f() -> int\n  let g = fn(x)\n    return \"s\"\n  end\n  0\nend").is_empty()
        );
    }

    #[test]
    fn nested_fn_return_checked_against_own_signature() {
        let w = warns("fn a() -> int\n  fn b() -> string\n    return 7\n  end\n  0\nend");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`b`"), "{w:?}");
    }

    // ── `var` cells: writes are checked, reads are not trusted ──────────────
    // docs/dev/var-next-steps.md (Cells).
    #[test]
    fn set_conflicting_with_declared_var_type_warns() {
        let w = warns("var n: int = 0\nset n = \"hello\"");
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(
            w[0],
            "type mismatch: `n` declared `int` but assigned `string`"
        );
    }

    #[test]
    fn set_matching_declared_var_type_no_warning() {
        assert!(warns("var n: int = 0\nset n = 5").is_empty());
    }

    #[test]
    fn set_int_promotes_to_float_var() {
        assert!(warns("var n: float = 0.0\nset n = 5").is_empty());
    }

    #[test]
    fn var_initializer_conflict_warns() {
        let w = warns("var n: int = \"hi\"");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn set_from_inside_a_function_is_checked() {
        // The point of a cell: the write is nowhere near the declaration.
        let w = warns("var n: int = 0\nfn f()\n  set n = \"s\"\nend\nf()");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("declared `int`"), "{w:?}");
    }

    #[test]
    fn set_from_inside_a_closure_under_control_flow_is_checked() {
        let w = warns("var n: int = 0\nlet g = fn(b)\n  if b then set n = \"s\" end\nend\ng(true)");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn unannotated_var_read_is_not_typed_by_its_initializer() {
        // `set` may retype the cell from anywhere, so trusting the initializer
        // would warn on a correct program.
        assert!(warns("var n = 0\nset n = \"hi\"\nlet s: string = n").is_empty());
        assert!(warns("fn g(s: string)\n  s\nend\nvar n = 0\nset n = \"hi\"\ng(n)").is_empty());
        assert!(warns("var n = 0\nset n = \"hi\"\nfn f() -> string\n  n\nend").is_empty());
    }

    #[test]
    fn annotated_var_read_uses_its_declared_type() {
        let w = warns("var n: int = 0\nlet s: string = n");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`s` declared `string`"), "{w:?}");
    }

    #[test]
    fn unannotated_state_var_is_unconstrained_in_both_directions() {
        // Without an annotation there is nothing to conflict with: a reactive
        // binding infers nothing, exactly like an un-annotated `var` cell.
        assert!(warns("state var n = 0\nset n = \"hi\"").is_empty());
        assert!(warns("state var n = 0\nset n = \"hi\"\nlet s: string = n").is_empty());
        assert!(warns("state n = 0\nn = \"hi\"").is_empty());
    }

    // ── `state` annotations: `state n: int = 0` behaves like `var n: int = 0` ─
    #[test]
    fn state_annotation_matching_type_no_warning() {
        assert!(warns("state n: int = 0").is_empty());
        assert!(warns("state var n: float = 0.0").is_empty());
        // int still promotes into a float slot.
        assert!(warns("state n: float = 0").is_empty());
    }

    #[test]
    fn state_initializer_conflict_warns() {
        let w = warns("state n: int = \"hi\"");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("`n` declared `int`") && w[0].contains("string"),
            "{w:?}"
        );
    }

    #[test]
    fn state_unknown_type_name_warns() {
        let w = warns("state n: banana = 0");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("banana"), "{w:?}");
    }

    #[test]
    fn annotated_state_read_uses_its_declared_type() {
        let w = warns("state n: int = 0\nlet s: string = n");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`s` declared `string`"), "{w:?}");
    }

    #[test]
    fn annotated_state_var_constrains_every_set() {
        // The point of a cell: the write is far from the declaration. An
        // annotated `state var` earns its typed reads by checking every `set`.
        let w = warns("state var n: int = 0\nset n = \"hi\"");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`n` declared `int`"), "{w:?}");
        assert!(warns("state var n: int = 0\nset n = 5").is_empty());
        let inner = warns(
            "state var n: int = 0\nlet g = fn(b)\n  if b then set n = \"s\" end\nend\ng(true)",
        );
        assert_eq!(inner.len(), 1, "{inner:?}");
    }

    #[test]
    fn annotated_state_with_an_explicit_key_is_checked() {
        assert!(warns("state(1) n: int = 0").is_empty());
        let w = warns("state(1) n: int = \"hi\"");
        assert_eq!(w.len(), 1, "{w:?}");
        // The key expression is still walked for nested diagnostics.
        let k = warns("fn g(s: string)\n  s\nend\nstate(g(1)) n: int = 0");
        assert_eq!(k.len(), 1, "{k:?}");
        assert!(k[0].contains("argument 1"), "{k:?}");
    }

    #[test]
    fn annotated_state_reassignment_is_checked() {
        let w = warns("state n: int = 0\nn = \"hi\"");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`n` declared `int`"), "{w:?}");
    }

    #[test]
    fn set_through_a_field_target_is_unchecked_but_walks_its_parts() {
        // No field types exist to check the value against; the subexpressions
        // are still visited, so a nested mismatch is still reported.
        assert!(warns("var r: record = {a: 1}\nset r.a = \"s\"").is_empty());
        let w = warns("fn g(s: string)\n  s\nend\nvar r: record = {a: 1}\nset r.a = g(1)");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("argument 1"), "{w:?}");
    }

    // ── Fix #3: `nil`/`enum` type names parse (checked via full pipeline) ────
    #[test]
    fn nil_type_annotation_parses_and_checks() {
        assert!(warns("let x: nil = nil").is_empty());
        let w = warns("let x: nil = 5");
        assert_eq!(w.len(), 1, "{w:?}");
    }
}
