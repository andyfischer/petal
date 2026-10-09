//! Return types for functions that end in a loop.
//!
//! A `for` in tail position is a mapping: it collects each iteration's last
//! expression into a list, and that list is the function's implicit return
//! (docs/implicit-return-values.md). An un-annotated function that ends in one
//! therefore says nothing about which of two things its author meant:
//!
//! ```petal
//! fn squares(xs)                 fn draw_all(items)
//!   for x in xs do x * x end       for it in items do draw(it) end
//! end                            end
//! ```
//!
//! The first is a mapping and wants `-> list`. The second is a side-effect
//! loop that builds, returns and drops a list of whatever `draw` yields on
//! every call; declaring it `-> nil` turns the implicit return off, so the
//! loop allocates nothing.
//!
//! Which one a function is shows in how it is *called*, so that is what this
//! looks at:
//!
//! - **some call uses the result** — suggest `-> list`;
//! - **called, and no call uses the result** — suggest `-> nil`;
//! - **never called, or `pub`** — the callers that would settle it are not in
//!   view (a host calling it by name, another module importing it), so both
//!   options are reported and neither is written.
//!
//! Unlike the other rewriting kinds, `-> nil` is **not** IR-preserving — the
//! point of it is that the compiled function changes. It is behaviour
//! preserving exactly when the evidence is complete, i.e. when no caller this
//! analysis cannot see reads the list. `-> list` changes nothing but the type
//! checker's knowledge, and `--apply` proves that by comparing IR.
//!
//! # How a call's use is decided
//!
//! By position, mirroring the compiler's own value-position rule: a call
//! written as a statement is discarded; one that is bound, passed, returned,
//! or collected is used. A call that is the *tail* of another un-annotated
//! function is neither — its result is that function's result — so use is
//! propagated through such forwarding until nothing changes. Everything is
//! matched by name, and every doubt resolves toward "used" or "unknown": a
//! function that is ever read as a value (`map(xs, f)`) rather than called is
//! unknown, since nothing here follows where the value goes, and so is one
//! whose name some inner scope rebinds (`let draw = …`, a parameter `draw`),
//! since a call through that name may never reach it.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{
    ElseBranch, Expr, ExprKind, ExprVisitor, Pattern, Stmt, StmtKind, declares_nil, walk_expr,
    walk_stmt,
};

/// How the callers in view treat a function's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Usage {
    /// Called, and every call drops the result.
    Unused,
    /// Not enough callers in view to tell.
    Unknown,
    /// At least one call reads the result.
    Used,
}

impl Usage {
    pub fn name(self) -> &'static str {
        match self {
            Usage::Unused => "unused",
            Usage::Unknown => "unknown",
            Usage::Used => "used",
        }
    }
}

/// One function whose loop tail is an undeclared implicit return.
#[derive(Debug, Clone)]
pub struct ReturnType {
    /// The declared name (`Class.method` for a method) and arity.
    pub function: (String, usize),
    /// 1-based line of the declaration.
    pub line: usize,
    pub usage: Usage,
    /// The type to write — `"list"` or `"nil"` — or `None` when the choice is
    /// the author's (see [`Usage::Unknown`]).
    pub ty: Option<&'static str>,
    /// Character offset where `text` is inserted: just past the `)` of the
    /// parameter list.
    pub at: usize,
    /// Exactly what to insert (`" -> nil"`). Empty when `ty` is `None`.
    pub text: String,
    /// Why, in one line.
    pub because: String,
}

impl ReturnType {
    /// Whether there is an edit to write. A choice left to the author has
    /// none, so `--apply` skips it and `--verify` does not count it.
    pub fn is_edit(&self) -> bool {
        self.ty.is_some()
    }
}

/// Where a call sits, as far as its result is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Position {
    /// A statement whose value is dropped.
    Discarded,
    /// Bound, passed, returned, collected: read by something.
    Value,
    /// The implicit return of the named top-level function, so used exactly
    /// when that function's own result is.
    TailOf(String),
}

/// What the loops at the end of a body amount to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopTail {
    /// The tail is not a loop on any path.
    None,
    /// Some paths end in a loop and some in another value.
    Some,
    /// Every path ends in a loop: the function returns a list.
    All,
}

/// What a top-level declaration says about its own return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Declared {
    Nothing,
    Nil,
    Other,
}

