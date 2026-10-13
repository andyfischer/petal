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

    let (_, err, _) = petal(&["tests"]);
    assert!(err.contains("'tests' is not a petal command"), "{err}");
    assert!(err.contains("Did you mean 'petal test'?"), "{err}");

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
fn strict_check_has_nothing_to_say_about_the_export_spelling() {
    // The old spelling is `petal lint`'s (`prefer-pub`), not `check`'s.
    let old = "export fn f()\n  1\nend\nprint(f())\n";
    let (_, err, code) = petal(&["check", "--strict", "-e", old]);
    assert_eq!(err, "");
    assert_eq!(code, 0, "{err}");

    // Any other warning still fails it.
    let (_, _, code) = petal(&["check", "--strict", "-e", "let x: int = \"s\""]);
    assert_eq!(code, 1);
}

#[test]
fn run_refuses_a_ui_script_at_load_and_says_which_runner_takes_it() {
    // (`ui.screen_width` is one of its exports; a member it does not export,
    // such as `ui.label`, is an error for `check` on its own.)
    let src = "import ui\nprint(\"ran\")\nui.screen_width()";
    // `check` accepts it: the bundled `ui` package declares the natives…
    assert_eq!(petal(&["check", "-e", src]).2, 0);
    // …and `run` refuses it before any line runs, naming what is missing.
    let (out, err, code) = petal(&["run", "-e", src]);
    assert_eq!(code, 1);
    assert!(!out.contains("ran"), "{out}");
    assert!(err.contains("module 'ui' needs native fns"), "{err}");
    assert!(err.contains("petal-ui runner"), "{err}");
    // Without the import the two agree as well: the name is unknown.
    assert_eq!(petal(&["check", "-e", "ui.label(\"x\", 0, 0)"]).2, 1);
    assert_eq!(petal(&["run", "-e", "ui.label(\"x\", 0, 0)"]).2, 1);

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
            ("main.ptl", "import ./m\nfn outer(v)\n  m.go(v)\nend\nouter([1][0])\n"),
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

/// `petal show-static`: the classifier's answer as text and as JSON, the
/// entry file's declarations by default and every module's with `--all`.
#[test]
fn show_static_reports_class_reason_and_readers() {
    let dir = dir_with(
        "show-static",
        &[
            ("lib.ptl", "pub let BASE = [1, 2, 3]\npub fn twice(x) x * 2 end\n"),
            (
                "app.ptl",
                "import ./lib\n\
                 native fn text_width(s)\n\
                 state live = 0\n\
                 let TABLE = [lib.twice(lib.BASE[0]), 2]\n\
                 fn label_w(s) text_width(s) + 4 end\n\
                 let LAB_W = label_w(\"x\")\n\
                 print(TABLE, live)\n",
            ),
        ],
    );
    let (out, err, code) = petal_in(&dir, &["show-static", "app.ptl"]);
    assert_eq!(code, 0, "{err}");
    let line = |needle: &str| -> &str {
        out.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle:?} in\n{out}"))
    };
    assert!(line("let TABLE").starts_with("static"), "{out}");
    assert!(line("let TABLE").contains("read by dynamic code"), "{out}");
    assert!(line("fn label_w").contains("not static-pure"), "{out}");
    assert!(line("let LAB_W").starts_with("dynamic"), "{out}");
    assert!(out.contains("= LAB_W calls label_w at app.ptl:6"), "{out}");
    assert!(
        out.contains(
            "= label_w calls text_width, a native that is not registered here at app.ptl:5"
        ),
        "{out}"
    );
    // The module's declarations are not the entry file's.
    assert!(!out.contains("BASE"), "{out}");
    assert!(out.contains("4 declarations: 1 static (1 read by dynamic code)"), "{out}");

    let (out, _, code) = petal_in(&dir, &["show-static", "--all", "app.ptl"]);
    assert_eq!(code, 0);
    assert!(out.lines().any(|l| l.starts_with("static") && l.contains("let lib::BASE")), "{out}");

    let (out, err, code) = petal_in(&dir, &["show-static", "--json", "app.ptl"]);
    assert_eq!(code, 0, "{err}");
    let json: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(json["summary"]["declarations"], 4);
    let decls = json["declarations"].as_array().unwrap();
    let table = decls.iter().find(|d| d["name"] == "TABLE").unwrap();
    assert_eq!(table["class"], "static");
    assert_eq!(table["read_by_dynamic"], true);
    let deps: Vec<&str> = table["depends_on"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect();
    assert_eq!(deps, ["lib::BASE", "lib::twice"]);
    let lab = decls.iter().find(|d| d["name"] == "LAB_W").unwrap();
    assert_eq!(lab["class"], "dynamic");
    assert_eq!(lab["reason"][0]["text"], "calls label_w");
    assert_eq!(lab["reason"][0]["file"], "app.ptl");

    // A compile error is reported as one, not as a classification.
    let (_, err, code) = petal_in(&dir, &["show-static", "-e", "let x = ("]);
    assert_eq!(code, 1);
    assert!(!err.is_empty());

    let (out, _, code) = petal(&["help", "show-static"]);
    assert_eq!(code, 0);
    assert!(out.contains("petal show-static [--json] [--all] <file>"), "{out}");
}

/// `petal run --static-audit` / `--static-only`: the summary goes to stderr,
/// a clean audit exits 0, and the static-only build runs a script whose host
/// natives are not here — printing nothing, since nothing dynamic runs.
#[test]
fn run_static_audit_and_static_only() {
    let dir = dir_with(
        "static-audit",
        &[
            (
                "app.ptl",
                "native fn host_width()\n\
                 fn row(a, b) { a: a, b: b * 2 } end\n\
                 let ROWS = [row(1, 2), row(3, 4)]\n\
                 let N = len(ROWS)\n\
                 print(\"drawn\", host_width() / N)\n",
            ),
            (
                "plain.ptl",
                "let ROWS = sort([3, 1, 2])\nstate n = 0\nn = n + len(ROWS)\nprint(n)\n",
            ),
            ("raises.ptl", "let ROWS = [1, 2]\nlet BAD = ROWS[0].missing.deeper\nprint(BAD)\n"),
        ],
    );
    // Without its host the script does not even link.
    let (_, err, code) = petal_in(&dir, &["run", "app.ptl"]);
    assert_ne!(code, 0, "{err}");

    let (out, err, code) = petal_in(&dir, &["run", "--static-only", "app.ptl"]);
    assert_eq!(code, 0, "{err}");
    assert!(!out.contains("drawn"), "{out}");
    assert!(err.contains("static audit: built 2 static declarations alone"), "{err}");
    assert!(err.contains("0 failures"), "{err}");

    let (out, err, code) = petal_in(&dir, &["run", "--static-audit", "plain.ptl"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains('3'), "{out}");
    assert!(err.contains("static audit: 1 static declarations"), "{err}");
    assert!(err.contains("0 failures"), "{err}");

    // A build that raises fails, and the audit names the declaration.
    let (_, err, code) = petal_in(&dir, &["run", "--static-only", "raises.ptl"]);
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("1 failure") && err.contains("BAD (") && err.contains(") raised:"), "{err}");
}

/// `petal --version` says which build this is: the version, the commit and
/// its date, and the build date. A stale binary gives no other sign.
#[test]
fn version_prints_the_commit_and_the_build_date() {
    for flag in ["--version", "version", "-V"] {
        let (out, err, code) = petal(&[flag]);
        assert_eq!(code, 0, "{err}");
        let line = out.trim();
        assert_eq!(line, petal::version::describe(), "{flag}");
        assert!(line.starts_with("petal 0."), "{line}");
        assert!(line.contains(", built 20"), "{line}");
        assert_eq!(out.lines().count(), 1, "{out}");
    }
    // This checkout is a git repository, so the commit is a real one.
    let commit = petal::version::GIT_COMMIT;
    assert_ne!(commit, "unknown");
    let hash = commit.trim_end_matches("-dirty");
    assert!(hash.len() == 10 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "{commit}");
    assert_eq!(petal::version::GIT_DATE.len(), 10);
}
