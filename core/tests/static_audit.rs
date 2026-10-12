//! `--static-audit`: the runtime oracle for the static-value classifier
//! (`petal::static_audit`, docs/tasks/static-value-optimization.md, S0).
//!
//! The classifier takes a `CONST` registration at its word. Each test here
//! registers a native that lies about being constant, builds a top-level
//! table with it, and checks that the audit names the declaration and the
//! input it touched — and that honest programs come back clean.

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::static_audit::{Input, Report, StaticAuditMode};
use petal::value::Value;

fn reads_mouse(cxt: &mut PetalCxt) -> NativeResult {
    let v = cxt.binding_named("mouse_x");
    let n = if let Value::Int(n) = v { n } else { 0 };
    cxt.push_int(n);
    Ok(1)
}

fn reads_host_data(cxt: &mut PetalCxt) -> NativeResult {
    cxt.note_host_read();
    cxt.push_int(7);
    Ok(1)
}

fn draws_random(cxt: &mut PetalCxt) -> NativeResult {
    let f = cxt.rng_next_f64();
    cxt.push_float(f);
    Ok(1)
}

fn prints(cxt: &mut PetalCxt) -> NativeResult {
    cxt.print("built".to_string());
    cxt.push_int(1);
    Ok(1)
}

fn doubles(cxt: &mut PetalCxt) -> NativeResult {
    let n = cxt.get_int(1)?;
    cxt.push_int(n * 2);
    Ok(1)
}

/// An env whose natives all claim to be constant; only `doubles` is.
fn lying_env() -> Env {
    let mut env = Env::new();
    env.register_native("reads_mouse", reads_mouse, NativeEffects::CONST);
    env.register_native("reads_host_data", reads_host_data, NativeEffects::CONST);
    env.register_native("draws_random", draws_random, NativeEffects::CONST);
    env.register_native("prints", prints, NativeEffects::CONST);
    env.register_native("doubles", doubles, NativeEffects::CONST);
    // Honest about not being constant: reads the pointer and says so.
    env.register_native(
        "pointer",
        reads_mouse,
        NativeEffects::probe(petal::native_fn::InputClasses::POINTER),
    );
    env
}

const DECLS: &str = "native fn reads_mouse()\nnative fn reads_host_data()\nnative fn draws_random()\nnative fn prints()\nnative fn doubles(n)\nnative fn pointer()\n";

fn audit(env: &mut Env, mode: StaticAuditMode, body: &str) -> (Result<Value, String>, Report) {
    env.set_static_audit(mode);
    let source = format!("{DECLS}{body}");
    let pid = env.load_program(&source).expect("compiles");
    // The premise of every test: the classifier believed the registration.
    let sid = env.create_stack(pid).expect("stack");
    let result = env.run(sid);
    (result, env.static_audit_report())
}

fn is_static(body: &str, name: &str) -> bool {
    let mut env = lying_env();
    let pid = env.load_program(&format!("{DECLS}{body}")).expect("compiles");
    let report = env.static_report(env.get_program(pid).unwrap());
    report
        .get(name)
        .is_some_and(|d| d.class == petal::static_classify::DeclClass::Static)
}

#[test]
fn an_honest_program_is_clean_in_both_modes() {
    let body = "\
fn row(a, b) { a: a, b: doubles(b) } end
let ROWS = [row(1, 2), row(3, 4)]
let TOTAL = reduce(ROWS, 0, fn(acc, r) acc + r.b end)
state clicks = 0
clicks = clicks + 1
let label = TOTAL * 100 + clicks
label
";
    for mode in [StaticAuditMode::Watch, StaticAuditMode::StaticOnly] {
        let mut env = lying_env();
        let (result, report) = audit(&mut env, mode, body);
        assert!(result.is_ok(), "{mode:?}: {result:?}");
        assert!(!report.has_failures(), "{mode:?}:\n{report}");
        assert_eq!(report.static_declarations, 2, "{mode:?}:\n{report}");
        // `doubles` ran twice under ROWS.
        assert!(report.native_calls >= 2, "{mode:?}:\n{report}");
    }
}

