//! `test "name" do … end` blocks, the recorded expectations (`expect`,
//! `expect_eq`, `expect_near`), and the host API a test runner is built on
//! (`Env::tests`, `test_begin`, `test_resume`, `PetalCxt::suspend`).
//! docs/tasks/moses-debrief.md 2.2; docs/testing.md.

use std::cell::Cell;
use std::rc::Rc;

use petal::ast::StmtKind;
use petal::env::{Env, TestStep};
use petal::native_fn::{NativeEffects, PetalCxt};
use petal::program::ProgramId;
use petal::stack::StackKey;

fn quiet() -> Env {
    let mut env = Env::new();
    env.set_echo(false);
    env
}

fn loaded_in(mut env: Env, src: &str) -> (Env, ProgramId, StackKey) {
    let pid = env.load_program(src).expect("load");
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).expect("run");
    (env, pid, sid)
}

fn loaded(src: &str) -> (Env, ProgramId, StackKey) {
    loaded_in(quiet(), src)
}

fn parse(src: &str) -> Result<Vec<petal::ast::Stmt>, String> {
    petal::cst::parse_source(src, petal::source_map::ENTRY_FILE).map(|(_, stmts)| stmts)
}

// ---------------------------------------------------------------------------
// Syntax
// ---------------------------------------------------------------------------

#[test]
fn a_test_block_parses_to_a_function_of_no_parameters_carrying_its_title() {
    let stmts = parse(
        "fn f() 1 end\ntest \"first\" do\n  expect(f() == 1)\nend\ntest \"second #2\" do end\n",
    )
    .unwrap();
    assert_eq!(stmts.len(), 3);
    assert_eq!(stmts[0].test, None);
    assert_eq!(stmts[1].test.as_deref(), Some("first"));
    assert_eq!(stmts[2].test.as_deref(), Some("second #2"));
    for (stmt, name) in stmts[1..].iter().zip(["__test_0", "__test_1"]) {
        let StmtKind::FnDecl { name: n, params, class, ret, .. } = &stmt.kind else {
            panic!("a test block is carried as a FnDecl");
        };
        assert_eq!(n, name);
        assert!(params.is_empty() && class.is_none() && ret.is_none());
    }
    assert!(petal::ast::is_test_fn_name("__test_0"));
    assert!(petal::ast::is_test_fn_name("sim::__test_12"));
    assert!(!petal::ast::is_test_fn_name("__test_"));
    assert!(!petal::ast::is_test_fn_name("__test_x"));
}

#[test]
fn test_is_still_an_ordinary_name() {
    let (mut env, _, _) = loaded(
        "let test = 3\nfn tests(test) test + 1 end\nlet r = { test: test }\nprint(test, tests(test), r.test)\n",
    );
    assert_eq!(env.take_output(), ["3 4 3"]);
    // A string on the next line is not a title.
    let (mut env, _, _) = loaded("let test = 1\nprint(test)\n\"x\"\n");
    assert_eq!(env.take_output(), ["1"]);
}

#[test]
fn a_test_block_is_top_level_only_and_its_mistakes_are_named() {
    let nested = parse("fn f()\n  test \"inner\" do end\nend\n").unwrap_err();
    assert!(nested.contains("only allowed at the top level"), "{nested}");
    assert!(nested.contains("line 2"), "{nested}");
    let in_test = parse("test \"a\" do\n  test \"b\" do end\nend\n").unwrap_err();
    assert!(in_test.contains("only allowed at the top level"), "{in_test}");

    let twice = parse("test \"a\" do end\ntest \"a\" do end\n").unwrap_err();
    assert!(twice.contains("already has a test named \"a\""), "{twice}");
    assert!(twice.contains("line 2"), "{twice}");

    let no_do = parse("test \"a\"\n  expect(true)\nend\n").unwrap_err();
    assert!(no_do.contains("Expected `do` after the name of the test"), "{no_do}");
    let hole = parse("let n = 1\ntest \"a {n}\" do end\n").unwrap_err();
    assert!(hole.contains("plain string"), "{hole}");

    let unnamed = parse("test \"  \" do end\n").unwrap_err();
    assert!(unnamed.contains("a test needs a name"), "{unnamed}");

    let open = parse("test \"a\" do\n  expect(true)\n").unwrap_err();
    assert!(open.contains("the test \"a\" started at line 1"), "{open}");
}

