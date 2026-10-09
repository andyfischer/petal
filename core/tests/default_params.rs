//! Default parameter values — `fn f(a, b = expr)`.
//!
//! A default is an *expression in the callee*, not a value stored with the
//! function: it runs on every call that leaves the argument out, after the
//! parameters before it are bound, and never otherwise. Most of this file is
//! that sentence, pinned from each side — then how defaults meet named
//! arguments, overload selection, methods, memoized calls and the tools.
//! See docs/language-guide.md (Default Parameter Values).

use petal::ast::{ExprKind, Stmt, StmtKind};
use petal::env::Env;
use petal::lexer::Lexer;
use petal::parse::Parser;

fn try_parse(src: &str) -> Result<Vec<Stmt>, String> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize()?;
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    parser.parse_program()
}

fn parse_err(src: &str) -> String {
    match try_parse(src) {
        Ok(_) => panic!("expected a parse error for {src:?}, but it parsed"),
        Err(e) => e,
    }
}

fn run(src: &str) -> Result<String, String> {
    let mut env = Env::new();
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

fn out(src: &str) -> String {
    run(src).unwrap_or_else(|e| panic!("run failed for {src:?}: {e}"))
}

/// The message `src` fails with — its first line, without the trailing
/// `[line N, column M]` and the source excerpt under it.
fn err(src: &str) -> String {
    let e = match run(src) {
        Ok(o) => panic!("expected an error for {src:?}, got output {o:?}"),
        Err(e) => e,
    };
    let first = e.lines().next().unwrap_or_default();
    match first.rfind(" [line ") {
        Some(at) => first[..at].to_string(),
        None => first.to_string(),
    }
}

/// Every diagnostic the compile attaches to `src`, as `(is_error, message)`.
fn diagnostics(src: &str) -> Vec<(bool, String)> {
    let mut env = Env::new();
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("compile failed for {src:?}: {e}"));
    env.get_program(pid)
        .unwrap()
        .warnings
        .iter()
        .map(|d| (d.is_error(), d.message.clone()))
        .collect()
}

fn errors(src: &str) -> Vec<String> {
    diagnostics(src)
        .into_iter()
        .filter(|(is_error, _)| *is_error)
        .map(|(_, m)| m)
        .collect()
}

// ── syntax ──────────────────────────────────────────────────────────────

#[test]
fn a_default_is_parsed_onto_its_parameter() {
    let stmts = try_parse("fn f(a, b = 1, c: num = a + b)\n  a\nend\n").unwrap();
    let StmtKind::FnDecl { params, .. } = &stmts[0].kind else {
        panic!("expected a fn declaration");
    };
    assert!(params[0].default.is_none() && !params[0].has_default());
    assert!(matches!(
        params[1].default.as_ref().unwrap().kind,
        ExprKind::Literal(_)
    ));
    // The annotation comes before the default, as on a `let`.
    assert_eq!(params[2].ty.as_ref().unwrap().name, "num");
    assert!(matches!(
        params[2].default.as_ref().unwrap().kind,
        ExprKind::BinaryOp { .. }
    ));
    assert_eq!(petal::ast::required_param_count(params), 1);
}

#[test]
fn a_required_parameter_cannot_follow_a_defaulted_one() {
    let e = parse_err("fn f(a = 1, b)\n  a\nend\n");
    assert!(
        e.contains("Parameter 'b' has no default value but follows 'a'"),
        "{e}"
    );
    assert!(e.contains("line 1"), "{e}");
    // The same rule for a lambda.
    let e = parse_err("let g = fn(a = 1, b) -> a\n");
    assert!(e.contains("Parameter 'b' has no default value"), "{e}");
}

#[test]
fn an_enum_variant_field_takes_no_default() {
    let e = parse_err("enum Shape\n  Circle(r = 1)\nend\n");
    assert!(e.contains("cannot have default values"), "{e}");
}