#[test]
fn a_binding_read_under_a_static_declaration_names_all_three() {
    let body = "\
fn cell(i) { i: i, x: reads_mouse() } end
let GRID = [cell(1), cell(2)]
len(GRID)
";
    let mut env = lying_env();
    assert!(is_static(body, "GRID"), "premise: GRID is classified static");
    let mouse_x = env.intern_symbol("mouse_x");
    env.set_binding(mouse_x, Value::Int(40));
    let (result, report) = audit(&mut env, StaticAuditMode::Watch, body);
    assert!(result.is_ok(), "{result:?}");
    assert!(report.has_failures(), "{report}");
    let f = &report.findings[0];
    assert_eq!(f.declaration.as_deref(), Some("GRID"), "{report}");
    assert_eq!(f.input, Input::Binding("mouse_x".to_string()), "{report}");
    assert_eq!(f.native.as_deref(), Some("reads_mouse"), "{report}");
    assert_eq!(f.count, 2, "one finding, seen once per cell:\n{report}");
    // The declaration's own line, and the line of the call inside `cell`.
    assert_eq!(f.at.as_deref(), Some("line 8"), "{report}");
    assert_eq!(f.site.as_deref(), Some("line 7"), "{report}");
    let text = report.to_string();
    assert!(text.contains("1 failure"), "{text}");
    assert!(
        text.contains("GRID") && text.contains("read the binding `mouse_x`") && text.contains("reads_mouse"),
        "{text}"
    );
}

#[test]
fn each_dynamic_input_is_reported_as_itself() {
    let cases: [(&str, Input); 3] = [
        ("reads_host_data()", Input::HostData),
        ("draws_random()", Input::Random),
        ("prints()", Input::Effect),
    ];
    for (call, input) in cases {
        let body = format!("let TABLE = [{call}, 2]\nlen(TABLE)\n");
        for mode in [StaticAuditMode::Watch, StaticAuditMode::StaticOnly] {
            let mut env = lying_env();
            let (result, report) = audit(&mut env, mode, &body);
            assert!(result.is_ok(), "{call} {mode:?}: {result:?}");
            let found: Vec<&Input> = report
                .findings
                .iter()
                .filter(|f| f.declaration.as_deref() == Some("TABLE"))
                .map(|f| &f.input)
                .collect();
            assert_eq!(found, [&input], "{call} {mode:?}:\n{report}");
        }
    }
}

/// A dynamic declaration may read what it likes: nothing is reported for it,
/// in the same program as a static one that is.
#[test]
fn only_static_terms_are_audited() {
    let body = "\
state n = 0
n = n + 1
let moving = [pointer() + n, random()]
let FIXED = [doubles(2)]
len(moving) + len(FIXED)
";
    assert!(is_static(body, "FIXED") && !is_static(body, "moving"));
    let mut env = lying_env();
    let (result, report) = audit(&mut env, StaticAuditMode::Watch, body);
    assert!(result.is_ok(), "{result:?}");
    assert!(!report.has_failures(), "{report}");
}

/// A term inside a top-level loop is audited each time round.
#[test]
fn a_static_term_inside_a_dynamic_region_is_audited() {
    let body = "\
state n = 0
n = n + 1
for i in range(0, 3) do
  let k = prints()
  n = n + k
end
n
";
    let mut env = lying_env();
    let (_, report) = audit(&mut env, StaticAuditMode::Watch, body);
    let f = report
        .findings
        .iter()
        .find(|f| f.input == Input::Effect)
        .unwrap_or_else(|| panic!("the print under `k` is caught:\n{report}"));
    assert_eq!(f.count, 3, "{report}");
    assert_eq!(f.native.as_deref(), Some("prints"), "{report}");
}

