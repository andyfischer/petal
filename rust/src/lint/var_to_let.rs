//! Rule — turn a `var` that never needed to be a cell into a `let`.
//!
//! ```text
//! var fx = PAD                          let fx = PAD
//! set fx = fx + chip(fx, fy, "P")  =>   fx = fx + chip(fx, fy, "P")
//! print(get fx)                         print(fx)
//! ```
//!
//! A `var` is a box: its point is that a function or closure that mentions it
//! shares it, so a write from inside one lands outside (docs/language-guide.md,
//! `var` and `set`). A `var` whose every read and write stays in the function
//! that declares it gets none of that. There, `set x = …` and a `let` rebind
//! `x = …` mean the same thing — a `let` rebind already carries through `if`,
//! `match`, `for` (statement and collecting forms) and `while`, `break` and
//! `continue` included — and the `let` spelling is the one the compiler can
//! trace as dataflow.
//!
//! The rewrite is a keyword swap and nothing else: `var` becomes `let`, each
//! `set ` in front of a write is dropped, and each written `get x` becomes
//! `x`. Every statement keeps its place and its nesting, so the result is
//! never more complicated than the original, however deep in control flow the
//! writes sit. Folding `x = x + e` into `x += e` is a separate rule
//! ([`super::compound`]) that runs afterwards.
//!
//! ## What it refuses, and why
//!
//! - **Any mention inside a nested `fn` or lambda.** This is the case `var`
//!   exists for. A write there must reach the outer binding, which `=` cannot
//!   do, and a read there sees the box's contents *now*, where a `let` capture
//!   would freeze the value at the point the function was written. A nested
//!   function that takes a parameter of the same name is fine: the name there
//!   is the parameter.
//! - **The name bound again in its scope** — `let`, `var`, `state`, a `for`
//!   variable, a match-pattern binding or a `fn` of the same name. Shadowing
//!   interacts differently with cells and rebinds; not worth reasoning about.
//! - **The name mentioned earlier in the same block.** Those mentions are of
//!   some other binding (or, for a hoisted `fn`, possibly of this one).
//! - **`@x`**, and a plain `=` on the name: neither compiles against a `var`,
//!   so either would mean the analysis misread the scope.
//! - **`export var`** and **`state var`**: other modules, or later frames,
//!   may be the ones reading it. (`state var` is a different statement and
//!   never reaches this rule; `config var` does not parse.)

use crate::ast::{
    AssignTarget, ElseBranch, Expr, ExprKind, ExprVisitor, Param, Pattern, Stmt, StmtKind,
    walk_expr, walk_stmt,
};
use crate::source_map::SourceSpan;

use super::Fix;
use super::to_match::Splice;

/// Plan every `var` → `let` rewrite in `stmts`, one [`Fix`] per `var`, in
/// source order.
pub(super) fn plan_var_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let mut finder = Finder {
        chars,
        fixes: Vec::new(),
    };
    finder.block(stmts);
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

struct Finder<'a> {
    chars: &'a [char],
    fixes: Vec<Fix>,
}

impl Finder<'_> {
    /// Visit a statement list: try each `var` it declares against the rest of
    /// the list, then descend.
    fn block(&mut self, stmts: &[Stmt]) {
        for (i, s) in stmts.iter().enumerate() {
            if let StmtKind::Let {
                name, is_var: true, ..
            } = &s.kind
                && !s.exported
                && let Some(edits) = plan_one(name, s, &stmts[..i], &stmts[i + 1..], self.chars)
            {
                self.fixes.push(Fix {
                    anchor: s.span.start.offset as usize,
                    message: format!(
                        "`var {name}` is never shared with a nested function; declare it with `let`"
                    ),
                    splices: edits,
                });
            }
            self.visit_stmt(s);
        }
    }
}

/// Route every statement list through [`Finder::block`], so a `var` declared
/// in any block — a function body, a loop body, an `if` arm — is considered.
impl ExprVisitor for Finder<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::FnDecl { body, .. } => self.block(body),
            StmtKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.block(body);
            }
            StmtKind::While { condition, body } => {
                self.visit_expr(condition);
                self.block(body);
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
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
                self.block(body);
            }
            ExprKind::Block(stmts) | ExprKind::Lambda { body: stmts, .. } => self.block(stmts),
            _ => walk_expr(self, e),
        }
    }
}