#[test]
fn the_ast_dump_shows_the_default() {
    let stmts = try_parse("fn f(a, b: num = a * 2)\n  a\nend\n").unwrap();
    let text = petal::ast_display::display_stmts(&stmts);
    assert!(text.contains("FnDecl f (a, b: num = …)"), "{text}");
    assert!(text.contains("Default b"), "{text}");
    let json = serde_json::to_string(&stmts).unwrap();
    assert!(json.contains("\"default\":{"), "{json}");
    // A parameter without one serializes exactly as it always has.
    let plain = serde_json::to_string(&try_parse("fn f(a)\n  a\nend\n").unwrap()).unwrap();
    assert!(!plain.contains("default"), "{plain}");
}

// ── evaluation: on every call that omits the argument ───────────────────

#[test]
fn trailing_defaults_may_be_omitted() {
    let f = "fn f(a, b = 10, c = 100)\n  a + b + c\nend\n";
    assert_eq!(out(&format!("{f}print(f(1))")), "111");
    assert_eq!(out(&format!("{f}print(f(1, 2))")), "103");
    assert_eq!(out(&format!("{f}print(f(1, 2, 3))")), "6");
}

#[test]
fn a_default_is_a_fresh_value_on_every_call() {
    // The classic trap in languages that evaluate a default once: the list is
    // built per call here, so the two calls cannot see each other's push.
    let src = "fn collect(x, into = [])\n  push(into, x)\nend\n\
               print(collect(1))\nprint(collect(2))\nprint(collect(3, [0]))";
    assert_eq!(out(src), "[1]\n[2]\n[0, 3]");
}

#[test]
fn a_default_runs_once_per_call_that_omits_it_and_not_otherwise() {
    let src = "var calls = 0\n\
               fn next()\n  set calls = get calls + 1\n  get calls\nend\n\
               fn f(a, n = next())\n  n\nend\n\
               print(f(0))\nprint(f(0))\nprint(f(0, 50))\nprint(f(0, n: 60))\nprint(f(0))\n\
               print(get calls)";
    // Three calls leaned on the default; the two that passed `n` ran nothing.
    assert_eq!(out(src), "1\n2\n50\n60\n3\n3");
}

#[test]
fn a_default_is_not_evaluated_at_declaration() {
    let src = "var calls = 0\n\
               fn next()\n  set calls = get calls + 1\n  get calls\nend\n\
               fn f(n = next())\n  n\nend\n\
               print(get calls)\nprint(f(7))\nprint(get calls)";
    assert_eq!(out(src), "0\n7\n0");
}

#[test]
fn a_default_sees_the_parameters_before_it() {
    let src = "fn span(lo, hi = lo + 10, mid = (lo + hi) / 2)\n  [lo, hi, mid]\nend\n\
               print(span(0))\nprint(span(0, 4))\nprint(span(2, mid: 0))";
    assert_eq!(out(src), "[0, 10, 5]\n[0, 4, 2]\n[2, 12, 0]");
}

#[test]
fn defaults_run_left_to_right() {
    let src = "var log = []\n\
               fn note(tag)\n  set log = push(get log, tag)\n  tag\nend\n\
               fn f(a = note(\"a\"), b = note(\"b\"), c = note(\"c\"))\n  0\nend\n\
               f()\nf(b: 1)\nprint(get log)";
    assert_eq!(out(src), "[\"a\", \"b\", \"c\", \"a\", \"c\"]");
}

#[test]
fn a_default_captures_what_the_body_could() {
    // A top-level binding, read at call time through the closure like any
    // other capture.
    let src = "let base = 100\n\
               fn f(x, k = base + 1)\n  x + k\nend\n\
               print(f(1))\nprint(f(1, 0))";
    assert_eq!(out(src), "102\n1");
    // A local of an enclosing function, and a default that is itself a call
    // to a function declared further down the file.
    let src = "fn make(step)\n  let add = fn(x, by = step * later()) -> x + by\n  add\nend\n\
               fn later()\n  10\nend\n\
               let add = make(3)\nprint(add(1))\nprint(add(1, 1))";
    assert_eq!(out(src), "31\n2");
}