/// The static-only build runs the static statements and nothing else: the
/// dynamic statements' prints never happen and their natives are never
/// called, so a script builds without its host.
#[test]
fn static_only_runs_no_dynamic_statement_and_needs_no_host() {
    let mut env = Env::new();
    env.set_static_audit(StaticAuditMode::StaticOnly);
    // `host_draw` and `host_width` are registered by nothing, so the script
    // only compiles unlinked.
    env.set_check_only(true);
    let source = "\
native fn host_draw(x)
native fn host_width()
fn pair(a, b) [a, b] end
let PAIRS = [pair(1, 2), pair(3, 4)]
let FLAT = reverse(PAIRS)
state frame = 0
frame = frame + 1
print(\"dynamic statement ran\")
let w = host_width() / len(FLAT)
host_draw(w)
assert_eq(len(FLAT), 2)
";
    let pid = env.load_program(source).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    let result = env.run(sid);
    let report = env.static_audit_report();
    assert!(result.is_ok(), "{result:?}\n{report}");
    assert!(!report.has_failures(), "{report}");
    assert_eq!(report.static_declarations, 2, "{report}");
    assert!(report.statements_skipped > 0 && report.statements_run > 0, "{report}");
    let out = env.take_output();
    assert!(out.is_empty(), "nothing dynamic ran: {out:?}");
    // `reverse`, `len` and `assert_eq` ran under static statements.
    assert!(report.native_calls >= 3, "{report}");

    // The same script with the audit off stops at the first host native.
    let mut plain = Env::new();
    assert!(plain.run_source(source).is_err());
}

/// A build that raises is a failure, with the message.
#[test]
fn a_static_only_build_that_raises_is_a_finding() {
    let mut env = Env::new();
    env.set_static_audit(StaticAuditMode::StaticOnly);
    let source = "let XS = [1, 2, 3]\nlet BAD = XS[0] / 0 + nth(XS, 9).field\nlen(XS)\n";
    let pid = env.load_program(source).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    let result = env.run(sid);
    let report = env.static_audit_report();
    assert!(result.is_err(), "premise: the build raises");
    assert!(report.has_failures(), "{report}");
    assert!(
        report.findings.iter().any(|f| matches!(f.input, Input::Raised(_))
            && f.declaration.as_deref() == Some("BAD")),
        "{report}"
    );
}

/// Turning the audit off puts the program back: the dynamic statements run
/// again, and nothing is recorded.
#[test]
fn turning_it_off_restores_the_whole_program() {
    let mut env = Env::new();
    let source = "let XS = [1, 2]\nstate n = 0\nn = n + len(XS)\nn\n";
    let pid = env.load_program(source).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    assert_eq!(env.run(sid).unwrap(), Value::Int(2));
    env.set_static_audit(StaticAuditMode::StaticOnly);
    env.reset_stack(sid).unwrap();
    assert_eq!(env.run(sid).unwrap(), Value::Nil, "the write and the read of `n` were skipped");
    assert!(env.static_audit_report().statements_skipped > 0);
    env.set_static_audit(StaticAuditMode::Off);
    env.reset_stack(sid).unwrap();
    assert!(matches!(env.run(sid).unwrap(), Value::Int(_)), "the whole program runs again");
    assert!(!env.static_audit_report().has_failures());
}

/// A host call after the run is not part of any top-level statement: what
/// the called function touches is no finding, even when the last statement
/// of the script is a static one.
#[test]
fn a_host_call_after_the_run_is_not_audited() {
    let mut env = lying_env();
    env.set_static_audit(StaticAuditMode::Watch);
    let pid = env
        .load_program(&format!(
            "{DECLS}fn poke()\n  state hits = 0\n  hits = hits + pointer()\n  hits\nend\nlet LAST = [doubles(1), doubles(2)]\n"
        ))
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.call_function(sid, "poke", &[]).unwrap();
    let report = env.static_audit_report();
    assert!(!report.has_failures(), "{report}");
}
