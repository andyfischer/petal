//! Rule — rewrite an `if`/`elsif` chain that tests one subject against
//! literals into a `match`.
//!
//! ```text
//! if ch == "@" then "spawn"          match ch
//! elsif ch == "o" then "coin"    =>    when "@" -> "spawn"
//! elsif ch == "w" then "walker"        when "o" -> "coin"
//! else nil                             when "w" -> "walker"
//! end                                  when _ -> nil
//!                                    end
//! ```
//!
//! The chain re-reads the subject and re-spells `==` once per arm; the `match`
//! says each of those things once. That is the whole of the win, so the rule
//! only fires where it is unambiguous: three or more arms, one pure subject,
//! and one expression per arm.
//!
//! Detection is over the AST, the rewrite is span splices (the linter-plan's
//! Pass 2 shape), and the splices only ever cover *glue* — `if`, `==`,
//! `then`, `elsif <subject> ==`, `else`. Every arm's pattern and body is
//! preserved as the author wrote it, comments and layout included, and the
//! chain's trailing `end` is left in place because `match` ends the same way.
//! Indentation is not this rule's problem: [`super::reindent`] runs after it.
//!
//! ## What it refuses, and why
//!
//! - **Numeric literals.** `==` compares an int against a float numerically
//!   (`1 == 1.0` is true), while [`crate::backend::pattern::match_pattern`]
//!   requires the tags to agree, so `when 1` does *not* match `1.0`. Rewriting
//!   `n == 1` to `when 1` would quietly change which arm a float subject
//!   takes. `String`, `Bool` and `Nil` have no such cross-type rule and are
//!   exactly equivalent; those are the literals the rule accepts.
//! - **A subject that could compute.** `match` reads the subject once where
//!   the chain read it per arm. A name or a field path off a name is safe to
//!   move; a call or an index is left alone.
//! - **A comment in the glue.** A comment between `then` and the next `elsif`
//!   has no home in the `match`, so the whole chain is skipped rather than
//!   dropping it.
//! - **Bodies that are not a single expression.** A `->` arm takes an
//!   expression. Multi-statement arms would need the `do … end` form, which
//!   this rule does not emit — and where a chain is really control flow rather
//!   than a lookup, the `match` is not obviously the better spelling anyway.
//!
//! A chain with no `else` gains `when _ -> nil`: an `if` that falls off the
//! end yields nil, but a `match` with no arm left is a runtime error, so the
//! fallback has to be written out to keep the two the same.

use crate::ast::{
    BinOp, ElseBranch, Expr, ExprKind, ExprVisitor, Literal, Stmt, StmtKind, walk_expr,
};
use crate::source_map::SourceSpan;

/// The shortest chain worth converting. At two arms `if … else … end` is the
/// plainer spelling and the repetition the rule exists to remove is one line.
const MIN_ARMS: usize = 3;

/// One replacement, as a char range and the text to put there. An empty range
/// is an insertion.
#[derive(Debug, Clone)]
pub(super) struct Splice {
    start: usize,
    end: usize,
    text: String,
}

/// Plan every chain rewrite in `stmts`. Returns the splices in source order.
pub(super) fn plan_match_edits(stmts: &[Stmt], chars: &[char]) -> (Vec<Splice>, usize) {
    let mut finder = Finder {
        chars,
        splices: Vec::new(),
        chains: 0,
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.splices.sort_by_key(|s| s.start);
    (finder.splices, finder.chains)
}

/// Apply the splices, highest offset first so earlier positions stay valid.
pub(super) fn apply_match_edits(chars: &[char], splices: &[Splice]) -> String {
    let mut out: Vec<char> = chars.to_vec();
    for s in splices.iter().rev() {
        out.splice(s.start..s.end, s.text.chars());
    }
    out.into_iter().collect()
}

struct Finder<'a> {
    chars: &'a [char],
    splices: Vec<Splice>,
    chains: usize,
}

