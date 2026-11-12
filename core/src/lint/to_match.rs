//! Rule — rewrite an `if`/`else if` chain that tests one subject against
//! literals into a `match`.
//!
//! ```text
//! if ch == "@" then "spawn"          match ch
//! else if ch == "o" then "coin"    =>  when "@" -> "spawn"
//! else if ch == "w" then "walker"      when "o" -> "coin"
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
//! `then`, `else if <subject> ==` (or its old spelling `elsif`), `else`. Every
//! arm's pattern and body is preserved as the author wrote it, comments and
//! layout included, and the chain's trailing `end` is left in place because
//! `match` ends the same way. A chain closed by its indentation alone becomes
//! a `match` closed the same way.
//!
//! Indentation is mostly not this rule's problem: the formatter runs after
//! it. But indentation delimits blocks, so what the rule writes must already
//! be readable. Arms whose bodies fit on a line are written a step in from
//! the chain. A body over several lines is not moved, and is tried in two
//! placements ([`Placement`]); the first whose result parses is the fix, and
//! a chain with none is left alone.
//!
//! ## What it refuses, and why
//!
//! - **Anything but a literal on the right.** A literal pattern matches
//!   exactly what the literal is `==` to — `when 1` matches `1.0`, as
//!   `n == 1` is true of it ([`crate::backend::pattern::match_pattern`] and
//!   `==` share one numeric comparison) — so every literal converts. A name
//!   does not: `when limit` binds `limit` rather than comparing against it.
//! - **A subject that could compute.** `match` reads the subject once where
//!   the chain read it per arm. A name or a field path off a name is safe to
//!   move; a call or an index is left alone.
//! - **A comment in the glue.** A comment between `then` and the next `else if`
//!   has no home in the `match`, so the whole chain is skipped rather than
//!   dropping it.
//! - **A chain inside a string.** A `"{…}"` hole cannot span lines, and the
//!   `match` this rule writes does.
//! - **Bodies that are not a single expression.** A `->` arm takes a
//!   statement list as readily as one expression, so this is a choice, not a
//!   limit: the rule is about lookups, where the `match` is plainly the
//!   better spelling. A chain whose branches are blocks of statements is
//!   control flow, and which of the two reads better there is the author's
//!   call.
//!
//! A chain with no `else` gains `when _ -> nil`: an `if` that falls off the
//! end yields nil, but a `match` with no arm left is a runtime error, so the
//! fallback has to be written out to keep the two the same.

use crate::ast::{
    BinOp, ElseBranch, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_expr,
};
use crate::source_map::SourceSpan;

use super::Fix;

/// The shortest chain worth converting. At two arms `if … else … end` is the
/// plainer spelling and the repetition the rule exists to remove is one line.
const MIN_ARMS: usize = 3;

// The splice type and its application are shared source-rewriting
// infrastructure (`petal apply-change` plans edits the same way).
pub(super) use crate::rewrite::{Splice, apply_splices as apply_match_edits};

/// Plan every chain rewrite in `stmts`, one [`Fix`] per chain, in source
/// order.
pub(super) fn plan_match_fixes(stmts: &[Stmt], chars: &[char]) -> Vec<Fix> {
    let mut finder = Finder {
        chars,
        fixes: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.fixes.sort_by_key(|f| f.anchor);
    finder.fixes
}

struct Finder<'a> {
    chars: &'a [char],
    fixes: Vec<Fix>,
}

