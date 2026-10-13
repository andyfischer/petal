//! A host that reloads a script while it is being edited can print what the
//! edit introduced: `ReloadReport::new_diagnostics`, rendered with
//! `Env::render_diagnostics`. Warnings as well as errors, and only the ones
//! the program did not already have, so a large program's standing warnings
//! are not printed again on every save (docs/tasks/moses-debrief.md, 2.1:
//! a `petal check` warning shipped as a bug because nothing ran `check` in
//! the edit loop).

use petal::diagnostic::Severity;
use petal::env::{Env, ReloadOutcome};
use petal::stack::StackKey;

fn loaded(src: &str) -> (Env, petal::program::ProgramId, StackKey) {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("load");
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).expect("run");
    env.take_output();
    (env, pid, sid)
}

const BASE: &str = "\
fn double(x: int) x end
let a = double(\"one\")
let drag = 4
print(drag)
";

#[test]
fn a_first_load_hands_out_every_finding() {
    let (env, pid, _) = loaded(BASE);
    let all = env.diagnostics(pid);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].severity, Severity::Warning);
    let text = env.diagnostics_text(pid);
    assert!(text.starts_with("warning: "), "{text}");
    assert!(text.contains("[line 2, column"), "{text}");
    // The same text from the list.
    assert_eq!(env.render_diagnostics(pid, env.diagnostics(pid)), text);
    // Errors only: nothing.
    assert_eq!(env.error_diagnostics_text(pid), "");
}

#[test]
fn a_reload_reports_the_warning_the_edit_introduced_and_not_the_standing_one() {
    let (mut env, pid, sid) = loaded(BASE);
    // The bug the debrief names: a continuation line that starts with `-`,
    // so the term is computed and dropped. Two lines are added above the
    // standing warning too, which moves it without making it new.
    let edited = "\
// a comment
// another
fn double(x: int) x end
let a = double(\"one\")
let drag = 4
  - 1
print(drag)
";
    let report = env.reload_program(sid, edited, None).expect("reload");
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    assert_eq!(env.diagnostics(pid).len(), 2);
    assert_eq!(report.new_diagnostics.len(), 1, "{:?}", report.new_diagnostics);
    let new = &report.new_diagnostics[0];
    assert_eq!(new.severity, Severity::Warning);
    assert_eq!(new.span.start.line, 6);
    let text = env.render_diagnostics(pid, &report.new_diagnostics);
    assert!(text.starts_with("warning: "), "{text}");
    assert!(text.contains("[line 6, column"), "{text}");
    assert!(text.contains("  - 1"), "{text}");
    assert!(!text.contains("double"), "the standing warning is not repeated: {text}");

    // Saving again with nothing new says nothing.
    let again = format!("{edited}print(a)\n");
    let report = env.reload_program(sid, &again, None).expect("reload");
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    assert_eq!(report.new_diagnostics, vec![]);
    assert_eq!(env.diagnostics(pid).len(), 2);
}

#[test]
fn a_reload_reports_a_new_error_and_a_second_copy_of_a_standing_warning() {
    let (mut env, pid, sid) = loaded(BASE);
    // A call to a name nothing defines is an error a reload still runs
    // with; a second identical bad call is one more finding, not zero.
    let edited = "\
fn double(x: int) x end
let a = double(\"one\")
let drag = 4
print(drag)
fn later()
  let a = double(\"one\")
  nowhere(a)
end
";
    let report = env.reload_program(sid, edited, None).expect("reload");
    let kinds: Vec<(Severity, u32)> = report
        .new_diagnostics
        .iter()
        .map(|d| (d.severity, d.span.start.line))
        .collect();
    assert_eq!(kinds.len(), 2, "{:?}", report.new_diagnostics);
    assert!(kinds.contains(&(Severity::Warning, 6)), "{kinds:?}");
    assert!(kinds.contains(&(Severity::Error, 7)), "{kinds:?}");
    assert_eq!(env.diagnostics(pid).len(), 3);
}

#[test]
fn a_reload_that_does_not_recompile_reports_nothing() {
    let (mut env, _pid, sid) = loaded(BASE);
    // A literal changes: the running program is patched, not recompiled.
    let report = env
        .reload_program(sid, &BASE.replace("let drag = 4", "let drag = 5"), None)
        .expect("reload");
    assert_ne!(report.outcome, ReloadOutcome::Recompiled);
    assert_eq!(report.new_diagnostics, vec![]);
    // Nothing changes at all.
    let report = env
        .reload_program(sid, &BASE.replace("let drag = 4", "let drag = 5"), None)
        .expect("reload");
    assert_eq!(report.outcome, ReloadOutcome::Unchanged);
    assert_eq!(report.new_diagnostics, vec![]);
}