impl ExprVisitor for Finder<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        let ExprKind::If { .. } = &e.kind else {
            walk_expr(self, e);
            return;
        };
        // Take the whole `elsif` spine at once. Descending normally would
        // reach each inner `If` again and mistake the chain's own tail for a
        // shorter chain of its own.
        let chain = collect_chain(e);
        if let Some(edits) = plan_chain(&chain, self.chars) {
            self.splices.extend(edits);
            self.chains += 1;
        }
        for arm in &chain.arms {
            self.visit_expr(arm.cond);
            for s in arm.body {
                self.visit_stmt(s);
            }
        }
        for s in chain.else_body.unwrap_or(&[]) {
            self.visit_stmt(s);
        }
    }
}

struct Arm<'a> {
    cond: &'a Expr,
    body: &'a [Stmt],
}

struct Chain<'a> {
    arms: Vec<Arm<'a>>,
    else_body: Option<&'a [Stmt]>,
    /// The whole `if … end`, trailing `end` included.
    span: SourceSpan,
}

/// Flatten `if / elsif / … / else` into its arms. `e` must be an `If`.
fn collect_chain(e: &Expr) -> Chain<'_> {
    let mut arms = Vec::new();
    let mut cur = e;
    loop {
        let ExprKind::If {
            condition,
            then_body,
            else_body,
        } = &cur.kind
        else {
            unreachable!("collect_chain walks only If nodes")
        };
        arms.push(Arm {
            cond: condition,
            body: then_body,
        });
        match else_body {
            None => {
                return Chain {
                    arms,
                    else_body: None,
                    span: e.span,
                };
            }
            Some(ElseBranch::Block(stmts)) => {
                return Chain {
                    arms,
                    else_body: Some(stmts),
                    span: e.span,
                };
            }
            Some(ElseBranch::ElseIf(inner)) => cur = inner,
        }
    }
}

