//! The manifest: `[dependencies]`, `[requires]`, discovery by walking up from
//! the file, and what a host provides.
//!
//! - a `[dependencies]` key is an import name; a `path` resolves against the
//!   manifest's directory, for the files of that package;
//! - a `git` dependency and a registry version parse and are clear errors
//!   where they are imported;
//! - every file finds its manifest by walking up, with no flag;
//! - `[requires]` is checked against `Env::provides` when a program is
//!   loaded to run, and only reported by a check-only compile.

use petal::env::Env;
use std::path::{Path, PathBuf};

/// A throwaway directory tree, removed on drop.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("petal-manifest-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tree {
            root: std::fs::canonicalize(&root).unwrap(),
        }
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn run_file(env: &mut Env, file: &Path) -> Result<Vec<String>, String> {
    let source = std::fs::read_to_string(file).unwrap();
    let pid = env.load_program_at(&source, file)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output())
}

/// app/ depends on ../libs/tween under the name `tw`; tween depends on
/// ../base.
fn app_tree(tag: &str) -> Tree {
    let t = Tree::new(tag);
    t.write(
        "app/petal.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
         tw = { path = \"../libs/tween\" }\n",
    );
    t.write(
        "libs/tween/petal.toml",
        "[package]\nname = \"tween\"\nversion = \"1.2.0\"\n\n[dependencies]\n\
         base = { path = \"../base\" }\n",
    );
    t.write("libs/base/petal.toml", "[package]\nname = \"base\"\n");
    t.write("libs/base/base.ptl", "pub fn two()\n  2\nend\n");
    t.write(
        "libs/tween/src/tween.ptl",
        "import base\nimport ./ease\npub fn go()\n  two() + ease.one()\nend\n",
    );
    t.write("libs/tween/src/ease.ptl", "pub fn one()\n  1\nend\n");
    t
}

// ── [dependencies] ───────────────────────────────────────────────────────

#[test]
fn a_path_dependency_is_imported_by_its_key() {
    let t = app_tree("key");
    let main = t.write(
        "app/main.ptl",
        "import tw\nimport tw/ease\nprint(tw.go(), go(), ease.one())\n",
    );
    let mut env = Env::new();
    assert_eq!(run_file(&mut env, &main).unwrap(), ["3 3 1"]);
}

#[test]
fn a_dependency_is_one_module_under_its_own_package_name() {
    // `tw` is the app's name for it; the module is `tween`, however reached.
    let t = app_tree("identity");
    let main = t.write("app/main.ptl", "import tw\nimport ./src/helper\nprint(helper.h())\n");
    t.write("app/src/helper.ptl", "import tw\npub fn h()\n  go() * 10\nend\n");
    let env = Env::new();
    assert_eq!(env.resolve_import("tw", &main).unwrap(), "tween");
    assert_eq!(env.resolve_import("tw/ease", &main).unwrap(), "tween/ease");
    let mut env = Env::new();
    assert_eq!(run_file(&mut env, &main).unwrap(), ["30"]);
}

#[test]
fn a_dependency_resolves_its_own_dependencies_from_its_own_manifest() {
    // The app never names `base`; tween's manifest does.
    let t = app_tree("transitive");
    let main = t.write("app/main.ptl", "import base\nprint(two())\n");
    let mut env = Env::new();
    // The app has no `base`…
    let err = run_file(&mut env, &main).unwrap_err();
    assert!(err.contains("there is no package named 'base'"), "{err}");
    // …but tween reaches it.
    let main = t.write("app/main.ptl", "import tw\nprint(go())\n");
    assert_eq!(run_file(&mut env, &main).unwrap(), ["3"]);
}

#[test]
fn the_manifest_is_found_by_walking_up_from_the_file() {
    let t = app_tree("walk");
    let deep = t.write("app/screens/menu/main.ptl", "import tw\nprint(go())\n");
    let mut env = Env::new();
    assert_eq!(run_file(&mut env, &deep).unwrap(), ["3"]);
    assert_eq!(
        petal::package::find_manifest(&deep).unwrap(),
        t.root.join("app/petal.toml")
    );
    let project = env.project_of(&deep).unwrap().unwrap();
    assert_eq!(project.name, "app");
    assert_eq!(project.dependencies.len(), 1);
    // A file with no manifest above it has no project.
    let loose = t.write("loose.ptl", "print(1)\n");
    if petal::package::find_manifest(&loose).is_none() {
        assert!(env.project_of(&loose).unwrap().is_none());
    }
}

#[test]
fn a_manifest_dependency_wins_over_a_bundled_package_of_the_same_name() {
    let t = Tree::new("override");
    t.write(
        "app/petal.toml",
        "[package]\nname = \"app\"\n[dependencies]\nnoise = { path = \"vendor/noise\" }\n",
    );
    t.write("app/vendor/noise/petal.toml", "[package]\nname = \"noise\"\n");
    t.write("app/vendor/noise/noise.ptl", "pub fn mine()\n  \"project\"\nend\n");
    let main = t.write("app/main.ptl", "import noise\nprint(mine())\n");
    let mut env = Env::new();
    assert_eq!(run_file(&mut env, &main).unwrap(), ["project"]);
    // A host registration still wins over the manifest.
    env.register_module("noise", "pub fn mine()\n  \"host\"\nend\n");
    assert_eq!(run_file(&mut env, &main).unwrap(), ["host"]);
}

#[test]
fn a_git_dependency_and_a_registry_version_are_clear_errors_where_imported() {
    let t = Tree::new("unfetched");
    t.write(
        "app/petal.toml",
        "[package]\nname = \"app\"\n[dependencies]\n\
         remote = { git = \"https://example.org/remote\", tag = \"v1.0.0\" }\n\
         pinned = { git = \"https://example.org/pinned\", rev = \"abc123\" }\n\
         shapes = \"0.3\"\n",
    );
    let mut env = Env::new();
    // Not imported: not a problem.
    let fine = t.write("app/fine.ptl", "print(1)\n");
    assert_eq!(run_file(&mut env, &fine).unwrap(), ["1"]);

    let main = t.write("app/main.ptl", "import remote\nprint(1)\n");
    let err = run_file(&mut env, &main).unwrap_err();
    assert!(err.contains("git dependency (https://example.org/remote)"), "{err}");

    let main = t.write("app/main.ptl", "import shapes\nprint(1)\n");
    let err = run_file(&mut env, &main).unwrap_err();
    assert!(err.contains("registries are not supported yet"), "{err}");

    let deps = env.project_dependencies(&main);
    let names: Vec<&str> = deps.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["remote", "pinned", "shapes"]);
    assert!(deps.iter().all(|(_, p)| p.is_err()));
}

