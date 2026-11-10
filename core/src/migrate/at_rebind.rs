//! Migration `at-rebind` — the removed `@` rebind operator.
//!
//! ```text
//! double(@a)                   =>   a = double(a)
//! let r = inc(double(@b))      =>   b = double(b)
//!                                   let r = inc(b)
//! ```
//!
//! Until 2026-11-09 a call argument written `@x` meant "assign the call's
//! result back to `x`": the desugar pass lifted the *nearest enclosing call*
//! of `@x` into `x = call(x)` just before the statement, and left `x` where
//! the call was. Where the call was the whole statement, the statement became
//! exactly the assignment. The operator was removed so that `@` is free for
//! annotations (docs/proposals/annotation-syntax.md); the parser now rejects
//! it with an error that names `petal migrate-old-code`.
//!
//! This is that desugaring, written back as source. It lifts exactly where
//! the old pass did — the expressions a statement evaluates once, at its own
//! level (`let`/assignment values, expression statements, `return`, `state`
//! initializers and keys, a `memo` key, `for` iterables, `if` conditions) and
//! each nested body (`if`/`for`/`while`/`fn`/lambda) as a statement list of
//! its own — and only a call with exactly one `@` of its own. Any other `@x`
//! (two in one call, a `while` condition, a `match` arm, a `memo`
//! initializer) was a compile or run-time error under the old pass, so it is
//! listed for a hand edit instead.
//!
//! **Recognition.** The current parser has no `@` expression, so the planner
//! keeps its own: it finds each `@` + identifier with the lexer, blanks the
//! `@` to a space (the text keeps every offset), parses that, and treats the
//! identifiers that stood behind an `@` as the markers.
//!
//! **Layout.** A statement that only becomes its assignment is rewritten in
//! place. One that gains assignments before it takes them on lines of their
//! own at its indentation; if it does not start its line (`if c then
//! f(g(@x)) end`), it is left for a hand edit.

use std::collections::{BTreeMap, HashSet};

use crate::ast::{ElseBranch, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_expr};
use crate::lexer::{Lexer, Token};
use crate::lint::Fix;
use crate::rewrite::Splice;

/// The message on a rewritten site.
const REWRITTEN: &str = "the `@` rebind operator was removed: `f(@x)` is written `x = f(x)`";
/// The message on a site whose statement does not start its line.
const MID_LINE: &str = "the `@` rebind operator was removed: this `f(@x)` becomes an assignment \
                        written before its statement, which shares its line with other code: \
                        rewrite it by hand as `x = f(x)` on a line of its own";
/// The message on a site left for a hand edit.
const HAND_EDIT: &str = "the `@` rebind operator was removed, and this `@` is not where it \
                         could be lifted into an assignment: rewrite it by hand as `x = f(x)`";

/// One `@x` in the source.
struct Marker {
    /// Char offset of the `@`.
    at: usize,
    /// Char offset just past the `@`'s trailing whitespace (where `x` starts).
    ident: usize,
    name: String,
}

/// Plan the rewrite of every `@x` in `source`. Returns no fixes when there is
/// no `@` rebind in it, and an error when the source does not parse even with
/// its `@`s read as rebinds.
pub(super) fn plan_at_rebind_fixes(source: &str) -> Result<Vec<Fix>, String> {
    let markers = find_markers(source)?;
    if markers.is_empty() {
        return Ok(Vec::new());
    }
    let chars: Vec<char> = source.chars().collect();
    let blanked = blank(&chars, &markers);
    let (_tree, stmts) = crate::rewrite::parse_ast(&blanked)
        .map_err(|e| format!("cannot read the `@` rebinds in this file: {e}"))?;

    let mut p = Planner {
        chars: &chars,
        by_ident: markers
            .iter()
            .enumerate()
            .map(|(i, m)| (m.ident, i))
            .collect(),
        markers: &markers,
        consumed: HashSet::new(),
        subs: BTreeMap::new(),
        hand_edit: HashSet::new(),
    };
    p.stmts(&stmts);

    let mut fixes = Vec::new();
    // Each outermost substitution left is one rewritten statement.
    for (&start, sub) in &p.subs {
        let anchor = sub
            .markers
            .iter()
            .map(|&i| markers[i].at)
            .min()
            .unwrap_or(start);
        fixes.push(Fix {
            anchor,
            message: REWRITTEN.to_string(),
            splices: vec![Splice {
                start,
                end: sub.end,
                text: sub.text.clone(),
            }],
        });
    }
    for (i, m) in markers.iter().enumerate() {
        if !p.consumed.contains(&i) {
            let message = if p.hand_edit.contains(&i) {
                MID_LINE
            } else {
                HAND_EDIT
            };
            fixes.push(Fix {
                anchor: m.at,
                message: message.to_string(),
                splices: Vec::new(),
            });
        }
    }
    fixes.sort_by_key(|f| f.anchor);
    Ok(fixes)
}

