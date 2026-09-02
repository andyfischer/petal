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
    /// The signatures this name may be *called* with, when it is known to hold
    /// a function: one entry per arity. Empty whenever the callee is unknown,
    /// which is what keeps every check below conservative. [`Type`] is a bare
    /// `function` with no arrow inside it, so a call through a binding can only
    /// be checked by carrying the signature alongside the type — see
    /// [`Checker::fn_candidates`].
    fns: Vec<FnSignature>,
    /// The parameter *names* of the same callables, one entry per arity —
    /// parallel to [`VarType::fns`] in what it describes, but matched by
    /// length rather than by index (see [`Checker::callee_param_names`]).
    /// Empty whenever the names are unknown, which suppresses the
    /// named-argument check.
    param_names: Vec<Vec<String>>,
    /// The class this binding's *declaration* implies, when the type system
    /// deliberately refuses to say so. A `state`/`var` binds `any` because the
    /// initializer describes at most the first read — the next frame re-runs
    /// against a persisted value and a `set` can replace it from anywhere. That
    /// is the right call for typing, and this is emphatically **not** a type:
    /// nothing is checked against it and it produces no warning. It is a *hint*
    /// for the one question the type cannot answer — when the class label a
    /// value carries names nothing in the program now running, which class did
    /// the code that declared the slot have in mind? See
    /// [`MethodDispatch::hints`].
    class_hint: Option<Type>,
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
    /// The parameter names of those same module functions, for checking a
    /// call's named arguments against the declaration. See
    /// [`crate::compiler::collect_fn_param_names`].
    fn_param_names: &'a HashMap<(String, usize), Vec<String>>,
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
    /// Method-call sites this pass pinned to one class. See [`MethodDispatch`].
    dispatch: MethodDispatch,
    /// The slot the next expression walked will occupy. Set immediately before
    /// the `check_expr` call that enters it, and read (and reset to the
    /// [`CastSlot::Operand`] default) on entry, so it describes that one
    /// expression and never leaks into its subexpressions.
    slot: CastSlot,
    /// True while walking the *access spine* on the left of a `??`. That spine
    /// compiles to the absence-tolerant field/index reads, so a field the class
    /// does not declare is the whole point of the expression, not a mistake to
    /// warn about. Set immediately before entering the spine and consumed on
    /// entry to each expression, like [`Checker::slot`].
    tolerant_access: bool,
}

/// Walk a module once, returning both products of the pass: the warnings and
/// the identity casts found along the way.
fn run(
    stmts: &[Stmt],
    fn_signatures: &HashMap<(String, usize), FnSignature>,
    fn_param_names: &HashMap<(String, usize), Vec<String>>,
    classes: &ClassTable,
) -> Outcome {
    let mut checker = Checker {
        fn_signatures,
        fn_param_names,
        classes,
        scopes: vec![HashMap::new()],
        ret_stack: Vec::new(),
        diags: Vec::new(),
        casts: Vec::new(),
        dispatch: MethodDispatch::new(),
        slot: CastSlot::Operand,
        tolerant_access: false,
    };
    checker.bind_enum_variants(stmts);
    for stmt in stmts {
        checker.check_stmt(stmt);
    }
    Outcome {
        diags: checker.diags,
        casts: checker.casts,
        dispatch: checker.dispatch,
    }
}

/// Everything one walk of a module produces.
struct Outcome {
    diags: Vec<Diagnostic>,
    casts: Vec<RedundantCast>,
    dispatch: MethodDispatch,
}

/// What this pass learned about each `recv.name(...)` site, keyed by the span
/// of the *call* expression. See [`check_module`].
#[derive(Debug, Default)]
pub struct MethodDispatch {
    /// Sites whose receiver was pinned to exactly one class. The compiler binds
    /// these straight to `fn Class.name`, skipping runtime dispatch entirely.
    pub pinned: HashMap<SourceSpan, String>,
    /// Sites whose receiver has no knowable type — an un-annotated `state` or
    /// `var` — but whose *declaration* named a class ([`VarType::class_hint`]).
    /// These keep runtime dispatch; the class travels with the call only as a
    /// last resort, for when the label the receiver carries names nothing in
    /// the program now running. See `rust/src/backend/bytecode/vm/calls.rs`.
    pub hints: HashMap<SourceSpan, String>,
}

impl MethodDispatch {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Type-check a module's statements against its (globally collected) function
/// signatures. Returns the non-fatal warnings and the statically resolved
/// method-call sites. Never fails.
///
/// **Why the second product.** A `recv.name()` whose receiver class this pass
/// pinned down does not need to be dispatched by the tag the receiver carries:
/// the compiler can bind the call straight to `fn Class.name`, which is what
/// [`Compiler::compile_expr`](crate::compiler::Compiler) does with this map.
/// That matters most across a live edit, where the value in `state` predates
/// the code now running — see `rust/tests/class_live_edit.rs`. Resolution is as
/// shallow as the rest of the pass: a site it cannot pin down keeps the runtime
/// dispatch it always had.
pub fn check_module(
    stmts: &[Stmt],
    fn_signatures: &HashMap<(String, usize), FnSignature>,
    fn_param_names: &HashMap<(String, usize), Vec<String>>,
    classes: &ClassTable,
) -> (Vec<Diagnostic>, MethodDispatch) {
    let out = run(stmts, fn_signatures, fn_param_names, classes);
    (out.diags, out.dispatch)
}

/// Every `int`/`float`/`str` call whose argument the checker already proved to
/// be that type. Same inference as [`check_module`], so it inherits the pass's
/// conservatism: anything ambiguous is `Any` and yields no result here.
pub fn find_redundant_casts(
    stmts: &[Stmt],
    fn_signatures: &HashMap<(String, usize), FnSignature>,
    classes: &ClassTable,
) -> Vec<RedundantCast> {
    // Casts are a type question; no call site's *names* are checked on this
    // path, so the pass runs with no parameter names at all.
    run(stmts, fn_signatures, &HashMap::new(), classes).casts
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
        self.bind_callable(name, declared, inferred, Vec::new(), Vec::new());
    }

