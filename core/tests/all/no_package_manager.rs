//! A host that does not register a package manager — every embedder of the
//! `petal` library, which leaves `petal-pkgman` out — loads path
//! dependencies as ever, and a git dependency is an error that says what to
//! do instead. (With the package manager: cli/tests/pkgman.rs.)

use petal::env::Env;

fn world(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("petal-no-pm-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::create_dir_all(root.join("lib")).unwrap();
    root
}

#[test]
fn a_git_dependency_needs_a_package_manager() {
    assert!(petal::package_manager::registered().is_none());
    let root = world("git");
    std::fs::write(
        root.join("app/petal.toml"),
        "[package]\nname = \"game\"\n[dependencies]\nease = { git = \"https://example.com/ease\", tag = \"v1.0.0\" }\n",
    )
    .unwrap();
    let main = root.join("app/main.ptl");
    std::fs::write(&main, "import ease\nprint(quad(2))\n").unwrap();
    let err = Env::new()
        .load_program_at(&std::fs::read_to_string(&main).unwrap(), &main)
        .unwrap_err();
    assert!(err.contains("is a git dependency"), "{err}");
    assert!(err.contains("no package manager"), "{err}");
    assert!(err.contains("petal bundle"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_path_dependency_needs_none() {
    let root = world("path");
    std::fs::write(
        root.join("app/petal.toml"),
        "[package]\nname = \"game\"\n[dependencies]\nlib = { path = \"../lib\" }\n",
    )
    .unwrap();
    std::fs::write(root.join("lib/petal.toml"), "[package]\nname = \"lib\"\n").unwrap();
    std::fs::write(root.join("lib/lib.ptl"), "pub fn twice(x)\n  x * 2\nend\n").unwrap();
    let main = root.join("app/main.ptl");
    std::fs::write(&main, "import lib\nprint(twice(21))\n").unwrap();
    let mut env = Env::new();
    let pid = env
        .load_program_at(&std::fs::read_to_string(&main).unwrap(), &main)
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["42"]);
    let _ = std::fs::remove_dir_all(&root);
}