#[test]
fn a_path_dependency_that_is_not_a_package_says_which_one() {
    let t = Tree::new("missing");
    t.write(
        "app/petal.toml",
        "[package]\nname = \"app\"\n[dependencies]\ngone = { path = \"../gone\" }\n",
    );
    let main = t.write("app/main.ptl", "import gone\nprint(1)\n");
    let err = run_file(&mut Env::new(), &main).unwrap_err();
    assert!(err.contains("dependency 'gone' of package 'app'"), "{err}");
    assert!(err.contains("path = \"../gone\""), "{err}");
}

#[test]
fn a_relative_import_may_not_leave_the_package_root() {
    let t = app_tree("confined");
    t.write("outside.ptl", "pub fn x()\n  1\nend\n");
    let main = t.write("app/main.ptl", "import ../outside\nprint(x())\n");
    let err = run_file(&mut Env::new(), &main).unwrap_err();
    assert!(err.contains("leaves the package 'app'"), "{err}");
    // A loose script — no manifest above it — has no such limit.
    let l = Tree::new("loose-up");
    l.write("shared/rng.ptl", "pub fn x()\n  4\nend\n");
    let main = l.write("game/main.ptl", "import ../shared/rng\nprint(x())\n");
    if petal::package::find_manifest(&main).is_none() {
        assert_eq!(run_file(&mut Env::new(), &main).unwrap(), ["4"]);
    }
}

#[test]
fn inline_code_uses_the_manifest_of_the_inline_import_directory() {
    let t = app_tree("inline");
    let mut env = Env::new();
    env.set_inline_import_dir(Some(t.root.join("app")));
    let pid = env.load_program("import tw\nprint(go())\n").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["3"]);
}

#[test]
fn a_nested_package_is_not_part_of_the_one_around_it() {
    let t = app_tree("nested");
    t.write("app/tools/petal.toml", "[package]\nname = \"tools\"\n");
    t.write("app/tools/gen.ptl", "pub fn g()\n  1\nend\n");
    t.write("app/target/junk.ptl", "pub fn j()\n  1\nend\n");
    t.write("app/level.ptl", "pub fn l()\n  1\nend\n");
    let project = Env::new()
        .project_of(&t.root.join("app/petal.toml"))
        .unwrap()
        .unwrap();
    assert_eq!(project.modules, ["level"]);
}

