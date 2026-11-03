//! Rule — `state(key)` is deprecated; a never-written one is a `memo`.
//!
//! ```text
//! fn label_width(text)                  fn label_width(text)
//!     state(text) w = text.length * 8  =>   memo(text) w = text.length * 8
//!     w                                     w
//! end                                   end
//! ```
//!
//! Keyed state is one slot per key value whatever call path reaches it: a
//! global hidden in a function ([`crate::compiler::keyed_state`], which also
//! reports every site as a compile warning). The rule reports each
//! declaration, and fixes the ones whose slot nothing writes *and* whose
//! initializer is plainly a pure function of the key: such a slot only ever
//! holds what its initializer computed for that key, which is what a `memo`
//! is.
//!
//! "Nothing writes it" is the compiler's own answer, not this rule's: a
//! `memo` rejects every write to its name (rebind, `+=`, `set`, `@x`, an
//! indexed or field write, from any closure), so the rewrite is kept only
//! when it compiles. A `state(k) var`, a `pub state(k)` (there is no
//! `pub memo`) and a file that does not compile here are reported without a
//! fix.
//!
//! Purity is the part the compiler cannot see. A memo is shared by every
//! caller with an equal key, and it is a cache: dropped by a reload that
//! changes the program and by a run that does not reach it, and recomputed
//! when next asked. An initializer that reads anything but its key
//! (`state(k) v = k * scale`) would hand the first caller's answer to every
//! caller, and one that is effectful (`state(id) tint = random(0.0, 1.0)`)
//! would give a new value after each drop. So the fix is offered only when
//! [`crate::compiler::keyed_state::init_is_pure_of_key`] can tell from the syntax that neither can happen:
//! no calls, no `get`, no name the key does not read. Every other site is
//! reported without a fix. Until the full purity check exists
//! (docs/tasks/memo-purity-check.md), a call such as `measure(text)` is
//! treated as possibly impure, and the author decides.

use crate::ast::Stmt;
use crate::compiler::keyed_state::{KeyedState, keyed_states, message};

use super::to_match::{Splice, apply_match_edits};
use super::{Fix, LintOptions, compiles};

const STATE: &str = "state";
const MEMO: &str = "memo";

/// Plan the report for every `state(key)` in the file `source` (parsed as
/// `stmts`), with the `state` → `memo` splice on each one that can take it.
pub(super) fn plan_state_key_fixes(
    source: &str,
    stmts: &[Stmt],
    chars: &[char],
    opts: &LintOptions,
) -> Vec<Fix> {
    let found = keyed_states(stmts);
    if found.is_empty() {
        return Vec::new();
    }
    // The candidates: a plain keyed `state` whose keyword is where the
    // statement starts, and whose initializer is plainly pure of its key.
    let candidate = |k: &KeyedState| {
        let at = k.span.start.offset as usize;
        (!k.is_var && !k.exported && k.init_pure_of_key && keyword_at(chars, at)).then_some(at)
    };
    let mut fixable: Vec<usize> = found.iter().filter_map(candidate).collect();
    if !fixable.is_empty() {
        if compiles(source, opts).is_err() {
            fixable.clear();
        } else if compiles(&rewrite(chars, &fixable), opts).is_err() {
            // Some slot is written. Find which, one declaration at a time.
            fixable.retain(|&at| compiles(&rewrite(chars, &[at]), opts).is_ok());
        }
    }
    found
        .iter()
        .map(|k| {
            let at = k.span.start.offset as usize;
            let splices = if fixable.contains(&at) {
                vec![keyword_splice(at)]
            } else {
                Vec::new()
            };
            Fix {
                anchor: at,
                message: message(&k.name),
                splices,
            }
        })
        .collect()
}

fn keyword_splice(at: usize) -> Splice {
    Splice {
        start: at,
        end: at + STATE.len(),
        text: MEMO.to_string(),
    }
}

/// `chars` with the `state` keyword at each of `sites` (ascending) spelled
/// `memo`.
fn rewrite(chars: &[char], sites: &[usize]) -> String {
    let splices: Vec<Splice> = sites.iter().map(|&at| keyword_splice(at)).collect();
    apply_match_edits(chars, &splices)
}

/// Is `state` written at `pos`, as a whole word?
fn keyword_at(chars: &[char], pos: usize) -> bool {
    let n = STATE.len();
    pos + n <= chars.len()
        && chars[pos..pos + n].iter().copied().eq(STATE.chars())
        && chars
            .get(pos + n)
            .is_none_or(|c| !(c.is_alphanumeric() || *c == '_'))
}
