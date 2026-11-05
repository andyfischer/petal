//! `petal lint` as the one command that checks code without running it: the
//! compile check of every program it walks, then the rules; modules checked
//! through their importers; `--single-file`; and what fails the exit code.

use std::path::{Path, PathBuf};
use std::process::Command;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// A throwaway directory, removed on drop.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("petal-lint-walk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tree {
            root: std::fs::canonicalize(&root).unwrap(),
        }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Run `petal <args>` in `dir`; returns (stdout, stderr, exit code).
fn petal_in(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(PETAL)
        .args(args)
        .current_dir(dir)
        .env_remove("PETAL_PATH")
        .env("PETAL_OFFLINE", "1")
        .output()
        .expect("failed to run petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// `main.ptl` imports `level.ptl`, which has a type warning; `bad.ptl` reads
/// a name nothing defines; `style.ptl` is clean but for a `var` that could be
/// a `let`.
fn project(tag: &str) -> Tree {
    let t = Tree::new(tag);
    t.write("main.ptl", "import ./level\nprint(level.size())\n");
    t.write("level.ptl", "pub fn size()\n  let x: int = \"s\"\n  return 3\nend\n");
    t.write("bad.ptl", "print(nope)\n");
    t.write("style.ptl", "var y = 2\nprint(y)\n");
    t
}

#[test]
fn a_walk_checks_each_program_and_a_module_through_its_importer() {
    let t = project("walk");
    let (out, err, code) = petal_in(&t.root, &["lint"]);
    assert_eq!(code, 1, "{err}");
    // The program with an error, under its header, and the closing line.
    assert!(err.contains("==> ./bad.ptl <=="), "{err}");
    assert!(err.contains("undefined variable `nope`"), "{err}");
    assert!(err.contains("check failed for 1 of 3 program(s): ./bad.ptl"), "{err}");
    // The module's warning comes through the program that imports it…
    assert!(err.contains("==> ./main.ptl <=="), "{err}");
    assert!(err.contains("[level.ptl line 2, column 16]"), "{err}");
    // …and the module is not checked again on its own.
    assert!(!err.contains("==> ./level.ptl <=="), "{err}");
    // A clean program prints no header; its rule finding goes to stdout.
    assert!(!err.contains("==> ./style.ptl <=="), "{err}");
    assert!(out.contains("./style.ptl:1:1: prefer-let:"), "{out}");
}

#[test]
fn warnings_and_rule_findings_fail_only_a_strict_lint() {
    let t = project("strict");
    let (_, err, code) = petal_in(&t.root, &["lint", "main.ptl", "level.ptl", "style.ptl"]);
    assert_eq!(code, 0, "{err}");
    let (_, err, code) = petal_in(&t.root, &["lint", "--strict", "style.ptl"]);
    assert_eq!(code, 1, "{err}");
    // Once fixed, a finding no longer counts.
    let (_, err, code) = petal_in(&t.root, &["lint", "--strict", "--fix", "style.ptl"]);
    assert_eq!(code, 0, "{err}");
    // `--lenient` passes a check error, but not a program that does not compile.
    assert_eq!(petal_in(&t.root, &["lint", "--lenient", "bad.ptl"]).2, 0);
    t.write("broken.ptl", "print(\n");
    assert_eq!(petal_in(&t.root, &["lint", "--lenient", "broken.ptl"]).2, 1);
}

#[test]
fn single_file_checks_a_module_as_its_own_program() {
    let t = project("single");
    let (_, err, code) = petal_in(&t.root, &["lint", "--single-file", "level.ptl"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("type mismatch"), "{err}");
    assert!(!err.contains("==>"), "{err}");

    let (_, err, code) = petal_in(&t.root, &["lint", "--single-file", "."]);
    assert_eq!(code, 1);
    assert!(err.contains("is a directory"), "{err}");
    let (_, err, code) = petal_in(&t.root, &["lint", "--single-file", "a.ptl", "b.ptl"]);
    assert_eq!(code, 1);
    assert!(err.contains("--single-file takes one file"), "{err}");
}

#[test]
fn json_is_one_object_per_file_or_a_files_array_over_a_walk() {
    let t = project("json");
    let (out, err, code) = petal_in(&t.root, &["lint", "--json", "--single-file", "style.ptl"]);
    assert_eq!(code, 0, "{err}");
    let one: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(one["ok"], true);
    assert_eq!(one["warnings"], serde_json::json!([]));
    assert_eq!(one["findings"][0]["rule"], "prefer-let");

    let (out, _, code) = petal_in(&t.root, &["lint", "--json"]);
    assert_eq!(code, 1);
    let all: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(all["ok"], false);
    let files = all["files"].as_array().unwrap();
    let entry = |name: &str| {
        files
            .iter()
            .find(|f| f["file"] == name)
            .unwrap_or_else(|| panic!("no entry for {name}: {out}"))
    };
    assert_eq!(entry("./bad.ptl")["ok"], false);
    assert_eq!(entry("./level.ptl")["module"], true);
    assert!(entry("./level.ptl").get("warnings").is_none());
    assert_eq!(entry("./main.ptl")["warnings"][0]["file"], "level.ptl");
}
