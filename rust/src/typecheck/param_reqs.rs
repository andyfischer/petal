//! What an un-annotated function's parameters must be, read off its body.
//!
//! Petal overloads by arity alone, so a plausible call lands on whichever
//! overload has its argument count, and when that overload wanted a different
//! shape the call fails inside it with a message about the overload's body
//! ("Expected int at arg 5, got record", "Cannot access field 'x' on int").
//! The prelude's draw overloads are written without annotations, so the
//! checker's ordinary parameter check has nothing to hold such a call to.
//!
//! This pass supplies the missing half: for each parameter, a requirement the
//! body *certainly* imposes on every call that reaches it.
//!
//! - [`ParamReq::Num`]: the parameter is handed, unchanged, to an argument slot
//!   that only accepts a number — a native's (directly, or through a module
//!   alias like the prelude's `let _native_rect = draw_rect`), or another
//!   function of the same module whose own requirement says so.
//! - [`ParamReq::Field`]: the body reads a field off the parameter
//!   (`c.r`), which fails on a number, a bool, nil, and anything else that has
//!   no fields.
//!
//! "Certainly" is the whole design. Only code that runs on every call counts:
//! the body's statements in order, up to the first one that could leave early
//! (anything containing a `return`, or a `while`, which need not end), and
//! within a statement only the parts evaluated unconditionally — not a branch
//! of an `if`/`match`, not the right of `&&`/`||`, not a loop body, not a
//! lambda, and nothing on an absence-tolerant `??`/`?.` spine. A parameter
//! that is rebound or shadowed anywhere in the body is left alone entirely.
//! Anything this cannot prove is simply not a requirement, so a function that
//! tells its shapes apart at runtime (`if _is_num(p1) then … else p1.x …`)
//! imposes nothing on `p1` and never warns.

use std::collections::{HashMap, HashSet};

use crate::ast::{
    self, AssignTarget, BinOp, Expr, ExprKind, ExprVisitor, Pattern, RecordField, Stmt, StmtKind,
};
use crate::types::Type;

use super::builtin_types::{ArgSlot, builtin_param_slots};

/// A requirement a function's body imposes on one parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamReq {
    /// Passed to a slot that only takes a number.
    Num,
    /// A field of this name is read from it.
    Field(String),
}

impl ParamReq {
    /// Whether an argument of static type `ty` can meet this requirement.
    /// Only a type that certainly fails is refused.
    pub fn accepts(&self, ty: Type) -> bool {
        match self {
            ParamReq::Num => ArgSlot::Num.accepts(ty),
            ParamReq::Field(name) => match ty {
                Type::Int
                | Type::Float
                | Type::Num
                | Type::Dual
                | Type::Bool
                | Type::Nil
                | Type::Function
                | Type::Symbol => false,
                // Lists and strings answer `.length` and nothing else.
                Type::List | Type::String => name == "length",
                _ => true,
            },
        }
    }

    /// What the body does with the parameter, for a diagnostic.
    pub fn describe(&self) -> String {
        match self {
            ParamReq::Num => "uses it as a number".to_string(),
            ParamReq::Field(name) => format!("reads field `{name}` from it"),
        }
    }
}

/// The requirements of every un-annotated-enough function in one module, by
/// `(name, arity)` — the same key as the checker's signatures. A function
/// with no requirement at all is absent; a `None` slot is an unconstrained
/// parameter.
pub type ParamReqs = HashMap<(String, usize), Vec<Option<ParamReq>>>;

