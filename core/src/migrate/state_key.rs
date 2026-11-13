//! Migration `state-key-deprecated` — the removed `state(key)`; a
//! never-written one is a `memo`.
//!
//! ```text
//! fn label_width(text)                  fn label_width(text)
//!     state(text) w = text.length * 8  =>   memo(text) w = text.length * 8
//!     w                                     w
//! ```
//!
//! `state(key) name = init` kept one slot per key value whatever call path
//! reached it: a global hidden in a function, which broke the rule that
//! stateful code is safe to call twice. It was deprecated on 2026-10-22 and
//! removed on 2026-11-13; the parser now rejects it with an error that names
//! `petal migrate-old-code`. Each use is written as one of:
//!
//! - `memo(key) name = init` when the slot is never written and the
//!   initializer is a pure function of the key: sharing a pure result between
//!   callers is unobservable (docs/features/memo.md);
//! - a keyed loop, `for t in xs key t.id do`, with a plain `state` in its
//!   body, for state per item of a list;
//! - a top-level `state var` holding a Map, written with `set m[k] = v`, when
//!   the sharing is the point;
//! - plain `state`, kept per call path, when each caller should own its slot.
//!
//! The migration reports each declaration, and rewrites to `memo` the ones
//! whose slot nothing writes *and* whose initializer is plainly a pure
//! function of the key: such a slot only ever held what its initializer
//! computed for that key, which is what a `memo` is. Which of the other three
//! a site wants is the author's call, so every other site is listed for a
//! hand edit.
//!
//! **Recognition.** The parser has no keyed `state`, so the planner keeps its
//! own reading: it finds each `state` + `(` with the lexer ([`find_sites`])
//! and respells the text with every offset unchanged ([`respell`]): a
//! candidate as `memo (key)`, which the parser reads as the `memo` it would
//! become, and any other site with its `(key)` blanked to spaces, which reads
//! as a plain `state`. [`blank_keys`] is the second reading alone; the other
//! migrations plan and gate on it, so a file with a `state(key)` left for a
//! hand edit still gets every other rewrite.
//!
//! "Nothing writes it" is the compiler's own answer, not this rule's: a
//! `memo` rejects every write to its name (rebind, `+=`, `set`, an indexed
//! or field write, from any closure), so the rewrite is kept only when it
//! compiles. A `state(k) var`, a `pub state(k)` (there is no `pub memo`) and
//! a file that does not compile here are reported without a fix.
//!
//! Purity is the part the compiler cannot see. A memo is shared by every
//! caller with an equal key, and it is a cache: dropped by a reload that
//! changes the program and by a run that does not reach it, and recomputed
//! when next asked. An initializer that reads anything but its key
//! (`state(k) v = k * scale`) would hand the first caller's answer to every
//! caller, and one that is effectful (`state(id) tint = random(0.0, 1.0)`)
//! would give a new value after each drop. So the fix is offered only when
//! [`init_is_pure_of_key`] can tell from the syntax that neither can happen:
//! no calls, no `get`, no name the key does not read. Until the full purity
//! check exists (docs/tasks/memo-purity-check.md), a call such as
//! `measure(text)` is treated as possibly impure, and the author decides.

use crate::ast::{Expr, ExprKind, ExprVisitor, RecordField, Stmt, StmtKind, walk_expr, walk_stmt};
use crate::lexer::{Lexer, Token};
use crate::lint::{Fix, LintOptions, compiles};
use crate::rewrite::Splice;

const STATE: &str = "state";
const MEMO: &str = "memo";

/// One `state(key)` declaration in the source, by its tokens. Offsets are
/// char offsets.
pub(super) struct Site {
    /// Where the `state` keyword starts.
    state: usize,
    /// The `(` and the `)` of the key group.
    open: usize,
    close: usize,
    /// The declared name, when one follows the key group.
    name: Option<String>,
    /// Written `state(key) var name`.
    is_var: bool,
    /// Written `pub state(…)` (or `export state(…)`).
    exported: bool,
}