impl ExprVisitor for Finder<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        // A `match` is written over several lines and a `"{…}"` hole holds
        // one, so a chain inside a string stays the chain it is.
        if let ExprKind::StringInterp { .. } = &e.kind {
            return;
        }
        let ExprKind::If { .. } = &e.kind else {
            walk_expr(self, e);
            return;
        };
        // Take the whole `else if` spine at once. Descending normally would
        // reach each inner `If` again and mistake the chain's own tail for a
        // shorter chain of its own.
        let chain = collect_chain(e);
        if let Some(edits) = plan_chain(&chain, self.chars) {
            let subject = match &chain.arms[0].cond.kind {
                ExprKind::BinaryOp { left, .. } => span_text(self.chars, left.span),
                _ => None,
            }
            .unwrap_or_default();
            self.fixes.push(Fix {
                anchor: e.span.start.offset as usize,
                message: format!(
                    "this if/else if chain compares `{subject}` against {} literals; \
                     write it as a `match`",
                    chain.arms.len()
                ),
                splices: edits,
            });
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

/// Where the arms of the `match` go when a body runs over several lines.
/// The lines of a body stay where they are, so the arms have to be put
/// where those lines still read as the body.
#[derive(Clone, Copy, PartialEq)]
enum Placement {
    /// Arms a step in from the chain, and a block body's first line a step
    /// in from its arm. The rest of the body is level with the arms until
    /// the formatter indents it by its `end`s, so this reads only by them
    /// ([`crate::rewrite::parse_ast_or_by_end`]): for a chain whose blocks
    /// all have their `end`.
    ByEnd,
    /// Arms one column in from the chain, and a body that started on a line
    /// of its own left there whole: every line of it is right of its arm,
    /// as it was right of its `if`. Reads by indentation, whatever closes
    /// the blocks; the formatter then moves each level to its column.
    ByLayout,
}

/// The ` -> ` that introduces an arm's body, replacing the `then` (or
/// `else`) glue `gap` that led to it.
///
/// A body on one line follows the arrow. Under [`Placement::ByEnd`], a body
/// that is itself a block over several lines (`if … then` with its branches
/// below, a loop) starts on a line of its own under the `when`, where its
/// lines indent as a body's do; left on the arrow's line, its branches would
/// sit level with the arms. Under [`Placement::ByLayout`], a body over
/// several lines that started on a line of its own keeps that line.
fn arrow(chars: &[char], body: SourceSpan, lead: &str, gap: &str, placement: Placement) -> String {
    let text = span_text(chars, body).unwrap_or_default();
    let multi_line = text.contains('\n');
    match placement {
        Placement::ByEnd => {
            let opens_block = ["if ", "for ", "while ", "match "]
                .iter()
                .any(|kw| text.trim_start().starts_with(kw));
            if opens_block && multi_line {
                format!(" ->\n{lead}    ")
            } else {
                " -> ".to_string()
            }
        }
        Placement::ByLayout => match gap.find('\n') {
            Some(at) if multi_line => format!(" ->{}", &gap[at..]),
            _ => " -> ".to_string(),
        },
    }
}

/// What follows the keyword that continues a chain at the start of `glue`:
/// `else if` (the two words on one line, any blanks between them) or its old
/// spelling `elsif`.
fn strip_else_if(glue: &str) -> Option<&str> {
    if let Some(rest) = glue.strip_prefix("elsif") {
        return Some(rest);
    }
    let rest = glue.strip_prefix("else")?;
    let after = rest.trim_start_matches([' ', '\t']);
    if after.len() == rest.len() {
        return None;
    }
    after.strip_prefix("if")
}

/// Flatten `if / else if / … / else` into its arms. `e` must be an `If`.
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
    let plan = plan_chain_placed(chain, chars, Placement::ByEnd)?;
    let reads = |splices: &[Splice], by_end: bool| {
        let text = apply_match_edits(chars, splices);
        if by_end {
            crate::rewrite::parse_ast_or_by_end(&text).is_ok()
        } else {
            crate::rewrite::parse_ast(&text).is_ok()
        }
    };
    if !plan.multi_line {
        // Every arm is one line, a step in from the chain: readable as
        // written. With no `end` to mark where the `match` stops, make sure.
        return (plan.has_end || reads(&plan.splices, true)).then_some(plan.splices);
    }
    // A body over several lines stays where it is, and which placement of
    // the arms still reads it as the body depends on how its blocks close.
    if plan.has_end && reads(&plan.splices, true) {
        return Some(plan.splices);
    }
    let plan = plan_chain_placed(chain, chars, Placement::ByLayout)?;
    reads(&plan.splices, false).then_some(plan.splices)
}

/// A chain's rewrite under one [`Placement`].
struct ChainPlan {
    /// In source order.
    splices: Vec<Splice>,
    /// An `end` closes the chain (and so the `match`).
    has_end: bool,
    /// Some arm's body runs over several lines.
    multi_line: bool,
}

