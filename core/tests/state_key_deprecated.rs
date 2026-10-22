//! `state(key)` is deprecated (core/src/compiler/keyed_state.rs): every
//! keyed declaration is a compile warning carrying the rule id
//! `state-key-deprecated`, silenced by `// petal-lint-ignore`, and
//! `petal lint` reports the same sites, fixing the never-written ones to
//! `memo(key)` (core/src/lint/state_key.rs). The feature itself still works.

use petal::env::Env;
use petal::lint::{LintOptions, LintOutcome, RuleFilter, STATE_KEY_DEPRECATED, lint_source};

/// Compile `src` and return its compile-time warnings.
fn warnings(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("should compile");
    env.get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

fn deprecations(src: &str) -> Vec<String> {
    warnings(src)
        .into_iter()
        .filter(|m| m.contains("[state-key-deprecated]"))
        .collect()
}

fn run(src: &str) -> String {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("should compile");
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).expect("should run");
    env.take_output().join("\n").trim().to_string()
}

fn lint(src: &str) -> LintOutcome {
    let opts = LintOptions {
        rules: RuleFilter {
            include: Some(vec![STATE_KEY_DEPRECATED.to_string()]),
            exclude: Vec::new(),
        },
        ..LintOptions::default()
    };
    lint_source(src, &opts).expect("lint_source should succeed")
}

const HEALTH: &str = "\
fn health(id, damage)
  state(id) hp = 100
  hp -= damage
  hp
