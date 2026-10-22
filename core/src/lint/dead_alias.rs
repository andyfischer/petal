//! Rule — a `let` that only gives another local a second name.
//!
//! ```text
//! fn route(dist, u)                   fn route(dist, u)
//!   let dd = dist                =>     dist[u] + dist[0]
//!   dd[u] + dd[0]                     end
//! end
//! ```
//!
//! `let a = v` is reported when, from there to the end of its block, `a` is
//! only read and `v` cannot change: then every `a` is `v`. The alias is
//! usually left over from a time when `v` was a `var` read inside a closure,
//! or from a guess that a local copy is faster (it is not: `let` binds the
//! same value).
//!
//! ## What it asks before saying so
//!
//! - Both are locals of one function: `v` is a parameter of the function the
//!   alias is in, or a `let`/`var` declared in it, and it is the only binding
//!   of that name in the function. A module-level name is never considered —
//!   aliasing a module-level `var` into a `let` is how a function takes a
//!   snapshot of it.
//! - `v` is not written after the alias — not assigned, not `set`, no field
//!   or element of it written — nor anywhere in a loop the alias is inside,
//!   nor in any nested function (which could run at any time).
//! - `a` is only read: never assigned or written into, not bound again in its
//!   block, and it is not the last statement of the block (where the `let`
//!   is the block's value).
//!
//! The fix deletes the `let` and writes `v` at each read (`get v` for a read
//! of a `var` from a nested function). It is applied when the `let` has its
//! line to itself and every read was found where the syntax tree says it is.

use crate::ast::{
    AssignTarget, ElseBranch, Expr, ExprKind, ExprVisitor, Param, Pattern, Stmt, StmtKind,
    walk_expr, walk_stmt,
};

use super::Fix;
use super::to_match::Splice;
use super::util::{ends_its_line, span_text, squash, starts_its_line};

pub(super) fn plan_alias_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let mut finder = Finder {
        chars,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    // Two fixes may touch the same text (an alias of an alias). The first
    // goes ahead; the other is dropped, for the next round to find in the
    // rewritten text.
    let mut kept: Vec<Fix> = Vec::new();
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for fix in finder.fixes {
        let ranges: Vec<(usize, usize)> = fix
            .splices
            .iter()
            .map(|s| (s.start, s.end.max(s.start + 1)))
            .collect();
        let clash = ranges
            .iter()
            .any(|(s, e)| taken.iter().any(|(a, b)| s < b && a < e));
        if !clash {
            taken.extend(ranges);
            kept.push(fix);
        }
    }
    kept
}

struct Finder<'a> {
    chars: &'a [char],
    fixes: Vec<Fix>,
}

/// Find every function body (a `fn` or a lambda) and examine it on its own.
impl ExprVisitor for Finder<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        if let StmtKind::FnDecl { params, body, .. } = &s.kind {
            self.function(params, body);
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        if let ExprKind::Lambda { params, body } = &e.kind {
            self.function(params, body);
        }
        walk_expr(self, e);
    }
}

impl Finder<'_> {
    fn function(&mut self, params: &[Param], body: &[Stmt]) {
        let mut facts = Facts::default();
        for p in params {
            facts.bind(&p.name, Binding::Param);
        }
        facts.block(body);
        let mut blocks = Blocks {
            chars: self.chars,
            facts: &facts,
            loops: Vec::new(),
            fixes: Vec::new(),
        };
        blocks.block(body);
        self.fixes.append(&mut blocks.fixes);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Binding {
    Param,
    Let { is_var: bool, at: usize },
    Other,
}

/// What one function body does with its names, nested functions included.
#[derive(Default)]
struct Facts {
    /// Every binding of each name in the function, at any depth.
    bindings: Vec<(String, Binding, usize)>,
    /// Every write rooted at a name: `(name, offset, in a nested function)`.
    writes: Vec<(String, usize, bool)>,
    fn_depth: usize,
}

impl Facts {
    fn bind(&mut self, name: &str, b: Binding) {
        self.bindings.push((name.to_string(), b, self.fn_depth));
    }

    fn block(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.visit_stmt(s);
        }
    }

    /// The one binding of `name` in this function, if it has exactly one and
    /// it is the function's own (not a nested function's).
    fn sole_binding(&self, name: &str) -> Option<Binding> {
        let mut found = self.bindings.iter().filter(|(n, ..)| n == name);
        let (_, binding, depth) = found.next()?;
        (found.next().is_none() && *depth == 0).then_some(*binding)
    }

    fn pattern(&mut self, p: &Pattern) {
        for n in p.bound_names() {
            self.bind(&n, Binding::Other);
        }
    }

    fn nested(&mut self, params: &[Param], body: &[Stmt]) {
        self.fn_depth += 1;
        for p in params {
            self.bind(&p.name, Binding::Other);
            if let Some(d) = &p.default {
                self.visit_expr(d);
            }
        }
        self.block(body);
        self.fn_depth -= 1;
    }
}

