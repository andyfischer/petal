//! Rule — a `state` that is really a constant should say so.
//!
//! ```text
//! state TILES = [                       const TILES = [
//!   {name: "grass", cost: 1},     =>      {name: "grass", cost: 1},
//!   {name: "road", cost: 0},              {name: "road", cost: 0},
//! ]                                     ]
//! ```
//!
//! A table parked in `state` is built once, which is why it was put there,
//! and for the same reason never follows an edit: a hot reload keeps every
//! `state` cell's old value. A `const` is built once too and *is* rebuilt
//! when its source changes (docs/language-guide.md, Constants).
//!
//! The rule fires on a declaration that is all of:
//!
//! - a plain top-level `state NAME = init` — no key (`state(k) x`), not a
//!   `state var`, not inside a function, not `pub`;
//! - **never assigned**: no write to the cell anywhere in the program the
//!   file compiles to, in any function or module;
//! - **static**: the classifier ([`crate::static_classify`]) says the
//!   initializer can only ever produce one value, *and* the declaration is
//!   still static once it is written `const`. The second half is checked by
//!   compiling the rewrite, so a fix never produces a `const` the compiler
//!   then rejects.
//!
//! The classifier's answer is this process's: a `native fn` of a host that
//! is not here is dynamic, so a table built with host natives is left alone.
//!
//! ## Why it is a suggestion, and opt-in
//!
//! Every other rule's fix is another spelling of the same program. This one
//! is a judgement, for three reasons the compiler cannot see:
//!
//! - a host can write a `state` by name, and saved state can be restored
//!   into it;
//! - the author may want the value to survive a reload on purpose;
//! - a `state` initializer runs once whatever the host, and a `const` is
//!   built once only where the static-value cache is on. Where it is off (a
//!   run that records observations or a trace, the `baseline` policy) an
//!   expensive build would run on every frame.
//!
//! So the rule runs only when `--rules-include` names it, and a line can
//! still be exempted with `// petal-lint-ignore state-as-constant`.

use crate::ast::{Stmt, StmtKind};
use crate::program::{Program, TermOp};
use crate::static_classify::{ConstCheck, DeclKind, StaticReport};

use super::to_match::Splice;
use super::{Fix, LintOptions};

/// One `state` declaration the rule would rewrite.
struct Candidate {
    name: String,
    /// Char offset of the `state` keyword.
    keyword: usize,
}

const STATE: &str = "state";
const CONST: &str = "const";

/// Plan every `state` → `const` rewrite in the file `source` (parsed as
/// `stmts`), in source order. Empty when the file declares no candidate, or
/// does not compile here.
pub(super) fn plan_state_fixes(
    source: &str,
    stmts: &[Stmt],
    chars: &[char],
    opts: &LintOptions,
) -> Vec<Fix> {
    // Cheap first: most files have no plain top-level `state` at all, and
    // those never pay for the compile below.
    let shaped: Vec<(&Stmt, &str)> = stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::State {
                name,
                key: None,
                is_var: false,
                ..
            } if !s.exported && keyword_at(chars, s.span.start.offset as usize) => {
                Some((s, name.as_str()))
            }
            _ => None,
        })
        .collect();
    if shaped.is_empty() {
        return Vec::new();
    }
    let Some((env, pid, report)) = classify(source, opts) else {
        return Vec::new();
    };
    let Some(program) = env.get_program(pid) else {
        return Vec::new();
    };

    let mut candidates: Vec<Candidate> = shaped
        .into_iter()
        .filter(|(stmt, name)| {
            // The classifier's entry for this statement: the entry file's
            // `state` of that name, written inside it.
            let (start, end) = (stmt.span.start.line, stmt.span.end.line);
            report.entry().any(|d| {
                d.kind == DeclKind::State
                    && d.name == *name
                    && d.line.is_some_and(|l| start <= l && l <= end)
                    && d.initializer_static == Some(true)
                    && !is_written(program, d.term)
            })
        })
        .map(|(stmt, name)| Candidate {
            name: name.to_string(),
            keyword: stmt.span.start.offset as usize,
        })
        .collect();

    // A static initializer is necessary, not sufficient: as a `const` the
    // expression runs where it is written instead of on the cell's first
    // use, which can change what a forward reference sees. Compile the
    // rewrite and keep only what the compiler accepts as constant.
    while !candidates.is_empty() {
        let rewritten = super::to_match::apply_match_edits(chars, &splices(&candidates));
        let Some((_, _, after)) = classify(&rewritten, opts) else {
            return Vec::new();
        };
        let constant = |c: &Candidate| {
            after.entry().any(|d| {
                d.kind == DeclKind::Const
                    && d.name == c.name
                    && d.const_check == Some(ConstCheck::Constant)
            })
        };
        let before = candidates.len();
        candidates.retain(constant);
        if candidates.len() == before {
            break;
        }
    }

    candidates
        .into_iter()
        .map(|c| Fix {
            anchor: c.keyword,
            message: format!(
                "`state {}` is never assigned and its initializer is constant; declare it \
                 with `const` so that it follows edits (keep `state` if a host writes it by name)",
                c.name
            ),
            splices: vec![keyword_splice(c.keyword)],
        })
        .collect()
}

fn keyword_splice(at: usize) -> Splice {
    Splice {
        start: at,
        end: at + STATE.len(),
        text: CONST.to_string(),
    }
}

fn splices(candidates: &[Candidate]) -> Vec<Splice> {
    let mut all: Vec<Splice> = candidates.iter().map(|c| keyword_splice(c.keyword)).collect();
    all.sort_by_key(|s| s.start);
    all
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

/// Compile `source` the way the lint's compile gate does and classify it.
/// `None` when it does not compile here.
fn classify(
    source: &str,
    opts: &LintOptions,
) -> Option<(crate::env::Env, crate::program::ProgramId, StaticReport)> {
    let (env, pid) =
        crate::ir_equiv::compile_for_compare(source, &opts.include_dirs, opts.origin.as_deref())
            .ok()?;
    let report = env.static_report(env.get_program(pid)?);
    Some((env, pid, report))
}

/// Does anything in the program write the `state` cell that `init` (its
/// `StateInit` term) declares? Every rebind, indexed write and field write
/// of a `state` name is a `StateWrite` carrying the cell's key, in whatever
/// function or module it is written.
fn is_written(program: &Program, init: crate::program::TermId) -> bool {
    let Some(key) = program.get_term(init).state_key else {
        // Not a cell the pass can follow: leave it alone.
        return true;
    };
    program
        .terms
        .iter()
        .any(|t| matches!(t.op, TermOp::StateWrite) && t.state_key == Some(key))
}