/// Compute [`ParamReqs`] for a module's top-level functions.
///
/// `imported_fn` answers whether a name is a function some *other* module
/// declares: such a name may reach this module through an import (the ui
/// prelude's `draw_rect` does, implicitly), so a call to it is not a call to
/// the native of that name.
pub fn collect(stmts: &[Stmt], imported_fn: &dyn Fn(&str) -> bool) -> ParamReqs {
    // Top-level functions by name: a call to one of these names means the
    // module's own function, never a native.
    let fn_names: HashSet<&str> = stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::FnDecl {
                name, class: None, ..
            } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    // Where each top-level function name is first declared, for ordering.
    let mut first_decl: HashMap<&str, usize> = HashMap::new();
    for (i, s) in stmts.iter().enumerate() {
        if let StmtKind::FnDecl {
            name, class: None, ..
        } = &s.kind
        {
            first_decl.entry(name.as_str()).or_insert(i);
        }
    }
    // `let _native_rect = draw_rect`: a module-level alias of a native. The
    // alias captures whatever the name meant *when the `let` ran*, so it names
    // the native only when it precedes this module's own declarations of that
    // name, and no other module's function of that name could be in scope.
    let mut aliases: HashMap<&str, &str> = HashMap::new();
    // Every other top-level value binding: a call through one of these is a
    // call to a value this pass knows nothing about.
    let mut top_values: HashSet<&str> = HashSet::new();
    for (i, s) in stmts.iter().enumerate() {
        match &s.kind {
            StmtKind::Let {
                name,
                value:
                    Expr {
                        kind: ExprKind::Ident(target),
                        ..
                    },
                is_var: false,
                ..
            } if first_decl.get(target.as_str()).is_none_or(|&d| d > i) && !imported_fn(target) => {
                aliases.insert(name.as_str(), target.as_str());
            }
            StmtKind::Let { name, .. } | StmtKind::State { name, .. } => {
                top_values.insert(name.as_str());
            }
            _ => {}
        }
    }
    // A name rebound at top level is not a reliable alias.
    for s in stmts {
        if let StmtKind::Assign {
            target: AssignTarget::Name(n),
            ..
        }
        | StmtKind::Set {
            target: AssignTarget::Name(n),
            ..
        } = &s.kind
            && aliases.remove(n.as_str()).is_some()
        {
            top_values.insert(n.as_str());
        }
    }

    let fns: Vec<(&str, &[ast::Param], &[Stmt])> = stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::FnDecl {
                name,
                class: None,
                params,
                body,
                ..
            } => Some((name.as_str(), params.as_slice(), body.as_slice())),
            _ => None,
        })
        .collect();
    // Two declarations of one `(name, arity)` are an error elsewhere; here
    // they would make "the" function ambiguous, so neither gets requirements.
    let mut seen: HashMap<(&str, usize), usize> = HashMap::new();
    for (name, params, _) in &fns {
        *seen.entry((name, params.len())).or_default() += 1;
    }

    // Iterate to a fixpoint so a requirement flows through a same-module
    // call (`draw_line(p1, p2, c)` forwarding to the flat form). Every round
    // only adds requirements, and there are finitely many slots.
    let mut reqs: ParamReqs = HashMap::new();
    // Bounded as a belt-and-braces guard; the rounds only ever add slots.
    for _ in 0..16 {
        let mut changed = false;
        for (name, params, body) in &fns {
            if seen[&(*name, params.len())] > 1 {
                continue;
            }
            let ctx = Ctx {
                fn_names: &fn_names,
                aliases: &aliases,
                top_values: &top_values,
                imported_fn,
                reqs: &reqs,
            };
            let found = infer_one(params, body, &ctx);
            if found.iter().all(Option::is_none) {
                continue;
            }
            let key = (name.to_string(), params.len());
            if reqs.get(&key) != Some(&found) {
                reqs.insert(key, found);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    reqs
}

struct Ctx<'a> {
    fn_names: &'a HashSet<&'a str>,
    aliases: &'a HashMap<&'a str, &'a str>,
    top_values: &'a HashSet<&'a str>,
    imported_fn: &'a dyn Fn(&str) -> bool,
    reqs: &'a ParamReqs,
}

/// The requirements one function body imposes on its parameters.
fn infer_one(params: &[ast::Param], body: &[Stmt], ctx: &Ctx) -> Vec<Option<ParamReq>> {
    let mut bound = BoundNames::default();
    for s in body {
        bound.visit_stmt(s);
    }
    let mut w = Walker {
        tracked: params
            .iter()
            .enumerate()
            .filter(|(_, p)| !bound.names.contains(&p.name))
            .map(|(i, p)| (p.name.as_str(), i))
            .collect(),
        locals: &bound.names,
        ctx,
        out: vec![None; params.len()],
    };
    for s in body {
        if stmt_may_exit(s) {
            break;
        }
        w.stmt(s);
    }
    w.out
}

/// Every name the body binds or rebinds, at any depth: a parameter among them
/// is not tracked, and a callee among them is not the global of that name.
#[derive(Default)]
struct BoundNames {
    names: HashSet<String>,
}

impl BoundNames {
    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Variable(n) => {
                self.names.insert(n.clone());
            }
            Pattern::Variant { fields, .. } => fields.iter().for_each(|f| self.pattern(f)),
            Pattern::List { elements, rest } => {
                elements.iter().for_each(|e| self.pattern(e));
                if let Some(r) = rest {
                    self.names.insert(r.clone());
                }
            }
            Pattern::Record(fields) => fields.iter().for_each(|(_, f)| self.pattern(f)),
            Pattern::Wildcard | Pattern::Literal(_) => {}
        }
    }
}

