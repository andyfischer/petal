//! `petal check` on a script whose project describes its host (`[host]` in
//! `petal.toml`): the script is compiled as that host would compile it, the
//! way `petal lsp` analyzes it, and what is found is reported against the
//! script's own lines.

use std::path::{Path, PathBuf};
use std::process::Command;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

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

/// A fresh project holding `files` (path, contents).
fn project(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petal-check-host-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (name, contents) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    dir
}

/// A host that binds `color` without an import and joins two library files
/// ahead of each script. `lib/more.ptl` imports a file beside it.
const FILES: &[(&str, &str)] = &[
    (
        "petal.toml",
        "[package]\nname = \"panels\"\n\n[host]\nimports = [\"color\"]\nprelude = [\"lib/base.ptl\", \"lib/more.ptl\"]\n",
    ),
    (
        "lib/base.ptl",
        "// not pub: the host joins files\nfn helper(x)\n  x + 1\nend\n",
    ),
    (
        "lib/more.ptl",
        "import ./util\n\nfn twice(x)\n  helper(helper(x))\nend\n",
    ),
    ("lib/util.ptl", "pub fn util_fn(x)\n  x * 2\nend\n"),
    (
        "screens/menu.ptl",
        "let c = color_lerp(#000000, #ffffff, 0.5)\nprint(twice(util_fn(1)))\n",
    ),
];

#[test]
fn a_script_is_checked_with_what_its_host_puts_in_scope() {
    let dir = project("clean", FILES);
    let (out, err, code) = petal_in(&dir, &["check", "screens/menu.ptl"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.is_empty(), "{err}");

    // A prelude file is checked with the files listed before it, and its
    // relative import resolves beside it.
    let (out, err, code) = petal_in(&dir, &["check", "lib/more.ptl"]);
    assert_eq!(code, 0, "{out}{err}");
}

#[test]
fn a_finding_is_reported_on_the_scripts_own_line() {
    let mut files = FILES.to_vec();
    files[4] = ("screens/menu.ptl", "print(twice(1))\n\nnope()\n");
    let dir = project("finding", &files);
    let (_, err, code) = petal_in(&dir, &["check", "screens/menu.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("unknown function `nope`"), "{err}");
    assert!(err.contains("[line 3, column 1]"), "{err}");
    // The snippet quotes the script, not the joined program.
    assert!(err.contains("3 | nope()"), "{err}");
    assert_eq!(err.matches("error:").count(), 1, "{err}");

    let (out, _, _) = petal_in(&dir, &["check", "--json", "screens/menu.ptl"]);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["warnings"][0]["line"], 3, "{out}");
}

#[test]
fn a_failed_compile_names_the_file_it_is_in() {
    let mut files = FILES.to_vec();
    files[1] = ("lib/base.ptl", "fn helper(x)\n  x +\nend\n");
    let dir = project("broken-prelude", &files);
    let (_, err, code) = petal_in(&dir, &["check", "screens/menu.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("lib/base.ptl: "), "{err}");
    assert!(!err.contains("screens/menu.ptl"), "{err}");

    let mut files = FILES.to_vec();
    files[4] = ("screens/menu.ptl", "print(1)\nlet x = (\n");
    let dir = project("broken-script", &files);
    let (_, err, code) = petal_in(&dir, &["check", "screens/menu.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(!err.contains("lib/"), "{err}");
    assert!(
        err.contains("[line 2, column") || err.contains("[line 3, column"),
        "{err}"
    );
}

#[test]
fn a_missing_prelude_file_is_a_warning() {
    let mut files = FILES.to_vec();
    files[0] = (
        "petal.toml",
        "[package]\nname = \"panels\"\n\n[host]\nprelude = [\"lib/gone.ptl\"]\n",
    );
    files[4] = ("screens/menu.ptl", "print(1)\n");
    let dir = project("missing", &files);
    let (_, err, code) = petal_in(&dir, &["check", "screens/menu.ptl"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("warning: petal.toml [host] prelude: cannot read lib/gone.ptl"),
        "{err}"
    );
}