fn plan_chain_placed(chain: &Chain, chars: &[char], placement: Placement) -> Option<ChainPlan> {

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
        let ExprKind::Literal(_) = &right.kind else {
            return None;
        };
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
    // Arms go one step in from the line the chain starts on, so what follows
    // them on continuation lines keeps its place when fmt re-indents.
    let line_start = chars[..if_start]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |p| p + 1);
    let lead: String = chars[line_start..if_start]
        .iter()
        .take_while(|c| **c == ' ' || **c == '\t')
        .collect();
    let step = if placement == Placement::ByLayout { " " } else { "  " };
    let arm = format!("\n{lead}{step}when ");
    let multi_line = bodies
        .iter()
        .chain(&else_span)
        .any(|b| span_text(chars, *b).is_some_and(|t| t.contains('\n')));

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
        // subject; on the rest it is `\n…else if <subject> == `, and the
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
            let after_kw = strip_else_if(g.trim())?;
            let subj_and_op = after_kw.trim_start();
            if subj_and_op.len() == after_kw.len() {
                return None; // `else ifx`, not `else if x`
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
            text: arm.clone(),
        });

        // ` then ` -> ` -> `.
        let then_from = rhs.end.offset as usize;
        let then_to = bodies[i].start.offset as usize;
        let gap = glue(chars, then_from, then_to)?;
        if gap.trim() != "then" {
            return None;
        }
        splices.push(Splice {
            start: then_from,
            end: then_to,
            text: arrow(chars, bodies[i], &lead, &gap, placement),
        });
    }

    // `match` closes exactly as the chain did, with an `end` or by its
    // indentation alone, so the tail is only checked to be the tail we think
    // it is — and, when its `end` sits on the last branch's line, moved to a
    // line of its own under the `match`. A comment may sit in here; it is
    // not rewritten, so it survives untouched.
    let last_body_end = bodies[bodies.len() - 1].end.offset as usize;
    let tail_from = else_span.map_or(last_body_end, |es| es.end.offset as usize);
    let tail: String = chars.get(tail_from..if_end)?.iter().collect();
    let tail = tail.trim_end();
    // With no `end`, the chain stops at its last body.
    let has_end = tail.ends_with("end");
    if !has_end && !tail.is_empty() {
        return None;
    }
    let (end_at, end_line) = if tail.trim_start() == "end" && !tail.contains('\n') {
        (tail_from + tail.chars().count() - 3, format!("\n{lead}"))
    } else {
        (tail_from, String::new())
    };
    match else_span {
        Some(es) => {
            let to = es.start.offset as usize;
            let gap = glue(chars, last_body_end, to)?;
            if gap.trim() != "else" {
                return None;
            }
            // The glue runs from the body before the `else`; the arrow wants
            // what follows the keyword.
            let after_else = gap.trim_start().strip_prefix("else").unwrap_or("");
            splices.push(Splice {
                start: last_body_end,
                end: to,
                text: format!("{arm}_{}", arrow(chars, es, &lead, after_else, placement)),
            });
            if !end_line.is_empty() {
                splices.push(Splice {
                    start: tail_from,
                    end: end_at,
                    text: end_line,
                });
            }
        }
        // Falling off the end of an `if` yields nil; falling off the end of
        // a `match` is a runtime error. Spell the nil out.
        None => splices.push(Splice {
            start: last_body_end,
            end: end_at,
            text: format!("{arm}_ -> nil{end_line}"),
        }),
    }

    splices.sort_by_key(|s| s.start);
    Some(ChainPlan {
        splices,
        has_end,
        multi_line,
    })
}

/// The source between two nodes, as long as it holds no comment.
///
/// Every region this rule rewrites is glue, so a comment inside one has
/// nowhere to go in the `match` and the caller skips the chain instead. Glue
/// never spans a string literal — it is keywords, an operator and at most the
/// repeated subject — so scanning for `#` or `//` cannot trip over one in quotes.
fn glue(chars: &[char], start: usize, end: usize) -> Option<String> {
    if start > end || end > chars.len() {
        return None;
    }
    let text: String = chars[start..end].iter().collect();
    if text.contains("//") || text.contains('#') {
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