#[test]
fn an_explicit_nil_is_an_argument_not_an_omission() {
    let src = "fn f(a = 5)\n  a\nend\nprint(f(nil))\nprint(f())";
    assert_eq!(out(src), "nil\n5");
}

#[test]
fn a_function_of_only_defaults_and_no_body_returns_nil() {
    assert_eq!(out("fn f(a = 1)\nend\nprint(f())"), "nil");
}

#[test]
fn a_default_may_not_read_a_later_parameter_or_itself() {
    let e = err("fn f(a = b, b = 1)\n  a\nend\nprint(f())");
    assert!(
        e.contains("The default value of parameter 'a' refers to 'b', which is declared after it"),
        "{e}"
    );
    let e = err("fn f(a = a)\n  a\nend\nprint(f())");
    assert!(e.contains("refers to 'a' itself"), "{e}");
    // A lambda's own parameter of that name is a different binding.
    let src = "fn f(g = fn(b) -> b + 1, b = 1)\n  g(b)\nend\nprint(f())";
    assert_eq!(out(src), "2");
    // ...but only that name: the lambda's body is still held to the rule for
    // every other later parameter.
    let e = err("fn f(g = fn(b) -> b + c, b = 1, c = 2)\n  g(b)\nend\nprint(f())");
    assert!(
        e.contains("The default value of parameter 'g' refers to 'c', which is declared after it"),
        "{e}"
    );
}

// ── named arguments ─────────────────────────────────────────────────────

#[test]
fn naming_a_later_parameter_skips_the_ones_between() {
    let f = "fn f(a, b = \"b\", c = \"c\", d = \"d\")\n  \"{a}{b}{c}{d}\"\nend\n";
    assert_eq!(out(&format!("{f}print(f(1, d: 4))")), "1bc4");
    assert_eq!(out(&format!("{f}print(f(1, c: 3))")), "1b3d");
    assert_eq!(out(&format!("{f}print(f(d: 4, a: 1))")), "1bc4");
    assert_eq!(out(&format!("{f}print(f(1, 2, d: 4))")), "12c4");
}

#[test]
fn a_missing_required_parameter_is_named() {
    let f = "fn f(a, b, c = 3)\n  a + b + c\nend\n";
    assert_eq!(
        err(&format!("{f}print(f(1))")),
        "f() is missing a value for parameter 'b'"
    );
    assert_eq!(
        err(&format!("{f}print(f(c: 1, b: 2))")),
        "f() is missing a value for parameter 'a'"
    );
    assert_eq!(
        err(&format!("{f}print(f(1, 2, 3, 4))")),
        "f() expects 2-3 arguments, got 4"
    );
    assert_eq!(
        err(&format!("{f}print(f(1, 2, nope: 4))")),
        "f() has no parameter named 'nope'"
    );
    assert_eq!(
        err(&format!("{f}print(f(1, 2, 3, c: 4))")),
        "f() expects 2-3 arguments, got 4"
    );
    assert_eq!(
        err(&format!("{f}print(f(1, 2, b: 4))")),
        "f() got multiple values for parameter 'b'"
    );
}

#[test]
fn a_function_without_defaults_reports_arity_as_it_always_has() {
    assert_eq!(
        err("fn g(a, b)\n  a\nend\nprint(g(1))"),
        "g() expects 2 arguments, got 1"
    );
}

// ── lambdas, callbacks, methods ─────────────────────────────────────────

#[test]
fn lambdas_take_defaults_in_both_forms() {
    assert_eq!(
        out("let add = fn(a, b = 10) -> a + b\nprint(add(1))\nprint(add(1, 2))"),
        "11\n3"
    );
    assert_eq!(
        out(
            "let add = fn(a, b = 10)\n  let s = a + b\n  s * 2\nend\nprint(add(1))\nprint(add(1, b: 2))"
        ),
        "22\n6"
    );
    // Invoked in place.
    assert_eq!(out("print((fn(a = 4) -> a * a)())"), "16");
}

#[test]
fn a_callback_may_leave_its_trailing_parameters_to_their_defaults() {
    assert_eq!(
        out("print(map([1, 2, 3], fn(x, scale = 10) -> x * scale))"),
        "[10, 20, 30]"
    );
}

