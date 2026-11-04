//! **`never-replays`** — a function whose calls are never replayed from
//! the memo table, and why, down to the native or the variable.
//!
//! ```text
//! fn draw_ship(s)                         never-replays: draw_ship reads `time`
//!   let t = time()                          on every call (class CLOCK), whose
//!   draw_mesh("ship", {pos: s.pos + bob(t)})  answer changes every run
//! end
//! ```
//!
//! A memoized scope replays when nothing it read changed. The dependency
//! classes (`crate::static_classify::dep_classes`) know the functions for
//! which that can never hold for their own record, because on every path
//! they:
//!
//! - do something no replay reproduces (`print`, a draw from the random
//!   stream, a host write declared `effect`), themselves or through a
//!   function they call on every path;
//! - read an input whose answer changes every run (a native declared
//!   `ticks`, such as `time`);
//! - use a top-level plain `var`, which is a new cell every run.
//!
//! It also reports, as replaying rarely, a function that reads a revisioned
//! class (`PHYSICS`, `NET`) on every call: it replays only on runs where
//! the host did not revise that class.
//!
//! Report only, and **opt-in** (`--rules-include never-replays`): a console
//! script that prints from a helper is not wrong, and the rule compiles the
//! file. The classes are this process's: a native of a host that is not
//! here is unknown and claims nothing, so a host that registers its natives
//! (`Env::replay_findings`) sees more.

use super::{Fix, LintOptions};

pub(super) fn plan_never_replays_reports(source: &str, chars: &[char], opts: &LintOptions) -> Vec<Fix> {
    let Ok((env, pid)) =
        crate::ir_equiv::compile_for_compare(source, &opts.include_dirs, opts.origin.as_deref())
    else {
        return Vec::new();
    };
    let Some(program) = env.get_program(pid) else {
        return Vec::new();
    };
    let analysis = env.static_analysis(program);
    let deps = analysis.dep_classes();
    let mut out = Vec::new();
    for finding in deps.findings(program, false) {
        let Some(line) = deps.function(finding.function).and_then(|f| f.line) else {
            continue;
        };
        let what = match finding.severity {
            crate::static_classify::ReplaySeverity::Never => "is never replayed",
            crate::static_classify::ReplaySeverity::Rarely => "is rarely replayed",
        };
        out.push(Fix {
            anchor: line_start(chars, line as usize),
            message: format!("`{}` {what}: it {}", finding.name, finding.reason),
            splices: Vec::new(),
        });
    }
    out
}

/// The char offset of the first non-blank character of 1-based `line`.
fn line_start(chars: &[char], line: usize) -> usize {
    let mut at = 0;
    let mut n = 1;
    while n < line && at < chars.len() {
        if chars[at] == '\n' {
            n += 1;
        }
        at += 1;
    }
    while at < chars.len() && (chars[at] == ' ' || chars[at] == '\t') {
        at += 1;
    }
    at
}
