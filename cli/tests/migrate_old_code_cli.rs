//! `petal migrate-old-code` on files on disk: rewriting in place, `--check`
//! (a diff and exit 1, writing nothing), idempotency, `--only` and `--list`,
//! the directory walk (which skips `target/` and `node_modules/`), `-e`,
//! `old-imports` running over the batch after the text migrations, and the
//! pointers from `petal lint` to it. The library half is
//! core/tests/all/migrate_old_code.rs.

use std::path::{Path, PathBuf};
use std::process::Command;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// A throwaway directory, removed on drop.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("petal-migrate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tree {
            root: std::fs::canonicalize(&root).unwrap(),
        }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap()
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

const OLD: &str = "\
export fn double(x)
  x * 2
end
const N = 4            # four
config let SPEED = 2.5
fn width(t)
  state(t) w = t * 8
  w
end
print(double(N) + SPEED + width(1))
";

const NEW: &str = "\
pub fn double(x)
  x * 2
end
let N = const(4)       # four
let SPEED = tunable(2.5)
fn width(t)
  memo(t) w = t * 8
  w
end
print(double(N) + SPEED + width(1))
";

#[test]
fn check_prints_a_diff_exits_one_and_writes_nothing() {
    let t = Tree::new("check");
    t.write("a.ptl", OLD);
    t.write("current.ptl", NEW);
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "--check", "."]);
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(out.contains("-const N = 4            # four"), "{out}");
    assert!(out.contains("+let N = const(4)       # four"), "{out}");
    assert!(out.contains("-export fn double(x)"), "{out}");
    assert!(out.contains("+pub fn double(x)"), "{out}");
    assert!(!out.contains("current.ptl"), "{out}");
    assert!(err.contains("1 of 2 file(s) need migrating"), "{err}");
    assert_eq!(t.read("a.ptl"), OLD);
    // `--dry-run` is the same flag.
    assert_eq!(petal_in(&t.root, &["migrate-old-code", "--dry-run", "."]).2, 1);
    assert_eq!(t.read("a.ptl"), OLD);
}

#[test]
fn it_rewrites_in_place_and_a_second_run_changes_nothing() {
    let t = Tree::new("write");
    t.write("a.ptl", OLD);
    let (before, _, code) = petal_in(&t.root, &["run", "a.ptl"]);
    assert_eq!(code, 0);

    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "a.ptl"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(t.read("a.ptl"), NEW);
    // It says what it did, file by file.
    assert!(out.starts_with("a.ptl\n"), "{out}");
    for rule in ["export-to-pub", "const-decl-deprecated", "config-let-deprecated", "state-key-deprecated"] {
        assert!(out.contains(rule), "{rule} missing from:\n{out}");
    }
    assert!(err.contains("migrated 1 of 1 file(s)"), "{err}");

    let (after, warn, code) = petal_in(&t.root, &["run", "a.ptl"]);
    assert_eq!(code, 0, "{warn}");
    assert_eq!(after, before);
    assert!(!warn.contains("migrate-old-code"), "{warn}");

    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "a.ptl"]);
    assert_eq!((out.as_str(), code), ("", 0), "{err}");
    assert_eq!(t.read("a.ptl"), NEW);
    assert_eq!(petal_in(&t.root, &["migrate-old-code", "--check", "a.ptl"]).2, 0);
}

#[test]
fn the_walk_skips_target_and_node_modules() {
    let t = Tree::new("walk");
    t.write("src/a.ptl", OLD);
    t.write("src/deep/b.ptl", OLD);
    t.write("target/debug/c.ptl", OLD);
    t.write("node_modules/pkg/d.ptl", OLD);
    t.write(".hidden/e.ptl", OLD);
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(t.read("src/a.ptl"), NEW);
    assert_eq!(t.read("src/deep/b.ptl"), NEW);
    assert_eq!(t.read("target/debug/c.ptl"), OLD);
    assert_eq!(t.read("node_modules/pkg/d.ptl"), OLD);
    assert_eq!(t.read(".hidden/e.ptl"), OLD);
    assert!(err.contains("migrated 2 of 2 file(s)"), "{err}");
}

