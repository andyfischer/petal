//! Removing the `end`s that indentation makes redundant.
//!
//! A block is closed by its indentation; its `end` is optional
//! (docs/syntax/indentation.md). This is the transform that drops the
//! optional ones from a file that still has them, for the `end-lines`
//! migration of `petal migrate-old-code` (docs/tasks/pythonic-syntax.md
//! §7.2). `petal fmt` does not run it: both spellings are laid out as
//! written.
//!
//! What is removed:
//!
//! - an `end` alone on its line: the line goes. A comment after it stays, on
//!   a line of its own at the block's column;
//! - an `end` that starts a line and is followed by a closing bracket
//!   (`end)`, `end])`): the `end` goes and the bracket keeps the column.
//!
//! What is kept:
//!
//! - an `end` on the line of the block it closes (`if c then a else b end`);
//! - an `end` followed by anything but a closing bracket (`end |> f`,
//!   `end.len()`, `end, fn()` between two block lambdas of one call);
//! - an `end` whose block has no statement, and one whose removal would let
//!   the next line continue the block's last statement (a line that starts
//!   with an operator);
//! - any other `end` the parse turns out to need: each removal that would
//!   change the program is undone.
//!
//! The gate is the formatter's: the result parses to the same AST as the
//! source. The source must already be indented the way its `end`s say
//! (`petal fmt` first), which is what makes the `end`s redundant.

use std::rc::Rc;

use crate::cst::{GreenChild, GreenNode, GreenToken, SyntaxKind};
use crate::lexer::{Lexer, Token};
use crate::trivia::TriviaKind;

/// The result of [`remove_block_ends`].
#[derive(Debug, Clone, PartialEq)]
pub struct EndRemoval {
    /// The source without its redundant `end`s.
    pub text: String,
    /// How many `end`s were removed.
    pub removed: usize,
    /// The 1-based lines of the `end`s that start a line and were kept
    /// because the program needs them.
    pub kept: Vec<u32>,
}

/// Columns a block is indented by, for the comment that outlives its `end`.
const INDENT: usize = 2;

/// Remove every `end` of `source` that indentation makes redundant (see the
/// module docs). Errors when `source` does not parse by the language's own
/// rules; a file whose indentation disagrees with its `end`s is `petal fmt`'s
/// to repair first.
pub fn remove_block_ends(source: &str) -> Result<EndRemoval, String> {
    let (tree, stmts) = crate::rewrite::parse_ast(source)?;
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let mut closers = Vec::new();
    let mut next = 0usize;
    collect_ends(&tree, &mut next, &mut closers);
    if next != lexer.tokens.len() {
        return Err("internal error: the syntax tree and the token stream disagree".to_string());
    }
    let chars: Vec<char> = source.chars().collect();
    let mut edits: Vec<Edit> = Vec::new();
    let mut kept = Vec::new();
    for closer in &closers {
        if !lexer.token_first_on_line[closer.token] {
            continue; // an inline `end`: still the way to close a one-liner
        }
        match plan(&lexer, &chars, closer) {
            Some(edit) => edits.push(edit),
            None => kept.push(lexer.token_spans[closer.token].start.line),
        }
    }

    let all = apply(&chars, &edits);
    if super::same_program(&stmts, &all).is_ok() {
        return Ok(EndRemoval {
            text: all,
            removed: edits.len(),
            kept,
        });
    }
    // Some `end` is needed after all. Find which: add the removals one at a
    // time, keeping each that leaves the program what it was.
    let mut accepted: Vec<Edit> = Vec::new();
    for edit in edits {
        accepted.push(edit);
        if super::same_program(&stmts, &apply(&chars, &accepted)).is_err() {
            let edit = accepted.pop().expect("just pushed");
            kept.push(edit.line);
        }
    }
    kept.sort_unstable();
    Ok(EndRemoval {
        text: apply(&chars, &accepted),
        removed: accepted.len(),
        kept,
    })
}

/// An `end` that closes a construct.
struct Closer {
    /// Index of the `end` among the lexer's tokens.
    token: usize,
    /// The block it closes has no statement.
    empty: bool,
}

/// One removal: the chars `start..end` become `with`.
struct Edit {
    start: usize,
    end: usize,
    with: String,
    /// The 1-based line of the `end`.
    line: u32,
}

fn apply(chars: &[char], edits: &[Edit]) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut at = 0usize;
    for e in edits {
        out.extend(&chars[at..e.start]);
        out.push_str(&e.with);
        at = e.end;
    }
    out.extend(&chars[at..]);
    out
}