#[test]
fn the_cst_keeps_a_test_block_whole_and_projects_the_same_ast() {
    let src = "// header\ntest \"a\" do   // why\n  expect(true)\nend\n\ntest \"b\" do end\n";
    let (tree, stmts) = petal::cst::parse_source(src, petal::source_map::ENTRY_FILE).unwrap();
    // Lossless.
    assert_eq!(tree.text(), src);
    // The statements handed back are the tree's projection; the parser's
    // own AST names the functions the same way.
    let projected =
        petal::cst_project::project(&petal::cst::SyntaxNode::new_root(tree.clone())).expect("project");
    let mut lexer = petal::lexer::Lexer::new(src);
    lexer.tokenize().unwrap();
    let direct = petal::parse::Parser::new(lexer.tokens.clone(), lexer.token_spans.clone())
        .parse_program()
        .unwrap();
    assert_eq!(
        serde_json::to_string(&projected).unwrap(),
        serde_json::to_string(&direct).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&projected).unwrap(),
        serde_json::to_string(&stmts).unwrap()
    );
    assert_eq!(projected[1].test.as_deref(), Some("b"));
}

#[test]
fn fmt_indents_a_test_body_and_is_stable() {
    let src = "test  \"a\"  do\n        expect( 1==1 )\n if true then\n  print(1)\n  end\nend\n";
    let once = petal::fmt::format_source(src).unwrap();
    assert_eq!(
        once,
        "test \"a\" do\n  expect(1 == 1)\n  if true then\n    print(1)\n  end\nend\n"
    );
    assert_eq!(petal::fmt::format_source(&once).unwrap(), once);
}

// ---------------------------------------------------------------------------
// The program's table, and an ordinary run
// ---------------------------------------------------------------------------

const SCORE: &str = "\
state var score = 0
fn hit(n)
  set score += n
  get score