#[test]
fn methods_take_defaults_after_the_receiver() {
    let src = "class Counter\n  n: int\nend\n\
               fn Counter.bump(c, by = 1, scale = by * 10)\n  c.n + by + scale\nend\n\
               let c = Counter(5)\n\
               print(c.bump())\nprint(c.bump(2))\nprint(c.bump(scale: 0))\nprint(c.bump(2, 3))";
    assert_eq!(out(src), "16\n27\n6\n10");
    // The written-argument count excludes the receiver, in both bounds.
    let e = err("class C\n  n: int\nend\nfn C.m(c, a, b = 1)\n  a\nend\nC(1).m(1, 2, 3)");
    assert_eq!(e, "C.m() expects 1-2 arguments, got 3");
}

#[test]
fn recursion_through_a_default() {
    let src = "fn countdown(n, acc = [])\n  if n == 0 then\n    acc\n  else\n    countdown(n - 1, push(acc, n))\n  end\nend\n\
               print(countdown(3))\nprint(countdown(2))";
    assert_eq!(out(src), "[3, 2, 1]\n[2, 1]");
}

// ── overloads ───────────────────────────────────────────────────────────

/// One exact arity, one variant whose defaults stretch below its own.
const BOX: &str = "fn box(w)\n  \"box1 {w}\"\nend\n\
                   fn box(w, h, depth = 1)\n  \"box3 {w} {h} {depth}\"\nend\n";

#[test]
fn an_exact_arity_match_wins_as_it_always_has() {
    assert_eq!(out(&format!("{BOX}print(box(1))")), "box1 1");
    assert_eq!(out(&format!("{BOX}print(box(1, 2, 3))")), "box3 1 2 3");
}

#[test]
fn a_count_no_variant_has_goes_to_the_one_whose_defaults_cover_it() {
    assert_eq!(out(&format!("{BOX}print(box(1, 2))")), "box3 1 2 1");
    assert_eq!(
        out(&format!("{BOX}print(box(1, 2, depth: 9))")),
        "box3 1 2 9"
    );
    assert_eq!(
        err(&format!("{BOX}print(box())")),
        "box() expects 1 or 2-3 arguments, got 0"
    );
    assert_eq!(
        err(&format!("{BOX}print(box(1, 2, 3, 4))")),
        "box() expects 1 or 2-3 arguments, got 4"
    );
}

#[test]
fn written_names_take_part_in_overload_selection() {
    // Two arguments either way; only the names tell the variants apart.
    let src = "fn at(x, y)\n  \"xy {x} {y}\"\nend\n\
               fn at(x, y, z = 0)\n  \"xyz {x} {y} {z}\"\nend\n\
               fn at(angle, radius, turns = 1, phase = 0)\n  \"polar {angle} {radius} {turns} {phase}\"\nend\n";
    // Exactly two, all positional: the two-parameter variant, as before.
    assert_eq!(out(&format!("{src}print(at(1, 2))")), "xy 1 2");
    // Three written, one of them `phase`: the exact-arity `at/3` has no such
    // parameter, so it is out; `at/4` is the only variant left.
    assert_eq!(
        out(&format!("{src}print(at(1, 2, phase: 9))")),
        "polar 1 2 1 9"
    );
    // A name only `at/3` has. The exact-arity variant for two arguments is
    // `at/2`, which does not accept it.
    assert_eq!(out(&format!("{src}print(at(1, y: 2, z: 3))")), "xyz 1 2 3");
    assert_eq!(
        out(&format!("{src}print(at(radius: 2, angle: 1))")),
        "polar 1 2 1 0"
    );
}

