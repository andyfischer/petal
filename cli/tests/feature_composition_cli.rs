//! The 2026-11 language features through the `petal` command, on files on
//! disk: the `const` / `config let` deprecation and its
//! `migrate-old-code` round trip over a program that uses a module's enum (with methods) through
//! qualified and nullable annotations, and `petal suggest` proposing a
//! nullable annotation of a module-scoped type the way the file writes it.
//! The in-memory side of the same compositions is
//! `core/tests/all/feature_composition.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// A throwaway directory, removed on drop.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("petal-compose-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tree { root: std::fs::canonicalize(&root).unwrap() }
    }

    fn write(&self, rel: &str, content: &str) {
        std::fs::write(self.root.join(rel), content).unwrap();
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

const LEVEL: &str = "\
pub enum Kind
  a, b(n),
end
fn Kind.label(k: Kind) -> string
  match k
    when Kind.b(n) -> \"b{n}\"
    when _ -> k.name
  end
end
const STEP = 3
pub fn step() -> int
  STEP
end
";

const MAIN: &str = "\
import ./level as lv
enum Kind
  a,
end
const K = lv.Kind.b(2)
config let SPEED = 2.5 # knob
pub config let G = tunable(9.8, 0, 20)
fn show(k: lv.Kind?) -> string
  if k == nil then \"none\" else k.label() end
end
print(show(K), show(nil), Kind.a.name, SPEED + G, lv.step())
";

/// Compile warnings, then `migrate-old-code`, then the same program again:
/// the migration removes every deprecation (the module's `const` too), the
/// output is unchanged, and a second run has nothing left to do. `lint --fix`
/// leaves the old forms alone.
#[test]
fn deprecated_declarations_warn_and_migrate_old_code_round_trips() {
    let t = Tree::new("fix");
    t.write("level.ptl", LEVEL);
    t.write("main.ptl", MAIN);

    let (before, warn, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!(code, 0, "{warn}");
    assert_eq!(before, "b2 none a 12.3 3\n");
    for needle in [
        "`const K = e` is deprecated; write `let K = const(e)`; run `petal migrate-old-code` \
         [const-decl-deprecated]",
        "`config let SPEED = v` is deprecated; write `let SPEED = tunable(v)`; run \
         `petal migrate-old-code` [config-let-deprecated]",
        "drop `config`; run `petal migrate-old-code` [config-let-deprecated]",
        "`const STEP = e` is deprecated",
    ] {
        assert!(warn.contains(needle), "missing {needle:?} in:\n{warn}");
    }

    // `lint --fix` carries only the current syntax.
    let (out, err, code) = petal_in(&t.root, &["lint", "--fix", "."]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(t.read("main.ptl"), MAIN);
    assert_eq!(t.read("level.ptl"), LEVEL);

    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "."]);
    assert_eq!(code, 0, "{out}\n{err}");
    let main = t.read("main.ptl");
    assert!(main.contains("let K = const(lv.Kind.b(2))\n"), "{main}");
    assert!(main.contains("let SPEED = tunable(2.5) # knob\n"), "{main}");
    assert!(main.contains("pub let G = tunable(9.8, 0, 20)\n"), "{main}");
    assert!(t.read("level.ptl").contains("let STEP = const(3)\n"), "{}", t.read("level.ptl"));

    let (after, warn, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!(code, 0, "{warn}");
    assert_eq!(after, before);
    assert!(!warn.contains("deprecated"), "{warn}");

    let fixed = (t.read("main.ptl"), t.read("level.ptl"));
    let (out, err, code) = petal_in(&t.root, &["migrate-old-code", "--check", "."]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.is_empty(), "{out}");
    let (out, err, _) = petal_in(&t.root, &["migrate-old-code", "."]);
    assert!(!out.contains("deprecated") && !err.contains("deprecated"), "{out}\n{err}");
    assert_eq!((t.read("main.ptl"), t.read("level.ptl")), fixed, "a second run changed the files");
}

/// `petal suggest` proposes `T?` for a parameter whose call sites pass an
/// imported module's enum and `nil`, spelled as the file writes it
/// (`lv.Kind?`) — not the identity a diagnostic prints (`./level::Kind?`),
/// which is not valid in an annotation — while the file's own same-named
/// enum stays bare. `--apply` writes both and the result compiles clean.
#[test]
fn suggest_spells_a_module_scoped_nullable_through_the_files_qualifier() {
    let t = Tree::new("suggest");
    t.write("level.ptl", &LEVEL.replace("const STEP = 3", "let STEP = const(3)"));
    t.write(
        "main.ptl",
        "import ./level as lv
enum Kind
  z,
end
fn show(k)
  if k == nil then \"none\" else k.label() end
end
fn mine(k)
  if k == nil then \"none\" else k.name end
end
print(show(lv.Kind.a), show(nil), mine(Kind.z), mine(nil))
",
    );
    let (out, err, code) = petal_in(&t.root, &["suggest", "main.ptl"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("suggest: k: lv.Kind?"), "{out}");
    assert!(out.contains("suggest: k: Kind?"), "{out}");
    assert!(!out.contains("./level::Kind?\n"), "{out}");

    let (out, err, code) = petal_in(&t.root, &["suggest", "--apply", "main.ptl"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(!err.contains("dropped"), "{err}");
    let main = t.read("main.ptl");
    assert!(main.contains("fn show(k: lv.Kind?)"), "{main}");
    assert!(main.contains("fn mine(k: Kind?)"), "{main}");
    let (out, err, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), err.as_str(), code), ("a none z none\n", "", 0));
}