/// Decide whether the `var` declared by `decl` can become a `let`, and if so
/// return the splices that make it one.
fn plan_one(
    name: &str,
    decl: &Stmt,
    before: &[Stmt],
    after: &[Stmt],
    chars: &[char],
) -> Option<Vec<Splice>> {
    // Anything earlier in the block that so much as mentions the name.
    let mut earlier = Uses::new(name, chars);
    for s in before {
        earlier.visit_stmt(s);
    }
    if earlier.refused || earlier.mentioned {
        return None;
    }

    let mut uses = Uses::new(name, chars);
    for s in after {
        uses.visit_stmt(s);
    }
    if uses.refused {
        return None;
    }

    let mut splices = vec![var_keyword_splice(decl, chars)?];
    for set_start in uses.sets {
        let end = keyword_end(chars, set_start, "set")?;
        splices.push(Splice {
            start: set_start,
            end,
            text: String::new(),
        });
    }
    for span in uses.gets {
        splices.push(Splice {
            start: span.start.offset as usize,
            end: span.end.offset as usize,
            text: name.to_string(),
        });
    }
    Some(splices)
}

/// `var` → `let` on the declaration. The statement starts at the keyword
/// (the `export` form is filtered out before this is called).
fn var_keyword_splice(decl: &Stmt, chars: &[char]) -> Option<Splice> {
    let start = decl.span.start.offset as usize;
    if !is_keyword_at(chars, start, "var") {
        return None;
    }
    Some(Splice {
        start,
        end: start + 3,
        text: "let".to_string(),
    })
}

/// Is `kw` written at `pos`, as a whole word?
fn is_keyword_at(chars: &[char], pos: usize, kw: &str) -> bool {
    let n = kw.chars().count();
    if pos + n > chars.len() {
        return false;
    }
    let word: String = chars[pos..pos + n].iter().collect();
    word == kw
        && chars
            .get(pos + n)
            .is_none_or(|c| !(c.is_alphanumeric() || *c == '_'))
}

/// The offset just past `kw` at `pos` and the whitespace after it, so
/// deleting `pos..end` removes the keyword without leaving a double space.
fn keyword_end(chars: &[char], pos: usize, kw: &str) -> Option<usize> {
    if !is_keyword_at(chars, pos, kw) {
        return None;
    }
    let mut end = pos + kw.chars().count();
    while chars.get(end).is_some_and(|c| *c == ' ' || *c == '\t') {
        end += 1;
    }
    Some(end)
}

/// Every use of one name across a stretch of statements, with the reasons
/// the conversion would have to be refused.
struct Uses<'a> {
    name: &'a str,
    chars: &'a [char],
    /// How many nested `fn`/lambda bodies deep the walk currently is.
    fn_depth: usize,
    refused: bool,
    /// Any mention at all, of any kind.
    mentioned: bool,
    /// Start offsets of the `set` statements that write the name.
    sets: Vec<usize>,
    /// Spans of written-out `get name` reads.
    gets: Vec<SourceSpan>,
}

impl<'a> Uses<'a> {
    fn new(name: &'a str, chars: &'a [char]) -> Self {
        Uses {
            name,
            chars,
            fn_depth: 0,
            refused: false,
            mentioned: false,
            sets: Vec::new(),
            gets: Vec::new(),
        }
    }

    /// A nested function body. One that binds the name as a parameter reads
    /// its own parameter throughout, so it is skipped whole.
    fn nested_fn(&mut self, params: &[Param], body: &[Stmt]) {
        if params.iter().any(|p| p.name == self.name) {
            return;
        }
        self.fn_depth += 1;
        for s in body {
            self.visit_stmt(s);
        }
        self.fn_depth -= 1;
    }

    /// A binding of the name (a shadow).
    fn rebinds(&mut self, bound: &str) {
        if bound == self.name {
            self.mentioned = true;
            self.refused = true;
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Wildcard | Pattern::Literal(_) => {}
            Pattern::Variable(v) => self.rebinds(v),
            Pattern::Variant { fields, .. } => fields.iter().for_each(|f| self.pattern(f)),
            Pattern::List { elements, rest } => {
                elements.iter().for_each(|e| self.pattern(e));
                if let Some(r) = rest {
                    self.rebinds(r);
                }
            }
            Pattern::Record(fields) => fields.iter().for_each(|(_, f)| self.pattern(f)),
        }
    }

    /// Is `get` written in the source at the start of `span`? A compound
    /// `set x += 1` synthesizes its `get x` over the text `x`, and that one
    /// has nothing to rewrite.
    fn written_get(&self, span: SourceSpan) -> bool {
        is_keyword_at(self.chars, span.start.offset as usize, "get")
    }
}

/// The name a write target is rooted at: `x`, `x.f`, `x[i].g`, …
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

