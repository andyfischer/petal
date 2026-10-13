//! Rules that read off the type pass ([`crate::typecheck::TypeNote`]).
//!
//! **`neg-zero-sub`** — `0 - x` is `-x`.
//!
//! ```text
//! let back = 0 - step        =>   let back = -step
//! ```
//!
//! The fix is applied only where the two are the same value: the zero is
//! written `0` and `x` is proven an `int`. With a float they are not quite
//! the same, which is why the other spellings are reported and left for the
//! author: `0.0 - x` is `0.0` when `x` is `0.0` and `-x` is `-0.0`, which
//! compares equal and prints, divides and goes through `atan2` differently;
//! and `0.0 - n` makes a float of an int `n` where `-n` is still an int.
//!
//! **`int-division`** — `a / b` on two ints truncates, and the quotient is
//! then used as a float: `float(w / 2)`, `(n / 4) * 0.5`. No fix, because
//! either reading can be meant. It is opt-in: a row or column index
//! (`i / W`) scaled by a float tile size is the same shape and is correct.

use crate::ast::Stmt;
use crate::typecheck::TypeNote;
use crate::types::Type;

use super::Fix;
use super::to_match::Splice;
use super::util::{span_text, starts_its_line, text};

pub(super) fn plan_neg_zero_fixes(notes: &[TypeNote], chars: &[char]) -> Vec<Fix> {
    let mut fixes: Vec<Fix> = notes
        .iter()
        .filter_map(|n| {
            let TypeNote::ZeroSub {
                whole,
                zero,
                operand,
                zero_is_int,
                operand_ty,
                operand_is_atomic,
            } = *n
            else {
                return None;
            };
            let start = whole.start.offset as usize;
            let zero_text = span_text(chars, zero)?;
            let operand_start = operand.start.offset as usize;
            let operand_end = operand.end.offset as usize;
            // The statement `0 - x` was desugared from nothing: the zero must
            // be written where the subtraction starts.
            if zero.start.offset as usize != start || !matches!(zero_text.as_str(), "0" | "0.0") {
                return None;
            }
            let glue = text(chars, zero.end.offset as usize, operand_start)?;
            let after_minus = glue.trim_start().strip_prefix('-')?;
            // Between the `-` and the operand: nothing, or the operand's own
            // opening parentheses.
            let opens: String = after_minus.chars().filter(|c| !c.is_whitespace()).collect();
            if !opens.chars().all(|c| c == '(') || glue.contains('\n') {
                return None;
            }
            let exact = zero_is_int && operand_ty == Type::Int;
            // A line that starts with `-` continues the line above it.
            let fixable = exact && !starts_its_line(chars, start);
            let message = if exact {
                "`0 - x` can be written `-x`".to_string()
            } else if zero_is_int {
                "`0 - x` is `-x` when `x` is an int (for a float they differ in the sign of a \
                 zero result)"
                    .to_string()
            } else {
                "`0.0 - x` is `-x` for a float `x`, apart from the sign of a zero result: \
                 `0.0 - 0.0` is `0.0` and `-0.0` is not, which `atan2` and a division tell apart"
                    .to_string()
            };
            let mut splices = Vec::new();
            if fixable {
                let wrap = !operand_is_atomic && opens.is_empty();
                splices.push(Splice {
                    start,
                    end: operand_start,
                    text: format!("-{opens}{}", if wrap { "(" } else { "" }),
                });
                if wrap {
                    splices.push(Splice {
                        start: operand_end,
                        end: operand_end,
                        text: ")".to_string(),
                    });
                }
            }
            Some(Fix {
                anchor: start,
                message,
                splices,
            })
        })
        .collect();
    fixes.sort_by_key(|f| f.anchor);
    fixes
}

pub(super) fn plan_int_division(notes: &[TypeNote], _stmts: &[Stmt]) -> Vec<Fix> {
    let mut fixes: Vec<Fix> = notes
        .iter()
        .filter_map(|n| {
            let TypeNote::IntDivMeetsFloat { div, .. } = *n else {
                return None;
            };
            Some(Fix {
                anchor: div.start.offset as usize,
                message: "`/` on two ints truncates, and the result is then used as a float: \
                          write `float(a) / b` for the fraction, or `floor(a / b)` to say the \
                          truncation is meant"
                    .to_string(),
                splices: Vec::new(),
            })
        })
        .collect();
    fixes.sort_by_key(|f| f.anchor);
    fixes.dedup_by_key(|f| f.anchor);
    fixes
}