#[test]
fn only_selects_migrations_and_list_shows_them() {
    let t = Tree::new("only");
    t.write("a.ptl", OLD);
    let (out, err, code) = petal_in(
        &t.root,
        &["migrate-old-code", "--only", "export-to-pub", "--only=config-let-deprecated", "a.ptl"],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    let a = t.read("a.ptl");
    assert!(a.starts_with("pub fn double(x)\n"), "{a}");
    assert!(a.contains("let SPEED = tunable(2.5)\n"), "{a}");
    assert!(a.contains("const N = 4"), "{a}");
    assert!(a.contains("state(t) w"), "{a}");

    let (_, err, code) = petal_in(&t.root, &["migrate-old-code", "--only", "prefer-pub", "a.ptl"]);
    assert_eq!(code, 1);
    assert!(err.contains("Unknown migration 'prefer-pub'"), "{err}");

    let (out, _, code) = petal_in(&t.root, &["migrate-old-code", "--list"]);
    assert_eq!(code, 0);
    for name in [
        "export-to-pub",
        "const-decl-deprecated",
        "config-let-deprecated",
        "state-key-deprecated",
        "old-imports",
    ] {
        let line = out.lines().find(|l| l.starts_with(name)).unwrap_or_else(|| panic!("{name}: {out}"));
        assert!(line.contains("2026-"), "{line}");
    }
}

#[test]
fn inline_code_prints_the_migration() {
    let t = Tree::new("inline");
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "-e", "const N = 4\nprint(N)\n"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "let N = const(4)\nprint(N)\n");
    let (out, _, code) = petal_in(&t.root, &["migrate-old-code", "--check", "-e", "const N = 4\n"]);
    assert_eq!(code, 1);
    assert!(out.contains("+let N = const(4)"), "{out}");
    let (_, _, code) = petal_in(&t.root, &["migrate-old-code", "--check", "-e", "let N = 4\n"]);
    assert_eq!(code, 0);
}

#[test]
fn a_site_left_for_a_hand_edit_is_reported_and_does_not_fail() {
    let t = Tree::new("hand");
    let src = "fn hp(id, d)\n  state(id) h = 100\n  h -= d\n  h\nend\nprint(hp(1, 2))\n";
    t.write("a.ptl", src);
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "--check", "."]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(err.contains("a.ptl:2:3: state-key-deprecated:"), "{err}");
    assert!(err.contains("(left for a hand edit)"), "{err}");
    assert!(err.contains("1 site(s) in an old form are left for a hand edit"), "{err}");
    assert_eq!(t.read("a.ptl"), src);
}

#[test]
fn an_at_rebind_is_rewritten_and_one_it_cannot_lift_is_listed() {
    let t = Tree::new("at-rebind");
    let src = "fn double(x)\n  x * 2\nend\nlet a = 1\ndouble(@a)\nprint(a)\n";
    t.write("a.ptl", src);
    let (_, err, code) = petal_in(&t.root, &["run", "a.ptl"]);
    assert_ne!(code, 0);
    assert!(err.contains("petal migrate-old-code"), "{err}");
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "."]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("5:8: at-rebind:"), "{out}");
    assert_eq!(t.read("a.ptl"), src.replace("double(@a)", "a = double(a)"));
    let (out, _, code) = petal_in(&t.root, &["run", "a.ptl"]);
    assert_eq!((out.trim(), code), ("2", 0));

    // Two in one call: listed, left, and the file's imports wait (no
    // second error for the parse the import pass would have needed).
    let stuck = "fn add(x, y)\n  x + y\nend\nlet a = 1\nlet b = 2\nadd(@a, @b)\n";
    t.write("b.ptl", stuck);
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "--check", "b.ptl"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(err.contains("b.ptl:6:5: at-rebind:"), "{err}");
    assert!(err.contains("2 site(s) in an old form are left for a hand edit"), "{err}");
    assert!(!err.contains("error:"), "{err}");
    assert_eq!(t.read("b.ptl"), stuck);
}

/// Every old form at once, over a module and the file importing it: `@`
/// rebinds, `export`, `const X`, both `config let` forms, a module's `const`,
/// `state(key)` and an old import. `state(key)` needs the file to compile to
/// prove its rewrite, which the old import (now an error) prevents until
/// `old-imports` has run, so one run must still finish the job.
#[test]
fn every_old_form_over_a_module_and_its_importer_in_one_run() {
    let t = Tree::new("all");
    let tune = "\
export const TABLE = {
  slow: 1,
  fast: 3,
}                         # speeds
export config let GRAVITY = 9.5
pub config let JUMP = tunable(4.0, 0, 10)   # already a tunable
export fn scale(x)
  x * 2
end
";
    let main = "\
import tune: *
const N = 4               # four
config let SPEED = 2.5    # px/frame
fn width(t)
  state(t) w = t * 8
  w
end
fn double(x)
  x * 2
end
let a = 5
double(@a)
let b = 3
let r = double(double(@b))
print(a, b, r)
print(N + SPEED + width(1))
print(TABLE.fast + GRAVITY + JUMP + scale(1))
";
    t.write("tune.ptl", tune);
    t.write("main.ptl", main);

    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "--check", "."]);
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(out.contains("-double(@a)\n+a = double(a)"), "{out}");
    assert!(out.contains("+pub let TABLE = const({"), "{out}");
    assert!(out.contains("+  memo(t) w = t * 8"), "{out}");
    assert!(!err.contains("hand edit"), "{err}");
    assert!(err.contains("2 of 2 file(s) need migrating"), "{err}");
    assert_eq!((t.read("tune.ptl"), t.read("main.ptl")), (tune.to_string(), main.to_string()));

    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "."]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(
        t.read("tune.ptl"),
        "\
pub let TABLE = const({
  slow: 1,
  fast: 3,
})                        # speeds
pub let GRAVITY = tunable(9.5)
pub let JUMP = tunable(4.0, 0, 10)          # already a tunable
pub fn scale(x)
  x * 2
