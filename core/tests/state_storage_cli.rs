//! `petal run --state-storage <file>`: `state` carried from one process to the
//! next through a JSON file, and the stderr warning a script with `state` gets
//! without it (docs/tasks/todo.md item 5, docs/CLI.md "State between runs").
//! These shell out to the built binary: the point is what two separate
//! processes print. Value encoding and slot addressing are unit-tested in
//! `src/env/state_storage.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");
const COUNTER: &str = "state hits = 0\nhits += 1\nprint(hits)\n";

/// Run `petal <args>` in `dir`; returns (stdout, stderr, exit code).
fn petal_in(dir: &Path, args: &[&str]) -> (String, String, i32) {
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

/// A fresh directory holding `counter.ptl`.
fn dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petal-state-storage-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("counter.ptl"), COUNTER).unwrap();
    dir
}

#[test]
fn the_same_file_twice_counts_one_then_two() {
    let d = dir("counts");
    let run = || petal_in(&d, &["run", "--state-storage", "s.json", "counter.ptl"]);
    assert_eq!(run(), ("1\n".to_string(), String::new(), 0));
    assert_eq!(run(), ("2\n".to_string(), String::new(), 0));
    // The shorthand takes the option too, on either side of the file.
    let (out, err, _) = petal_in(&d, &["counter.ptl", "--state-storage", "s.json"]);
    assert_eq!((out.as_str(), err.as_str()), ("3\n", ""));

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("s.json")).unwrap()).unwrap();
    assert_eq!(saved["format"], "petal-state");
    assert_eq!(saved["slots"][0]["name"], "hits");
    assert_eq!(saved["slots"][0]["value"], 3);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn without_the_option_state_warns_on_stderr_only() {
    let d = dir("warns");
    for _ in 0..2 {
        let (out, err, code) = petal_in(&d, &["run", "counter.ptl"]);
        assert_eq!((out.as_str(), code), ("1\n", 0));
        assert!(err.starts_with("warning: this script declares `state` (`hits`)"), "{err}");
        assert!(err.contains("--state-storage <file>"), "{err}");
    }
    // No `state`, no warning.
    let (out, err, _) = petal_in(&d, &["run", "-e", "let x = 1\nprint(x)"]);
    assert_eq!((out.as_str(), err.as_str()), ("1\n", ""));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_value_that_cannot_be_stored_is_a_warning_and_the_rest_is_saved() {
    let d = dir("skips");
    let src = "state f = fn(x) -> x + 1\nstate n = 0\nn += f(1)\nprint(n)\n";
    let run = || petal_in(&d, &["run", "--state-storage", "s.json", "-e", src]);
    let (out, err, code) = run();
    assert_eq!((out.as_str(), code), ("2\n", 0));
    assert!(err.contains("state `f` holds a function, which cannot be saved"), "{err}");
    // `f` starts over from its initializer; `n` carried.
    assert_eq!(run().0, "4\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_file_that_cannot_be_used_is_an_error_and_is_left_alone() {
    let d = dir("rejects");
    let run = |file: &str| petal_in(&d, &["run", "--state-storage", file, "counter.ptl"]);
    let cases = [
        ("corrupt.json", "{oops", "it is not valid JSON"),
        ("foreign.json", "{\"hits\": 3}", "it is not a Petal state storage file"),
        (
            "newer.json",
            "{\"format\": \"petal-state\", \"version\": 99, \"slots\": []}",
            "this petal reads version 1",
        ),
        (
            "other.json",
            "{\"format\": \"petal-state\", \"version\": 1, \
             \"slots\": [{\"name\": \"score\", \"key\": \"1\", \"value\": 9}]}",
            "written by a different script",
        ),
    ];
    for (file, contents, expected) in cases {
        std::fs::write(d.join(file), contents).unwrap();
        let (out, err, code) = run(file);
        // The script never ran, and the file was not overwritten.
        assert_eq!((out.as_str(), code), ("", 1), "{file}: {err}");
        assert!(err.contains(&format!("Cannot load state storage '{file}'")), "{err}");
        assert!(err.contains(expected), "{file}: {err}");
        assert_eq!(std::fs::read_to_string(d.join(file)).unwrap(), contents);
    }
    // An empty file is a first run, not a corrupt one.
    std::fs::write(d.join("empty.json"), "").unwrap();
    assert_eq!(run("empty.json"), ("1\n".to_string(), String::new(), 0));
    assert_eq!(run("empty.json").0, "2\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failed_run_keeps_the_last_good_file_and_stale_slots_are_dropped() {
    let d = dir("keeps");
    let run = |src: &str| petal_in(&d, &["run", "--state-storage", "s.json", "-e", src]);
    assert_eq!(run("state hits = 0\nstate old = 7\nhits += 1\nprint(hits)").0, "1\n");

    let (out, _, code) = run("state hits = 0\nstate old = 7\nhits += 100\nprint(hits)\nprint(nope)");
    assert_eq!((out.as_str(), code), ("101\n", 1));

    // `old` is gone from the script: named, dropped, and `hits` is still 1.
    let (out, err, code) = run(COUNTER);
    assert_eq!((out.as_str(), code), ("2\n", 0));
    assert!(err.contains("holds state this script does not declare (`old`)"), "{err}");
    assert_eq!(run(COUNTER), ("3\n".to_string(), String::new(), 0));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_option_is_in_the_help_and_needs_a_value() {
    let d = dir("help");
    let (out, _, _) = petal_in(&d, &["help", "run"]);
    assert!(out.contains("--state-storage <file>"), "{out}");
    let (_, err, code) = petal_in(&d, &["run", "counter.ptl", "--state-storage"]);
    assert_eq!(code, 1);
    assert!(err.contains("Expected a file path after --state-storage"), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}