// ── [requires] and Env::provides ─────────────────────────────────────────

/// app requires `gfx >=0.2`; its dependency tween requires `audio ^1.0`.
fn requiring_tree(tag: &str) -> (Tree, PathBuf) {
    let t = Tree::new(tag);
    t.write(
        "app/petal.toml",
        "[package]\nname = \"app\"\n[dependencies]\ntween = { path = \"../tween\" }\n\
         [requires]\ngfx = \">=0.2\"\n",
    );
    t.write(
        "tween/petal.toml",
        "[package]\nname = \"tween\"\n[requires]\naudio = \"^1.0\"\n",
    );
    t.write("tween/tween.ptl", "pub fn go()\n  3\nend\n");
    let main = t.write("app/main.ptl", "import tween\nprint(go())\n");
    (t, main)
}

#[test]
fn loading_fails_when_the_host_does_not_provide_a_required_interface() {
    let (_t, main) = requiring_tree("requires");
    let mut env = Env::new();
    let err = run_file(&mut env, &main).unwrap_err();
    assert!(
        err.contains("package 'app' requires the host interface `gfx` >=0.2, which this host does not provide"),
        "{err}"
    );
    // Transitive: the dependency's requirement is the program's too.
    assert!(err.contains("package 'tween' requires the host interface `audio` ^1.0"), "{err}");
    assert!(err.contains("the program cannot run here"), "{err}");

    env.provides("gfx", "0.2.0").unwrap();
    env.provides("audio", "1.4").unwrap();
    assert_eq!(run_file(&mut env, &main).unwrap(), ["3"]);
}

#[test]
fn an_incompatible_version_is_a_load_error_naming_both_versions() {
    let (_t, main) = requiring_tree("versions");
    let mut env = Env::new();
    env.provides("gfx", "0.1.9").unwrap();
    env.provides("audio", "2.0.0").unwrap();
    let err = run_file(&mut env, &main).unwrap_err();
    assert!(err.contains("`gfx` >=0.2, and this host provides `gfx` 0.1.9"), "{err}");
    // `^1.0` does not accept the next major version.
    assert!(err.contains("`audio` ^1.0, and this host provides `audio` 2.0.0"), "{err}");
    // Announcing again replaces the version.
    env.provides("gfx", "3").unwrap();
    env.provides("audio", "1.0").unwrap();
    assert_eq!(run_file(&mut env, &main).unwrap(), ["3"]);
    assert!(env.provides("gfx", "soon").is_err());
}

#[test]
fn a_requirement_applies_only_when_its_package_is_loaded() {
    // The app's own requirement always applies; a dependency's only when one
    // of its modules is part of the program.
    let (t, _main) = requiring_tree("reached");
    let alone = t.write("app/alone.ptl", "print(1)\n");
    let mut env = Env::new();
    env.provides("gfx", "1.0").unwrap();
    assert_eq!(run_file(&mut env, &alone).unwrap(), ["1"]);
}

#[test]
fn a_check_only_compile_reports_requirements_and_does_not_fail_on_them() {
    let (_t, main) = requiring_tree("check");
    let mut env = Env::new();
    env.set_check_only(true);
    let source = std::fs::read_to_string(&main).unwrap();
    let pid = env.load_program_at(&source, &main).unwrap();
    let requires: Vec<String> = env
        .get_program(pid)
        .unwrap()
        .requires
        .iter()
        .map(|r| r.to_string())
        .collect();
    assert_eq!(requires, ["audio ^1.0 (tween)", "gfx >=0.2 (app)"]);
}

#[test]
fn every_env_provides_core() {
    let env = Env::new();
    let provided = env.provided_interfaces();
    assert_eq!(provided.len(), 1);
    assert_eq!(provided[0].0, "core");
    assert_eq!(env.provided_version("core").as_deref(), Some(env!("CARGO_PKG_VERSION")));

    let t = Tree::new("core");
    t.write("app/petal.toml", "[package]\nname = \"app\"\n[requires]\ncore = \">=0.0.1\"\n");
    let main = t.write("app/main.ptl", "print(1)\n");
    assert_eq!(run_file(&mut Env::new(), &main).unwrap(), ["1"]);
    t.write("app/petal.toml", "[package]\nname = \"app\"\n[requires]\ncore = \">=999\"\n");
    let err = run_file(&mut Env::new(), &main).unwrap_err();
    assert!(err.contains("`core` >=999, and this host provides `core`"), "{err}");
}