end
"
    );
    assert_eq!(
        t.read("main.ptl"),
        "\
import ./tune
let N = const(4)          # four
let SPEED = tunable(2.5)  # px/frame
fn width(t)
  memo(t) w = t * 8
  w
end
fn double(x)
  x * 2
end
let a = 5
a = double(a)
let b = 3
b = double(b)
let r = double(b)
print(a, b, r)
print(N + SPEED + width(1))
print(TABLE.fast + GRAVITY + JUMP + scale(1))
"
    );
    // What the old program printed (`@` rebinds included), with no warning.
    let (out, err, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), code), ("10 6 12\n14.5\n18.5\n", 0), "{err}");
    assert_eq!(err, "");

    // A second run, and `--check`, find nothing.
    let migrated = (t.read("tune.ptl"), t.read("main.ptl"));
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "."]);
    assert_eq!((out.as_str(), code), ("", 0), "{err}");
    assert_eq!((t.read("tune.ptl"), t.read("main.ptl")), migrated);
    assert_eq!(petal_in(&t.root, &["migrate-old-code", "--check", "."]).2, 0);

    // `lint --fix` touches none of the old forms.
    t.write("tune.ptl", tune);
    t.write("main.ptl", main);
    petal_in(&t.root, &["lint", "--fix", "."]);
    assert_eq!((t.read("tune.ptl"), t.read("main.ptl")), (tune.to_string(), main.to_string()));
}

#[test]
fn a_file_that_does_not_parse_is_reported_and_the_rest_are_migrated() {
    let t = Tree::new("broken");
    t.write("a.ptl", OLD);
    t.write("broken.ptl", "const = \n");
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "."]);
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(err.contains("broken.ptl"), "{err}");
    assert_eq!(t.read("a.ptl"), NEW);
}

#[test]
fn old_imports_run_after_the_text_migrations_over_the_batch() {
    let t = Tree::new("imports");
    t.write("level.ptl", "export const NAME = \"one\"\n");
    t.write("main.ptl", "import level: *\nprint(NAME)\n");
    let (_, err, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_ne!(code, 0);
    assert!(err.contains("petal migrate-old-code"), "{err}");

    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "."]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(t.read("level.ptl"), "pub let NAME = const(\"one\")\n");
    assert_eq!(t.read("main.ptl"), "import ./level\nprint(NAME)\n");
    assert!(out.contains("old-imports: line 1: `import level` -> `import ./level`"), "{out}");
    assert!(out.contains("old-imports: line 1: `import level: *` -> `import level`"), "{out}");
    let (out, err, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), code), ("one\n", 0), "{err}");
    assert!(!err.contains("deprecated"), "{err}");

    // `--only` without it leaves the imports alone.
    t.write("main.ptl", "import level: *\nprint(NAME)\n");
    let (_, _, code) = petal_in(&t.root, &["migrate-old-code", "--only", "export-to-pub", "--check", "."]);
    assert_eq!(code, 0);
}

#[test]
fn lint_fix_leaves_old_syntax_and_points_here() {
    let t = Tree::new("lint");
    t.write("a.ptl", OLD);
    let (out, err, _) = petal_in(&t.root, &["lint", "--fix", "a.ptl"]);
    assert_eq!(t.read("a.ptl"), OLD, "{out}\n{err}");
    // The compile check warns about the ones the compiler sees …
    assert!(err.contains("run `petal migrate-old-code` [const-decl-deprecated]"), "{err}");
    assert!(err.contains("run `petal migrate-old-code` [state-key-deprecated]"), "{err}");
    // … and `old-syntax` reports the spelling it does not.
    assert!(out.contains("a.ptl:1:1: old-syntax:"), "{out}");

    // A rule that moved says where it went.
    for rule in ["prefer-pub", "const-decl-deprecated", "state-key-deprecated"] {
        let flag = format!("--rules-include={rule}");
        let (_, err, code) = petal_in(&t.root, &["lint", &flag, "a.ptl"]);
        assert_eq!(code, 1, "{rule}");
        assert!(err.contains("'petal migrate-old-code'"), "{rule}: {err}");
    }
}

#[test]
fn help_lists_the_command() {
    let t = Tree::new("help");
    let (out, _, code) = petal_in(&t.root, &["help"]);
    assert_eq!(code, 0);
    assert!(out.contains("migrate-old-code"), "{out}");
    let (out, _, code) = petal_in(&t.root, &["help", "migrate-old-code"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("NAME\n       petal-migrate-old-code"), "{out}");
}