end
print(health(\"goblin\", 10), health(\"orc\", 30), health(\"goblin\", 5))
";

#[test]
fn a_keyed_state_is_a_deprecation_warning_naming_the_replacements() {
    let found = deprecations(HEALTH);
    let [message] = found.as_slice() else {
        panic!("one deprecation warning: {:?}", warnings(HEALTH));
    };
    assert!(
        message.starts_with("`state(…) hp` is deprecated"),
        "{message}"
    );
    for replacement in ["`memo(key)`", "`state var` holding a Map", "plain `state`"] {
        assert!(message.contains(replacement), "{message}");
    }
}

#[test]
fn the_warning_points_at_the_declaration() {
    let mut env = Env::new();
    let pid = env.load_program(HEALTH).expect("should compile");
    let program = env.get_program(pid).unwrap();
    let d = program
        .warnings
        .iter()
        .find(|d| d.message.contains("state-key-deprecated"))
        .expect("the warning");
    assert_eq!((d.span.start.line, d.span.start.column), (2, 3));
    assert_eq!(d.severity, petal::diagnostic::Severity::Warning);
}

#[test]
fn keyed_state_still_works() {
    assert_eq!(run(HEALTH), "90 70 85");
}

#[test]
fn every_keyed_form_is_reported_and_nothing_else() {
    let src = "\
state top = 1
state(1) keyed = 2
fn f(k)
  state plain = 0
  state(k) var cell = 0
  memo(k) cached = k * 2
  get cell + cached + plain
end
print(top + keyed + f(3))
";
    let found = deprecations(src);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("`state(…) keyed`"), "{found:?}");
    assert!(found[1].contains("`state(…) cell`"), "{found:?}");
}

#[test]
fn a_comment_on_the_line_before_silences_it() {
    let src = "\
fn f(id)
  // petal-lint-ignore state-key-deprecated -- shared on purpose
  state(id) n = 1
  n
end
print(f(1))
";
    assert_eq!(deprecations(src), Vec::<String>::new());
}

#[test]
fn a_trailing_comment_silences_its_own_line_only() {
    let src = "\
fn f(id)
  state(id) a = 1 // petal-lint-ignore state-key-deprecated
  state(id) b = 2
  a + b
end
print(f(1))
";
    let found = deprecations(src);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`state(…) b`"), "{found:?}");
}

#[test]
fn a_file_directive_or_a_bare_ignore_silences_it_too() {
    let file = "// petal-lint-ignore-file state-key-deprecated\nstate(1) a = 1\nprint(a)\n";
    assert_eq!(deprecations(file), Vec::<String>::new());
    let bare = "// petal-lint-ignore\nstate(1) a = 1\nprint(a)\n";
    assert_eq!(deprecations(bare), Vec::<String>::new());
    // Naming another rule does not.
    let other = "// petal-lint-ignore prefer-let\nstate(1) a = 1\nprint(a)\n";
    assert_eq!(deprecations(other).len(), 1);
}

#[test]
fn an_imported_module_is_warned_about_where_it_is_written() {
    let mut env = Env::new();
    env.register_module(
        "counter",
        "pub fn bump(id)\n  state(id) n = 0\n  n += 1\n  n\nend\n",
    );
    let pid = env
        .load_program("import counter\nprint(counter.bump(1))\n")
        .expect("should compile");
    let program = env.get_program(pid).unwrap();
    let d = program
        .warnings
        .iter()
        .find(|d| d.message.contains("state-key-deprecated"))
        .expect("the module's warning");
    assert_eq!(d.span.start.line, 2);
    assert_eq!(
        program.source_map.file_name_for_span(&d.span).as_deref(),
        Some("counter"),
    );
}

// ---------------------------------------------------------------------------
// `petal lint`
// ---------------------------------------------------------------------------

#[test]
fn lint_rewrites_a_never_written_keyed_state_to_memo() {
    let src = "\
fn width(text)
  state(text) w = len(text) * 8
  w
end
print(width(\"hi\"), width(\"hi\"))
";
    let out = lint(src);
    let [f] = out.findings.as_slice() else {
        panic!("one finding: {:?}", out.findings);
    };
    assert_eq!(
        (f.rule, f.line, f.column, f.fixable),
        (STATE_KEY_DEPRECATED, 2, 3, true)
    );
    let expected = src.replace("state(text)", "memo(text)");
    assert_eq!(out.output, expected);
    // A fixed point, and the same program.
    let again = lint(&expected);
    assert_eq!(again.findings.len(), 0, "{:?}", again.findings);
    assert_eq!(run(src), run(&expected));
    assert_eq!(deprecations(&expected), Vec::<String>::new());
}

#[test]
fn lint_reports_a_written_slot_without_a_fix() {
    let out = lint(HEALTH);
    let [f] = out.findings.as_slice() else {
        panic!("one finding: {:?}", out.findings);
    };
    assert!(!f.fixable);
    assert_eq!(out.output, HEALTH);
}

#[test]
fn every_kind_of_write_keeps_the_state() {
    let mut checked = 0;
    for write in [
        "n = 2",
        "n += 1",
        "n.x = 1",
        "n[0] = 1",
        "n ??= 3",
        "let g = fn() n = 5 end",
    ] {
        let src =
            format!("fn f(id)\n  state(id) n = [{{x: 0}}]\n  {write}\n  n\nend\nprint(f(1))\n");
        // Only sources that compile as written tell us anything.
        if Env::new().load_program(&src).is_err() {
            continue;
        }
        let out = lint(&src);
        assert_eq!(
            out.count(STATE_KEY_DEPRECATED),
            1,
            "{write}: {:?}",
            out.findings
        );
        assert_eq!(out.fixable(), 0, "{write}: {}", out.output);
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} of the writes compiled");
}

#[test]
fn lint_leaves_a_keyed_state_var_and_a_pub_one_unfixed() {
    let var = "fn f(id)\n  state(id) var c = 0\n  get c\nend\nprint(f(1))\n";
    let out = lint(var);
    assert_eq!((out.count(STATE_KEY_DEPRECATED), out.fixable()), (1, 0));
    let exported = "pub state(1) shared = 0\nprint(shared)\n";
    let out = lint(exported);
    assert_eq!((out.count(STATE_KEY_DEPRECATED), out.fixable()), (1, 0));
}

#[test]
fn lint_fixes_the_unwritten_ones_beside_a_written_one() {
    let src = "\
fn f(id)
  state(id) a = id * 2
  state(id) b = 0
  b += a
  b
end
print(f(1))
";
    let out = lint(src);
    assert_eq!((out.count(STATE_KEY_DEPRECATED), out.fixable()), (2, 1));
    assert!(out.output.contains("memo(id) a = id * 2"), "{}", out.output);
    assert!(out.output.contains("state(id) b = 0"), "{}", out.output);
}

#[test]
fn a_lint_ignore_silences_the_finding_and_its_fix() {
    let src = "\
fn width(text)
  // petal-lint-ignore state-key-deprecated
  state(text) w = len(text) * 8
  w
end
print(width(\"hi\"))
";
    let out = lint(src);
    assert_eq!(out.findings.len(), 0, "{:?}", out.findings);
    assert_eq!(out.output, src);
}

#[test]
fn the_rule_is_on_by_default() {
    let out = lint_source(HEALTH, &LintOptions::default()).expect("lint");
    assert_eq!(out.count(STATE_KEY_DEPRECATED), 1, "{:?}", out.findings);
    assert!(petal::lint::is_rule(STATE_KEY_DEPRECATED));
}