/// `source` with the `@` of every `@x` turned into a space: the same text,
/// offsets and all, with each rebind read as a plain use of `x`.
pub(super) fn blank_markers(source: &str) -> Result<String, String> {
    let markers = find_markers(source)?;
    Ok(blank(&source.chars().collect::<Vec<_>>(), &markers))
}

fn blank(chars: &[char], markers: &[Marker]) -> String {
    let mut out = chars.to_vec();
    for m in markers {
        out[m.at] = ' ';
    }
    out.into_iter().collect()
}

/// Every `@` directly followed (whitespace allowed) by an identifier. An `@`
/// in a string or a comment is not a token, so it is not seen.
fn find_markers(source: &str) -> Result<Vec<Marker>, String> {
    if !source.contains('@') {
        return Ok(Vec::new());
    }
    let mut lexer = Lexer::new(source).without_trivia();
    lexer.tokenize()?;
    let toks: Vec<(&Token, usize)> = lexer
        .tokens_with_spans()
        .map(|(t, s)| (t, s.start.offset as usize))
        .collect();
    Ok(toks
        .windows(2)
        .filter_map(|w| match (w[0].0, w[1].0) {
            (Token::At, Token::Ident(name)) => Some(Marker {
                at: w[0].1,
                ident: w[1].1,
                name: name.clone(),
            }),
            _ => None,
        })
        .collect())
}

/// A pending replacement of `[start, end)` (the key it is stored under).
struct Sub {
    end: usize,
    text: String,
    /// The markers it rewrites.
    markers: Vec<usize>,
}

struct Planner<'a> {
    chars: &'a [char],
    markers: &'a [Marker],
    /// Marker index by the offset of its identifier.
    by_ident: BTreeMap<usize, usize>,
    /// Markers already lifted into an assignment.
    consumed: HashSet<usize>,
    /// Disjoint replacements by start offset; an outer one absorbs the ones
    /// inside it.
    subs: BTreeMap<usize, Sub>,
    /// Lifted markers whose statement could not be laid out.
    hand_edit: HashSet<usize>,
}