/// Every `state` directly followed by `(`, in source order. `state` is a
/// keyword, so nothing else is written that way; text in a string or a
/// comment is not a token and is not seen. A source that does not lex has no
/// sites: its own error is the one to report.
pub(super) fn find_sites(source: &str) -> Vec<Site> {
    if !source.contains(STATE) {
        return Vec::new();
    }
    let mut lexer = Lexer::new(source).without_trivia();
    if lexer.tokenize().is_err() {
        return Vec::new();
    }
    let toks: Vec<(&Token, usize)> = lexer
        .tokens_with_spans()
        .map(|(t, s)| (t, s.start.offset as usize))
        .collect();
    let mut sites = Vec::new();
    let mut i = 0;
    while i + 1 < toks.len() {
        let is_site = matches!(toks[i].0, Token::State)
            && matches!(toks[i + 1].0, Token::LParen)
            && !(i > 0 && matches!(toks[i - 1].0, Token::Dot));
        if !is_site {
            i += 1;
            continue;
        }
        // The matching `)`.
        let mut depth = 0usize;
        let mut close = None;
        for (j, (t, at)) in toks.iter().enumerate().skip(i + 1) {
            match t {
                Token::LParen => depth += 1,
                Token::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some((j, *at));
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some((j, close)) = close else {
            i += 1;
            continue;
        };
        let is_var = matches!(toks.get(j + 1), Some((Token::Var, _)));
        let name = match toks.get(j + 1 + usize::from(is_var)) {
            Some((Token::Ident(n), _)) => Some(n.clone()),
            _ => None,
        };
        sites.push(Site {
            state: toks[i].1,
            open: toks[i + 1].1,
            close,
            name,
            is_var,
            exported: i > 0 && matches!(toks[i - 1].0, Token::Pub | Token::Export),
        });
        // A key may itself hold a `state(…)` (in a lambda): step inside.
        i += 2;
    }
    sites
}

/// `chars` with every offset unchanged and no `state(key)` left: the sites
/// in `memo` (offsets of their `state` keyword) spelled `memo (key)`, and
/// every other with its `(key)` turned to spaces (line breaks kept), so it
/// reads as a plain `state`.
fn respell(chars: &[char], sites: &[Site], memo: &[usize]) -> String {
    let mut out = chars.to_vec();
    for s in sites {
        if memo.contains(&s.state) {
            for (i, c) in "memo ".chars().enumerate() {
                out[s.state + i] = c;
            }
        } else {
            for c in &mut out[s.open..=s.close] {
                if *c != '\n' {
                    *c = ' ';
                }
            }
        }
    }
    out.into_iter().collect()
}

/// `source` as today's parser reads it, every offset unchanged: each
/// `state(key) name` as the plain `state name` (its key group turned to
/// spaces). Not the same program; what the migrations that do not care about
/// the key plan against, and what the compile gates compile on both sides.
/// A source with no `state(key)` comes back as it is.
pub(super) fn blank_keys(source: &str) -> String {
    let sites = find_sites(source);
    if sites.is_empty() {
        return source.to_string();
    }
    respell(&source.chars().collect::<Vec<_>>(), &sites, &[])
}

/// The message for a keyed declaration of `name`.
fn message(name: Option<&str>) -> String {
    let name = name.map(|n| format!(" {n}")).unwrap_or_default();
    format!(
        "`state(…){name}` was removed: keyed state is a hidden shared global (one slot per \
         key value, whichever caller reaches it). Use `memo(key)` for a cache whose initializer \
         reads only its key, a keyed loop (`for … key`) for state per item, a top-level \
         `state var` holding a Map for state shared on purpose, or plain `state` (kept per call \
         path) for per-instance state"
    )
}

/// Plan the report for every `state(key)` in `source`, with the `state` →
/// `memo` splice on each one that can take it.
pub(super) fn plan_state_key_fixes(source: &str, opts: &LintOptions) -> Vec<Fix> {
    let sites = find_sites(source);
    if sites.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = source.chars().collect();
    // The candidates: a plain keyed `state` (no `var`, no `pub`) whose
    // initializer is plainly pure of its key, as the `memo` it would become
    // reads.
    let plain: Vec<usize> = sites
        .iter()
        .filter(|s| !s.is_var && !s.exported)
        .map(|s| s.state)
        .collect();
    let mut fixable: Vec<usize> = match crate::rewrite::parse_ast_or_by_end(&respell(&chars, &sites, &plain)) {
        Ok((_, stmts, _)) => pure_memos(&stmts).into_iter().filter(|at| plain.contains(at)).collect(),
        Err(_) => Vec::new(),
    };
    if !fixable.is_empty() {
        let compiles_as = |memo: &[usize]| compiles(&super::indented_by_end(&respell(&chars, &sites, memo)), opts);
        if compiles_as(&[]).is_err() {
            fixable.clear();
        } else if compiles_as(&fixable).is_err() {
            // Some slot is written. Find which, one declaration at a time.
            fixable.retain(|&at| compiles_as(&[at]).is_ok());
        }
    }
    sites
        .iter()
        .map(|s| Fix {
            anchor: s.state,
            message: message(s.name.as_deref()),
            splices: if fixable.contains(&s.state) {
                vec![Splice {
                    start: s.state,
                    end: s.state + STATE.len(),
                    text: MEMO.to_string(),
                }]
            } else {
                Vec::new()
            },
        })
        .collect()
}

/// `chars` with the `state` keyword of each site in `sites` spelled `memo`.
#[cfg(test)]
fn rewrite(chars: &[char], sites: &[usize]) -> String {
    let splices: Vec<Splice> = sites
        .iter()
        .map(|&at| Splice {
            start: at,
            end: at + STATE.len(),
            text: MEMO.to_string(),
        })
        .collect();
    crate::rewrite::apply_splices(chars, &splices)
}

/// The offset of every `memo` declaration in `stmts`, nested ones included,
/// whose initializer is a pure function of its key.
fn pure_memos(stmts: &[Stmt]) -> Vec<usize> {
    struct Finder(Vec<usize>);
    impl ExprVisitor for Finder {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::Memo { key, init, .. } = &s.kind
                && init_is_pure_of_key(init, key)
            {
                self.0.push(s.span.start.offset as usize);
            }
            walk_stmt(self, s);
        }
    }
    let mut finder = Finder(Vec::new());
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.0
}

/// Is `init` a pure function of `key`, judged from its syntax alone?
///
/// A `memo` hands one caller's value to every caller with an equal key, and
/// recomputes it after a reload or a run that does not reach it, so the
/// rewrite from `state(key)` is sound only when the initializer cannot give
/// a different answer for an equal key. This accepts the initializers that
/// provably cannot: literals, the values the key holds, operators, and list,
/// record, field, index and interpolation forms over those. Anything else
/// (any call, a `get`, a value the key does not hold, a lambda, a branch or a
/// loop) is rejected, though many such initializers are pure: the full
/// purity check, with the static classifier, is docs/tasks/memo-purity-check.md.
///
/// "The values the key holds" are access paths, not names: the key
/// `[g.code_hash]` holds `g.code_hash` and whatever is read through it, but
/// not `g.uniforms`, and not `g`. And a path counts only where the key keeps
/// its whole value: as the key itself, or as an element of a list or the
/// value of a field the key is built of. Under an operator or an index
/// (`state(x % 2)`) equal keys do not mean equal `x`.
fn init_is_pure_of_key(init: &Expr, key: &Expr) -> bool {
    struct HasLambda(bool);
    impl ExprVisitor for HasLambda {
        fn visit_expr(&mut self, e: &Expr) {
            if matches!(e.kind, ExprKind::Lambda { .. }) {
                self.0 = true;
            }
            walk_expr(self, e);
        }
    }
    let mut lambda = HasLambda(false);
    lambda.visit_expr(key);
    let mut held = Vec::new();
    key_paths(key, &mut held);
    !lambda.0 && pure_over(init, &held)
}

/// `e` as an access path: a name, then the fields read through it
/// (`g.pass.code_hash` is `[g, pass, code_hash]`). `None` for anything else.
fn access_path(e: &Expr) -> Option<Vec<&str>> {
    match &e.kind {
        ExprKind::Ident(n) => Some(vec![n.as_str()]),
        ExprKind::FieldAccess { object, field } => {
            let mut path = access_path(object)?;
            path.push(field.as_str());
            Some(path)
        }
        _ => None,
    }
}

/// The access paths whose whole value `key` holds.
fn key_paths<'a>(key: &'a Expr, out: &mut Vec<Vec<&'a str>>) {
    if let Some(path) = access_path(key) {
        out.push(path);
        return;
    }
    match &key.kind {
        ExprKind::List(items) => items.iter().for_each(|item| key_paths(item, out)),
        ExprKind::Record(fields) => {
            for f in fields {
                if let RecordField::Named(_, v) = f {
                    key_paths(v, out);
                }
            }
        }
        _ => {}
    }
}