impl ExprVisitor for BoundNames {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, .. }
            | StmtKind::State { name, .. }
            | StmtKind::FnDecl { name, .. }
            | StmtKind::For { var: name, .. } => {
                self.names.insert(name.clone());
            }
            StmtKind::Assign {
                target: AssignTarget::Name(n),
                ..
            }
            | StmtKind::Set {
                target: AssignTarget::Name(n),
                ..
            } => {
                self.names.insert(n.clone());
            }
            _ => {}
        }
        if let StmtKind::FnDecl { params, .. } = &s.kind {
            self.names.extend(params.iter().map(|p| p.name.clone()));
        }
        ast::walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lambda { params, .. } => {
                self.names.extend(params.iter().map(|p| p.name.clone()));
            }
            ExprKind::For { var, .. } => {
                self.names.insert(var.clone());
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    self.pattern(&arm.pattern);
                }
            }
            _ => {}
        }
        ast::walk_expr(self, e);
    }
}

/// Whether a statement could end the call early, or never finish, so the
/// statements after it do not run on every call.
fn stmt_may_exit(s: &Stmt) -> bool {
    struct Finder(bool);
    impl ExprVisitor for Finder {
        fn visit_stmt(&mut self, s: &Stmt) {
            if matches!(
                s.kind,
                StmtKind::Return(_) | StmtKind::While { .. } | StmtKind::Break | StmtKind::Continue
            ) {
                self.0 = true;
                return;
            }
            ast::walk_stmt(self, s);
        }
    }
    let mut f = Finder(false);
    f.visit_stmt(s);
    f.0
}

struct Walker<'a> {
    /// Parameter name → index, for the parameters still worth tracking.
    tracked: HashMap<&'a str, usize>,
    /// Names bound anywhere in the body (see [`BoundNames`]).
    locals: &'a HashSet<String>,
    ctx: &'a Ctx<'a>,
    out: Vec<Option<ParamReq>>,
}

