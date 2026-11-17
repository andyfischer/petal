//! The `regex` package end to end: its `.ptl` tests run with the natives
//! registered (`petal test` has no way to register a host's natives yet),
//! its declarations agree with the registrations, and the errors a script
//! sees.

use std::path::PathBuf;

use petal::env::Env;

fn registered() -> Env {
    let mut env = Env::new();
    petal_regex::register(&mut env);
    env.set_echo(false);
    env
}

/// Run `source` to completion and return its runtime error, if any.
fn run(source: &str) -> Result<(), String> {
    let mut env = registered();
    let pid = env.load_program(source).map_err(|e| e.to_string())?;
    let stack = env.create_stack(pid).map_err(|e| e.to_string())?;
    env.run(stack).map(|_| ()).map_err(|e| e.to_string())
}

#[test]
fn the_package_tests_pass() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/regex.ptl");
    let source = std::fs::read_to_string(&path).unwrap();
    let mut env = registered();
    let pid = env
        .load_program_at(&source, &path)
        .unwrap_or_else(|e| panic!("{e}"));
    let stack = env.create_stack(pid).unwrap();
    env.run(stack).unwrap();
    let tests = env.tests(pid).to_vec();
    assert!(tests.len() >= 10, "found {} tests", tests.len());
    let mut failed = Vec::new();
    for t in &tests {
        let r = env.run_test(stack, &t.function);
        if !r.passed {
            failed.push(format!(
                "{} (line {}): {:?} {:?}",
                t.name, t.line, r.failures, r.error
            ));
        }
    }
    assert!(failed.is_empty(), "failing tests:\n{}", failed.join("\n"));
}

#[test]
fn the_declarations_agree_with_the_natives() {
    let env = registered();
    if let Err(mismatch) = env.verify_interface(&[petal_regex::PACKAGE_NAME]) {
        panic!("regex.ptl and register disagree: {mismatch}");
    }
}

#[test]
fn a_bad_pattern_names_the_native_the_pattern_and_the_problem() {
    let err = run("import regex\nprint(regex_find(\"abc\", \"a(b\"))").unwrap_err();
    assert!(
        err.contains("regex_find: bad pattern \"a(b\": unclosed group (at character 1)"),
        "{err}"
    );
}

#[test]
fn a_text_that_is_not_a_string_is_an_error() {
    let err = run("import regex\nprint(regex_test(42, \"a\"))").unwrap_err();
    assert!(
        err.contains("regex_test: `text` should be a string or nil, got int"),
        "{err}"
    );
}

#[test]
fn named_arguments_reach_the_natives() {
    run(
        "import regex\nassert_eq(regex_replace(text: \"aa\", pattern: \"a\", with: \"b\"), \"bb\")",
    )
    .unwrap();
}

#[test]
fn an_env_without_the_package_cannot_import_it() {
    let mut env = Env::new();
    assert!(
        env.load_program("import regex\nprint(regex_test(\"a\", \"a\"))")
            .is_err()
    );
}
