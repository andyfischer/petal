//! CLI-level tests for `run --observe` — the flag that dumps the last value
//! bound to every named term after a run.
//!
//! These shell out to the built `petal` binary (via `CARGO_BIN_EXE_petal`, which
//! Cargo sets for integration tests) so they cover argument parsing, the
//! enable-before-run ordering, and the two output shapes end to end. The unit
//! tests for the naming rule itself live in `src/env/tests.rs`; what is under
//! test here is the *command*.

use std::process::Command;

/// Path to the freshly built `petal` binary for this test run.
const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// A program with a top-level `let`, a function-local `let`, a loop temp, and a
/// `state` var — one of each kind of binding a reader would look for.
const PROGRAM: &str = "\
let scale = 3
state counter: int = 0

fn row_label(i)
  let prefix = \"row-\"
  return prefix ++ str(i)
end

counter = counter + 1

for i in range(0, 3) do
  let cell = i * scale
  print(row_label(cell))
end

let total = scale * 10
";

fn run(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(PETAL)
        .args(args)
        .output()
        .expect("failed to run petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

/// The text dump reports both a top-level name and a function-local one, and
/// qualifies the local with its function — the collision the facility exists to
/// fix, seen from the CLI.
#[test]
fn observe_dumps_top_level_and_function_local_bindings() {
    let (stdout, stderr, ok) = run(&["run", "--observe", "-e", PROGRAM]);
    assert!(ok, "run --observe exited non-zero; stderr:\n{stderr}");

    // The program's own output is still there, and the dump is announced rather
    // than merged into it.
    assert!(
        stdout.contains("row-0\n"),
        "program output missing:\n{stdout}"
    );
    assert!(
        stdout.contains("Observed values ("),
        "expected a dump header:\n{stdout}"
    );

    for expected in [
        "scale            = 3",        // top-level let
        "counter          = 1",        // state var, after its one write
        "row_label.prefix = \"row-\"", // function-local, qualified
        "cell             = 6",        // loop temp: final iteration, 2 * 3
        "total            = 30",
    ] {
        assert!(
            stdout.contains(expected),
            "missing `{expected}` in dump:\n{stdout}"
        );
    }
}

/// Without the flag nothing is recorded and nothing is printed — observation is
/// opt-in, and `run`'s output is unchanged by its existence.
#[test]
fn without_observe_nothing_is_dumped() {
    let (stdout, _, ok) = run(&["run", "-e", PROGRAM]);
    assert!(ok);
    assert!(
        !stdout.contains("Observed values"),
        "run without --observe should print no dump:\n{stdout}"
    );
}

/// `--observe --json` replaces the aligned text with one parseable object.
#[test]
fn observe_json_emits_a_parseable_object() {
    let (stdout, stderr, ok) = run(&["run", "--observe", "--json", "-e", PROGRAM]);
    assert!(ok, "run exited non-zero; stderr:\n{stderr}");

    // The program printed first; the JSON document is the tail of stdout.
    let start = stdout.find('{').expect("no JSON object in stdout");
    let obs: serde_json::Value = serde_json::from_str(&stdout[start..])
        .unwrap_or_else(|e| panic!("dump was not valid JSON ({e}):\n{stdout}"));

    assert_eq!(obs.get("scale").and_then(|v| v.as_i64()), Some(3));
    assert_eq!(obs.get("counter").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(
        obs.get("row_label.prefix").and_then(|v| v.as_str()),
        Some("row-")
    );
    assert_eq!(obs.get("total").and_then(|v| v.as_i64()), Some(30));
}

/// The values bound *before* a runtime error are exactly what a user debugging
/// that error wants, so the dump survives the failure — and the command still
/// fails.
#[test]
fn observe_dumps_bindings_made_before_a_runtime_error() {
    let code = "let a = 1\nfn f(x)\n  let inner = x * 2\n  return inner + nope\nend\nprint(f(a))\n";
    let (stdout, stderr, ok) = run(&["run", "--observe", "-e", code]);

    assert!(!ok, "a failing program must still exit non-zero");
    assert!(
        stderr.contains("Undefined variable: nope"),
        "the error is still reported:\n{stderr}"
    );
    assert!(
        stdout.contains("a       = 1") && stdout.contains("f.inner = 2"),
        "expected the pre-error bindings in the dump:\n{stdout}"
    );
}

/// In `--json` mode the observed values ride on the error object, so stdout
/// stays a single JSON document that a tool can parse.
#[test]
fn observe_json_attaches_observations_to_a_runtime_error() {
    let code = "let a = 1\nfn f(x)\n  let inner = x * 2\n  return inner + nope\nend\nprint(f(a))\n";
    let (stdout, _, ok) = run(&["run", "--observe", "--json", "-e", code]);

    assert!(!ok, "a failing program must still exit non-zero");
    let err: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout was not a single JSON document ({e}):\n{stdout}"));

    assert_eq!(err.get("phase").and_then(|p| p.as_str()), Some("runtime"));
    let obs = err.get("observations").expect("observations field");
    assert_eq!(obs.get("a").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(obs.get("f.inner").and_then(|v| v.as_i64()), Some(2));
}
