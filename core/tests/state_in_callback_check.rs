//! `petal check` reports `state` that a higher-order builtin's callback is
//! sure to reach (core/src/compiler/callback_state.rs): an error carrying the
//! rule id `state-in-callback`, at the declaration, citing the builtin call.
//! Only what can be proved is reported; a callback that arrives through a
//! parameter or a `var` is left to the runtime error
//! (docs/proposals/state-in-callbacks.md).

use petal::diagnostic::Diagnostic;
use petal::env::Env;

const RULE: &str = "[state-in-callback]";

/// Compile `src` and return its state-in-callback findings.
fn findings_in(env: &mut Env, src: &str) -> Vec<Diagnostic> {
    let pid = env.load_program(src).expect("should compile");
    env.get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .filter(|d| d.message.contains(RULE))
        .cloned()
        .collect()
}

fn findings(src: &str) -> Vec<Diagnostic> {
    findings_in(&mut Env::new(), src)
}

/// The one finding of `src`, which must be an error.
fn one(src: &str) -> Diagnostic {
    let found = findings(src);
    let [d] = found.as_slice() else {
        panic!("one finding for:\n{src}\ngot {found:?}");
    };
    assert!(d.is_error(), "{d:?}");
    d.clone()
}

fn none(src: &str) {
    let found = findings(src);
    assert!(found.is_empty(), "no finding for:\n{src}\ngot {found:?}");
}

const SEEN: &str = "\
fn seen(x)
    state n = 0
    n += 1
    n
end
";

#[test]
fn a_named_function_passed_to_each_builtin_is_reported() {
    for (call, builtin) in [
        ("map([1, 2], seen)", "map"),
        ("filter([1, 2], seen)", "filter"),
        ("reduce([1, 2], 0, seen2)", "reduce"),
        ("forEach([1, 2], seen)", "forEach"),
        ("sort_by([1, 2], seen)", "sort_by"),
        ("sort([1, 2], seen2)", "sort"),
        ("[1, 2].map(seen)", "map"),
        ("[1, 2].reduce(0, seen2)", "reduce"),
    ] {
        let src = format!("{SEEN}fn seen2(a, b)\n    state m = 0\n    m\nend\nprint({call})\n");
        let d = one(&src);
        assert!(d.message.starts_with("`state "), "{call}: {}", d.message);
        assert!(
            d.message
                .contains(&format!("cannot be used inside a callback of `{builtin}`")),
            "{call}: {}",
            d.message
        );
    }
}

#[test]
fn the_error_is_at_the_declaration_and_cites_the_call() {
    let src = format!("{SEEN}print(map([10, 20, 30], seen))\n");
    let d = one(&src);
    assert_eq!((d.span.start.line, d.span.start.column), (2, 5));
    assert_eq!(d.span.end.line, 2);
    assert!(
        d.message.starts_with(
            "`state n` cannot be used inside a callback of `map`: `map` runs `seen` on line 6, once per element."
        ),
        "{}",
        d.message
    );
    for help in [
        "use a loop: `for x in xs do f(x) end`",
        "reduce's accumulator",
    ] {
        assert!(d.message.contains(help), "{}", d.message);
    }
    assert!(d.message.ends_with(RULE), "{}", d.message);
}

#[test]
fn a_stateful_comparator_is_reported_but_sort_without_one_is_not() {
    none("fn by(a, b)\n    state n = 0\n    a < b\nend\nprint(sort([2, 1]))\n");
    let d = one("fn by(a, b)\n    state n = 0\n    a < b\nend\nprint(sort([2, 1], by))\n");
    assert!(d.message.contains("once per comparison"), "{}", d.message);
}

#[test]
fn a_named_argument_is_the_callback_too() {
    one(&format!("{SEEN}print(map([1], f: seen))\n"));
}

#[test]
fn a_lambda_that_declares_state_is_reported() {
    let d = one("print(map([1, 2], fn(x)\n    state n = 0\n    n + x\nend))\n");
    assert!(
        d.message
            .contains("`map` runs the callback on line 1, once per element."),
        "{}",
        d.message
    );
}