#[test]
fn a_call_two_variants_could_take_is_ambiguous_not_guessed() {
    let src = "fn pad(s, left = 1)\n  1\nend\nfn pad(s, left = 1, right = 1)\n  2\nend\n";
    let e = err(&format!("{src}print(pad(\"x\"))"));
    assert_eq!(
        e,
        "pad() is ambiguous: pad(s, left = …) and pad(s, left = …, right = …) both accept \
         this call — pass or name another argument to pick one"
    );
    // Passing or naming more resolves it.
    assert_eq!(out(&format!("{src}print(pad(\"x\", 2))")), "1");
    assert_eq!(out(&format!("{src}print(pad(\"x\", right: 2))")), "2");
    assert_eq!(out(&format!("{src}print(pad(\"x\", 2, 3))")), "2");
    // The checker reports the same call without running it.
    let errs = errors(&format!("{src}fn never()\n  pad(\"x\")\nend\n"));
    assert!(
        errs.iter().any(|m| m.contains("pad() is ambiguous")),
        "{errs:?}"
    );
}

#[test]
fn a_name_that_skips_a_parameter_outranks_the_exact_count() {
    // Two shapes that meet at four arguments ending in `width`.
    let src = "fn ring(center, radius, a = 255, width = 1)\n  \"centre {center} {radius} {a} {width}\"\nend\n\
               fn ring(cx, cy, radius, a = 255, width = 1)\n  \"flat {cx} {cy} {radius} {a} {width}\"\nend\n";
    // In the four-parameter variant `width` sits where a fourth positional
    // argument would have gone; in the five-parameter one it skips `a`, which
    // no positional call can do. The name was written for that one.
    assert_eq!(
        out(&format!("{src}print(ring(1, 2, 3, width: 9))")),
        "flat 1 2 3 255 9"
    );
    // All positional, and names that skip nothing: the exact count, as ever.
    assert_eq!(out(&format!("{src}print(ring(1, 2, 3, 9))")), "centre 1 2 3 9");
    assert_eq!(
        out(&format!("{src}print(ring(1, 2, a: 3, width: 9))")),
        "centre 1 2 3 9"
    );
    let pad = "fn pad(s, left = 1)\n  1\nend\nfn pad(s, left = 1, right = 1)\n  2\nend\n";
    assert_eq!(out(&format!("{pad}print(pad(\"x\", left: 2))")), "1");
    // The checker selects the same variant: nothing to report.
    assert_eq!(
        errors(&format!("{src}print(ring(1, 2, 3, width: 9))")),
        Vec::<String>::new()
    );

    // Skipping into two variants picks neither.
    let two = "fn v(a, b, c, width = 1)\n  4\nend\n\
               fn v(a, b, c, d = 0, width = 1)\n  5\nend\n\
               fn v(a, b, c, d = 0, e = 0, width = 1)\n  6\nend\n";
    let e = err(&format!("{two}print(v(1, 2, 3, width: 9))"));
    assert!(e.starts_with("v() is ambiguous"), "{e}");
    let errs = errors(&format!("{two}fn never()\n  v(1, 2, 3, width: 9)\nend\n"));
    assert!(errs.iter().any(|m| m.contains("v() is ambiguous")), "{errs:?}");
}

#[test]
fn a_misnamed_argument_to_an_exact_arity_variant_still_says_which_name() {
    let e = err(&format!("{BOX}print(box(nope: 1))"));
    assert_eq!(e, "box() has no parameter named 'nope'");
    // No variant has this many parameters, and none has the name.
    let e = err(&format!("{BOX}print(box(1, nope: 2))"));
    assert!(
        e.contains("box() has no variant that accepts 2 arguments with one named 'nope'"),
        "{e}"
    );
    assert!(e.contains("box(w, h, depth = …)"), "{e}");
}

// ── the static checker ──────────────────────────────────────────────────

#[test]
fn the_checker_accepts_calls_that_lean_on_defaults() {
    let src = "fn f(a, b = 1, c = 2)\n  a\nend\n\
               f(1)\nf(1, 2)\nf(1, c: 3)\nf(c: 3, a: 1)\nf(1, 2, 3)\n";
    assert_eq!(diagnostics(src), vec![]);
}