#[test]
fn a_package_registered_from_memory_may_carry_its_manifest() {
    let mut env = Env::new();
    let info = env
        .register_package(
            "kit",
            [
                ("kit", "pub fn k()\n  1\nend\n"),
                (
                    "petal.toml",
                    "[package]\nname = \"kit\"\nversion = \"2.1.0\"\n[requires]\ngfx = \">=1\"\n",
                ),
            ],
        )
        .unwrap();
    assert_eq!(info.version.as_deref(), Some("2.1.0"));
    assert_eq!(info.modules, ["kit"]);
    let err = env.load_program("import kit\nprint(k())\n").unwrap_err();
    assert!(err.contains("package 'kit' requires the host interface `gfx` >=1"), "{err}");

    // `provides_package` announces a registered package at its own version.
    env.register_package(
        "gfx",
        [("gfx", ""), ("petal.toml", "[package]\nname = \"gfx\"\nversion = \"1.5\"\n")],
    )
    .unwrap();
    env.provides_package("gfx").unwrap();
    assert_eq!(env.provided_version("gfx").as_deref(), Some("1.5"));
    assert!(env.load_program("import kit\nprint(k())\n").is_ok());
    assert!(env.provides_package("nope").is_err());
    assert!(env.provides_package("kit").is_ok());

    // A manifest for some other package is a mistake worth hearing about.
    let err = env
        .register_package("kit", [("petal.toml", "[package]\nname = \"other\"\n")])
        .unwrap_err()
        .to_string();
    assert!(err.contains("names the package 'other'"), "{err}");
}

#[test]
fn bundled_bloom_depends_on_text_layout_and_requires_ui() {
    let bloom = Env::bundled_packages()
        .into_iter()
        .find(|p| p.name == "bloom")
        .unwrap();
    assert!(bloom.dependency("text_layout").is_some());
    let requires: Vec<String> = bloom
        .requires
        .iter()
        .map(|r| format!("{} {}", r.interface, r.version))
        .collect();
    assert_eq!(requires, ["ui >=0.1"]);

    // Checking a bloom script needs no host; it reports what a run will need.
    let mut env = Env::new();
    env.set_check_only(true);
    let pid = env.load_program("import ui\nimport bloom\nprint(1)\n").unwrap();
    let requires = &env.get_program(pid).unwrap().requires;
    assert!(requires.iter().any(|r| r.interface == "ui" && r.package == "bloom"), "{requires:?}");

    // Running it in a host that does not provide `ui` fails on that, with
    // the runner hint, before any line runs.
    let err = Env::new()
        .load_program("import ui\nimport bloom\nprint(1)\n")
        .unwrap_err();
    assert!(err.contains("package 'bloom' requires the host interface `ui`"), "{err}");
    assert!(err.contains("petal-ui runner"), "{err}");
}

// ── tools ────────────────────────────────────────────────────────────────

#[test]
fn importable_paths_include_the_projects_dependencies() {
    let t = app_tree("importable");
    let main = t.write("app/main.ptl", "print(1)\n");
    let env = Env::new();
    let paths = env.importable_paths_at(&main);
    assert!(paths.contains(&"tw".to_string()), "{paths:?}");
    assert!(!env.importable_paths().contains(&"tw".to_string()));
    let (exports, _) = env.exports_and_modules_of_at("tw", Some(&main)).unwrap();
    assert_eq!(exports, ["go"]);
}

#[test]
fn fix_imports_adds_the_import_of_a_manifest_dependency() {
    let t = app_tree("fix");
    let main = t.write("app/main.ptl", "print(go())\n");
    let mut fixer = petal::fix_imports::ImportFixer::new(&[]);
    let fixed = fixer.fix("print(go())\n", Some(&main)).unwrap();
    assert_eq!(fixed.source, "import tw\n\nprint(go())\n", "{:?}", fixed.notes);
}

#[test]
fn the_language_server_resolves_a_manifest_dependency_with_no_configuration() {
    let t = app_tree("lsp");
    let source = "import tw\nprint(go())\n";
    let main = t.write("app/main.ptl", source);
    let analysis = petal::lsp::document::analyze_at(source, Some(&main), &[]);
    assert!(analysis.diagnostics.is_empty(), "{:?}", analysis.diagnostics);
    let uri = petal::lsp::document::path_to_uri(&main);
    let completions = petal::lsp::document::import_path_completions("t", &uri);
    assert!(completions.contains(&"tw".to_string()), "{completions:?}");
}