#[test]
fn a_lambda_that_calls_a_stateful_helper_is_reported() {
    let d = one(&format!(
        "{SEEN}print(filter([1, 2], fn(x) seen(x) > 1 end))\n"
    ));
    assert!(
        d.message.contains("reaches it through `seen`"),
        "{}",
        d.message
    );
}

#[test]
fn state_reached_through_named_calls_is_reported() {
    let d = one(&format!(
        "{SEEN}fn middle(x) seen(x) end\nfn outer(x) middle(x) end\nprint(map([1], outer))\n"
    ));
    assert!(
        d.message
            .contains("`map` runs `outer` on line 8, once per element, which reaches it through `middle` → `seen`."),
        "{}",
        d.message
    );
}

#[test]
fn a_function_declared_after_its_use_is_followed() {
    // Its name is read through the hoisting cell.
    one(&format!("fn use_it() map([1], seen) end\n{SEEN}"));
    one(&format!(
        "fn use_it() map([1], fn(x) seen(x) end) end\n{SEEN}"
    ));
}

#[test]
fn nested_lambdas_and_functions_are_followed() {
    // A local lambda called by the callback.
    one(
        "print(map([1], fn(x)\n    let g = fn()\n        state k = 0\n        k\n    end\n    g()\nend))\n",
    );
    // A nested `fn` passed by name.
    one(
        "fn rows(xs)\n    fn row(x)\n        state k = 0\n        k\n    end\n    map(xs, row)\nend\nprint(rows([1]))\n",
    );
    // A `map` inside a callback: the inner callback is the one reported.
    let d = one(&format!(
        "{SEEN}print(map([[1]], fn(xs) map(xs, seen) end))\n"
    ));
    assert!(d.message.contains("`map` runs `seen`"), "{}", d.message);
}

#[test]
fn state_in_a_loop_inside_the_callback_is_reported() {
    one(
        "print(map([1], fn(x)\n    for i in [1, 2] do\n        state n = 0\n    end\n    x\nend))\n",
    );
}

#[test]
fn state_var_is_reported() {
    let d =
        one("fn tally(x)\n    state var n = 0\n    set n += x\n    n\nend\nforEach([1], tally)\n");
    assert!(
        d.message.starts_with("`state var n` cannot be used"),
        "{}",
        d.message
    );
}

#[test]
fn the_std_helpers_that_go_through_a_builtin_are_reported_at_the_users_line() {
    for (call, called, builtin) in [
        ("count([1, 2], seen)", "count", "filter"),
        ("min_by([1, 2], seen)", "min_by", "map"),
        ("max_by([1, 2], seen)", "max_by", "map"),
        ("group_by([1, 2], seen)", "group_by", "map"),
        ("flat_map([1, 2], fn(x) [seen(x)] end)", "flat_map", "map"),
    ] {
        let d = one(&format!("{SEEN}print({call})\n"));
        assert!(
            d.message.contains(&format!(
                "a callback of `{builtin}`: `{called}` runs, through `{builtin}`, "
            )),
            "{call}: {}",
            d.message
        );
        assert!(d.message.contains("on line 6"), "{call}: {}", d.message);
    }
}

#[test]
fn a_callback_through_an_optional_parameter_is_left_to_the_runtime() {
    // `top_k`'s `key` defaults to nil, so the name is a join of the argument
    // and the default by the time it reaches `map`.
    none(&format!("{SEEN}print(top_k([1, 2], 1, seen))\n"));
}

#[test]
fn a_users_function_that_hands_its_parameter_to_a_builtin_is_followed() {
    one(&format!(
        "{SEEN}fn each(xs, f) map(xs, f) end\nprint(each([1], seen))\n"
    ));
    one(&format!(
        "{SEEN}fn each(xs, f) map(xs, f) end\nfn each2(xs, g) each(xs, g) end\nprint(each2([1], seen))\n"
    ));
}