#[derive(Default)]
struct Evidence {
    /// Every call, by the callee's bare name (the last segment of a method or
    /// module-qualified call).
    calls: Vec<(String, Position)>,
    /// Names read as a value rather than called.
    read_as_value: BTreeSet<String>,
    /// Names some inner scope binds for itself: a `let`/`var`/`state`, a
    /// parameter, a loop variable, a pattern variable, a nested `fn`. A call
    /// through such a name may be a call to the local rather than to the
    /// top-level function it shadows, and calls are matched by name alone.
    rebound: BTreeSet<String>,
}

/// Find every return-type suggestion for the target file's own top-level
/// functions. `extra` is the parsed `--from` entry points: they contribute
/// call sites and nothing else.
pub fn find(source: &str, stmts: &[Stmt], extra: &[Vec<Stmt>]) -> Vec<ReturnType> {
    let mut evidence = Evidence::default();
    Walker::new(&mut evidence, true).walk_root(stmts);
    for other in extra {
        Walker::new(&mut evidence, false).walk_root(other);
    }

    // What each top-level name declares. A name declared several times (arity
    // overloads) is one entry: calls are matched by name alone, so the
    // variants share their evidence, and any variant with a declared type
    // settles forwarding through the name.
    let mut declared: BTreeMap<&str, (Declared, bool)> = BTreeMap::new();
    for stmt in stmts {
        if let StmtKind::FnDecl { name, ret, .. } = &stmt.kind {
            let d = match ret {
                None => Declared::Nothing,
                Some(_) if declares_nil(ret.as_ref()) => Declared::Nil,
                Some(_) => Declared::Other,
            };
            let entry = declared.entry(bare(name)).or_insert((d, false));
            if d != Declared::Nothing {
                entry.0 = d;
            }
            entry.1 |= stmt.exported;
        }
    }

    let usage = usages(&declared, &evidence);
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    for stmt in stmts {
        let StmtKind::FnDecl {
            name,
            params,
            ret: None,
            body,
            ..
        } = &stmt.kind
        else {
            continue;
        };
        let tail = loop_tail(body);
        // A body with its own `return <value>` has a second way out that a
        // declared type would also have to describe; leave it to the author.
        if tail == LoopTail::None || returns_a_value(body) {
            continue;
        }
        let key = bare(name);
        let usage = usage.get(key).copied().unwrap_or(Usage::Unknown);
        let calls = evidence.calls.iter().filter(|(n, _)| n == key).count();
        let ty = match (usage, tail) {
            (Usage::Used, LoopTail::All) => Some("list"),
            // Used, but only some paths yield the loop's list: no one type to
            // name, and `-> nil` would break the caller.
            (Usage::Used, _) => continue,
            (Usage::Unused, _) => Some("nil"),
            (Usage::Unknown, _) => None,
        };
        let Some(at) = insertion_point(stmt, &chars) else {
            continue;
        };
        let what = match tail {
            LoopTail::All => "ends in a `for` loop, which collects a list as its implicit return",
            _ => "can end in a `for` loop, which collects a list as its implicit return",
        };
        let because = match usage {
            Usage::Used => format!("{what}, and a caller uses that list"),
            Usage::Unused => format!(
                "{what}, but {} uses it — `-> nil` turns the implicit return off, so the \
                 loop builds no list",
                match calls {
                    1 => "its one call never".to_string(),
                    n => format!("none of its {n} calls"),
                }
            ),
            Usage::Unknown => {
                let why = if declared.get(key).is_some_and(|d| d.1) {
                    "it is `pub`, so its callers are not all in this file"
                } else if calls == 0 {
                    "nothing in view calls it"
                } else if evidence.read_as_value.contains(key) {
                    "it is passed around as a value, so its calls cannot all be seen"
                } else if evidence.rebound.contains(key) {
                    "its name is also bound as a local, so a call by that name may not be a \
                     call to it"
                } else {
                    "its result is forwarded by a function whose own callers are not in view"
                };
                format!(
                    "{what}; {why}. Declare `-> list` if callers read the list, or `-> nil` \
                     if the loop is run for its side effects (no list is built)"
                )
            }
        };
        out.push(ReturnType {
            function: (name.clone(), params.len()),
            line: stmt.span.start.line as usize,
            usage,
            ty,
            at,
            text: ty.map(|t| format!(" -> {t}")).unwrap_or_default(),
            because,
        });
    }
    out
}