fn pure_over(e: &Expr, held: &[Vec<&str>]) -> bool {
    let all = |es: &[Expr]| es.iter().all(|x| pure_over(x, held));
    // A path the key holds, or one read through such a path.
    if let Some(path) = access_path(e) {
        return held.iter().any(|k| path.starts_with(k));
    }
    match &e.kind {
        ExprKind::Literal(_) => true,
        ExprKind::BinaryOp { left, right, .. } => pure_over(left, held) && pure_over(right, held),
        ExprKind::UnaryOp { operand, .. } => pure_over(operand, held),
        ExprKind::List(items) => all(items),
        ExprKind::Record(fields) => fields.iter().all(|f| match f {
            RecordField::Named(_, v) => pure_over(v, held),
            // A computed key is a spread of an internal call; a spread
            // of a value the key holds is pure.
            RecordField::Spread(v) => pure_over(v, held),
        }),
        ExprKind::FieldAccess { object, .. } => pure_over(object, held),
        ExprKind::IndexAccess { object, index } => pure_over(object, held) && pure_over(index, held),
        ExprKind::OptionalAccess(inner) => pure_over(inner, held),
        ExprKind::StringInterp { exprs, .. } => all(exprs),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(src: &str) -> Vec<Fix> {
        plan_state_key_fixes(src, &LintOptions::default())
    }

    fn migrated(src: &str) -> String {
        let chars: Vec<char> = src.chars().collect();
        let sites: Vec<usize> = plan(src)
            .iter()
            .filter(|f| !f.splices.is_empty())
            .map(|f| f.anchor)
            .collect();
        rewrite(&chars, &sites)
    }

    #[test]
    fn the_sites_are_found_by_their_tokens() {
        let src = "# state(a) in a comment\nlet s = \"state(b) x = 1\"\nstate n = 0\n\
                   fn f(k)\n  state(k) var cell = 0\n  state (k) w = k\n  w\npub state([1, (2)]) top = 3\n";
        let sites = find_sites(src);
        let names: Vec<_> = sites.iter().map(|s| s.name.clone().unwrap()).collect();
        assert_eq!(names, ["cell", "w", "top"]);
        assert!(sites[0].is_var && !sites[0].exported);
        assert!(!sites[1].is_var && !sites[1].exported);
        assert!(sites[2].exported);
        let chars: Vec<char> = src.chars().collect();
        assert_eq!(chars[sites[2].close], ')');
        assert_eq!(chars[sites[2].close + 2..sites[2].close + 5], ['t', 'o', 'p']);
    }

    #[test]
    fn blanking_keeps_every_offset_and_parses() {
        let src = "fn f(k)\n  state(k) var cell = 0\n  set cell = k\n  cell\nprint(f(1))\n";
        let blanked = blank_keys(src);
        assert_eq!(blanked.chars().count(), src.chars().count());
        assert_eq!(blanked, "fn f(k)\n  state    var cell = 0\n  set cell = k\n  cell\nprint(f(1))\n");
        assert!(crate::rewrite::parse_ast(src).is_err());
        assert!(crate::rewrite::parse_ast(&blanked).is_ok());
        // Nothing to blank: the text itself.
        assert_eq!(blank_keys("state n = 0\n"), "state n = 0\n");
    }

    #[test]
    fn a_never_written_slot_pure_of_its_key_becomes_a_memo() {
        let src = "fn width(text)\n  state(text) w = text.length * 8\n  w\nprint(width(\"hi\"))\n";
        assert_eq!(
            migrated(src),
            "fn width(text)\n  memo(text) w = text.length * 8\n  w\nprint(width(\"hi\"))\n"
        );
    }

    #[test]
    fn a_written_slot_a_var_and_a_pub_are_left_for_a_hand_edit() {
        for src in [
            "fn hp(id, d)\n  state(id) h = 100\n  h -= d\n  h\nprint(hp(1, 2))\n",
            "fn f(k)\n  state(k) var c = 0\n  c\nprint(f(1))\n",
            "pub state(1) top = 2\n",
        ] {
            let fixes = plan(src);
            assert_eq!(fixes.len(), 1, "{src}");
            assert!(fixes[0].splices.is_empty(), "{src}");
            assert!(fixes[0].message.contains("was removed"), "{}", fixes[0].message);
        }
    }

    #[test]
    fn an_initializer_that_reads_past_its_key_is_left() {
        for src in [
            "let scale = 3\nfn f(k)\n  state(k) v = k * scale\n  v\nprint(f(1))\n",
            "fn f(id)\n  state(id) tint = random(0.0, 1.0)\n  tint\nprint(f(1))\n",
            "fn f(g)\n  state([g.a]) first = g.b\n  first\nprint(f({a: 1, b: 2}))\n",
        ] {
            let fixes = plan(src);
            assert_eq!(fixes.len(), 1, "{src}");
            assert!(fixes[0].splices.is_empty(), "{src}");
        }
    }

    #[test]
    fn one_fixable_site_beside_one_that_is_not() {
        let src = "fn f(k)\n  state(k) a = k + 1\n  state(k) b = 0\n  b += a\n  b\nprint(f(1))\n";
        assert_eq!(
            migrated(src),
            "fn f(k)\n  memo(k) a = k + 1\n  state(k) b = 0\n  b += a\n  b\nprint(f(1))\n"
        );
    }
}