/// The removal of the `end` at `closer`, the first token of its line, or
/// `None` when the rules of the module docs keep it.
fn plan(lexer: &Lexer, chars: &[char], closer: &Closer) -> Option<Edit> {
    if closer.empty {
        return None;
    }
    let (tokens, spans) = (&lexer.tokens, &lexer.token_spans);
    let k = closer.token;
    let span = spans[k];
    let (tok_start, tok_end) = (span.start.offset as usize, span.end.offset as usize);
    let line = span.start.line;
    let line_start = tok_start - (span.start.column as usize - 1);
    match tokens.get(k + 1) {
        // Alone on its line (a `;` is a `Newline` too, and is not a line end).
        Some(Token::Newline | Token::Eof) | None
            if spans.get(k + 1).is_none_or(|s| {
                !matches!(tokens[k + 1], Token::Newline) || chars.get(s.start.offset as usize) != Some(&';')
            }) =>
        {
            // The next line must not continue what would become the last
            // line above it.
            let after = tokens[k + 1..].iter().find(|t| !matches!(t, Token::Newline));
            if after.is_some_and(leads_a_continuation) {
                return None;
            }
            let comment = lexer
                .token_leading_trivia
                .get(k + 1)
                .into_iter()
                .flatten()
                .find(|t| t.kind != TriviaKind::Whitespace)
                .map(|t| t.start as usize);
            match comment {
                Some(at) => Some(Edit {
                    start: line_start,
                    end: at,
                    with: " ".repeat(tok_start - line_start + INDENT),
                    line,
                }),
                None => {
                    // The whole line, with its line break: the one after it,
                    // or (on the last line) the one before.
                    let mut end = tok_end;
                    while end < chars.len() && chars[end] != '\n' {
                        end += 1;
                    }
                    if end < chars.len() {
                        Some(Edit { start: line_start, end: end + 1, with: String::new(), line })
                    } else {
                        let start = line_start.saturating_sub(1);
                        let with = if line_start > 0 { "\n" } else { "" };
                        Some(Edit { start, end, with: with.to_string(), line })
                    }
                }
            }
        }
        // `end)`: the bracket takes the `end`'s place.
        Some(Token::RParen | Token::RBracket | Token::RBrace) => {
            let next = spans[k + 1].start.offset as usize;
            // Only blanks may sit between the two (no comment to lose).
            if chars[tok_end..next].iter().any(|c| !matches!(c, ' ' | '\t')) {
                return None;
            }
            Some(Edit { start: tok_start, end: next, with: String::new(), line })
        }
        _ => None,
    }
}

/// Would a line starting with `tok` continue the line above it? (The
/// parser's `continues_line`, plus `-` and the field accesses, to be safe.)
fn leads_a_continuation(tok: &Token) -> bool {
    use Token::*;
    matches!(
        tok,
        Plus | Minus
            | Star
            | Slash
            | Percent
            | PlusPlus
            | And
            | Or
            | DoubleQuestion
            | Pipe
            | Bar
            | Dot
            | QuestionDot
            | Eq
            | Ne
            | Lt
            | Le
            | Gt
            | Ge
    )
}

/// Collect the `end`s that close a construct under `node`, `next` being the
/// lexer index of the next token. An `end` that only spells a name (`{end:
/// 3}`, `row.end`) is no child of a block construct and is never collected.
fn collect_ends(node: &Rc<GreenNode>, next: &mut usize, out: &mut Vec<Closer>) {
    let closes = matches!(
        node.kind(),
        SyntaxKind::IfExpr
            | SyntaxKind::ElseBranch
            | SyntaxKind::ForStmt
            | SyntaxKind::ForExpr
            | SyntaxKind::WhileStmt
            | SyntaxKind::MatchExpr
            | SyntaxKind::MatchArm
            | SyntaxKind::EnumDecl
            | SyntaxKind::ClassDecl
            | SyntaxKind::FnDecl
            | SyntaxKind::LambdaExpr
            | SyntaxKind::TestDecl
    );
    // Whether the last block seen under this node holds a statement.
    let mut last_block_empty = false;
    for child in node.children() {
        match child {
            GreenChild::Token(t) => {
                let GreenToken::Token { token, .. } = &**t else { continue };
                if closes && matches!(token, Token::End) {
                    out.push(Closer { token: *next, empty: last_block_empty });
                }
                *next += 1;
            }
            GreenChild::Node(n) => {
                if n.kind() == SyntaxKind::Block {
                    last_block_empty =
                        !n.children().iter().any(|c| matches!(c, GreenChild::Node(_)));
                }
                collect_ends(n, next, out);
            }
        }
    }
}