/// The name a call site would use for a declaration: `draw` for both
/// `fn draw` and the method `fn Sprite.draw`.
fn bare(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Resolve every declared name's [`Usage`], propagating through calls that
/// are another function's implicit return until nothing changes. Usage only
/// ever rises (`Unused` → `Unknown` → `Used`), so this terminates.
fn usages<'a>(
    declared: &BTreeMap<&'a str, (Declared, bool)>,
    evidence: &Evidence,
) -> BTreeMap<&'a str, Usage> {
    let mut usage: BTreeMap<&str, Usage> = declared
        .iter()
        .map(|(name, (_, exported))| {
            let called = evidence.calls.iter().any(|(n, _)| n == name);
            let open = *exported
                || !called
                || evidence.read_as_value.contains(*name)
                || evidence.rebound.contains(*name);
            (*name, if open { Usage::Unknown } else { Usage::Unused })
        })
        .collect();
    loop {
        let mut changed = false;
        for (callee, position) in &evidence.calls {
            let Some(&current) = usage.get(callee.as_str()) else {
                continue;
            };
            let seen = match position {
                Position::Discarded => Usage::Unused,
                Position::Value => Usage::Used,
                Position::TailOf(outer) => match declared.get(bare(outer)) {
                    Some((Declared::Nil, _)) => Usage::Unused,
                    Some((Declared::Other, _)) => Usage::Used,
                    Some((Declared::Nothing, _)) => usage[bare(outer)],
                    None => Usage::Used,
                },
            };
            if seen > current {
                *usage.get_mut(callee.as_str()).unwrap() = seen;
                changed = true;
            }
        }
        if !changed {
            return usage;
        }
    }
}

/// Classify the loops in tail position of a statement list.
fn loop_tail(stmts: &[Stmt]) -> LoopTail {
    match stmts.last().map(|s| &s.kind) {
        Some(StmtKind::For { .. }) => LoopTail::All,
        Some(StmtKind::Expr(e)) => loop_tail_expr(e),
        _ => LoopTail::None,
    }
}

fn loop_tail_expr(e: &Expr) -> LoopTail {
    let join = |parts: &[LoopTail]| {
        if parts.iter().all(|p| *p == LoopTail::All) {
            LoopTail::All
        } else if parts.iter().all(|p| *p == LoopTail::None) {
            LoopTail::None
        } else {
            LoopTail::Some
        }
    };
    match &e.kind {
        ExprKind::For { .. } => LoopTail::All,
        ExprKind::Block(stmts) => loop_tail(stmts),
        ExprKind::If {
            then_body,
            else_body,
            ..
        } => join(&[
            loop_tail(then_body),
            match else_body {
                Some(ElseBranch::Block(stmts)) => loop_tail(stmts),
                Some(ElseBranch::ElseIf(e)) => loop_tail_expr(e),
                // A missing `else` yields nil on that path.
                None => LoopTail::None,
            },
        ]),
        ExprKind::Match { arms, .. } => {
            let arms: Vec<LoopTail> = arms.iter().map(|a| loop_tail_expr(&a.body)).collect();
            join(&arms)
        }
        _ => LoopTail::None,
    }
}

/// Whether `body` contains a `return <value>` of its own — one that belongs
/// to this function rather than to a function or lambda nested in it.
fn returns_a_value(body: &[Stmt]) -> bool {
    struct Returns(bool);
    impl ExprVisitor for Returns {
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Return(Some(_)) => self.0 = true,
                StmtKind::FnDecl { .. } => {}
                _ => walk_stmt(self, s),
            }
        }
        fn visit_expr(&mut self, e: &Expr) {
            if !matches!(e.kind, ExprKind::Lambda { .. }) {
                walk_expr(self, e);
            }
        }
    }
    let mut r = Returns(false);
    for s in body {
        r.visit_stmt(s);
    }
    r.0
}

/// The character offset just past a declaration's parameter list, or `None`
/// when a return type (or anything unexpected) already sits there.
fn insertion_point(decl: &Stmt, chars: &[char]) -> Option<usize> {
    let list = super::param_list(decl, chars)?;
    let mut i = list.close + 1;
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        i += 1;
    }
    if chars.get(i) == Some(&'-') && chars.get(i + 1) == Some(&'>') {
        return None;
    }
    Some(list.close + 1)
}