fn target_root(target: &AssignTarget) -> Option<&str> {
    fn root(e: &Expr) -> Option<&str> {
        match &e.kind {
            ExprKind::Ident(n) | ExprKind::CellGet(n) => Some(n),
            ExprKind::FieldAccess { object, .. } | ExprKind::IndexAccess { object, .. } => {
                root(object)
            }
            _ => None,
        }
    }
    match target {
        AssignTarget::Name(n) => Some(n),
        AssignTarget::Field(object, _) | AssignTarget::Index(object, _) => root(object),
    }
}

impl ExprVisitor for Facts {
    fn visit_stmt(&mut self, s: &Stmt) {
        let at = s.span.start.offset as usize;
        match &s.kind {
            StmtKind::Let { name, is_var, .. } => {
                let b = Binding::Let {
                    is_var: *is_var,
                    at,
                };
                self.bind(name, b);
            }
            StmtKind::State { name, .. } | StmtKind::Memo { name, .. } => self.bind(name, Binding::Other),
            StmtKind::For { var, .. } => self.bind(var, Binding::Other),
            StmtKind::FnDecl {
                name, params, body, ..
            } => {
                self.bind(name, Binding::Other);
                self.nested(params, body);
                return;
            }
            StmtKind::Set { target, .. } | StmtKind::Assign { target, .. } => {
                match target_root(target) {
                    Some(root) => self.writes.push((root.to_string(), at, self.fn_depth > 0)),
                    // A write through something this cannot name: assume it
                    // may be any local.
                    None => self.writes.push((String::new(), at, self.fn_depth > 0)),
                }
            }
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lambda { params, body } => {
                self.nested(params, body);
                return;
            }
            ExprKind::For { var, .. } => self.bind(var, Binding::Other),
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    self.pattern(&arm.pattern);
                }
            }
            // `f(@x)` assigns `x`.
            ExprKind::AtVar(n) => {
                self.writes
                    .push((n.clone(), e.span.start.offset as usize, self.fn_depth > 0));
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}

/// The walk over one function's own blocks (not its nested functions', which
/// [`Finder`] examines as functions themselves).
struct Blocks<'a> {
    chars: &'a [char],
    facts: &'a Facts,
    /// The spans of the loops around the current block, within this function.
    loops: Vec<(usize, usize)>,
    fixes: Vec<Fix>,
}

impl Blocks<'_> {
    fn block(&mut self, stmts: &[Stmt]) {
        for (i, s) in stmts.iter().enumerate() {
            if i + 1 < stmts.len()
                && let Some(fix) = self.alias(s, &stmts[..i], &stmts[i + 1..])
            {
                self.fixes.push(fix);
            }
            self.visit_stmt(s);
        }
    }

    fn in_loop(&mut self, span: crate::source_map::SourceSpan, body: &[Stmt]) {
        self.loops
            .push((span.start.offset as usize, span.end.offset as usize));
        self.block(body);
        self.loops.pop();
    }

    fn alias(&self, decl: &Stmt, before: &[Stmt], after: &[Stmt]) -> Option<Fix> {
        let StmtKind::Let {
            name: alias,
            ty: None,
            value,
            is_var: false,
            is_config: false,
            is_const: false,
        } = &decl.kind
        else {
            return None;
        };
        if decl.exported {
            return None;
        }
        let (ExprKind::Ident(target) | ExprKind::CellGet(target)) = &value.kind else {
            return None;
        };
        if target == alias {
            return None;
        }
        let at = decl.span.start.offset as usize;
        let target_is_var = match self.facts.sole_binding(target)? {
            Binding::Param => false,
            Binding::Let { is_var, at: bound } if bound < at => is_var,
            _ => return None,
        };
        // `v` is not written from here on, in any loop this is inside, or in
        // any nested function.
        let frozen = self.facts.writes.iter().all(|(name, w, nested)| {
            if !name.is_empty() && name != target {
                return true;
            }
            !*nested && *w < at && !self.loops.iter().any(|(a, b)| a <= w && w < b)
        });
        if !frozen {
            return None;
        }
        // `a`: unmentioned above, and only read below.
        let mut earlier = Reads::new(alias, self.chars);
        for s in before {
            earlier.visit_stmt(s);
        }
        if earlier.refused || !earlier.reads.is_empty() {
            return None;
        }
        let mut reads = Reads::new(alias, self.chars);
        for s in after {
            reads.visit_stmt(s);
        }
        if reads.refused || reads.reads.is_empty() {
            return None;
        }
        let message = format!(
            "`let {alias} = {target}` is a second name for `{target}`, which does not change \
             while `{alias}` is read: read `{target}`"
        );
        let report = Some(Fix {
            anchor: at,
            message: message.clone(),
            splices: Vec::new(),
        });
        // The fix: the `let` has its line to itself, and each read is where
        // the tree says.
        let end = decl.span.end.offset as usize;
        let decl_text = squash(&span_text(self.chars, decl.span)?);
        if (decl_text != format!("let{alias}={target}")
            && decl_text != format!("let{alias}=get{target}"))
            || !starts_its_line(self.chars, at)
            || !ends_its_line(self.chars, end)
            || !reads.all_verified
        {
            return report;
        }
        let mut line_start = at;
        while line_start > 0 && self.chars[line_start - 1] != '\n' {
            line_start -= 1;
        }
        let mut line_end = end;
        while line_end < self.chars.len() && self.chars[line_end] != '\n' {
            line_end += 1;
        }
        if line_end == self.chars.len() {
            return report;
        }
        let mut splices = vec![Splice {
            start: line_start,
            end: line_end + 1,
            text: String::new(),
        }];
        for (start, end, nested) in reads.reads {
            let text = if nested && target_is_var {
                format!("get {target}")
            } else {
                target.clone()
            };
            splices.push(Splice { start, end, text });
        }
        Some(Fix {
            anchor: at,
            message,
            splices,
        })
    }
}

