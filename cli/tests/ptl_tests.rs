//! Petal's own tests written in Petal: the `test "name" do … end` blocks
//! under `test/ptl/` and in the examples that carry them, run through the
//! runner behind `petal test` (`petal::test_runner`), and the `petal test`
//! command itself, run as a process. docs/testing.md.

#![allow(clippy::disallowed_methods, reason = "tests run natively")]

use std::path::{Path, PathBuf};
use std::process::Command;

use petal::test_runner::{self as runner, Options, Status, Summary};

fn repo(rel: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel)
        .display()
        .to_string()
}

fn petal(args: &[&str], cwd: Option<&Path>) -> (String, String, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_petal"));
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd.output().expect("failed to spawn petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn dir_with(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petal-test-cmd-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (name, contents) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    dir
}

// ---------------------------------------------------------------------------
// The repository's Petal tests
// ---------------------------------------------------------------------------

#[test]
fn the_repositorys_petal_tests_pass() {
    let paths = [
        repo("test/ptl"),
        repo("examples/console"),
        repo("examples/games/physics-playground/physics.ptl"),
    ];
    let files = runner::run_paths(&paths, &Options::default());
    let summary = Summary::of(&files);
    assert!(summary.ok(), "{}", runner::render(&files, false));
    // Guard against the runner quietly finding nothing.
    assert!(summary.files >= 5, "{summary:?}");
    assert!(summary.tests >= 20, "{summary:?}");
    assert_eq!(summary.passed, summary.tests);
    for f in &files {
        assert!(!f.tests.is_empty(), "{} was listed with no tests", f.file);
    }
}

#[test]
fn an_example_with_tests_still_runs_as_it_did() {
    // The tests are not part of the run: the output is the golden one.
    let (out, _, code) = petal(&["run", &repo("examples/console/imports.ptl")], None);
    assert_eq!(code, 0);
    assert!(out.starts_with("hello!\nthe quick...\n"), "{out}");
    let (out, _, code) = petal(&["run", &repo("examples/console/fibonacci.ptl")], None);
    assert_eq!(code, 0);
    assert!(out.contains("fib(19) = 4181"), "{out}");
    assert!(!out.contains("known values"), "{out}");
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

const LIVES: &str = "\
fn after_hit(lives: int) -> int
  max(lives - 2, 0 - 9)
end

test \"a hit costs one life\" do
  expect_eq(after_hit(3), 2)
  expect_eq(after_hit(0), 0, \"never below zero\")
end

test \"passes\" do
  print(\"quiet unless asked\")
  expect(after_hit(9) == 7)
end
";

#[test]
fn the_report_lists_each_test_and_the_exit_status_says_whether_all_passed() {
    let dir = dir_with("report", &[("lives.ptl", LIVES), ("plain.ptl", "print(\"no tests here\")\n")]);
    let (out, err, code) = petal(&["test"], Some(&dir));
    assert_eq!(code, 1, "{out}{err}");
    assert_eq!(
        out,
        "lives.ptl\n  FAIL  a hit costs one life\n        lives.ptl:6: expected 2, got 1\n        \
         lives.ptl:7: expected 0, got -2 (never below zero)\n  ok    passes\n\n\
         2 tests: 1 passed, 1 failed (1 file)\n"
    );
    assert_eq!(err, "");

    let (out, _, code) = petal(&["test", "--filter", "pass", "--show-output", "lives.ptl"], Some(&dir));
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        out,
        "lives.ptl\n  ok    passes\n        output:\n          | quiet unless asked\n\n1 test: 1 passed (1 file)\n"
    );

    let (out, _, code) = petal(&["test", "--list"], Some(&dir));
    assert_eq!(code, 0);
    assert!(out.contains("a hit costs one life") && out.contains("2 tests in 1 file"), "{out}");

    // A file with no tests, and a filter nothing matches: nothing to fail.
    let (out, _, code) = petal(&["test", "plain.ptl"], Some(&dir));
    assert_eq!((out.as_str(), code), ("no tests found\n", 0));
    let (out, _, code) = petal(&["test", "--filter", "zzz"], Some(&dir));
    assert_eq!((out.as_str(), code), ("no tests found\n", 0));

    let (_, err, code) = petal(&["test", "--timeout", "soon"], Some(&dir));
    assert_eq!(code, 1);
    assert!(err.contains("--timeout takes a number of seconds"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn json_output_carries_every_failure_with_its_position_and_values() {
    let dir = dir_with("json", &[("lives.ptl", LIVES)]);
    let (out, _, code) = petal(&["test", "--json", "lives.ptl"], Some(&dir));
    assert_eq!(code, 1);
    let doc: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["summary"]["tests"], 2);
    assert_eq!(doc["summary"]["passed"], 1);
    assert_eq!(doc["summary"]["failed"], 1);
    let tests = doc["files"][0]["tests"].as_array().unwrap();
    assert_eq!(tests[0]["name"], "a hit costs one life");
    assert_eq!(tests[0]["status"], "failed");
    assert_eq!(tests[0]["line"], 5);
    assert_eq!(tests[0]["checks"], 2);
    let f = &tests[0]["failures"][1];
    assert_eq!(f["check"], "expect_eq");
    assert_eq!(f["file"], "lives.ptl");
    assert_eq!((f["line"].as_u64(), f["column"].as_u64()), (Some(7), Some(3)));
    assert_eq!(f["actual"], "-2");
    assert_eq!(f["expected"], "0");
    assert_eq!(f["note"], "never below zero");
    assert_eq!(f["message"], "expected 0, got -2 (never below zero)");
    assert!(f.get("tolerance").is_none());
    assert_eq!(tests[1]["status"], "passed");
    assert_eq!(tests[1]["output"][0], "quiet unless asked");
    assert!(tests[1]["duration_ms"].as_f64().unwrap() >= 0.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn each_test_starts_from_the_files_initial_state() {
    let src = "\
state var hits = 0
var log = []
fn hit()
  set hits += 1
  set log = append(get log, get hits)
end
hit()

test \"first\" do
  hit()
  expect_eq(get hits, 2)
  expect_eq(get log, [1, 2])
end

test \"second sees nothing of the first\" do
  expect_eq(get hits, 1)
  hit()
  hit()
  expect_eq(get log, [1, 2, 3])
end
";
    let report = runner::run_source("state.ptl", None, src, &Options::default());
    assert!(!report.failed(), "{}", runner::render(&[report.clone()], true));
    assert_eq!(report.tests.len(), 2);
}

#[test]
fn a_module_is_tested_through_its_own_file_and_imports_resolve() {
    let dir = dir_with(
        "modules",
        &[
            (
                "main.ptl",
                "import ./lib/sim\nprint(sim.step(1))\ntest \"entry\" do\n  expect_eq(sim.step(2), 20)\n  sim.check(3, 31)\nend\n",
            ),
            (
                "lib/sim.ptl",
                "let k = 10\npub fn step(n) n * k end\npub fn check(n, want)\n  expect_eq(step(n), want)\nend\n\
                 test \"module\" do\n  expect_eq(step(4), 40)\nend\n",
            ),
        ],
    );
    let files = runner::run_paths(&[dir.display().to_string()], &Options::default());
    assert_eq!(files.len(), 2);
    // Sorted by path: lib/sim.ptl, then main.ptl. Each ran its own test once.
    assert!(files[0].file.ends_with("sim.ptl") && files[1].file.ends_with("main.ptl"));
    assert_eq!(files[0].tests.len(), 1);
    assert_eq!(files[0].tests[0].status, Status::Passed);
    assert_eq!(files[1].tests.len(), 1);
    assert_eq!(files[1].tests[0].name, "entry");
    // A failure in an imported file is named by that file.
    assert_eq!(files[1].tests[0].status, Status::Failed);
    let failure = &files[1].tests[0].failures[0];
    assert_eq!(failure.file, "lib/sim.ptl");
    assert_eq!(failure.line, 4);
    assert_eq!(failure.message, "expected 31, got 30");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_raise_a_timeout_and_an_unrunnable_file_are_errors() {
    let src = "\
test \"raises\" do
  expect(false, \"recorded first\")
  assert(1 > 2, \"boom\")
end
test \"spins\" do
  var i = 0
  while true do set i += 1 end
end
test \"fine\" do expect(true) end
";
    let opts = Options {
        timeout: Some(std::time::Duration::from_millis(100)),
        ..Options::default()
    };
    let report = runner::run_source("errors.ptl", None, src, &opts);
    let [raises, spins, fine] = &report.tests[..] else {
        panic!("three tests");
    };
    assert_eq!(raises.status, Status::Error);
    assert_eq!(raises.failures.len(), 1);
    let err = raises.error.as_deref().unwrap();
    assert!(err.contains("assertion failed: boom") && err.contains("line 3"), "{err}");
    assert!(err.contains("in test \"raises\""), "{err}");
    assert_eq!(spins.status, Status::Error);
    assert!(spins.error.as_deref().unwrap().contains("timed out after 0.1s"));
    assert_eq!(fine.status, Status::Passed);
    let s = Summary::of(&[report]);
    assert_eq!((s.tests, s.passed, s.failed, s.errors), (3, 1, 0, 2));
    assert!(!s.ok());

    // Does not compile, does not pass check, needs a host: the file is the error.
    for (src, want) in [
        ("test \"a\" do\n  let = 1\nend\n", "Expected"),
        ("test \"a\" do\n  expect(nope)\nend\n", "nope"),
        ("import ui\ntest \"a\" do\n  expect(screen_width() > 0)\nend\n", "host's own test runner"),
    ] {
        let report = runner::run_source("bad.ptl", None, src, &Options::default());
        let err = report.error.clone().unwrap_or_else(|| panic!("{src}: {report:?}"));
        assert!(err.contains(want), "{src}: {err}");
        assert!(report.tests.is_empty());
        assert!(report.failed());
    }
}

#[test]
fn a_top_level_that_never_finishes_is_held_to_the_timeout() {
    let src = "\
var n = 0
while true do set n += 1 end
test \"a\" do expect(true) end
test \"b\" do expect(true) end
";
    let opts = Options {
        timeout: Some(std::time::Duration::from_millis(100)),
        ..Options::default()
    };
    let started = std::time::Instant::now();
    let report = runner::run_source("hangs.ptl", None, src, &opts);
    // The second test is not given the same hang to sit through.
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let [a, b] = &report.tests[..] else {
        panic!("two tests");
    };
    for t in [a, b] {
        assert_eq!(t.status, Status::Error);
        let err = t.error.as_deref().unwrap();
        assert!(err.contains("timed out after 0.1s in the file's top level"), "{err}");
    }
}

#[test]
fn top_level_expectations_fail_the_file_once_and_petal_run_reports_them() {
    let src = "expect_eq(1 + 1, 3, \"sums\")\nprint(\"ran\")\ntest \"a\" do expect(true) end\ntest \"b\" do expect(true) end\n";
    let report = runner::run_source("top.ptl", None, src, &Options::default());
    assert_eq!(report.top_level.len(), 1);
    assert_eq!(report.top_level[0].render(), "top.ptl:1: expected 3, got 2 (sums)");
    assert!(report.tests.iter().all(|t| t.status == Status::Passed && t.failures.is_empty()));
    assert!(report.failed());
    let text = runner::render(&[report], false);
    assert!(text.contains("  FAIL  (top level)\n        top.ptl:1: expected 3, got 2 (sums)\n"), "{text}");
    assert!(text.ends_with("2 tests: 2 passed; 1 file failed at the top level (1 file)\n"), "{text}");

    let dir = dir_with("run", &[("top.ptl", src)]);
    let (out, err, code) = petal(&["run", "top.ptl"], Some(&dir));
    assert_eq!(out, "ran\n");
    assert_eq!(err, "expectation failed: top.ptl:1: expected 3, got 2 (sums)\n");
    assert_eq!(code, 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn has_tests_reads_the_syntax_not_the_word() {
    assert!(runner::has_tests("test \"a\" do end\n"));
    assert!(runner::has_tests("fn f() 1 end\n\n  test   \"a\" do\n  end\n"));
    assert!(!runner::has_tests("let test = 1\nprint(test)\n"));
    assert!(!runner::has_tests("// test \"a\" do end\nprint(\"test \\\"a\\\" do\")\n"));
    assert!(!runner::has_tests("print(1)\n"));
    // A file that does not parse is run, so its error is reported.
    assert!(runner::has_tests("test \"a\" do\n  let = \nend\n"));
    assert!(!runner::has_tests("let = \n"));
}

#[test]
fn suggest_annotates_functions_and_leaves_test_blocks_alone() {
    let src = "fn area(w: int, h: int)\n  w * h\nend\n\ntest \"area\" do\n  let a = area(2, 3)\n  expect_eq(a, 6)\nend\n";
    let dir = dir_with("suggest", &[("area.ptl", src)]);
    let (out, err, _) = petal(&["suggest", "area.ptl"], Some(&dir));
    assert!(out.contains("fn area") && out.contains("suggest: -> int"), "{out}{err}");
    assert!(!out.contains("__test_"), "{out}");
    let (_, err, code) = petal(&["suggest", "--apply", "area.ptl"], Some(&dir));
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("dropped"), "{err}");
    let applied = std::fs::read_to_string(dir.join("area.ptl")).unwrap();
    assert!(applied.contains("fn area(w: int, h: int) -> int\n"), "{applied}");
    assert!(applied.contains("test \"area\" do\n"), "{applied}");
    let _ = std::fs::remove_dir_all(dir);
}