fn plan_chain(chain: &Chain, chars: &[char]) -> Option<Vec<Splice>> {
    if chain.arms.len() < MIN_ARMS {
        return None;
    }

    // Every arm has to be `<subject> == <literal>` over the same subject, with
    // a single expression for a body.
    let mut conds: Vec<(SourceSpan, SourceSpan)> = Vec::with_capacity(chain.arms.len());
    let mut bodies: Vec<SourceSpan> = Vec::with_capacity(chain.arms.len());
    for arm in &chain.arms {
        let ExprKind::BinaryOp {
            op: BinOp::Eq,
            left,
            right,
        } = &arm.cond.kind
        else {
            return None;
        };
        if !is_pure_subject(left) {
            return None;
        }
        let ExprKind::Literal(lit) = &right.kind else {
            return None;
        };
        if !is_exact_pattern_literal(lit) {
            return None;
        }
        conds.push((left.span, right.span));
        bodies.push(single_expr_span(arm.body)?);
    }
    let else_span = match chain.else_body {
        Some(stmts) => Some(single_expr_span(stmts)?),
        None => None,
    };

    // The subject is written once in the `match` head, so every arm has to
    // spell it identically. Comparing source text rather than AST shape also
    // keeps the rule honest about what it is about to delete.
    let subject = span_text(chars, conds[0].0)?;
    let subject = subject.trim().to_string();
    for (lhs, _) in &conds {
        if span_text(chars, *lhs)?.trim() != subject {
            return None;
        }
    }

    let if_start = chain.span.start.offset as usize;
    let if_end = chain.span.end.offset as usize;
    if if_end > chars.len() {
        return None;
    }
    let mut splices = Vec::new();

    // `if ` -> `match `, leaving the subject text that follows it in place.
    let head_end = conds[0].0.start.offset as usize;
    if glue(chars, if_start, head_end)?.trim() != "if" {
        return None;
    }
    splices.push(Splice {
        start: if_start,
        end: head_end,
        text: "match ".to_string(),
    });

    for (i, (lhs, rhs)) in conds.iter().enumerate() {
        // The arm head. On the first arm that is just the ` == ` after the
        // subject; on the rest it is `\n…elsif <subject> == `, and the
        // repeated subject goes away with it.
        let from = if i == 0 {
            lhs.end.offset as usize
        } else {
            bodies[i - 1].end.offset as usize
        };
        let to = rhs.start.offset as usize;
        let g = glue(chars, from, to)?;
        let rest = if i == 0 {
            g.trim()
        } else {
            let after_kw = g.trim().strip_prefix("elsif")?;
            let subj_and_op = after_kw.trim_start();
            if subj_and_op.len() == after_kw.len() {
                return None; // `elsifx`, not `elsif x`
            }
            // Whatever follows the subject must be the operator and nothing
            // else, which is also what rules out a subject that is only a
            // prefix of what was written (`ch` against `chr`).
            subj_and_op.strip_prefix(subject.as_str())?.trim_start()
        };
        if rest.trim() != "==" {
            return None;
        }
        splices.push(Splice {
            start: from,
            end: to,
            text: "\nwhen ".to_string(),
        });

        // ` then ` -> ` -> `.
        let then_from = rhs.end.offset as usize;
        let then_to = bodies[i].start.offset as usize;
        if glue(chars, then_from, then_to)?.trim() != "then" {
            return None;
        }
        splices.push(Splice {
            start: then_from,
            end: then_to,
            text: " -> ".to_string(),
        });
    }

    let last_body_end = bodies[bodies.len() - 1].end.offset as usize;
    match else_span {
        Some(es) => {
            let to = es.start.offset as usize;
            if glue(chars, last_body_end, to)?.trim() != "else" {
                return None;
            }
            splices.push(Splice {
                start: last_body_end,
                end: to,
                text: "\nwhen _ -> ".to_string(),
            });
        }
        None => {
            // Falling off the end of an `if` yields nil; falling off the end
            // of a `match` is a runtime error. Spell the nil out.
            splices.push(Splice {
                start: last_body_end,
                end: last_body_end,
                text: "\nwhen _ -> nil".to_string(),
            });
        }
    }

    // `match` closes with `end` exactly as the chain did, so the tail needs no
    // edit — only a check that it is the tail we think it is. A comment may
    // sit in here; nothing is being rewritten, so it survives untouched.
    let tail_from = else_span.map_or(last_body_end, |es| es.end.offset as usize);
    let tail: String = chars.get(tail_from..if_end)?.iter().collect();
    if !tail.trim_end().ends_with("end") {
        return None;
    }

    Some(splices)
}

/// The source between two nodes, as long as it holds no comment.
///
/// Every region this rule rewrites is glue, so a comment inside one has
/// nowhere to go in the `match` and the caller skips the chain instead. Glue
/// never spans a string literal — it is keywords, an operator and at most the
/// repeated subject — so scanning for `//` cannot trip over one in quotes.
fn glue(chars: &[char], start: usize, end: usize) -> Option<String> {
    if start > end || end > chars.len() {
        return None;
    }
    let text: String = chars[start..end].iter().collect();
    if text.contains("//") {
        return None;
    }
    Some(text)
}

fn span_text(chars: &[char], span: SourceSpan) -> Option<String> {
    let start = span.start.offset as usize;
    let end = span.end.offset as usize;
    Some(chars.get(start..end)?.iter().collect())
}

/// A name, or a field path off a name. Nothing that could compute: `match`
/// reads the subject once where the chain read it once per arm.
fn is_pure_subject(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(_) => true,
        ExprKind::FieldAccess { object, .. } => is_pure_subject(object),
        _ => false,
    }
}

/// The literals whose `==` and whose pattern agree exactly.
///
/// `Int`/`Float` are deliberately absent — see the module docs.
fn is_exact_pattern_literal(lit: &Literal) -> bool {
    matches!(lit, Literal::String(_) | Literal::Bool(_) | Literal::Nil)
}

/// The span of `body` when it is exactly one expression statement.
fn single_expr_span(body: &[Stmt]) -> Option<SourceSpan> {
    match body {
        [
            Stmt {
                kind: StmtKind::Expr(e),
                ..
            },
        ] => Some(e.span),
        _ => None,
    }
}
