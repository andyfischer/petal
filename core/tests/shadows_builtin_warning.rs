//! A `fn` that takes the place of a builtin, a prelude function or a
//! function of a bundled package the file imports whole is a compile warning
//! carrying the rule id `shadows-builtin`, the same finding `petal lint`
//! reports (core/src/lint/builtin_names.rs), silenced by the same
//! `// petal-lint-ignore` comment. `petal check` prints it.

use petal::env::Env;

/// Compile `src` and return its `shadows-builtin` warnings, each with the
/// line and column its caret starts at.
fn shadows(src: &str) -> Vec<(u32, u32, String)> {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("should compile");
    env.get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .filter(|d| d.message.contains("[shadows-builtin]"))
        .map(|d| (d.span.start.line, d.span.start.column, d.message.clone()))
        .collect()
}

#[test]
fn a_fn_named_for_a_core_builtin_is_a_warning() {
    let found = shadows("fn commas(n)\n  str(n)\nend\nprint(commas(1000))\n");
    let [(line, _, message)] = found.as_slice() else {
        panic!("one warning expected, got {found:?}");
    };
    assert_eq!(*line, 1);
    assert!(message.contains("the builtin `commas`"), "{message}");
}

#[test]
fn a_fn_named_for_a_prelude_function_is_a_warning() {
    let found = shadows("fn plural(n, word)\n  word\nend\nprint(plural(2, \"cat\"))\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].2.contains("`plural`"), "{found:?}");
}

#[test]
fn a_fn_named_for_an_imported_package_function_is_a_warning() {
    let found = shadows("import color\n\nfn hsv(h, s, v)\n  nil\nend\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].2.contains("`color`'s `hsv`"), "{found:?}");
    // The caret is on the declaration's line, not the import's.
    assert_eq!(found[0].0, 3);
}

#[test]
fn a_package_function_is_not_shadowed_without_its_import() {
    assert!(shadows("fn hsv(h, s, v)\n  nil\nend\n").is_empty());
}

#[test]
fn an_overload_with_another_argument_count_is_not_a_warning() {
    assert!(shadows("fn len(a, b, c)\n  0\nend\nprint(len(1, 2, 3))\n").is_empty());
}

#[test]
fn a_lint_ignore_comment_silences_it() {
    let src = "// petal-lint-ignore shadows-builtin -- on purpose\nfn sum(xs)\n  0\nend\n";
    assert!(shadows(src).is_empty());
    let file = "// petal-lint-ignore-file shadows-builtin\nfn sum(xs)\n  0\nend\n";
    assert!(shadows(file).is_empty());
}

#[test]
fn a_nested_fn_is_reported_too() {
    let found = shadows("fn outer()\n  fn plural(n, w) w end\n  plural(1, \"x\")\nend\nprint(outer())\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].0, found[0].1), (2, 3));
}

#[test]
fn an_imported_module_may_export_its_own_version() {
    // `pub fn clamp` in a library is its design; the importer is warned
    // where a call reaches it (module.rs). A private one still warns.
    let mut env = Env::new();
    env.register_module("tween", "pub fn clamp(a)\n  a\nend\nfn sum(xs)\n  0\nend\npub fn go()\n  sum([])\nend\n");
    let pid = env.load_program("import tween: go\nprint(go())\n").expect("should compile");
    let found: Vec<String> = env
        .get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .filter(|d| d.message.contains("[shadows-builtin]"))
        .map(|d| d.message.clone())
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("`fn sum`"), "{found:?}");
}