    /// Bind a name that is known to hold a function, carrying the signatures it
    /// may be called with (see [`VarType::fns`]).
    fn bind_callable(
        &mut self,
        name: String,
        declared: Option<Type>,
        inferred: Type,
        fns: Vec<FnSignature>,
        param_names: Vec<Vec<String>>,
    ) {
        self.scopes.last_mut().expect("at least one scope").insert(
            name,
            VarType {
                declared,
                inferred,
                fns,
                param_names,
                class_hint: None,
            },
        );
    }

    /// Record the class a just-bound name's declaration implies, for the
    /// bindings whose *type* is deliberately `any`. See [`VarType::class_hint`].
    fn set_class_hint(&mut self, name: &str, hint: Option<Type>) {
        let Some(hint @ Type::Class(_)) = hint else {
            return;
        };
        if let Some(vt) = self.scopes.last_mut().expect("a scope").get_mut(name) {
            vt.class_hint = Some(hint);
        }
    }

    fn lookup(&self, name: &str) -> Option<&VarType> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    /// Replace the callable signatures recorded for an already-bound name, in
    /// the innermost scope that binds it. A re-assignment puts a *different*
    /// function in the slot, so the old signature must not outlive it.
    fn rebind_fns(&mut self, name: &str, fns: Vec<FnSignature>, param_names: Vec<Vec<String>>) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(vt) = scope.get_mut(name) {
                vt.fns = fns;
                vt.param_names = param_names;
                return;
            }
        }
    }

    /// The signatures a callee expression may be invoked with, or empty when
    /// nothing is known — which suppresses every call-site check. Three things
    /// are knowable: a lambda written in place, a name bound to a function
    /// (including a chain of aliases, since each binding carries the same list
    /// forward), and an un-shadowed module function, whose overloads are one
    /// candidate per declared arity.
    fn fn_candidates(&self, expr: &Expr) -> Vec<FnSignature> {
        match &expr.kind {
            ExprKind::Lambda { params, .. } => vec![FnSignature {
                params: params
                    .iter()
                    .map(|p| p.ty.as_ref().and_then(|t| self.resolve_ann(t)))
                    .collect(),
                // Lambdas have no return-type slot (type-declarations-plan §2).
                ret: None,
            }],
            // A bare class name has no signature here — a constructor is
            // checked on its own path, against the class's fields.
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(vt) => vt.fns.clone(),
                None => self.module_signatures(name),
            },
            _ => Vec::new(),
        }
    }

    /// Every declared overload of a module-level function, by arity.
    fn module_signatures(&self, name: &str) -> Vec<FnSignature> {
        let mut sigs: Vec<FnSignature> = self
            .fn_signatures
            .iter()
            .filter(|((n, _), _)| n == name)
            .map(|(_, sig)| sig.clone())
            .collect();
        sigs.sort_by_key(|s| s.params.len());
        sigs
    }

    /// The parameter names a callee expression may be invoked with, one entry
    /// per arity — the naming half of [`Self::fn_candidates`], kept beside it
    /// because a [`FnSignature`] carries types only. Empty when the names are
    /// not statically known, which is what leaves the runtime check as the
    /// only one.
    fn param_name_candidates(&self, expr: &Expr) -> Vec<Vec<String>> {
        match &expr.kind {
            ExprKind::Lambda { params, .. } => {
                vec![params.iter().map(|p| p.name.clone()).collect()]
            }
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(vt) => vt.param_names.clone(),
                None => self
                    .fn_param_names
                    .iter()
                    .filter(|((n, _), _)| n == name)
                    .map(|(_, names)| names.clone())
                    .collect(),
            },
            _ => Vec::new(),
        }
    }

    /// The parameter names of the overload this call selects, or `None` when
    /// nothing declares them. Overloads differ by arity, so the argument count
    /// picks the entry out — the same rule the runtime resolves by.
    fn callee_param_names(&self, expr: &Expr, arity: usize) -> Option<Vec<String>> {
        self.param_name_candidates(expr)
            .into_iter()
            .find(|names| names.len() == arity)
    }

    /// Check a call's named arguments against the parameter names its callee
    /// declares: every name must name a parameter, and no slot may be filled
    /// twice. Runs only where the callee is statically known; the VM's own
    /// binding (`backend/calls.rs`) stays the backstop for everywhere else.
    ///
    /// `what` is the already-quoted callee for the message. Positional
    /// arguments always precede named ones (the parser enforces it), so
    /// argument `i` fills slot `i` until the first name appears.
    fn check_named_args(
        &mut self,
        what: &str,
        params: &[String],
        args: &[Expr],
        arg_names: &[Option<String>],
    ) {
        if arg_names.len() != args.len() || params.len() != args.len() {
            return;
        }
        let mut filled = vec![false; params.len()];
        for (i, name) in arg_names.iter().enumerate() {
            let Some(name) = name else {
                filled[i] = true;
                continue;
            };
            let Some(slot) = params.iter().position(|p| p == name) else {
                self.warn(
                    args[i].span,
                    format!("{what} has no parameter named `{name}`"),
                );
                continue;
            };
            if filled[slot] {
                self.warn(
                    args[i].span,
                    format!("{what} got multiple values for parameter `{name}`"),
                );
                continue;
            }
            filled[slot] = true;
        }
    }

    /// Warn that no candidate accepts `got` arguments. `expected` is every
    /// arity that would have worked — Petal overloads by arity
    /// (docs/function-overloading.md), so a call is wrong only when it matches
    /// *none* of them.
    fn warn_arity(&mut self, span: SourceSpan, what: &str, expected: &[usize], got: usize) {
        let list: Vec<String> = expected.iter().map(|a| a.to_string()).collect();
        self.warn(
            span,
            format!(
                "{what} expects {} argument{}, got {got}",
                list.join(" or "),
                if expected == [1] { "" } else { "s" },
            ),
        );
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
    ///
    /// The warning underlines the annotation itself — `nosuch` in
    /// `fn f(a: nosuch)`, not the whole `fn … end`. `fallback` is used only for
    /// a synthesized annotation that carries no span of its own.
    fn check_type_ann(&mut self, ann: &TypeAnn, fallback: SourceSpan) {
        if self.resolve_ann(ann).is_none() {
            let span = if ann.span.start.line > 0 {
                ann.span
            } else {
                fallback
            };
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
                ..
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
                // (docs/var.md, Cells). So the initializer
                // says nothing about what a later *read* observes, and trusting
                // it produces warnings on correct code — the one outcome this
                // pass is built to avoid. Only a written annotation constrains a
                // cell, and it earns that by constraining every `set` too.
                let (fns, param_names) = if *is_var {
                    (Vec::new(), Vec::new())
                } else {
                    (self.fn_candidates(value), self.param_name_candidates(value))
                };
                self.bind_callable(
                    name.clone(),
                    declared,
                    if *is_var { Type::Any } else { inferred },
                    fns,
                    param_names,
                );
                // The cell's type stays `any`, but the declaration still names a
                // class, and that is worth keeping for dispatch alone.
                if *is_var {
                    self.set_class_hint(name, Some(inferred));
                }
            }
            // A `set` writes the same value into the same target shape as `=`;
            // the declared type of a `var` constrains its writes identically.
            StmtKind::Assign { target, value } | StmtKind::Set { target, value } => match target {
                AssignTarget::Name(n) => {
                    self.slot = CastSlot::Delimited;
                    let vt = self.check_expr(value);
                    let declared = self.lookup(n).and_then(|v| v.declared);
                    self.check_assignment(value.span, n, declared, vt);
                    // The slot holds a different value now: whatever signature
                    // the old one had says nothing about the new one.
                    let fns = self.fn_candidates(value);
                    let param_names = self.param_name_candidates(value);
                    self.rebind_fns(n, fns, param_names);
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
                // `var` cell does (docs/var.md, Cells).
                self.bind(name.clone(), declared, Type::Any);
                // …but the initializer still says which class the slot was
                // meant to hold, and that is the only thing that can answer a
                // stale label after a live edit reshapes the class.
                self.set_class_hint(name, Some(inferred));
            }
            StmtKind::Import(_) => {}
        }
    }

    /// Walk a block's statements in a scope the caller already entered, returning
    /// the block's tail-expression type and span: the last statement's expression
    /// when it is a bare `Expr`, else `Any`/none.
    fn check_block_body(&mut self, stmts: &[Stmt]) -> (Type, Option<SourceSpan>) {
        self.bind_nested_fns(stmts);
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

    /// Bind every `fn` declared directly in a *nested* block, before walking it,
    /// so a call anywhere in the block sees the local declaration rather than a
    /// same-named module function. Only [`Self::fn_signatures`] knows the
    /// top-level ones, and a nested declaration shadows them; binding it here is
    /// what stops the outer signature being checked against the inner function.
    /// Method declarations are excluded — their name is qualified, so they are
    /// never called as a bare identifier.
    fn bind_nested_fns(&mut self, stmts: &[Stmt]) {
        self.bind_enum_variants(stmts);
        let mut sigs: HashMap<&str, Vec<FnSignature>> = HashMap::new();
        // The parameter names of the same declarations, matched by arity where
        // they are read back (see `callee_param_names`).
        let mut names: HashMap<&str, Vec<Vec<String>>> = HashMap::new();
        for stmt in stmts {
            let StmtKind::FnDecl {
                name,
                class: None,
                params,
                ret,
                ..
            } = &stmt.kind
            else {
                continue;
            };
            let sig = FnSignature {
                params: params
                    .iter()
                    .map(|p| p.ty.as_ref().and_then(|t| self.resolve_ann(t)))
                    .collect(),
                ret: ret.as_ref().and_then(|t| self.resolve_ann(t)),
            };
            let entry = sigs.entry(name.as_str()).or_default();
            // Same name, same arity: the later declaration wins, as in
            // `collect_fn_signatures`.
            entry.retain(|s| s.params.len() != sig.params.len());
            entry.push(sig);
            let entry = names.entry(name.as_str()).or_default();
            entry.retain(|n| n.len() != params.len());
            entry.push(params.iter().map(|p| p.name.clone()).collect());
        }
        for (name, mut fns) in sigs {
            fns.sort_by_key(|s| s.params.len());
            let param_names = names.remove(name).unwrap_or_default();
            self.bind_callable(name.to_string(), None, Type::Function, fns, param_names);
        }
    }

    /// Bind every variant an `enum` in these statements declares, as an
    /// unknown value. A variant name is an ordinary binding at runtime, and it
    /// shadows anything else of that name — including a class constructor, which
    /// `enum Shape … Rect(w, h) … end` really does shadow. Binding it is what
    /// stops the class's fields being checked against `Rect(3, 4)`. Nothing is
    /// inferred: a fieldless variant is a value and a variant with fields is a
    /// constructor, and the pass has no type for either.
    fn bind_enum_variants(&mut self, stmts: &[Stmt]) {
        let names: Vec<String> = stmts
            .iter()
            .filter_map(|s| match &s.kind {
                StmtKind::EnumDecl { variants, .. } => Some(variants),
                _ => None,
            })
            .flatten()
            .map(|v| v.name.clone())
            .collect();
        for name in names {
            self.bind(name, None, Type::Any);
        }
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
        let tolerant = std::mem::take(&mut self.tolerant_access);
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
            // `get x` has the type of the cell's contents, which is exactly
            // what a bare read of `x` would report — the annotation on a `var`
            // types every read and constrains every write.
            ExprKind::CellGet(name) => self
                .lookup(name)
                .map(|v| v.effective())
                .unwrap_or(Type::Any),
            ExprKind::AtVar(_) => Type::Any,
            ExprKind::BinaryOp { op, left, right } => {
                // `a.b ?? d` explicitly tolerates `b` being absent — walk the
                // left side with the missing-field warning turned off.
                self.tolerant_access = *op == BinOp::Coalesce;
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
            ExprKind::Call {
                function,
                args,
                arg_names,
                ..
            } => {
                // `recv.name(...)` is method syntax, not a field read followed
                // by a call: `name` is looked up among the receiver's methods
                // (and, failing those, the globals), so walking it as a field
                // would report every method as a missing field.
                let mut method_sig = None;
                match &function.kind {
                    ExprKind::FieldAccess { object, field } => {
                        let recv = self.check_expr(object);
                        method_sig = self.check_method_call(recv, field, args.len(), expr.span);
                        // The receiver has no knowable type, but its declaration
                        // named a class. Not enough to bind the call — the slot
                        // may legitimately hold another class by now — but
                        // enough to answer a label that has gone stale.
                        if recv == Type::Any
                            && let Some(hint) = self.receiver_hint(object)
                        {
                            self.note_dispatch_hint(hint, field, args.len(), expr.span);
                        }
                    }
                    _ => {
                        self.check_expr(function);
                    }
                }
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
                let mut arg_types: Vec<Type> = args
                    .iter()
                    .map(|a| {
                        self.slot = arg_slot;
                        self.check_expr(a)
                    })
                    .collect();
                // Every check below pairs argument *i* with parameter *i*, a
                // pairing named arguments break. Arity and the result type are
                // still right (selection is by total count), so only the
                // per-argument types are dropped — each argument's own
                // expression has already been walked above.
                if !arg_names.is_empty() {
                    arg_types.iter_mut().for_each(|t| *t = Type::Any);
                }
                self.note_redundant_cast(expr, function, args, &arg_types, slot);
                // A pinned method call is already fully resolved — its
                // signature answers both the argument check and the result
                // type, and `check_call` has no name to look up for it.
                if let Some(sig) = method_sig {
                    let name = match &function.kind {
                        ExprKind::FieldAccess { field, .. } => field.clone(),
                        _ => String::new(),
                    };
                    return self.check_method_args(&sig, &name, args, &arg_types);
                }
                self.check_call(function, args, arg_names, &arg_types, expr.span)
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
                self.tolerant_access = tolerant;
                let obj = self.check_expr(object);
                // A class instance has declared field types; a plain record
                // does not (`Type` is unparameterized), so everything else
                // stays `Any`.
                let Type::Class(id) = obj else {
                    return Type::Any;
                };
                match self.classes.get(id).field(field) {
                    Some(f) => f.ty.unwrap_or(Type::Any),
                    None if tolerant => Type::Any,
                    None => {
                        // The class table lists this class's fields exactly, so
                        // a name that is not among them cannot be read off an
                        // instance. (Method syntax never lands here — the call
                        // arm above intercepts it.)
                        let class = self.spell(obj);
                        self.warn(expr.span, format!("class `{class}` has no field `{field}`"));
                        Type::Any
                    }
                }
            }
            ExprKind::IndexAccess { object, index } => {
                self.tolerant_access = tolerant;
                self.check_expr(object);
                self.check_expr(index);
                Type::Any
            }
            // `a?.b` asks for absence-tolerance outright, so the "class has no
            // field" warning is suppressed down its spine exactly as it is on a
            // `??` left-hand side.
            ExprKind::OptionalAccess(inner) => {
                self.tolerant_access = true;
                self.check_expr(inner)
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

    /// The class a receiver *expression*'s declaration implied, for a receiver
    /// this pass could not type. Only a bare name has one: anything else is an
    /// expression whose value came from somewhere the declaration cannot speak
    /// for. See [`VarType::class_hint`].
    fn receiver_hint(&self, object: &Expr) -> Option<Type> {
        let ExprKind::Ident(name) = &object.kind else {
            return None;
        };
        self.lookup(name)?.class_hint
    }

    /// Record the fallback class for a `recv.name(args)` whose receiver is
    /// untypeable. Guarded exactly like [`Checker::check_method_call`]'s pin,
    /// and for the same reasons — a field of that name outranks the method, and
    /// a name the class does not declare can still reach a global native — with
    /// one addition: a hint is only useful if some overload actually accepts
    /// this call, since dispatch has already failed by the time it is consulted.
    fn note_dispatch_hint(&mut self, hint: Type, name: &str, args: usize, span: SourceSpan) {
        let Type::Class(id) = hint else { return };
        let def = self.classes.get(id);
        if def.field(name).is_some() {
            return;
        }
        if !def
            .methods
            .iter()
            .any(|m| m.name == name && m.arity == args + 1)
        {
            return;
        }
        self.dispatch
            .hints
            .insert(span, self.classes.name_of(id).to_string());
    }

    /// Check a `recv.name(args)` site and, where possible, pin it to one class.
    ///
    /// Warns when the call supplies an argument count no declared overload of
    /// that method accepts (site 7). Dispatch consults, in order, a callable
    /// field, then the receiver class's methods, then the globals
    /// (docs/language-guide.md, Classes & Methods) — so this only fires when the
    /// receiver's class is statically known, declares no field of that name (it
    /// would win), and *does* declare the method under some other arity. Arities
    /// count the receiver, as the class table records them.
    ///
    /// A site that survives every one of those guards *and* matches an arity is
    /// recorded in [`Checker::dispatch`], letting the compiler call
    /// `fn Class.name` directly instead of dispatching on the receiver's tag.
    /// The guards are exactly the cases where the two would disagree: a field of
    /// the same name outranks the method, and an unknown class or a
    /// no-such-method site can still reach a global native.
    /// Returns the method's declared signature when the call resolves to
    /// exactly one — which is also exactly when [`Self::check_call`] may use
    /// its return type. Every path that leaves the call *dispatched* (a field
    /// of that name, an unknown method, an arity no overload accepts, a
    /// receiver of unknown class) returns `None`, so a call this pass could not
    /// pin keeps inferring `any` rather than claiming a type from a
    /// declaration it may never reach.
    fn check_method_call(
        &mut self,
        recv: Type,
        name: &str,
        args: usize,
        span: SourceSpan,
    ) -> Option<FnSignature> {
        let Type::Class(id) = recv else { return None };
        let def = self.classes.get(id);
        if def.field(name).is_some() {
            return None;
        }
        let mut arities: Vec<usize> = def
            .methods
            .iter()
            .filter(|m| m.name == name)
            .map(|m| m.arity)
            .collect();
        // No method of that name: dispatch falls through to a global native
        // with the receiver prepended (`r.len()`), which this pass knows
        // nothing about.
        if arities.is_empty() {
            return None;
        }
        if arities.contains(&(args + 1)) {
            self.dispatch
                .pinned
                .insert(span, self.classes.name_of(id).to_string());
            return def
                .methods
                .iter()
                .find(|m| m.name == name && m.arity == args + 1)
                .map(|m| m.sig.clone());
        }
        arities.sort_unstable();
        // Report what the call site writes, which excludes the receiver the
        // method syntax supplies for you.
        let written: Vec<usize> = arities.iter().map(|a| a - 1).collect();
        let what = format!("method `{}.{}`", self.spell(recv), name);
        self.warn_arity(span, &what, &written, args);
        None
    }

    /// Check a resolved method call's written arguments against the method's
    /// declared parameters, and answer with its declared return type.
    ///
    /// The receiver occupies slot 0 of the signature but is never *written* at
    /// the call site — `r.inset(4)` writes one argument against a two-slot
    /// signature — so parameter `i + 1` pairs with written argument `i`, and
    /// the numbering in the message counts what the reader typed.
    fn check_method_args(
        &mut self,
        sig: &FnSignature,
        name: &str,
        args: &[Expr],
        arg_types: &[Type],
    ) -> Type {
        for (i, at) in arg_types.iter().enumerate() {
            let Some(Some(pt)) = sig.params.get(i + 1) else {
                continue;
            };
            if *pt == Type::Any || *at == Type::Any {
                continue;
            }
            if !at.is_assignable_to(pt) {
                self.warn(
                    args[i].span,
                    format!(
                        "argument {} to `{}`: expected `{}`, found `{}`",
                        i + 1,
                        name,
                        self.spell(*pt),
                        self.spell(*at)
                    ),
                );
            }
        }
        sig.ret.unwrap_or(Type::Any)
    }

    /// Resolve a call's result type and check the call against whatever the
    /// callee's signature is known to be: each argument against its declared
    /// parameter type (site 5), and the argument *count* against every declared
    /// arity (site 6 — Petal overloads by arity, so a call is wrong only when it
    /// matches none of them). Named arguments are checked against the callee's
    /// declared parameter *names* wherever those are known
    /// ([`Self::check_named_args`]). Assumes the args were already visited;
    /// `arg_types` are their inferred types.
    fn check_call(
        &mut self,
        function: &Expr,
        args: &[Expr],
        arg_names: &[Option<String>],
        arg_types: &[Type],
        call_span: SourceSpan,
    ) -> Type {
        if let ExprKind::Ident(f) = &function.kind
            && self.lookup(f).is_none()
        {
            // Sanctioned cast builtins produce a concrete type. These sit
            // ahead of the class table only because a class may not take a
            // built-in type name (`class int` is rejected at declaration), so
            // nothing user-declared can ever be hidden here.
            match f.as_str() {
                "int" => return Type::Int,
                "float" => return Type::Float,
                "str" => return Type::String,
                _ => {}
            }
            // A class name is its constructor: `Point(1, 2)` builds an
            // instance, and the declared field types check the arguments
            // positionally — the same rule as a function's parameters, because
            // that is what they are. The field count is the only arity there
            // is; a class declares no overloads.
            // …unless a `fn` of that name shadows it, the way a user binding
            // shadows any builtin: the declaration wins at runtime, so the
            // constructor's shape must not be checked against its calls.
            if let Some(id) = self.classes.lookup(f)
                && self.module_signatures(f).is_empty()
            {
                let fields = self.classes.get(id).fields.clone();
                if fields.len() != args.len() {
                    let what = format!("`{f}`");
                    self.warn_arity(call_span, &what, &[fields.len()], args.len());
                    return Type::Class(id);
                }
                if !arg_names.is_empty() {
                    // A constructor's parameters *are* its fields, in order —
                    // which is exactly how the VM binds `Point(y: 2, x: 1)`.
                    let field_names: Vec<String> =
                        fields.iter().map(|fd| fd.name.clone()).collect();
                    let what = format!("`{f}`");
                    self.check_named_args(&what, &field_names, args, arg_names);
                }
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
        }
        // Everything else callable that this pass can pin down: a module
        // function, a name bound to one, or a lambda. `fn_candidates` returns
        // one signature per arity, and empty when nothing is known — which is
        // every builtin, every parameter, and every value that merely happens
        // to hold a function.
        let candidates = self.fn_candidates(function);
        if candidates.is_empty() {
            if let ExprKind::Ident(f) = &function.kind
                && self.lookup(f).is_none()
            {
                // Not a module function: fall back to the builtin table. It
                // knows only result types (builtins declare no parameter
                // types), so there is nothing to check the arguments against.
                return builtin_types::builtin_return_type(f, arg_types).unwrap_or(Type::Any);
            }
            return Type::Any;
        }
        let Some(sig) = candidates
            .iter()
            .find(|s| s.params.len() == args.len())
            .cloned()
        else {
            // No overload takes this many arguments — the call cannot resolve
            // at runtime, whatever the argument types are.
            if let ExprKind::Ident(f) = &function.kind {
                let expected: Vec<usize> = candidates.iter().map(|s| s.params.len()).collect();
                let what = format!("`{f}`");
                self.warn_arity(call_span, &what, &expected, args.len());
            }
            return Type::Any;
        };
        // A lambda invoked in place has no name to blame the argument on.
        let callee = match &function.kind {
            ExprKind::Ident(f) => format!(" to `{f}`"),
            _ => String::new(),
        };
        if !arg_names.is_empty()
            && let Some(params) = self.callee_param_names(function, args.len())
        {
            let what = match &function.kind {
                ExprKind::Ident(f) => format!("`{f}`"),
                _ => "this lambda".to_string(),
            };
            self.check_named_args(&what, &params, args, arg_names);
        }
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
                        "argument {}{}: expected `{}`, found `{}`",
                        i + 1,
                        callee,
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
        crate::compiler::collect_classes(&mut classes, &stmts, None);
        let sigs = crate::compiler::collect_fn_signatures(&stmts, &classes);
        let names = crate::compiler::collect_fn_param_names(&stmts);
        check_module(&stmts, &sigs, &names, &classes)
            .0
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    /// A class field that is not declared warns on a plain read — but not when
    /// the read is the left side of `??`, which explicitly asks to tolerate the
    /// field being absent (and compiles to the tolerant `GetFieldOpt`).
    #[test]
    fn coalesced_missing_class_field_does_not_warn() {
        let cls = "class B\n  x: int\nend\n";
        let w = warns(&format!("{cls}let b = B(1)\nlet y = b.nosuch"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("has no field"), "{w:?}");

        assert!(warns(&format!("{cls}let b = B(1)\nlet y = b.nosuch ?? 0")).is_empty());
        assert!(warns(&format!("{cls}let b = B(1)\nlet y = b.nosuch.deeper ?? 0")).is_empty());
        // Only the left side is tolerant.
        let w = warns(&format!("{cls}let b = B(1)\nlet y = 0 ?? b.nosuch"));
        assert_eq!(w.len(), 1, "{w:?}");
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
    // docs/var.md (Cells).
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

    // ── Field reads on a class-typed value are checked ──────────────────────
    const B: &str = "class B\n  b: int\nend\n";

    #[test]
    fn field_not_on_the_declared_class_warns() {
        let w = warns(&format!("{B}fn f(x: B)\n  x.nosuch\nend"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("nosuch") && w[0].contains('B'), "{w:?}");
    }

    #[test]
    fn a_declared_field_does_not_warn() {
        assert!(warns(&format!("{B}fn f(x: B)\n  x.b\nend")).is_empty());
        // Also through a locally inferred instance, and on a built-in class.
        assert!(warns(&format!("{B}let v = B(1)\nprint(v.b)")).is_empty());
        assert!(warns("let r = Rect(0, 0, 1, 1)\nprint(r.w)").is_empty());
    }

    #[test]
    fn field_check_does_not_fire_on_records_or_any() {
        // A plain record has no declared fields to check against, and an
        // un-annotated parameter is `any`.
        assert!(warns("let r = {a: 1}\nprint(r.nosuch)").is_empty());
        assert!(warns("fn f(x)\n  x.nosuch\nend").is_empty());
        assert!(warns("fn f(x: record)\n  x.nosuch\nend").is_empty());
    }

    #[test]
    fn a_method_name_is_not_read_as_a_field() {
        // `x.go()` is method syntax, and dispatch reaches user methods,
        // built-in class methods and globals — none of which are fields.
        assert!(warns(&format!("{B}fn B.go(x: B)\n  x.b\nend\nprint(B(1).go())")).is_empty());
        assert!(warns("let r = Rect(0, 0, 4, 4)\nprint(r.center_x())").is_empty());
        assert!(warns(&format!("{B}print(B(1).str())")).is_empty());
    }

    // ── Lambda / bound-function parameter types are checked at call sites ───
    #[test]
    fn calling_a_lambda_binding_checks_its_parameter_types() {
        let w = warns("let f = fn(n: int) -> n\nprint(f(\"hi\"))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("argument 1"), "{w:?}");
        assert!(warns("let f = fn(n: int) -> n\nprint(f(5))").is_empty());
    }

    #[test]
    fn calling_a_function_through_a_binding_checks_it() {
        let w = warns("fn g(s: string)\n  s\nend\nlet h = g\nprint(h(1))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("argument 1"), "{w:?}");
        // The alias chains, and a matching call stays silent.
        let c = warns("fn g(s: string)\n  s\nend\nlet h = g\nlet i = h\nprint(i(1))");
        assert_eq!(c.len(), 1, "{c:?}");
        assert!(warns("fn g(s: string)\n  s\nend\nlet h = g\nprint(h(\"x\"))").is_empty());
    }

    #[test]
    fn a_bound_functions_declared_return_type_flows_out() {
        let w = warns("fn g(n: int) -> string\n  str(n)\nend\nlet h = g\nlet x: int = h(1)");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`x` declared `int`"), "{w:?}");
    }

    #[test]
    fn a_lambda_immediately_invoked_is_checked() {
        let w = warns("print((fn(n: int) -> n)(\"hi\"))");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn rebinding_a_function_name_drops_its_old_signature() {
        // The binding now holds a different function, so the old parameter
        // types say nothing about the new one.
        assert!(warns("let f = fn(n: int) -> n\nf = fn(s) -> s\nprint(f(\"hi\"))").is_empty());
        // A `var` cell is never trusted in the first place.
        assert!(warns("var f = fn(n: int) -> n\nprint(f(\"hi\"))").is_empty());
    }

    #[test]
    fn a_nested_function_shadows_a_top_level_one() {
        // The inner `f` is the one being called, so the outer signature must
        // not be used against it.
        assert!(
            warns("fn f(n: int)\n  n\nend\nfn outer()\n  fn f(s: string)\n    s\n  end\n  f(\"x\")\nend")
                .is_empty()
        );
        // …and the inner signature *is* used.
        let w = warns("fn outer()\n  fn f(s: string)\n    s\n  end\n  f(1)\nend");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    // ── Arity: no overload matches (finding B5) ─────────────────────────────
    #[test]
    fn a_call_with_no_matching_arity_warns() {
        let w = warns("fn f(a, b)\n  a\nend\nprint(f(1))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`f`") && w[0].contains('2'), "{w:?}");
    }

    #[test]
    fn arity_overloads_are_all_candidates() {
        assert!(warns("fn f(a)\n  a\nend\nfn f(a, b)\n  a\nend\nf(1)\nf(1, 2)").is_empty());
        let w = warns("fn f(a)\n  a\nend\nfn f(a, b)\n  a\nend\nf(1, 2, 3)");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("1 or 2"), "{w:?}");
    }

    #[test]
    fn constructor_arity_is_checked() {
        let w = warns("class P\n  x: int,\n  y: int,\nend\nprint(P(1))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`P`"), "{w:?}");
        assert!(warns("class P\n  x: int,\n  y: int,\nend\nprint(P(1, 2))").is_empty());
        assert!(warns("print(Rect(0, 0, 1, 1))").is_empty());
    }

    /// The motivating case for `num` (plan §12 Q5): `Rect`'s edges take either
    /// numeric width, so before `num` they were un-annotated and this was
    /// caught only at runtime.
    #[test]
    fn a_non_numeric_rect_field_warns() {
        let w = warns(r#"let r = Rect("a", 1, 2, 3)"#);
        assert_eq!(w.len(), 1, "expected exactly one warning, got {w:?}");
        assert!(w[0].contains("num"), "{}", w[0]);
        assert!(w[0].contains("string"), "{}", w[0]);
    }

    /// Both numeric widths are accepted, mixed and matched, with no warning —
    /// that is the entire point of the type.
    #[test]
    fn either_numeric_width_fills_a_num_slot() {
        assert!(warns("let r = Rect(1, 2, 3, 4)").is_empty());
        assert!(warns("let r = Rect(1.5, 2.5, 3.5, 4.5)").is_empty());
        assert!(warns("let r = Rect(1, 2.5, 3, 4.5)").is_empty());
    }

    #[test]
    fn a_num_annotation_checks_bindings_params_and_returns() {
        assert!(warns("let x: num = 1").is_empty());
        assert!(warns("let x: num = 1.5").is_empty());
        assert_eq!(warns(r#"let x: num = "hi""#).len(), 1);

        assert!(warns("fn f(a: num) a end\nf(1)\nf(2.5)").is_empty());
        assert_eq!(warns("fn f(a: num) a end\nf(\"hi\")").len(), 1);

        assert!(warns("fn f() -> num\n1\nend").is_empty());
        assert_eq!(warns("fn f() -> num\n\"hi\"\nend").len(), 1);
    }

    /// `num` widens; it never narrows. Passing one to an `int` slot needs the
    /// explicit cast, or the checker would be sanctioning an implicit one.
    #[test]
    fn a_num_does_not_fill_an_int_slot_without_a_cast() {
        let src = "fn takes_int(a: int) a end\nfn f(n: num)\ntakes_int(n)\nend";
        assert_eq!(warns(src).len(), 1, "{:?}", warns(src));
        let cast = "fn takes_int(a: int) a end\nfn f(n: num)\ntakes_int(int(n))\nend";
        assert!(warns(cast).is_empty(), "{:?}", warns(cast));
    }

    #[test]
    fn method_arity_is_checked() {
        let src =
            "class P\n  x: int,\n  y: int,\nend\nfn P.shift(p: P, dx: int)\n  p.x + dx\nend\n";
        let w = warns(&format!("{src}print(P(1, 2).shift())"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("shift"), "{w:?}");
        assert!(warns(&format!("{src}print(P(1, 2).shift(3))")).is_empty());
        // A method overloaded by arity accepts either.
        let two =
            "class Q\n  x: int,\nend\nfn Q.f(q: Q)\n  1\nend\nfn Q.f(q: Q, n: int)\n  n\nend\n";
        assert!(warns(&format!("{two}print(Q(1).f())\nprint(Q(1).f(2))")).is_empty());
    }

    // ── Method return types (chunk T) ───────────────────────────────────────

    const P: &str = "class P\n  x: int,\nend\n";

    #[test]
    fn a_methods_declared_return_type_is_inferred() {
        let src = format!("{P}fn P.n(p: P) -> int\n  p.x\nend\n");
        let w = warns(&format!("{src}let s: string = P(1).n()"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("int"), "{w:?}");
        assert!(warns(&format!("{src}let i: int = P(1).n()")).is_empty());
    }

    #[test]
    fn a_method_with_no_return_annotation_still_infers_any() {
        let src = format!("{P}fn P.n(p: P)\n  p.x\nend\n");
        assert!(warns(&format!("{src}let s: string = P(1).n()")).is_empty());
    }

    #[test]
    fn a_methods_declared_parameters_check_its_arguments() {
        let src = format!("{P}fn P.shift(p: P, dx: int)\n  p.x + dx\nend\n");
        let w = warns(&format!(r#"{src}print(P(1).shift("a"))"#));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("int"), "{w:?}");
        assert!(warns(&format!("{src}print(P(1).shift(2))")).is_empty());
    }

    /// Every guard that leaves a call *dispatched* must also leave its type
    /// unknown — inferring from a declaration the call may not reach would be
    /// worse than inferring nothing.
    #[test]
    fn the_dispatch_guards_also_block_return_type_inference() {
        // A field of that name outranks the method: data beats declarations.
        let shadow = "class F\n  n: function,\nend\nfn F.n(f: F) -> int\n  1\nend\n";
        assert!(warns(&format!("{shadow}let s: string = F(fn() -> 1).n()")).is_empty());
        // A method the class does not declare can still reach a global native.
        assert!(warns(&format!("{P}let s: string = P(1).keys()")).is_empty());
        // An arity no overload accepts resolves to nothing at runtime, so the
        // arity warning is the only one — no type claim rides along with it.
        let src = format!("{P}fn P.n(p: P) -> int\n  p.x\nend\n");
        let w = warns(&format!("{src}let s: string = P(1).n(1, 2)"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("argument"), "{w:?}");
        // An un-annotated receiver is `any`, so there is no class to consult.
        let any = format!("{P}fn P.n(p: P) -> int\n  p.x\nend\n");
        assert!(warns(&format!("{any}fn f(v)\n  let s: string = v.n()\nend")).is_empty());
    }

    /// The built-in `Rect` methods are the ones every UI script calls, and
    /// `num` (chunk S) is exactly what the four edge accessors return — they
    /// run the same arithmetic the language does, so an int rect stays int.
    #[test]
    fn the_builtin_rect_methods_are_typed() {
        for m in ["center_x", "center_y", "right", "bottom"] {
            let w = warns(&format!("let s: string = Rect(0, 0, 4, 4).{m}()"));
            assert_eq!(w.len(), 1, "{m}: {w:?}");
            assert!(w[0].contains("num"), "{m}: {w:?}");
            assert!(warns(&format!("let n: num = Rect(0, 0, 4, 4).{m}()")).is_empty());
        }
        // `inset` and `offset` return another Rect, so they chain.
        let w = warns("let s: string = Rect(0, 0, 4, 4).inset(1)");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("Rect"), "{w:?}");
        assert!(warns("let r: Rect = Rect(0, 0, 8, 8).inset(1).offset(2, 3)").is_empty());
        // And their margin arguments are numbers.
        let bad = warns(r#"print(Rect(0, 0, 4, 4).inset("a"))"#);
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(bad[0].contains("num"), "{bad:?}");
    }

    #[test]
    fn a_lambda_binding_called_with_the_wrong_count_warns() {
        let w = warns("let f = fn(a) -> a\nprint(f(1, 2))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`f`"), "{w:?}");
    }

    #[test]
    fn arity_checks_stay_off_unknown_callables() {
        // Builtins take flexible argument counts and declare no signature.
        assert!(warns("print(1, 2, 3)\nprint()").is_empty());
        // A name the module never declares says nothing.
        assert!(warns("mystery(1, 2)").is_empty());
        // Nor does a method the class does not declare — dispatch can still
        // reach a global native with the receiver prepended.
        assert!(warns("class P\n  x: int\nend\nprint(P(1).keys())").is_empty());
        // A field holding a function is called through the field, not a method.
        assert!(warns("class P\n  f: function\nend\nprint(P(fn(a) -> a).f(1))").is_empty());
    }

    #[test]
    fn an_enum_variant_shadowing_a_class_name_is_not_a_constructor() {
        // `enum Shape … Rect(w, h) … end` shadows the built-in `Rect`, so its
        // four fields say nothing about `Rect(3, 4)`.
        let src = "enum Shape\n  Circle(radius),\n  Rect(w, h),\nend\n";
        assert!(warns(&format!("{src}print(Rect(3, 4))")).is_empty());
        assert!(warns(&format!("{src}print(Circle(1))")).is_empty());
        // Inside a function body too, where the walk binds the block's own
        // declarations.
        assert!(warns(&format!("fn f()\n  {src}  Rect(3, 4)\nend\nprint(f())")).is_empty());
    }

    #[test]
    fn a_fn_shadowing_a_class_name_is_checked_as_the_fn() {
        // `fn Rect(a, b)` wins over the built-in constructor at runtime, so the
        // class's four fields say nothing about this call.
        assert!(warns("fn Rect(a, b)\n  a + b\nend\nprint(Rect(1, 2))").is_empty());
        let w = warns("fn Rect(a, b)\n  a + b\nend\nprint(Rect(1))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("expects 2 arguments"), "{w:?}");
    }

    // ── named arguments ─────────────────────────────────────────────────

    #[test]
    fn a_named_argument_matching_a_parameter_is_silent() {
        let f = "fn sub(a, b)\n  a - b\nend\n";
        assert!(warns(&format!("{f}print(sub(b: 1, a: 2))")).is_empty());
        assert!(warns(&format!("{f}print(sub(1, b: 2))")).is_empty());
        assert!(warns(&format!("{f}print(sub(1, 2))")).is_empty());
    }

    #[test]
    fn an_unknown_parameter_name_warns() {
        let w = warns("fn sub(a, b)\n  a - b\nend\nprint(sub(a: 1, c: 2))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`sub` has no parameter named `c`"), "{w:?}");
    }

    #[test]
    fn a_slot_filled_twice_warns() {
        // Positionally and then by name…
        let w = warns("fn sub(a, b)\n  a - b\nend\nprint(sub(1, a: 2))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("`sub` got multiple values for parameter `a`"),
            "{w:?}"
        );
        // …and twice by name.
        let w = warns("fn sub(a, b)\n  a - b\nend\nprint(sub(b: 1, b: 2))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("parameter `b`"), "{w:?}");
    }

    #[test]
    fn the_overload_the_count_selects_is_the_one_checked() {
        let src = "fn g(x)\n  x\nend\nfn g(x, limit)\n  x + limit\nend\n";
        assert!(warns(&format!("{src}print(g(x: 1))")).is_empty());
        assert!(warns(&format!("{src}print(g(1, limit: 2))")).is_empty());
        // `limit` belongs to the two-argument overload, not the one-argument
        // one the count selects here.
        let w = warns(&format!("{src}print(g(limit: 1))"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("no parameter named `limit`"), "{w:?}");
    }

    #[test]
    fn a_constructors_parameters_are_its_fields() {
        let cls = "class P\n  x: int, y: int\nend\n";
        assert!(warns(&format!("{cls}print(P(y: 2, x: 1))")).is_empty());
        let w = warns(&format!("{cls}print(P(x: 1, z: 2))"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`P` has no parameter named `z`"), "{w:?}");
        let w = warns(&format!("{cls}print(P(1, x: 2))"));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("multiple values for parameter `x`"), "{w:?}");
    }

    #[test]
    fn lambdas_and_nested_fns_are_checked_too() {
        let w = warns("let f = fn(a, b) -> a + b\nprint(f(a: 1, c: 2))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("`f` has no parameter named `c`"), "{w:?}");
        assert!(warns("let f = fn(a, b) -> a + b\nprint(f(b: 1, a: 2))").is_empty());

        // A lambda invoked in place has no name to blame.
        let w = warns("print((fn(a) -> a)(nope: 1))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("this lambda has no parameter named"), "{w:?}");

        // A nested declaration shadows the module function, names and all.
        let src = "fn outer()\n  fn inner(q)\n    q\n  end\n  inner(q: 1)\nend\nprint(outer())";
        assert!(warns(src).is_empty());
        let bad = "fn outer()\n  fn inner(q)\n    q\n  end\n  inner(r: 1)\nend\nprint(outer())";
        let w = warns(bad);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("no parameter named `r`"), "{w:?}");
    }

    #[test]
    fn a_rebound_name_is_checked_against_the_function_it_now_holds() {
        let src = "fn a_fn(alpha)\n  alpha\nend\nfn b_fn(beta)\n  beta\nend\n";
        // The binding carries the names forward through an alias…
        assert!(warns(&format!("{src}let f = a_fn\nprint(f(alpha: 1))")).is_empty());
        let w = warns(&format!("{src}let f = a_fn\nprint(f(beta: 1))"));
        assert_eq!(w.len(), 1, "{w:?}");
        // …and a re-assignment replaces them.
        let w = warns(&format!(
            "{src}var f = a_fn\nset f = b_fn\nprint(f(alpha: 1))"
        ));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("no parameter named `alpha`"), "{w:?}");
    }

    #[test]
    fn unknown_callees_keep_the_runtime_check_as_their_only_one() {
        // A builtin declares no parameter names; the VM refuses the call.
        assert!(warns("print(append(a: 1, b: 2))").is_empty());
        // Nor does a name this module never declares.
        assert!(warns("mystery(whatever: 1)").is_empty());
        // A method's parameter names are not in the class table, so a named
        // argument to one is left to the VM as well.
        let cls = "class P\n  x: int\nend\nfn P.shift(p, dx)\n  p.x + dx\nend\n";
        assert!(warns(&format!("{cls}print(P(1).shift(dx: 2))")).is_empty());
        // A parameter holding a function says nothing about its parameters.
        assert!(
            warns("fn call_it(g)\n  g(anything: 1)\nend\nprint(call_it(fn(a) -> a))").is_empty()
        );
    }
}