impl ExprVisitor for Uses<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { name, .. } | StmtKind::State { name, .. } => self.rebinds(name),
            StmtKind::For { var, .. } => self.rebinds(var),
            StmtKind::FnDecl {
                name, params, body, ..
            } => {
                self.rebinds(name);
                self.nested_fn(params, body);
                return;
            }
            StmtKind::Set { target, .. } if target_root(target) == Some(self.name) => {
                self.mentioned = true;
                if self.fn_depth > 0 {
                    self.refused = true;
                } else {
                    self.sets.push(s.span.start.offset as usize);
                }
            }
            StmtKind::Assign { target, .. } if target_root(target) == Some(self.name) => {
                self.mentioned = true;
                self.refused = true;
            }
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Ident(n) if n == self.name => {
                self.mentioned = true;
                if self.fn_depth > 0 {
                    self.refused = true;
                }
            }
            ExprKind::CellGet(n) if n == self.name => {
                self.mentioned = true;
                if self.fn_depth > 0 {
                    self.refused = true;
                } else if self.written_get(e.span) {
                    self.gets.push(e.span);
                }
            }
            ExprKind::AtVar(n) if n == self.name => {
                self.mentioned = true;
                self.refused = true;
            }
            ExprKind::Lambda { params, body } => {
                self.nested_fn(params, body);
                return;
            }
            ExprKind::For { var, .. } => self.rebinds(var),
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    self.pattern(&arm.pattern);
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

    /// Run the rule alone and return the rewritten text.
    fn rewrite(src: &str) -> String {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        let chars: Vec<char> = src.chars().collect();
        apply_match_edits(
            &chars,
            &super::super::flatten(plan_var_fixes(&stmts, &chars)),
        )
    }

    fn unchanged(src: &str) {
        assert_eq!(rewrite(src), src, "expected no rewrite");
    }

    #[test]
    fn converts_a_straight_line_accumulator() {
        let src = "let fy = 1\nvar fx = 2\nset fx = fx + fy\nset fx += 3\nprint(get fx)\n";
        assert_eq!(
            rewrite(src),
            "let fy = 1\nlet fx = 2\nfx = fx + fy\nfx += 3\nprint(fx)\n"
        );
    }

    #[test]
    fn converts_writes_inside_control_flow() {
        let src = "fn f(xs)\n  var out = []\n  for x in xs do\n    if x > 1 then\n      set out = append(out, x)\n    end\n  end\n  out\nend\n";
        assert_eq!(
            rewrite(src),
            src.replace("var out", "let out").replace("set out", "out")
        );
    }

    #[test]
    fn converts_field_and_index_writes() {
        let src = "var r = {a: 1, xs: [1]}\nset r.a += 1\nset r.xs[0] = 5\nprint(r)\n";
        assert_eq!(
            rewrite(src),
            "let r = {a: 1, xs: [1]}\nr.a += 1\nr.xs[0] = 5\nprint(r)\n"
        );
    }

    #[test]
    fn keeps_a_var_written_from_a_callback() {
        unchanged("var n = 0\neach([1, 2], fn(x) set n = get n + x end)\nprint(n)\n");
    }

    #[test]
    fn keeps_a_var_written_from_a_function() {
        unchanged("var hits = 0\nfn hit()\n  set hits += 1\nend\nhit()\nprint(hits)\n");
    }

    #[test]
    fn keeps_a_var_read_from_a_function() {
        // A `let` capture would freeze the value where the function is written.
        unchanged(
            "var hits = 0\nfn describe() \"{get hits}\" end\nset hits = 3\nprint(describe())\n",
        );
    }

    #[test]
    fn a_shadowing_parameter_does_not_block() {
        let src = "var x = 1\nlet f = fn(x) x * 2 end\nset x = f(x)\nprint(x)\n";
        assert_eq!(
            rewrite(src),
            "let x = 1\nlet f = fn(x) x * 2 end\nx = f(x)\nprint(x)\n"
        );
    }

    #[test]
    fn keeps_a_redeclared_name() {
        unchanged("var x = 1\nif true then\n  let x = 2\n  print(x)\nend\nset x = 3\n");
        unchanged("var i = 0\nfor i in [1, 2] do print(i) end\nset i = 3\n");
        unchanged("var v = 0\nmatch 1\n  when v -> print(v)\nend\nset v = 3\n");
    }

    #[test]
    fn keeps_a_name_mentioned_before_it() {
        unchanged("fn f() get x end\nvar x = 1\nset x = 2\nprint(f())\n");
    }

    #[test]
    fn keeps_state_vars() {
        unchanged("state var n = 0\nset n += 1\n");
    }
}