/// Walks a file recording where each call's result goes. The position of the
/// expression (or statement) about to be visited rides in a field that the
/// visit takes, the way the compiler threads `value_used`: everything is in
/// value position unless something said otherwise on the way in.
struct Walker<'e> {
    evidence: &'e mut Evidence,
    /// Whether this is the target file: only there does "the tail of `g`" name
    /// a function whose usage is being resolved. In a `--from` file a tail
    /// call is conservatively a use.
    target: bool,
    /// How many `fn`/lambda bodies enclose the current node.
    depth: usize,
    expr_position: Position,
    stmt_position: Position,
}

impl<'e> Walker<'e> {
    fn new(evidence: &'e mut Evidence, target: bool) -> Self {
        Walker {
            evidence,
            target,
            depth: 0,
            expr_position: Position::Value,
            stmt_position: Position::Discarded,
        }
    }

    /// A file's top level: every statement is discarded.
    fn walk_root(&mut self, stmts: &[Stmt]) {
        self.walk_body(stmts, Position::Discarded);
    }

    /// A statement list whose last statement is in position `tail`.
    fn walk_body(&mut self, stmts: &[Stmt], tail: Position) {
        for (i, s) in stmts.iter().enumerate() {
            self.stmt_position = if i + 1 == stmts.len() {
                tail.clone()
            } else {
                Position::Discarded
            };
            self.visit_stmt(s);
        }
        self.stmt_position = Position::Discarded;
    }

    /// A loop body. A collecting loop gathers its body's tail into the list,
    /// which is a use; a side-effect loop drops it.
    fn walk_loop_body(&mut self, body: &[Stmt], loop_position: &Position) {
        let tail = match loop_position {
            Position::Discarded => Position::Discarded,
            _ => Position::Value,
        };
        self.walk_body(body, tail);
    }
}

impl ExprVisitor for Walker<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        let position = std::mem::replace(&mut self.stmt_position, Position::Discarded);
        match &s.kind {
            StmtKind::Expr(e) => {
                self.expr_position = position;
                self.visit_expr(e);
            }
            StmtKind::For { var, iter, body } => {
                self.evidence.rebound.insert(var.clone());
                self.visit_expr(iter);
                self.walk_loop_body(body, &position);
            }
            StmtKind::Let { name, .. } | StmtKind::State { name, .. } => {
                self.evidence.rebound.insert(name.clone());
                walk_stmt(self, s);
            }
            StmtKind::While { condition, body } => {
                self.visit_expr(condition);
                self.walk_body(body, Position::Discarded);
            }
            StmtKind::FnDecl {
                name,
                params,
                ret,
                body,
                ..
            } => {
                // A top-level `fn` is the declaration being resolved; one
                // nested in a body is a local that can shadow it.
                if self.depth > 0 {
                    self.evidence.rebound.insert(bare(name).to_string());
                }
                for p in params {
                    self.evidence.rebound.insert(p.name.clone());
                }
                for d in params.iter().filter_map(|p| p.default.as_ref()) {
                    self.visit_expr(d);
                }
                let tail = match ret {
                    Some(_) if declares_nil(ret.as_ref()) => Position::Discarded,
                    Some(_) => Position::Value,
                    None if self.target && self.depth == 0 => Position::TailOf(name.clone()),
                    None => Position::Value,
                };
                self.depth += 1;
                self.walk_body(body, tail);
                self.depth -= 1;
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        let position = std::mem::replace(&mut self.expr_position, Position::Value);
        match &e.kind {
            ExprKind::Call { function, args, .. } => {
                // `f(@x)` is sugar for `x = f(x)`: the result is assigned.
                let position = if args.iter().any(|a| matches!(a.kind, ExprKind::AtVar(_))) {
                    Position::Value
                } else {
                    position
                };
                match &function.kind {
                    ExprKind::Ident(name) => self.evidence.calls.push((name.clone(), position)),
                    // A method call, or a module-qualified one.
                    ExprKind::FieldAccess { object, field } => {
                        self.evidence.calls.push((field.clone(), position));
                        self.visit_expr(object);
                    }
                    _ => self.visit_expr(function),
                }
                for a in args {
                    self.visit_expr(a);
                }
            }
            ExprKind::Ident(name) => {
                self.evidence.read_as_value.insert(name.clone());
            }
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.visit_expr(condition);
                self.walk_body(then_body, position.clone());
                match else_body {
                    Some(ElseBranch::Block(stmts)) => self.walk_body(stmts, position),
                    Some(ElseBranch::ElseIf(e)) => {
                        self.expr_position = position;
                        self.visit_expr(e);
                    }
                    None => {}
                }
            }
            ExprKind::Match { subject, arms } => {
                self.visit_expr(subject);
                for arm in arms {
                    bind_pattern(&arm.pattern, &mut self.evidence.rebound);
                    if let Some(guard) = &arm.guard {
                        self.visit_expr(guard);
                    }
                    self.expr_position = position.clone();
                    self.visit_expr(&arm.body);
                }
            }
            ExprKind::Block(stmts) => self.walk_body(stmts, position),
            ExprKind::For { var, iter, body } => {
                self.evidence.rebound.insert(var.clone());
                self.visit_expr(iter);
                self.walk_loop_body(body, &position);
            }
            ExprKind::Lambda { params, body } => {
                for p in params {
                    self.evidence.rebound.insert(p.name.clone());
                }
                for d in params.iter().filter_map(|p| p.default.as_ref()) {
                    self.visit_expr(d);
                }
                // A lambda always returns its tail, and nothing here follows
                // what its caller does with it.
                self.depth += 1;
                self.walk_body(body, Position::Value);
                self.depth -= 1;
            }
            _ => walk_expr(self, e),
        }
    }
}