impl Walker<'_> {
    fn note(&mut self, param: &str, req: ParamReq) {
        if let Some(&i) = self.tracked.get(param)
            && self.out[i].is_none()
        {
            self.out[i] = Some(req);
        }
    }

    /// The slots a call to `callee` with `arity` arguments certainly
    /// imposes, when the callee is statically known here.
    fn callee_slots(&self, callee: &str, arity: usize) -> Option<Vec<Option<ParamReq>>> {
        if self.locals.contains(callee) {
            return None;
        }
        if self.ctx.fn_names.contains(callee) {
            return self.ctx.reqs.get(&(callee.to_string(), arity)).map(|r| {
                r.iter()
                    .map(|x| x.clone().filter(|r| *r == ParamReq::Num))
                    .collect()
            });
        }
        let native = match self.ctx.aliases.get(callee) {
            Some(target) => *target,
            None if self.ctx.top_values.contains(callee) || (self.ctx.imported_fn)(callee) => {
                return None;
            }
            None => callee,
        };
        let slots = builtin_param_slots(native, arity)?;
        Some(
            slots
                .iter()
                .map(|s| (*s == ArgSlot::Num).then_some(ParamReq::Num))
                .collect(),
        )
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { value, .. } => self.expr(value),
            StmtKind::Assign { target, value } | StmtKind::Set { target, value } => {
                self.expr(value);
                match target {
                    AssignTarget::Name(_) => {}
                    AssignTarget::Field(object, _) => self.expr(object),
                    AssignTarget::Index(object, index) => {
                        self.expr(object);
                        self.expr(index);
                    }
                }
            }
            StmtKind::Expr(e) => self.expr(e),
            // The iterable is evaluated once, before the (possibly empty)
            // body.
            StmtKind::For { iter, .. } => self.expr(iter),
            StmtKind::State { init, key, .. } => {
                // `init` runs only on the first frame; the key every frame.
                if let Some(k) = key {
                    self.expr(k);
                }
                let _ = init;
            }
            StmtKind::FnDecl { .. }
            | StmtKind::EnumDecl { .. }
            | StmtKind::ClassDecl { .. }
            | StmtKind::While { .. }
            | StmtKind::Return(_)
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import(_) => {}
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::FieldAccess { object, field } => {
                if let ExprKind::Ident(p) = &object.kind {
                    self.note(p, ParamReq::Field(field.clone()));
                } else {
                    self.expr(object);
                }
            }
            ExprKind::Call {
                function,
                args,
                arg_names,
            } => {
                match &function.kind {
                    ExprKind::Ident(callee) => {
                        if arg_names.iter().all(Option::is_none)
                            && let Some(slots) = self.callee_slots(callee, args.len())
                        {
                            for (arg, slot) in args.iter().zip(slots) {
                                if let (ExprKind::Ident(p), Some(req)) = (&arg.kind, slot) {
                                    self.note(p, req);
                                }
                            }
                        }
                    }
                    // `recv.method(...)`: a method call, not a field read —
                    // only the receiver is evaluated.
                    ExprKind::FieldAccess { object, .. } => {
                        if !matches!(object.kind, ExprKind::Ident(_)) {
                            self.expr(object);
                        }
                    }
                    _ => self.expr(function),
                }
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::BinaryOp { op, left, right } => match op {
                // The right side is conditional; the left of `??` is an
                // absence-tolerant spine.
                BinOp::And | BinOp::Or => self.expr(left),
                BinOp::Coalesce => {}
                _ => {
                    self.expr(left);
                    self.expr(right);
                }
            },
            ExprKind::UnaryOp { operand, .. } => self.expr(operand),
            ExprKind::IndexAccess { object, index } => {
                self.expr(object);
                self.expr(index);
            }
            ExprKind::If { condition, .. } => self.expr(condition),
            ExprKind::Match { subject, .. } => self.expr(subject),
            ExprKind::For { iter, .. } => self.expr(iter),
            ExprKind::List(items) => items.iter().for_each(|i| self.expr(i)),
            ExprKind::Record(fields) => {
                for f in fields {
                    match f {
                        RecordField::Named(_, v) | RecordField::Spread(v) => self.expr(v),
                    }
                }
            }
            ExprKind::StringInterp { exprs, .. } => exprs.iter().for_each(|x| self.expr(x)),
            ExprKind::Block(stmts) => {
                for s in stmts {
                    if stmt_may_exit(s) {
                        break;
                    }
                    self.stmt(s);
                }
            }
            ExprKind::Literal(_)
            | ExprKind::Ident(_)
            | ExprKind::AtVar(_)
            | ExprKind::CellGet(_)
            | ExprKind::OptionalAccess(_)
            | ExprKind::Lambda { .. }
            | ExprKind::Element { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reqs_of(src: &str) -> ParamReqs {
        reqs_with(src, &|_| false)
    }

    fn reqs_with(src: &str, imported: &dyn Fn(&str) -> bool) -> ParamReqs {
        let (_, mut stmts) = crate::rewrite::parse_ast(src).expect("parse");
        crate::desugar::desugar(&mut stmts);
        collect(&stmts, imported)
    }

    fn get(r: &ParamReqs, name: &str, arity: usize) -> Option<Vec<Option<ParamReq>>> {
        r.get(&(name.to_string(), arity)).cloned()
    }

    fn field(n: &str) -> Option<ParamReq> {
        Some(ParamReq::Field(n.to_string()))
    }

    #[test]
    fn field_reads_and_native_slots() {
        let r = reqs_of("fn f(a, b, c)\n  let s = sqrt(a)\n  b.x + s\nend");
        assert_eq!(
            get(&r, "f", 3),
            Some(vec![Some(ParamReq::Num), field("x"), None])
        );
    }

    #[test]
    fn alias_of_a_native_carries_its_slots() {
        let src = "let _native_rect = draw_rect\n\
                   fn draw_rect(x, y, w, h, c)\n  _native_rect(x, y, w, h, c.r, c.g, c.b)\nend";
        let r = reqs_of(src);
        let n = Some(ParamReq::Num);
        assert_eq!(
            get(&r, "draw_rect", 5),
            Some(vec![n.clone(), n.clone(), n.clone(), n, field("r")])
        );
    }

    /// An alias written after the module's own declaration of that name
    /// captures the module's function, not the native.
    #[test]
    fn alias_after_own_declaration_is_not_native() {
        let src = "fn draw_rect(c)\n  c\nend\nlet dr = draw_rect\n\
                   fn g(x)\n  dr(x, 0, 0, 0, 0, 0, 0)\nend";
        assert_eq!(get(&reqs_of(src), "g", 1), None);
    }

    /// A name another module declares (an import could bind it) is not
    /// resolved to the native of that name.
    #[test]
    fn imported_names_are_not_natives() {
        let src = "fn g(x)\n  sqrt(x)\nend";
        assert!(get(&reqs_of(src), "g", 1).is_some());
        assert_eq!(get(&reqs_with(src, &|n| n == "sqrt"), "g", 1), None);
    }

    #[test]
    fn only_unconditional_code_counts() {
        for src in [
            "fn f(p)\n  if cond() then p.x else 0 end\nend",
            "fn f(p)\n  cond() && p.x\nend",
            "fn f(p)\n  p.x ?? 0\nend",
            "fn f(p)\n  p?.x\nend",
            "fn f(p)\n  for i in range(3) do print(p.x) end\nend",
            "fn f(p)\n  let g = fn() -> p.x\n  g\nend",
            "fn f(p)\n  if cond() then return 0 end\n  p.x\nend",
            "fn f(p)\n  while cond() do print(1) end\n  p.x\nend",
            "fn f(p)\n  match p\n    when 1 -> 0\n    when _ -> p.x\n  end\nend",
            // A method call is not a field read.
            "fn f(p)\n  p.shift(1)\nend",
            // A rebound parameter is not tracked at all.
            "fn f(p)\n  p = {x: p}\n  p.x\nend",
            "fn f(p)\n  let q = if cond() then {x: 1} else p end\n  q.x\nend",
            // Unknown callees impose nothing.
            "fn f(p)\n  mystery(p)\nend",
        ] {
            assert_eq!(get(&reqs_of(src), "f", 1), None, "{src}");
        }
    }

    /// The condition of an `if` runs on every call even though its branches
    /// do not.
    #[test]
    fn an_if_condition_is_unconditional() {
        let r = reqs_of("fn f(p)\n  if p.on then 1 else 0 end\nend");
        assert_eq!(get(&r, "f", 1), Some(vec![field("on")]));
    }

    /// Requirements flow through a call to another function of the module.
    #[test]
    fn requirements_propagate_through_module_calls() {
        let src = "fn inner(a, b)\n  sqrt(a) + b.y\nend\nfn outer(q, r)\n  inner(q, r)\nend";
        let r = reqs_of(src);
        // Only numeric requirements propagate: a field read inside `inner`
        // is reported against `inner`'s own call, not re-derived for `outer`.
        assert_eq!(get(&r, "outer", 2), Some(vec![Some(ParamReq::Num), None]));
    }

    #[test]
    fn field_requirement_accepts_only_possible_types() {
        let f = ParamReq::Field("x".into());
        for t in [
            Type::Int,
            Type::Float,
            Type::Bool,
            Type::Nil,
            Type::String,
            Type::List,
        ] {
            assert!(!f.accepts(t), "{t:?}");
        }
        for t in [
            Type::Record,
            Type::Any,
            Type::Vec2,
            Type::Vec3,
            Type::Pending,
            Type::Handle,
        ] {
            assert!(f.accepts(t), "{t:?}");
        }
        assert!(ParamReq::Field("length".into()).accepts(Type::String));
    }
}