#[test]
fn memo_is_not_state_here() {
    none("print(map([1, 2], fn(x)\n    memo(x) sq = x * x\n    sq\nend))\n");
}

#[test]
fn top_level_state_used_from_a_callback_is_not_reported() {
    none("state total = 0\nprint(map([1, 2], fn(x) x + total end))\n");
    none("state var score = 0\nforEach([1, 2], fn(x) set score += x end)\nprint(score)\n");
}

#[test]
fn a_callback_that_arrives_through_a_parameter_or_var_is_not_reported() {
    // The builtin's callback is a parameter nothing stateful is passed to.
    none(&format!(
        "{SEEN}fn each(xs, f) map(xs, f) end\nprint(each([1], fn(x) x end))\nprint(seen(1))\n"
    ));
    // A stateful function called through a parameter of a user function.
    none(&format!(
        "{SEEN}fn apply(f, x) f(x) end\nprint(map([1], fn(x) apply(seen, x) end))\n"
    ));
    // Through a `var`.
    none(&format!("{SEEN}var f = seen\nprint(map([1], f))\n"));
}

#[test]
fn a_function_the_callback_only_builds_is_not_reported() {
    none("print(map([1], fn(x)\n    fn()\n        state k = 0\n        k\n    end\nend))\n");
}

#[test]
fn a_shadowed_builtin_is_not_reported() {
    none(&format!(
        "{SEEN}fn map(xs, f) f(xs[0]) end\nprint(map([1], seen))\n"
    ));
    none(&format!(
        "{SEEN}let filter = fn(xs, f) xs end\nprint(filter([1], seen))\n"
    ));
    none(&format!(
        "{SEEN}fn go(forEach) forEach([1], seen) end\nprint(go(fn(xs, f) nil end))\n"
    ));
}

#[test]
fn the_method_form_is_not_reported_when_something_else_can_take_the_call() {
    // A record field of the name is called first.
    none(&format!(
        "{SEEN}let box = {{map: fn(f) f(1) end}}\nprint(box.map(seen))\nprint([1].map(seen))\n"
    ));
    // So is a user method.
    none(&format!(
        "{SEEN}class Bag\n    items\nend\nfn Bag.map(f) map(self.items, fn(x) x end) end\nprint([1].map(seen))\n"
    ));
}

#[test]
fn state_in_another_module_is_reported_at_the_callback() {
    let mut env = Env::new();
    env.register_module(
        "lib",
        "pub fn widget(x)\n    state hovered = false\n    hovered\nend\n",
    );
    let found = findings_in(&mut env, "import lib\nprint(map([1, 2], lib.widget))\n");
    let [d] = found.as_slice() else {
        panic!("{found:?}")
    };
    assert!(d.is_error());
    assert_eq!(d.span.file, petal::source_map::ENTRY_FILE);
    assert_eq!((d.span.start.line, d.span.start.column), (2, 19));
    assert!(
        d.message.starts_with(
            "`state hovered` cannot be used inside a callback of `map` (declared in lib): `map` runs `widget` here, once per element."
        ),
        "{}",
        d.message
    );
}

#[test]
fn it_fails_the_check() {
    let mut env = Env::new();
    env.set_check_only(true);
    let pid = env
        .load_program(&format!("{SEEN}print(map([1], seen))\n"))
        .unwrap();
    let report = env.check_program(pid);
    assert!(!report.passed());
    assert_eq!(report.error_count(), 1, "{:?}", report.diagnostics);
}

#[test]
fn the_lsp_shows_it() {
    use petal::lsp::document::analyze;
    use petal::lsp::lsp_types::DiagnosticSeverity;

    let analysis = analyze(&format!("{SEEN}print(map([1], seen))\n"));
    let [d] = analysis.diagnostics.as_slice() else {
        panic!("one diagnostic: {:?}", analysis.diagnostics);
    };
    assert!(d.message.contains(RULE), "{}", d.message);
    assert!(matches!(d.severity, Some(DiagnosticSeverity::Error)));
    assert_eq!(d.range.start.line, 1);
}
