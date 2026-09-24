//! Rule — drop identity casts (`int(n)` where `n` is already an `int`).
//!
//! Detection is the type checker's ([`crate::typecheck::find_redundant_casts`]),
//! so this file is only the rewrite: turn the reported spans into the two
//! minimal edits that delete a call's head and its closing paren, leaving the
//! argument text — comments and layout included — exactly as written.
//!
//! Every edit is re-checked against the source text before it is accepted. A
//! span that doesn't look like `<name> (` … `)` is skipped rather than spliced,
//! so a surprising span can only ever cost us a fix, never corrupt a file.

use crate::typecheck::{CastSlot, RedundantCast};

use super::Fix;
use super::to_match::Splice;

/// One accepted rewrite, as the two char ranges to replace: the call's head
/// `int(` becomes `` or `(`, and its tail `)` becomes `` or `)`.
#[derive(Debug, Clone, Copy)]
struct CastEdit {
    head: (usize, usize),
    tail: (usize, usize),
    keep_parens: bool,
}

/// Turn the checker's reports into fixes, dropping any whose spans don't match
/// the source text they claim to cover.
pub(super) fn plan_cast_fixes(casts: &[RedundantCast], chars: &[char]) -> Vec<Fix> {
    let mut fixes: Vec<Fix> = casts
        .iter()
        .filter_map(|c| {
            let edit = plan_one(c, chars)?;
            let (open, close) = if edit.keep_parens {
                ("(", ")")
            } else {
                ("", "")
            };
            let kind = match c.name {
                "int" => "an int",
                "float" => "a float",
                _ => "a string",
            };
            Some(Fix {
                anchor: edit.head.0,
                message: format!(
                    "`{}(…)` does nothing here: its argument is already {kind}",
                    c.name
                ),
                splices: vec![
                    Splice {
                        start: edit.head.0,
                        end: edit.head.1,
                        text: open.to_string(),
                    },
                    Splice {
                        start: edit.tail.0,
                        end: edit.tail.1,
                        text: close.to_string(),
                    },
                ],
            })
        })
        .collect();
    fixes.sort_by_key(|f| f.anchor);
    fixes
}

fn plan_one(cast: &RedundantCast, chars: &[char]) -> Option<CastEdit> {
    let call_start = cast.call.start.offset as usize;
    let call_end = cast.call.end.offset as usize;
    let arg_start = cast.arg.start.offset as usize;
    let arg_end = cast.arg.end.offset as usize;
    if !(call_start < arg_start
        && arg_start < arg_end
        && arg_end < call_end
        && call_end <= chars.len())
    {
        return None;
    }
    // The head must be exactly the cast name followed by an opening paren, and
    // the tail exactly a closing paren. Whitespace either side is fine;
    // anything else (a comment, a second nested paren) means the spans don't
    // describe the call we think they do, so leave it alone.
    let head: String = chars[call_start..arg_start].iter().collect();
    let rest = head.trim_end().strip_suffix('(')?;
    if rest.trim_end() != cast.name {
        return None;
    }
    let tail: String = chars[arg_end..call_end].iter().collect();
    if tail.trim() != ")" {
        return None;
    }
    // Decide what has to stand in for the call's parentheses.
    let keep_parens = if cast.arg_is_atomic {
        false
    } else {
        match cast.slot {
            CastSlot::Delimited => false,
            CastSlot::Operand => true,
            // Commas are required between elements, so a list/argument slot is
            // always bounded by a real separator: dropping the parens cannot
            // let a neighbouring element bind across the boundary.
            CastSlot::ListElement => false,
        }
    };

    Some(CastEdit {
        head: (call_start, arg_start),
        tail: (arg_end, call_end),
        keep_parens,
    })
}