#[test]
fn the_checker_reports_what_the_runtime_would() {
    let f = "fn f(a, b, c = 3)\n  a\nend\nfn never()\n";
    let one = |call: &str| errors(&format!("{f}  {call}\nend\n"));
    assert_eq!(one("f(1)"), ["f() is missing a value for parameter 'b'"]);
    assert_eq!(one("f(1, 2, 3, 4)"), ["`f` expects 2-3 arguments, got 4"]);
    assert_eq!(
        one("f(1, 2, d: 4)"),
        ["f() has no parameter named 'd' (parameters: 'a', 'b', 'c')"]
    );
    assert_eq!(
        one("f(b: 2, c: 3)"),
        ["f() is missing a value for parameter 'a'"]
    );
}

#[test]
fn a_default_is_checked_against_its_annotation() {
    let warn = diagnostics("fn f(a, b: str = 5)\n  a\nend\n");
    assert_eq!(
        warn,
        vec![(
            false,
            "default value for parameter `b`: expected `string`, found `int`".to_string()
        )]
    );
    assert_eq!(diagnostics("fn f(a, b: num = 5)\n  a\nend\n"), vec![]);
    // Inside a lambda too.
    let warn = diagnostics("let g = fn(a, b: int = \"x\") -> a\n");
    assert_eq!(warn.len(), 1, "{warn:?}");
}

#[test]
fn a_named_argument_is_type_checked_against_the_slot_it_fills() {
    // `b` is written first but fills the second slot.
    let warn = diagnostics("fn f(a: int, b: str = \"x\")\n  a\nend\nf(b: \"y\", a: 1)\n");
    assert_eq!(warn, vec![]);
    let warn = diagnostics("fn f(a: int, b: str = \"x\")\n  a\nend\nf(1, b: 2)\n");
    assert_eq!(warn.len(), 1, "{warn:?}");
    assert!(
        warn[0].1.contains("expected `string`, found `int`"),
        "{warn:?}"
    );
}

#[test]
fn the_checker_follows_a_method_with_defaults() {
    let src = "class C\n  n: int\nend\nfn C.m(c: C, a: int, b: int = 1) -> int\n  a + b\nend\n\
               let c = C(1)\nlet x: str = c.m(1)\n";
    // The call resolves (no arity error) and its declared return type flows.
    let d = diagnostics(src);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].1.contains("`x` declared `string`"), "{d:?}");
    let errs =
        errors("class C\n  n: int\nend\nfn C.m(c: C, a, b = 1)\n  a\nend\nlet c = C(1)\nc.m()\n");
    assert_eq!(errs, ["method `C.m` expects 1-2 arguments, got 0"]);
}

// ── state and memoized calls ────────────────────────────────────────────

/// Run `src` `times` times on one stack, as a host's frame loop does.
fn run_frames(src: &str, times: usize) -> (Vec<String>, petal::memo::MemoStats) {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for i in 0..times {
        if i > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
    }
    let stats = env.memo_stats(sid).unwrap();
    (env.take_output(), stats)
}

#[test]
fn state_in_a_function_with_defaults_stays_per_callsite() {
    let src = "fn counter(step = 1)\n  state n = 0\n  n = n + step\n  n\nend\n\
               print(counter())\nprint(counter(10))";
    let (output, _) = run_frames(src, 3);
    assert_eq!(output, ["1", "10", "2", "20", "3", "30"]);
}

/// Big enough to be worth a memo record (see `memo::MIN_SCOPE_INSTS`).
const WORK: &str = "fn work(x, k = 3)\n  let acc = 0\n  for i in range(0, 20) do acc = acc + x end\n  acc * k\nend\n";