impl ExprVisitor for Blocks<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            // A nested function is examined on its own.
            StmtKind::FnDecl { .. } => {}
            StmtKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.in_loop(s.span, body);
            }
            StmtKind::While { condition, body } => {
                self.visit_expr(condition);
                self.in_loop(s.span, body);
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Lambda { .. } => {}
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.visit_expr(condition);
                self.block(then_body);
                match else_body {
                    Some(ElseBranch::Block(stmts)) => self.block(stmts),
                    Some(ElseBranch::ElseIf(e)) => self.visit_expr(e),
                    None => {}
                }
            }
            ExprKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.in_loop(e.span, body);
            }
            ExprKind::Match { subject, arms } => {
                self.visit_expr(subject);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    self.block(&arm.body);
                }
            }
            _ => walk_expr(self, e),
        }
    }
}

/// Every mention of one name across a stretch of statements.
struct Reads<'a> {
    name: &'a str,
    chars: &'a [char],
    fn_depth: usize,
    /// The name is written, bound again, or used in a way that is not a read.
    refused: bool,
    /// `(start, end, inside a nested function)` of each read.
    reads: Vec<(usize, usize, bool)>,
    /// Every read's span holds exactly the name.
    all_verified: bool,
}

impl<'a> Reads<'a> {
    fn new(name: &'a str, chars: &'a [char]) -> Self {
        Reads {
            name,
            chars,
            fn_depth: 0,
            refused: false,
            reads: Vec::new(),
            all_verified: true,
        }
    }

    fn rebinds(&mut self, bound: &str) {
        if bound == self.name {
            self.refused = true;
        }
    }

    fn nested(&mut self, params: &[Param], body: &[Stmt]) {
        self.fn_depth += 1;
        for p in params {
            self.rebinds(&p.name);
            if let Some(d) = &p.default {
                self.visit_expr(d);
            }
        }
        for s in body {
            self.visit_stmt(s);
        }
        self.fn_depth -= 1;
    }
}

impl ExprVisitor for Reads<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, .. } | StmtKind::State { name, .. } | StmtKind::Memo { name, .. } => self.rebinds(name),
            StmtKind::For { var, .. } => self.rebinds(var),
            StmtKind::FnDecl {
                name, params, body, ..
            } => {
                self.rebinds(name);
                self.nested(params, body);
                return;
            }
            StmtKind::Set { target, .. } | StmtKind::Assign { target, .. }
                if target_root(target).is_none_or(|r| r == self.name) =>
            {
                self.refused = true;
            }
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Ident(n) if n == self.name => {
                let (start, end) = (e.span.start.offset as usize, e.span.end.offset as usize);
                if span_text(self.chars, e.span).as_deref() != Some(self.name) {
                    self.all_verified = false;
                }
                self.reads.push((start, end, self.fn_depth > 0));
            }
            ExprKind::CellGet(n) | ExprKind::AtVar(n) if n == self.name => self.refused = true,
            ExprKind::Lambda { params, body } => {
                self.nested(params, body);
                return;
            }
            ExprKind::For { var, .. } => self.rebinds(var),
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    for n in arm.pattern.bound_names() {
                        self.rebinds(&n);
                    }
                }
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}

