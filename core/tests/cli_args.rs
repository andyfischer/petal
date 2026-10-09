//! CLI-level tests for how `petal` answers a command line it cannot use:
//! mistyped commands and options, and the diagnostics a newcomer hits first
//! (docs/tasks/todo.md item 9). These shell out to the built binary, since
//! every one of them is about what the process prints and how it exits.

use std::path::PathBuf;
use std::process::Command;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// Run `petal <args>` in `dir`; returns (stdout, stderr, exit code).
fn petal_in(dir: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(PETAL)
        .args(args)
        .current_dir(dir)
        .env_remove("PETAL_OPT")
        .env_remove("PETAL_POLICY")
        .env_remove("PETAL_PATH")
        .output()
        .expect("failed to run petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn petal(args: &[&str]) -> (String, String, i32) {
    petal_in(&std::env::temp_dir(), args)
}

/// A fresh directory holding `files` (name, contents).
fn dir_with(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petal-cli-args-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, contents) in files {
        std::fs::write(dir.join(name), contents).unwrap();
    }
    dir
}

#[test]
fn a_mistyped_command_is_an_unknown_command_not_a_missing_file() {
    let (_, err, code) = petal(&["repl"]);
    assert_eq!(code, 1);
    assert!(err.contains("'repl' is not a petal command"), "{err}");
    assert!(err.contains("petal run -e"), "{err}");
    assert!(!err.contains("Error reading file"), "{err}");

    let (_, err, _) = petal(&["test"]);
    assert!(err.contains("'test' is not a petal command"), "{err}");

    let (_, err, _) = petal(&["chek", "x.ptl"]);
    assert!(err.contains("Did you mean 'petal check'?"), "{err}");
}

#[test]
fn the_file_shorthand_survives() {
    // A missing `.ptl` is still a missing file, and options still lead.
    let (_, err, code) = petal(&["no-such-file.ptl"]);
    assert_eq!(code, 1);
    assert!(err.contains("Error reading file 'no-such-file.ptl'"), "{err}");

    let (out, _, code) = petal(&["-e", "print(1)"]);
    assert_eq!((out.as_str(), code), ("1\n", 0));

    // An existing file with no extension runs.
    let dir = dir_with("shorthand", &[("script", "print(2)\n")]);
    let (out, err, code) = petal_in(&dir, &["script"]);
    assert_eq!((out.as_str(), code), ("2\n", 0), "{err}");
}

#[test]
fn an_unknown_option_is_named_instead_of_read_as_the_file() {
    for cmd in ["run", "bench", "check", "show-ir"] {
        let (_, err, code) = petal(&[cmd, "-e", "print(1)", "--iter", "3"]);
        assert_eq!(code, 1, "{cmd}");
        assert!(err.contains("Unknown option '--iter'"), "{cmd}: {err}");
        assert!(err.contains(&format!("petal help {cmd}")), "{cmd}: {err}");
        assert!(!err.contains("Error reading file"), "{cmd}: {err}");
    }
    // The shorthand reports against `run`.
    let (_, err, _) = petal(&["--jsno", "x.ptl"]);
    assert!(err.contains("Unknown option '--jsno'") && err.contains("petal help run"), "{err}");
}

#[test]
fn a_second_source_is_refused() {
    let (_, err, code) = petal(&["run", "a.ptl", "b.ptl"]);
    assert_eq!(code, 1);
    assert!(err.contains("Unexpected argument 'b.ptl'"), "{err}");
}

#[test]
fn strict_check_does_not_fail_on_the_export_deprecation_alone() {
    let old = "export fn f()\n  1\nend\nprint(f())\n";
    let (_, err, code) = petal(&["check", "--strict", "-e", old]);
    assert!(err.contains("`export` is deprecated"), "{err}");
    assert_eq!(code, 0, "{err}");

    // Any other warning still fails it.
    let (_, _, code) = petal(&["check", "--strict", "-e", "let x: int = \"s\""]);
    assert_eq!(code, 1);
}

#[test]
fn run_explains_a_name_only_the_ui_host_provides() {
    let src = "ui.label(\"x\")";
    // `check` defaults to `--host ui` and accepts it…
    assert_eq!(petal(&["check", "-e", src]).2, 0);
    // …`check --host core` is what agrees with `run`…
    assert_eq!(petal(&["check", "--host", "core", "-e", src]).2, 1);
    // …and `run` says why it cannot.
    let (_, err, code) = petal(&["run", "-e", src]);
    assert_eq!(code, 1);
    assert!(err.contains("Undefined variable: ui"), "{err}");
    assert!(err.contains("comes from the petal-ui host"), "{err}");
    assert!(err.contains("check --host core"), "{err}");

    // A name nobody provides gets no such note.
    let (_, err, _) = petal(&["run", "-e", "frob()"]);
    assert!(!err.contains("petal-ui"), "{err}");
}

#[test]
fn an_unclosed_block_names_its_opener() {
    let (_, err, _) = petal(&["run", "-e", "let a = 1\nfn f()\n  print(a)\n"]);
    assert!(
        err.contains("the function `f` started at line 2 column 1 is unclosed; expected `end`"),
        "{err}"
    );
    let (_, err, _) = petal(&["run", "-e", "for x in [1] do\n  print(x)\n"]);
    assert!(err.contains("a `for` loop started at line 1 column 1 is unclosed"), "{err}");
    let (_, err, _) = petal(&["run", "-e", "let x = 1\nwhile x < 2 do\n  print(x)\n"]);
    assert!(err.contains("a `while` loop started at line 2 column 1 is unclosed"), "{err}");
    let (_, err, _) = petal(&["run", "-e", "let g = fn(x)\n  x\n"]);
    assert!(err.contains("a `fn` expression started at line 1 column 9 is unclosed"), "{err}");
}

#[test]
fn a_cross_module_stack_trace_names_the_entry_file() {
    let dir = dir_with(
        "trace",
        &[
            ("m.ptl", "fn boom(x)\n  x.foo\nend\npub fn go(v)\n  boom(v)\nend\n"),
            ("main.ptl", "import m\nfn outer(v)\n  m.go(v)\nend\nouter([1][0])\n"),
        ],
    );
    let (_, err, code) = petal_in(&dir, &["run", "main.ptl"]);
    assert_eq!(code, 1);
    assert!(err.contains("in boom() [m.ptl line 5, column 3]"), "{err}");
    assert!(err.contains("in go() [main.ptl line 3, column 3]"), "{err}");
    assert!(err.contains("in outer() [main.ptl line 5, column 1]"), "{err}");

    // A single-file trace keeps the bare entry-file position.
    let (_, err, _) = petal(&["run", "-e", "fn boom(x)\n  x.foo\nend\nboom([1][0])"]);
    assert!(err.contains("in boom() [line 4, column 1]"), "{err}");
}