#[test]
fn a_memoized_call_keeps_omitted_and_passed_arguments_apart() {
    // `work(1)` and `work(1, nil)` bind the same placeholder for `k`; only
    // the was-it-passed flag tells their records apart.
    let src = format!(
        "{WORK}fn safe(x, k = 3)\n  let acc = 0\n  for i in range(0, 20) do acc = acc + x end\n  [acc, k]\nend\n\
         print(work(1))\nprint(work(1, 5))\nprint(work(1, k: 7))\n\
         for pass in [1, 2] do\n  print(safe(1))\n  print(safe(1, nil))\nend"
    );
    let (output, stats) = run_frames(&src, 3);
    let frame = [
        "60",
        "100",
        "140",
        "[20, 3]",
        "[20, nil]",
        "[20, 3]",
        "[20, nil]",
    ];
    let expected: Vec<&str> = frame
        .iter()
        .cycle()
        .take(frame.len() * 3)
        .copied()
        .collect();
    assert_eq!(output, expected);
    assert!(
        stats.hits > 0,
        "later frames replay recorded calls: {stats:?}"
    );
}

#[test]
fn a_replayed_call_does_not_freeze_a_default_that_changed() {
    // The default reads a binding that is different on every frame; a record
    // from the frame before must not answer for this one.
    let src = "state tick = 0\ntick = tick + 1\n\
               fn work(x, k = tick * 100)\n  let acc = 0\n  for i in range(0, 20) do acc = acc + x end\n  acc + k\nend\n\
               print(work(1))\nprint(work(1, 5))";
    let (output, _) = run_frames(src, 3);
    assert_eq!(output, ["120", "25", "220", "25", "320", "25"]);
}

// ── IR and bytecode shape ───────────────────────────────────────────────

#[test]
fn the_function_records_how_many_parameters_are_optional() {
    let mut env = Env::new();
    let pid = env
        .load_program("fn f(a, b = 1, c = 2)\n  a\nend\nfn g(a)\n  a\nend\n")
        .unwrap();
    let program = env.get_program(pid).unwrap();
    let by_name = |n: &str| {
        program
            .functions
            .iter()
            .find(|f| f.name.as_deref() == Some(n))
            .unwrap()
    };
    let f = by_name("f");
    assert_eq!(f.params, ["a", "b", "c"]);
    assert_eq!((f.optional_params, f.required_params()), (2, 1));
    // One was-it-passed flag per optional parameter, seated after the params.
    assert_eq!(
        program.get_block(f.body_block).param_names,
        ["a", "b", "c", "b#given", "c#given"]
    );
    // A function without defaults is exactly what it was.
    let g = by_name("g");
    assert_eq!(g.optional_params, 0);
    assert_eq!(program.get_block(g.body_block).param_names, ["a"]);
    let json = serde_json::to_value(g).unwrap();
    assert!(json.get("optional_params").is_none(), "{json}");
}

// ── tools ───────────────────────────────────────────────────────────────

#[test]
fn the_formatter_spaces_a_default_and_is_then_stable() {
    let src = "fn f(a,b=1,c:num   =   a*2)\na+b+c\nend\nlet g = fn(x,y=[1,2])->x\n";
    let formatted = petal::fmt::format_source(src).unwrap();
    assert_eq!(
        formatted,
        "fn f(a, b = 1, c: num = a * 2)\n  a + b + c\nend\nlet g = fn(x, y = [1, 2]) -> x\n"
    );
    assert_eq!(petal::fmt::format_source(&formatted).unwrap(), formatted);
    // Formatting changes nothing about what runs.
    let call = "print(f(1))\nprint(g(1))\n";
    assert_eq!(
        out(&format!("{src}{call}")),
        out(&format!("{formatted}{call}"))
    );
}

#[test]
fn suggest_annotates_a_defaulted_parameter_before_its_default() {
    use petal::suggest::{SuggestOptions, apply, suggest_source};
    // The default contains everything a character scan trips on: a comma, a
    // bracket and a quoted parenthesis.
    let src = "fn label(n, sep = join([\",\", \")\"], \"\"))\n  str(n) ++ sep\nend\nprint(label(1))\nprint(label(2))\n";
    let got = suggest_source(src, None, &SuggestOptions::default()).expect("suggest");
    let applied = apply(src, &got.suggestions);
    assert!(
        applied.starts_with("fn label(n: num, sep = join([\",\", \")\"], \"\"))"),
        "{applied}"
    );
    // Whatever was inserted, the default expression is untouched and the
    // program still compiles and runs the same.
    assert_eq!(out(&applied), out(src));
}