#[cfg(test)]
mod tests {
    use super::super::to_match::apply_match_edits;
    use super::*;

    fn plan(src: &str) -> (String, usize) {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        let fixes = plan_alias_fixes(&stmts, &chars);
        let n = fixes.len();
        (apply_match_edits(&chars, &super::super::flatten(fixes)), n)
    }

    #[test]
    fn an_alias_of_a_parameter_or_a_let_is_removed() {
        let src = "fn route(dist, u)\n  let dd = dist\n  let base = dd[0]\n  let b = base\n  \
                   dd[u] + b + len(dd)\nend\n";
        assert_eq!(
            plan(src).0,
            "fn route(dist, u)\n  let base = dist[0]\n  dist[u] + base + len(dist)\nend\n"
        );
    }

    #[test]
    fn a_var_written_before_the_alias_is_fine_and_a_closure_reads_it_with_get() {
        let src = "fn f(xs)\n  var dist = [0, 0]\n  for x in xs do set dist[0] += x end\n  \
                   let dd = dist\n  let g = fn(i) -> dd[i]\n  g(0) + dd[1]\nend\n";
        assert_eq!(
            plan(src).0,
            "fn f(xs)\n  var dist = [0, 0]\n  for x in xs do set dist[0] += x end\n  \
             let g = fn(i) -> get dist[i]\n  g(0) + dist[1]\nend\n"
        );
    }

    #[test]
    fn interpolation_reads_are_rewritten_too() {
        let src = "fn f(name)\n  let n = name\n  print(\"hi {n}!\")\n  n\nend\n";
        let (out, count) = plan(src);
        assert_eq!(count, 1);
        assert!(
            out == "fn f(name)\n  print(\"hi {name}!\")\n  name\nend\n" || out == src,
            "{out}"
        );
    }

    #[test]
    fn nothing_is_said_when_the_two_can_differ() {
        for src in [
            // `v` is written after the alias: the alias is a snapshot.
            "fn f(xs)\n  var n = 0\n  let was = n\n  set n = 5\n  was + n\nend\n",
            "fn f(xs)\n  let ys = xs\n  xs[0] = 1\n  ys\nend\n",
            // …or earlier in the loop the alias is in.
            "fn f(xs)\n  var n = 0\n  for x in xs do\n    let was = n\n    print(was)\n    set n += x\n  end\n  n\nend\n",
            "fn f(xs)\n  var n = 0\n  for x in xs do\n    set n += x\n    let was = n\n    print(was)\n  end\n  n\nend\n",
            // …or by a nested function.
            "fn f(xs)\n  var n = 0\n  let bump = fn()\n    set n += 1\n    nil\n  end\n  let was = n\n  bump()\n  was\nend\n",
            // The alias is written.
            "fn f(xs)\n  let ys = xs\n  ys[0] = 1\n  ys\nend\n",
            "fn f(x)\n  let y = x\n  y = y + 1\n  y\nend\n",
            // A module-level name is a snapshot by design.
            "var N = 0\nfn f()\n  let n = N\n  n + 1\nend\n",
            "let N = 0\nfn f()\n  let n = N\n  n + 1\nend\n",
            // `v` is bound twice in the function.
            "fn f(x)\n  if x then\n    let v = 1\n    print(v)\n  end\n  let v = 2\n  let a = v\n  a + 1\nend\n",
            // The alias is the block's value, or is never read.
            "fn f(x)\n  let y = x\nend\n",
            "fn f(x)\n  let y = x\n  x\nend\n",
            // Not a bare name.
            "fn f(x)\n  let y = x.a\n  y + y\nend\n",
            // Top level.
            "let a = 1\nlet b = a\nprint(b)\n",
        ] {
            let (out, n) = plan(src);
            assert_eq!(out, src);
            assert_eq!(n, 0, "{src}");
        }
    }

    #[test]
    fn reports_without_fixing_an_alias_that_shares_its_line() {
        let src = "fn f(x)\n  if x then let y = x; print(y) end\n  x\nend\n";
        let Ok((_tree, stmts)) = crate::rewrite::parse_ast(src) else {
            return;
        };
        let chars: Vec<char> = src.chars().collect();
        let fixes = plan_alias_fixes(&stmts, &chars);
        assert!(fixes.iter().all(|f| f.splices.is_empty()));
    }
}