/// Record every name `pattern` binds.
fn bind_pattern(pattern: &Pattern, out: &mut BTreeSet<String>) {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => {}
        Pattern::Variable(name) => {
            out.insert(name.clone());
        }
        Pattern::Variant { fields, .. } => fields.iter().for_each(|p| bind_pattern(p, out)),
        Pattern::List { elements, rest } => {
            elements.iter().for_each(|p| bind_pattern(p, out));
            out.extend(rest.clone());
        }
        Pattern::Record(fields) => fields.iter().for_each(|(_, p)| bind_pattern(p, out)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_in(src: &str) -> Vec<ReturnType> {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).unwrap();
        find(src, &stmts, &[])
    }

    fn one(src: &str) -> ReturnType {
        let mut found = find_in(src);
        assert_eq!(found.len(), 1, "{found:?}");
        found.remove(0)
    }

    #[test]
    fn a_discarded_call_suggests_nil() {
        let r = one("fn draw_all(xs)\n  for x in xs do print(x) end\nend\ndraw_all([1])\n");
        assert_eq!((r.usage, r.ty), (Usage::Unused, Some("nil")));
        assert_eq!(r.text, " -> nil");
    }

    #[test]
    fn a_used_call_suggests_list() {
        let r = one("fn sq(xs)\n  for x in xs do x * x end\nend\nlet s = sq([1])\n");
        assert_eq!((r.usage, r.ty), (Usage::Used, Some("list")));
    }

    #[test]
    fn one_use_among_discards_is_a_use() {
        let r = one("fn sq(xs)\n  for x in xs do x * x end\nend\nsq([1])\nprint(sq([2]))\n");
        assert_eq!(r.ty, Some("list"));
    }

    #[test]
    fn uncalled_and_pub_functions_are_left_to_the_author() {
        let r = one("fn sq(xs)\n  for x in xs do x * x end\nend\n");
        assert_eq!((r.usage, r.ty), (Usage::Unknown, None));
        assert!(!r.is_edit());
        let r = one("pub fn sq(xs)\n  for x in xs do x * x end\nend\nsq([1])\n");
        assert_eq!((r.usage, r.ty), (Usage::Unknown, None));
        assert!(r.because.contains("`pub`"), "{}", r.because);
    }

    #[test]
    fn a_pub_function_whose_result_is_used_is_a_list() {
        let r = one("pub fn sq(xs)\n  for x in xs do x * x end\nend\nlet s = sq([1])\n");
        assert_eq!(r.ty, Some("list"));
    }

    #[test]
    fn a_function_read_as_a_value_is_unknown() {
        let r = one("fn show(x)\n  for i in x do print(i) end\nend\nshow([1])\nmap([[1]], show)\n");
        assert_eq!(r.usage, Usage::Unknown);
    }

    #[test]
    fn a_function_shadowed_by_a_local_is_unknown() {
        // The only call by this name reaches the local, not the function.
        let f = "fn show(xs)\n  for x in xs do print(x) end\nend\n";
        for user in [
            "fn user()\n  let show = fn(a) a end\n  show(1)\n  1\nend\nuser()\n",
            "fn user(show)\n  show(1)\n  1\nend\nuser(print)\n",
            "fn user()\n  fn show(a) a end\n  show(1)\n  1\nend\nuser()\n",
            "let k = fn(show)\n  show(1)\n  1\nend\n",
            "match [print]\n  when [show] -> show(1)\nend\n",
        ] {
            let r = one(&format!("{f}{user}"));
            assert_eq!((r.usage, r.ty), (Usage::Unknown, None), "{user}");
            assert!(r.because.contains("bound as a local"), "{}", r.because);
        }
        // A use still settles it: `-> list` is true of the function whoever
        // the other calls reach.
        let used = format!("{f}fn user(show)\n  show(1)\n  1\nend\nlet v = show([1])\n");
        assert_eq!(one(&used).ty, Some("list"));
    }

    #[test]
    fn use_propagates_through_an_implicit_return() {
        // `inner`'s only call is `outer`'s tail, so it is used iff `outer` is.
        let src = "fn inner(xs)\n  for x in xs do x end\nend\nfn outer(xs)\n  inner(xs)\nend\n";
        let used = format!("{src}let v = outer([1])\n");
        assert_eq!(one(&used).ty, Some("list"));
        let dropped = format!("{src}outer([1])\n");
        assert_eq!(one(&dropped).ty, Some("nil"));
        // Nothing calls `outer`: unknown, and so is what it forwards.
        assert_eq!(one(src).usage, Usage::Unknown);
    }

    #[test]
    fn the_tail_of_a_nil_function_is_a_discard() {
        let src = "fn inner(xs)\n  for x in xs do x end\nend\n\
                   fn outer(xs) -> nil\n  inner(xs)\nend\nlet v = outer([1])\n";
        assert_eq!(one(src).ty, Some("nil"));
    }

    #[test]
    fn branch_tails_count_and_mixed_ones_never_suggest_list() {
        let both = "fn f(n)\n  if n > 0 then\n    for i in range(0, n) do i end\n  else\n    \
                    for i in range(0, 3) do i end\n  end\nend\nlet v = f(1)\n";
        assert_eq!(one(both).ty, Some("list"));
        let mixed = "fn f(n)\n  if n > 0 then\n    for i in range(0, n) do i end\n  else\n    \
                     7\n  end\nend\n";
        assert!(find_in(&format!("{mixed}let v = f(1)\n")).is_empty());
        assert_eq!(one(&format!("{mixed}f(1)\n")).ty, Some("nil"));
    }

    #[test]
    fn annotated_valued_return_and_non_loop_functions_are_skipped() {
        assert!(find_in("fn f(xs) -> list\n  for x in xs do x end\nend\nf([1])\n").is_empty());
        assert!(find_in("fn f(xs) -> nil\n  for x in xs do x end\nend\nf([1])\n").is_empty());
        assert!(find_in("fn f(xs)\n  len(xs)\nend\nf([1])\n").is_empty());
        let early = "fn f(xs)\n  if len(xs) == 0 then return 0 end\n  for x in xs do x end\nend\n\
                     f([1])\n";
        assert!(find_in(early).is_empty());
    }

    #[test]
    fn an_in_out_argument_call_is_a_use() {
        let src = "fn grow(xs)\n  for x in xs do x + 1 end\nend\nlet a = [1]\ngrow(@a)\n";
        assert_eq!(one(src).ty, Some("list"));
    }

    #[test]
    fn a_method_is_matched_by_its_method_name() {
        let src = "class Bag\n  items: list,\nend\nfn Bag.show(self)\n  \
                   for i in self.items do print(i) end\nend\nlet b = Bag([1])\nb.show()\n";
        let r = one(src);
        assert_eq!(r.function.0, "Bag.show");
        assert_eq!(r.ty, Some("nil"));
    }
}