impl Planner<'_> {
    fn marker(&self, e: &Expr) -> Option<usize> {
        match &e.kind {
            ExprKind::Ident(n) => {
                let i = *self.by_ident.get(&(e.span.start.offset as usize))?;
                (self.markers[i].name == *n && !self.consumed.contains(&i)).then_some(i)
            }
            _ => None,
        }
    }

    /// `[start, end)` of the source with every substitution inside it applied
    /// (and absorbed: removed from the map), plus the markers they carried.
    fn absorb(&mut self, start: usize, end: usize) -> (String, Vec<usize>) {
        let inner: Vec<usize> = self.subs.range(start..end).map(|(&s, _)| s).collect();
        let mut out = String::new();
        let mut markers = Vec::new();
        let mut at = start;
        for s in inner {
            let sub = self.subs.remove(&s).expect("listed");
            out.extend(&self.chars[at..s]);
            out.push_str(&sub.text);
            markers.extend(sub.markers);
            at = sub.end;
        }
        out.extend(&self.chars[at..end]);
        (out, markers)
    }

    fn put(&mut self, start: usize, end: usize, text: String, markers: Vec<usize>) {
        self.subs.insert(start, Sub { end, text, markers });
    }

    /// A statement list: each statement's own-level `@`s become assignments
    /// written before it.
    fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        let mut hoisted: Vec<Hoist> = Vec::new();
        match &stmt.kind {
            StmtKind::Let { value, .. } => self.lift(value, &mut hoisted),
            StmtKind::Assign { target, value } | StmtKind::Set { target, value } => {
                self.lift(value, &mut hoisted);
                match target {
                    crate::ast::AssignTarget::Name(_) => {}
                    crate::ast::AssignTarget::Field(object, _) => self.lift(object, &mut hoisted),
                    crate::ast::AssignTarget::Index(object, index) => {
                        self.lift(object, &mut hoisted);
                        self.lift(index, &mut hoisted);
                    }
                }
            }
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => self.lift(e, &mut hoisted),
            StmtKind::State { init, key, .. } => {
                self.lift(init, &mut hoisted);
                if let Some(k) = key {
                    self.lift(k, &mut hoisted);
                }
            }
            // Only the key ran unconditionally; the initializer is a hand edit.
            StmtKind::Memo { key, .. } => self.lift(key, &mut hoisted),
            StmtKind::FnDecl { body, .. } => self.stmts(body),
            StmtKind::For { iter, body, .. } => {
                self.lift(iter, &mut hoisted);
                self.stmts(body);
            }
            // The condition re-evaluates each iteration: not lifted.
            StmtKind::While { body, .. } => self.stmts(body),
            StmtKind::EnumDecl { variants, .. } => {
                for d in variants.iter().flat_map(|v| &v.data) {
                    self.lift(&d.value, &mut hoisted);
                }
            }
            _ => {}
        }
        if hoisted.is_empty() {
            return;
        }

        let (start, end) = (
            stmt.span.start.offset as usize,
            stmt.span.end.offset as usize,
        );
        // `f(@x)` as the whole statement is exactly `x = f(x)`: the read of
        // `x` left in its place is dropped.
        let whole = match (&stmt.kind, hoisted.last()) {
            (StmtKind::Expr(e), Some(h)) => {
                e.span.start.offset as usize == h.start && e.span.end.offset as usize == h.end
            }
            _ => false,
        };
        let (rest, mut markers) = self.absorb(start, end);
        let mut lines: Vec<String> = hoisted.iter().map(|h| h.text.clone()).collect();
        markers.extend(hoisted.iter().flat_map(|h| h.markers.iter().copied()));
        if !whole {
            lines.push(rest);
        }
        let text = if lines.len() == 1 {
            lines.pop().expect("one line")
        } else {
            match self.indent_of(start) {
                Some(indent) => lines.join(&format!("\n{indent}")),
                None => {
                    // Mid-line: keep the original text (the `@`s unconsumed
                    // in it) and list the site.
                    self.hand_edit.extend(markers.iter().copied());
                    for &i in &markers {
                        self.consumed.remove(&i);
                    }
                    return;
                }
            }
        };
        self.put(start, end, text, markers);
    }

    /// The whitespace before `at` on its line, if nothing else precedes it.
    fn indent_of(&self, at: usize) -> Option<String> {
        let line_start = self.chars[..at]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |i| i + 1);
        let before = &self.chars[line_start..at];
        before
            .iter()
            .all(|c| *c == ' ' || *c == '\t')
            .then(|| before.iter().collect())
    }

    /// Walk an expression evaluated at the current statement level, lifting
    /// each call with exactly one `@` of its own (post-order: inner calls
    /// first) into `hoisted`. Nested bodies are statement lists of their own.
    fn lift(&mut self, e: &Expr, hoisted: &mut Vec<Hoist>) {
        Lift { p: self, hoisted }.visit_expr(e);
    }

    /// Lift `e` if it is a call with exactly one unconsumed `@` of its own.
    fn lift_call(&mut self, e: &Expr, hoisted: &mut Vec<Hoist>) {
        let ExprKind::Call { function, args, .. } = &e.kind else {
            return;
        };
        let mut own = Own {
            p: self,
            found: Vec::new(),
        };
        own.visit_expr(function);
        for a in args {
            own.visit_expr(a);
        }
        let [i] = own.found[..] else {
            return;
        };
        let m = &self.markers[i];
        let (start, end) = (e.span.start.offset as usize, e.span.end.offset as usize);
        // The `@` (and any space after it) goes; then the call's text.
        self.put(m.at, m.ident, String::new(), Vec::new());
        self.consumed.insert(i);
        let (call, mut markers) = self.absorb(start, end);
        markers.push(i);
        let name = m.name.clone();
        hoisted.push(Hoist {
            start,
            end,
            text: format!("{name} = {call}"),
            markers,
        });
        self.put(start, end, name, Vec::new());
    }
}

/// The lifting walk over one statement-level expression (see
/// [`Planner::lift`]).
struct Lift<'p, 'a, 'h> {
    p: &'p mut Planner<'a>,
    hoisted: &'h mut Vec<Hoist>,
}

impl ExprVisitor for Lift<'_, '_, '_> {
    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.visit_expr(condition);
                self.p.stmts(then_body);
                match else_body {
                    Some(ElseBranch::Block(b)) => self.p.stmts(b),
                    Some(ElseBranch::ElseIf(x)) => self.visit_expr(x),
                    None => {}
                }
            }
            ExprKind::Lambda { body, .. } => self.p.stmts(body),
            // Arm bodies are conditional: not lifted.
            ExprKind::Match { subject, .. } => self.visit_expr(subject),
            ExprKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.p.stmts(body);
            }
            ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::CellGet(_) => {}
            _ => walk_expr(self, e),
        }
        self.p.lift_call(e, self.hoisted);
    }
}

/// The unconsumed markers that belong to one call: found in its callee and
/// arguments without crossing another call or a block.
struct Own<'p, 'a> {
    p: &'p Planner<'a>,
    found: Vec<usize>,
}

impl ExprVisitor for Own<'_, '_> {
    fn visit_expr(&mut self, e: &Expr) {
        if let Some(i) = self.p.marker(e) {
            self.found.push(i);
            return;
        }
        match &e.kind {
            ExprKind::Call { .. }
            | ExprKind::If { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::Match { .. }
            | ExprKind::For { .. }
            | ExprKind::Ident(_)
            | ExprKind::Literal(_) => {}
            _ => walk_expr(self, e),
        }
    }
}

/// A call lifted into `name = call` before its statement.
struct Hoist {
    start: usize,
    end: usize,
    text: String,
    markers: Vec<usize>,
}