end
print(\"top\")

test \"hit adds\" do
  print(\"in test\")
  expect_eq(hit(2), 2)
  expect_eq(hit(3), 5)
end

test \"fails\" do
  expect_eq(hit(1), 2, \"one hit\")
  expect(get score > 5)
  expect_near(0.5, 0.6, 0.01)
  expect_near(1, 1.0, 0)
  print(\"still here\")
end
";

#[test]
fn an_ordinary_run_does_not_run_a_test() {
    let (mut env, pid, sid) = loaded(SCORE);
    assert_eq!(env.take_output(), ["top"]);
    assert_eq!(env.expect_counts(sid), (0, 0));
    let tests = env.tests(pid);
    assert_eq!(tests.len(), 2);
    assert_eq!(tests[0].name, "hit adds");
    assert_eq!(tests[0].function, "__test_0");
    assert_eq!(tests[0].line, 8);
    assert_eq!(tests[0].module, None);
    assert_eq!(tests[1].name, "fails");
    assert_eq!(tests[1].line, 14);
    // The functions are there to be called.
    for t in tests.to_vec() {
        assert!(env.has_function(sid, &t.function), "{}", t.function);
    }
}

#[test]
fn a_test_records_every_failed_expectation_and_runs_to_its_end() {
    let (mut env, _, sid) = loaded(SCORE);
    env.take_output();

    let ok = env.run_test(sid, "__test_0");
    assert!(ok.passed, "{ok:?}");
    assert_eq!((ok.checks, ok.failed), (2, 0));
    assert_eq!(env.take_output(), ["in test"]);

    // The same stack: the first test's hits are still there (isolation is
    // the runner's choice of a stack per test).
    let bad = env.run_test(sid, "__test_1");
    assert!(!bad.passed);
    assert_eq!(bad.error, None);
    assert_eq!((bad.checks, bad.failed), (4, 2));
    assert_eq!(env.take_output(), ["still here"]);
    let f = &bad.failures[0];
    assert_eq!(f.check, "expect_eq");
    assert_eq!((f.line, f.column), (15, 3));
    assert_eq!(f.file, "<entry>");
    assert_eq!(f.actual.as_deref(), Some("6"));
    assert_eq!(f.expected.as_deref(), Some("2"));
    assert_eq!(f.note.as_deref(), Some("one hit"));
    assert_eq!(f.message, "expected 2, got 6 (one hit)");
    assert_eq!(f.render(), "<entry>:15: expected 2, got 6 (one hit)");
    let f = &bad.failures[1];
    assert_eq!(f.check, "expect_near");
    assert_eq!(f.line, 17);
    assert_eq!(f.tolerance.as_deref(), Some("0.01"));
    assert_eq!(f.message, "expected 0.6 within 0.01, got 0.5 (off by 0.1)");
    // Nothing of a test's is left in the context's own log.
    assert!(env.take_expect_failures(sid).is_empty());
    assert_eq!(env.expect_counts(sid), (0, 0));
}

#[test]
fn expectations_answer_whether_they_held_and_show_values_unambiguously() {
    let (mut env, _, sid) = loaded(
        "print(expect(true), expect(1), expect(\"\"), expect(nil), expect(false, \"why\"))\n\
         print(expect_eq([1, {a: 2}], [1, {a: 2}]), expect_eq(\"1\", 1))\n\
         print(expect_near(1, 1.25, 0.25), expect_near(\"x\", 1, 1), expect_near(1, 2, 0.5, \"far\"))\n",
    );
    assert_eq!(
        env.take_output(),
        ["true true true false false", "true false", "true false false"]
    );
    assert_eq!(env.expect_counts(sid), (10, 5));
    let fails = env.take_expect_failures(sid);
    let messages: Vec<&str> = fails.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "expected a true value",
            "why",
            "expected 1, got \"1\"",
            "expected 1 within 1, got \"x\" (not a number)",
            "expected 2 within 0.5, got 1 (off by 1) (far)",
        ]
    );
    assert_eq!(fails[0].actual.as_deref(), Some("nil"));
    assert_eq!(fails[0].line, 1);
    assert_eq!(fails[4].line, 3);
    // Taken once.
    assert!(env.take_expect_failures(sid).is_empty());
}

#[test]
fn a_bad_call_of_an_expectation_is_an_error_not_a_failure() {
    let mut env = quiet();
    for (src, want) in [
        ("expect()", "expect() expects 1 or 2 arguments"),
        ("expect_eq(1)", "expect_eq() expects 2 or 3 arguments"),
        ("expect_near(1, 2)", "expect_near() expects 3 or 4 arguments"),
        ("expect_near(1, 2, -1)", "tolerance of zero or more"),
        ("expect_near(1, 2, \"a\")", "tolerance of zero or more"),
    ] {
        let err = match env.load_program(src) {
            Err(e) => e,
            Ok(pid) => {
                let sid = env.create_stack(pid).unwrap();
                env.run(sid).unwrap_err()
            }
        };
        assert!(err.contains(want), "{src}: {err}");
    }
}

#[test]
fn a_raise_ends_the_test_with_its_position_and_what_it_recorded_first() {
    let (mut env, _, sid) = loaded(
        "fn boom(x) x.nope.deeper end\n\
         test \"raises\" do\n  expect(false, \"first\")\n  boom(1)\n  expect(false, \"never\")\nend\n\
         test \"asserts\" do\n  assert_eq(1, 2)\nend\n",
    );
    let r = env.run_test(sid, "__test_0");
    assert!(!r.passed);
    assert_eq!(r.failures.len(), 1);
    assert_eq!(r.failures[0].message, "first");
    let err = r.error.expect("error");
    assert!(err.contains("[line 1, column"), "{err}");
    assert!(err.contains("in boom() [line 4, column 3]"), "{err}");
    assert!(err.contains("in test \"raises\""), "{err}");
    assert!(!err.contains("__test_"), "{err}");

    // The stack is usable afterwards: the next test, and an ordinary run.
    let r = env.run_test(sid, "__test_1");
    assert!(r.error.unwrap().contains("assertion failed: assert_eq: left=1 right=2"));
    env.reset_stack(sid).unwrap();
    env.run(sid).expect("run after a failed test");
}

#[test]
fn tests_of_modules_are_listed_with_their_module_and_run_by_qualified_function() {
    let mut env = quiet();
    env.register_module(
        "sim",
        "let k = 10\npub fn step(n) n * k end\nfn hidden() 7 end\n\
         test \"hidden is seven\" do\n  expect_eq(hidden(), 7)\n  expect_eq(step(2), 21)\nend\n",
    );
    let (mut env, pid, sid) = loaded_in(
        env,
        "import sim\nprint(sim.step(1))\ntest \"entry\" do\n  expect_eq(sim.step(3), 30)\nend\n",
    );
    let tests = env.tests(pid).to_vec();
    assert_eq!(tests.len(), 2, "{tests:?}");
    assert_eq!(tests[0].module.as_deref(), Some("sim"));
    assert_eq!(tests[0].function, "sim::__test_0");
    assert_eq!(tests[0].line, 4);
    assert_eq!(tests[1].module, None);
    assert_eq!(tests[1].function, "__test_0");

    let r = env.run_test(sid, &tests[0].function);
    assert_eq!(r.checks, 2);
    assert_eq!(r.failures.len(), 1, "{r:?}");
    assert_eq!(r.failures[0].file, "sim");
    assert_eq!(r.failures[0].line, 6);
    assert_eq!(r.failures[0].message, "expected 21, got 20");
    assert!(env.run_test(sid, &tests[1].function).passed);

    let missing = env.run_test(sid, "nope");
    assert!(missing.error.unwrap().contains("No top-level function named 'nope'"));
}

// ---------------------------------------------------------------------------
// A test that spans the host's frames
// ---------------------------------------------------------------------------

/// A host with a frame clock: `wait_frames(n)` notes `n` and suspends.
fn frame_host() -> (Env, Rc<Cell<i64>>) {
    let mut env = quiet();
    let wanted = Rc::new(Cell::new(0i64));
    let w = wanted.clone();
    env.register_native_boxed(
        "wait_frames",
        Box::new(move |cxt: &mut PetalCxt| {
            let n = cxt.get_int(1)?;
            cxt.suspend()?;
            w.set(n);
            cxt.push_int(n);
            Ok(1)
        }),
        NativeEffects::EFFECT,
    );
    (env, wanted)
}

const GAME: &str = "\
native fn wait_frames(n: int) -> int
state var frame = 0
set frame += 1
let at_load = frame

fn wait_twice()
  wait_frames(1)
  wait_frames(1)
end

test \"three frames later\" do
  let before = get frame
  expect_eq(wait_frames(3), 3)
  expect_eq(get frame, before + 3)
  for i in range(0, 2) do
    wait_frames(1)
  end
  expect_eq(get frame, before + 5)
  wait_twice()
  expect_eq(get frame, before + 7)
  // A captured `let` is the value from when the test began.
  expect_eq(at_load, before)
  expect_eq(get frame, 0, \"deliberate\")
end

test \"waits in a callback\" do
  map([1, 2], fn(n) wait_frames(n) end)
end
";

/// Drive one test to its end, running `n` frames whenever it asks.
fn drive(env: &mut Env, sid: StackKey, wanted: &Cell<i64>, function: &str) -> (petal::env::TestResult, u32) {
    env.test_begin(sid, function).expect("begin");
    assert_eq!(env.test_in_progress(sid), Some(function));
    let mut suspensions = 0;
    loop {
        match env.test_resume(sid) {
            TestStep::Finished(result) => {
                assert_eq!(env.test_in_progress(sid), None);
                return (result, suspensions);
            }
            TestStep::Suspended => {
                suspensions += 1;
                for _ in 0..wanted.replace(0) {
                    env.reset_stack(sid).unwrap();
                    env.run(sid).expect("frame");
                }
            }
            TestStep::OutOfSteps => panic!("unbounded"),
        }
    }
}

#[test]
fn a_native_suspends_a_test_and_the_host_runs_frames_before_resuming_it() {
    let (env, wanted) = frame_host();
    let (mut env, _, sid) = loaded_in(env, GAME);
    let (result, suspensions) = drive(&mut env, sid, &wanted, "__test_0");
    // 1 + 2 in the loop + 2 in the helper.
    assert_eq!(suspensions, 5);
    assert_eq!(result.error, None);
    assert_eq!(result.checks, 6);
    assert_eq!(result.failures.len(), 1, "{result:?}");
    assert_eq!(result.failures[0].note.as_deref(), Some("deliberate"));
    assert_eq!(result.failures[0].actual.as_deref(), Some("8"));
}

#[test]
fn a_suspended_test_survives_collections_and_frames_that_expect() {
    let (env, wanted) = frame_host();
    // Every frame allocates, and fails an expectation of its own.
    let src = format!(
        "{GAME}\nlet junk = for i in range(0, 2000) do {{ i: i, s: \"x{{i}}\" }} end\nexpect(len(junk) == 1, \"frame\")\n\
         test \"holds a list across frames\" do\n  let mine = for i in range(0, 500) do [i, \"v{{i}}\"] end\n  wait_frames(40)\n  expect_eq(mine[499], [499, \"v499\"])\n  expect_eq(len(mine), 500)\nend\n"
    );
    let (mut env, _, sid) = loaded_in(env, &src);
    assert_eq!(env.take_expect_failures(sid).len(), 1);
    let (result, _) = drive(&mut env, sid, &wanted, "__test_2");
    assert!(result.passed, "{result:?}");
    assert_eq!(result.checks, 2);
    // The frames' own failures are the context's, not the test's.
    let around = env.take_expect_failures(sid);
    assert_eq!(around.len(), 40);
    assert!(around.iter().all(|f| f.message == "frame"));
}

#[test]
fn suspending_where_nothing_can_resume_is_an_error() {
    let (env, wanted) = frame_host();
    let (mut env, _, sid) = loaded_in(env, GAME);
    // Inside a callback: its frames are on the native stack.
    let (result, _) = drive(&mut env, sid, &wanted, "__test_1");
    let err = result.error.expect("error");
    assert!(err.contains("cannot wait inside a callback"), "{err}");

    // A host call that is not a driven test.
    let err = env.call_function(sid, "wait_twice", &[]).unwrap_err();
    assert!(err.contains("can only wait inside a test"), "{err}");
    // An ordinary run.
    let (mut env2, _) = frame_host();
    let pid = env2.load_program("native fn wait_frames(n: int) -> int\nwait_frames(1)\n").unwrap();
    let sid2 = env2.create_stack(pid).unwrap();
    let err = env2.run(sid2).unwrap_err();
    assert!(err.contains("can only wait inside a test"), "{err}");

    // `run_test` has no frames to give: the test is ended, not left parked.
    let r = env.run_test(sid, "__test_0");
    assert!(r.error.unwrap().contains("asked to wait"));
    assert_eq!(env.test_in_progress(sid), None);
}

#[test]
fn one_test_at_a_time_and_an_abort_keeps_what_was_recorded() {
    let (env, _wanted) = frame_host();
    let (mut env, _, sid) = loaded_in(
        env,
        "native fn wait_frames(n: int) -> int\n\
         test \"a\" do\n  expect(false, \"before\")\n  wait_frames(1)\n  expect(false, \"after\")\nend\n",
    );
    assert_eq!(env.test_abort(sid, "nothing"), None);
    match env.test_resume(sid) {
        TestStep::Finished(r) => assert!(r.error.unwrap().contains("no test is in progress")),
        other => panic!("{other:?}"),
    }
    env.test_begin(sid, "__test_0").unwrap();
    let again = env.test_begin(sid, "__test_0").unwrap_err().to_string();
    assert!(again.contains("still in progress"), "{again}");
    assert_eq!(env.test_resume(sid), TestStep::Suspended);
    let r = env.test_abort(sid, "the window closed").expect("in progress");
    assert_eq!(r.error.as_deref(), Some("the window closed"));
    assert_eq!(r.failures.len(), 1);
    assert_eq!(r.failures[0].message, "before");
    assert_eq!(env.test_in_progress(sid), None);
}

#[test]
fn a_step_bound_sets_the_test_aside_and_it_carries_on() {
    let (mut env, _, sid) = loaded(
        "test \"long\" do\n  var n = 0\n  for i in range(0, 20000) do\n    set n += i\n  end\n  expect_eq(n, 199990000)\nend\n",
    );
    env.test_begin(sid, "__test_0").unwrap();
    let mut slices = 0;
    let result = loop {
        match env.test_resume_bounded(sid, 1000) {
            TestStep::OutOfSteps => {
                slices += 1;
                // A frame of the program in between changes nothing.
                env.reset_stack(sid).unwrap();
                env.run(sid).unwrap();
            }
            TestStep::Finished(r) => break r,
            TestStep::Suspended => panic!("nothing suspends"),
        }
    };
    assert!(slices > 10, "{slices}");
    assert!(result.passed, "{result:?}");
    assert_eq!(result.checks, 1);
}

#[test]
fn a_reload_under_a_waiting_test_ends_it() {
    let (env, _wanted) = frame_host();
    let src = "native fn wait_frames(n: int) -> int\nlet k = 1\n\
               test \"a\" do\n  wait_frames(1)\n  expect_eq(k, 1)\nend\n";
    let (mut env, _, sid) = loaded_in(env, src);
    env.test_begin(sid, "__test_0").unwrap();
    assert_eq!(env.test_resume(sid), TestStep::Suspended);
    let edited = src.replace("expect_eq(k, 1)", "expect_eq(k, 2)\n  print(k)");
    env.reload_program(sid, &edited, None).expect("reload");
    match env.test_resume(sid) {
        TestStep::Finished(r) => {
            assert!(!r.passed);
            assert!(r.error.unwrap().contains("reloaded while the test was waiting"));
        }
        other => panic!("{other:?}"),
    }
    // The reloaded program's test runs from the top.
    env.run(sid).unwrap();
    env.test_begin(sid, "__test_0").unwrap();
    assert_eq!(env.test_resume(sid), TestStep::Suspended);
    match env.test_resume(sid) {
        TestStep::Finished(r) => assert_eq!(r.failures[0].message, "expected 2, got 1"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_host_native_records_expectations_of_its_own() {
    fn expect_even(cxt: &mut PetalCxt) -> Result<u32, String> {
        let n = cxt.get_int(1)?;
        if n % 2 == 0 {
            cxt.pass_expectation();
        } else {
            let at = cxt.call_location().expect("a call in source has a position");
            assert_eq!(at.line, 3);
            cxt.fail_expectation("expect_even", format!("{n} is odd"));
        }
        cxt.push_bool(n % 2 == 0);
        Ok(1)
    }
    let mut env = quiet();
    env.register_native("expect_even", expect_even, NativeEffects::EFFECT);
    let (mut env, _, sid) = loaded_in(
        env,
        "native fn expect_even(n: int) -> bool\ntest \"evens\" do\n  expect_even(3)\n  expect_even(4)\nend\n",
    );
    let r = env.run_test(sid, "__test_0");
    assert_eq!((r.checks, r.failed), (2, 1));
    assert_eq!(r.failures[0].check, "expect_even");
    assert_eq!(r.failures[0].message, "3 is odd");
    assert_eq!((r.failures[0].line, r.failures[0].column), (3, 3));
}

#[test]
fn the_log_stops_recording_at_its_limit_and_keeps_counting() {
    let (mut env, _, sid) = loaded(
        "test \"many\" do\n  for i in range(0, 1500) do\n    expect_eq(i, -1)\n  end\nend\n",
    );
    let r = env.run_test(sid, "__test_0");
    assert_eq!(r.failures.len(), petal::expect::MAX_RECORDED_FAILURES);
    assert_eq!((r.checks, r.failed), (1500, 1500));
    assert!(!r.passed);
}

// ---------------------------------------------------------------------------
// check, lint
// ---------------------------------------------------------------------------

#[test]
fn check_reads_a_test_body_like_any_function() {
    let mut env = quiet();
    let pid = env
        .load_program("fn double(x: int) x * 2 end\ntest \"typed\" do\n  expect_eq(double(\"a\"), 2)\nend\n")
        .unwrap();
    let text = env.diagnostics_text(pid);
    assert!(text.contains("[line 3, column"), "{text}");
    // A name that does not exist is found there too.
    let pid = env.load_program("test \"a\" do\n  expect(nope)\nend\n").unwrap();
    let report = env.check_program(pid);
    assert!(!report.passed());
    let text = env.error_diagnostics_text(pid);
    assert!(text.contains("nope") && text.contains("line 2"), "{text}");
    // A module-level cell is read with `get`, as in any function.
    let err = env
        .load_program("var n = 1\ntest \"a\" do\n  expect_eq(n, 1)\nend\n")
        .unwrap_err();
    assert!(err.contains("get n"), "{err}");
}

#[test]
fn lint_fixes_inside_a_test_body_and_keeps_the_block() {
    let src = "test \"a\" do\n  var x = 1\n  set x = x + 1\n  expect_eq(float(x) * 1.0, 2.0)\nend\n";
    let outcome = petal::lint::lint_source(src, &Default::default()).expect("lint");
    let fixed = outcome.output.clone();
    assert!(fixed.starts_with("test \"a\" do\n"), "{fixed}");
    assert!(fixed.trim_end().ends_with("end"), "{fixed}");
    // Whatever the rules did, it is still one test that passes.
    let (mut env, pid, sid) = loaded(&fixed);
    assert_eq!(env.tests(pid).len(), 1);
    assert!(env.run_test(sid, "__test_0").passed);
    assert!(!outcome.findings.is_empty(), "the body is linted");
    assert_ne!(fixed, src);
    // No rule complains about the block itself or its generated name.
    for f in &outcome.findings {
        assert!(!f.message.contains("__test_"), "{f:?}");
    }
}
