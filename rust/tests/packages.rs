// Packages: `petal.toml` manifests and one-call registration (src/package.rs,
// docs/module-system.md#packages).
//
// The embedder-facing half of the feature — `Env::add_package`,
// `Env::register_package`, `Env::packages` — which the CLI cannot reach. The
// CLI half is covered by ts/test/packages.test.ts.

use petal::env::Env;
use std::path::PathBuf;

/// Run `entry` in `env` and return its print output.
fn run(env: &mut Env, entry: &str) -> Vec<String> {
    let pid = env.load_program(entry).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.take_output()
}

/// Write a two-module library to `<tmp>/petal-pkg-test-<tag>/bloom/`, with the
/// given manifest text. `menu` imports its sibling with a flat name.
fn write_library(tag: &str, manifest: &str, module_dir: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("petal-pkg-test-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("bloom");
    let modules = if module_dir.is_empty() {
        lib.clone()
    } else {
        lib.join(module_dir)
    };
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(lib.join("petal.toml"), manifest).unwrap();
    std::fs::write(
        modules.join("menu.ptl"),
        "import motion\nexport fn open()\n  motion.ease(\"open\")\nend\n",
    )
    .unwrap();
    std::fs::write(
        modules.join("motion.ptl"),
        "export fn ease(x)\n  x ++ \" eased\"\nend\n",
    )
    .unwrap();
    root
}

const MANIFEST: &str = "[package]\nname = \"bloom\"\nversion = \"0.1.0\"\nmodules = \"src\"\n";

#[test]
fn add_package_makes_a_whole_library_reachable_in_one_call() {
    let root = write_library("add", MANIFEST, "src");
    let mut env = Env::new();
    let info = env.add_package(root.join("bloom")).unwrap();
    assert_eq!(info.name, "bloom");
    assert_eq!(info.version.as_deref(), Some("0.1.0"));
    assert_eq!(info.modules, vec!["menu", "motion"]);

    // One call, and every module of the library is importable by path.
    assert_eq!(
        run(&mut env, "import bloom/menu\nprint(menu.open())"),
        vec!["open eased"]
    );
    // The selective form works the same.
    assert_eq!(
        run(&mut env, "import bloom/menu: open\nprint(open())"),
        vec!["open eased"]
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_search_path_picks_up_the_libraries_under_it() {
    let root = write_library("search", MANIFEST, "src");
    let mut env = Env::new();
    // Pointed at the *parent*: discovery finds `bloom/` one level down.
    env.add_module_path(root.clone());
    assert!(env.package_errors().is_empty());
    assert_eq!(env.packages().len(), 1);
    assert_eq!(
        run(&mut env, "import bloom/motion\nprint(motion.ease(\"x\"))"),
        vec!["x eased"]
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_manifest_with_no_modules_key_defaults_to_the_manifest_directory() {
    let root = write_library("flat", "[package]\nname = \"bloom\"\n", "");
    let mut env = Env::new();
    let info = env.add_package(root.join("bloom")).unwrap();
    assert_eq!(info.module_dir, root.join("bloom"));
    assert_eq!(info.version, None);
    assert_eq!(
        run(&mut env, "import bloom/menu\nprint(menu.open())"),
        vec!["open eased"]
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_broken_manifest_is_an_error_naming_the_file() {
    let root = write_library("broken", "[package]\nname = bloom\n", "src");
    let mut env = Env::new();
    let err = env.add_package(root.join("bloom")).unwrap_err().to_string();
    assert!(err.contains("petal.toml"), "{err}");
    assert!(err.contains("must be a quoted string"), "{err}");

    // Reached through a search path instead, the same problem is collected
    // rather than thrown — a search directory may hold anything.
    let mut env = Env::new();
    env.add_module_path(root.clone());
    assert_eq!(env.packages().len(), 0);
    assert_eq!(env.package_errors().len(), 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn register_package_registers_a_library_from_memory() {
    let mut env = Env::new();
    let info = env
        .register_package(
            "bloom",
            [
                (
                    "menu.ptl",
                    "import motion\nexport fn open()\n  motion.ease(\"open\")\nend\n",
                ),
                ("motion", "export fn ease(x)\n  x ++ \" eased\"\nend\n"),
                (
                    "widgets/button",
                    "import bloom/motion\nexport fn label()\n  motion.ease(\"button\")\nend\n",
                ),
            ],
        )
        .unwrap();
    assert!(info.in_memory);
    assert_eq!(info.modules, vec!["menu", "motion", "widgets/button"]);

    // Internal imports work both ways with no filesystem at all: `menu` uses
    // the flat sibling name, `widgets/button` the package-qualified one.
    assert_eq!(
        run(
            &mut env,
            "import bloom/menu\nimport bloom/widgets/button\nprint(menu.open())\nprint(button.label())"
        ),
        vec!["open eased", "button eased"]
    );
}

#[test]
fn packages_lists_what_is_available() {
    let mut env = Env::new();
    assert!(env.packages().is_empty());
    env.register_package("bloom", [("menu", "export fn open() 1 end")])
        .unwrap();
    env.register_package("widgets", [("panel", "export fn draw() 2 end")])
        .unwrap();
    let names: Vec<String> = env.packages().iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, vec!["bloom", "widgets"]);
    assert_eq!(env.packages()[0].label(), "bloom");
}

#[test]
fn a_package_name_that_is_not_an_identifier_is_rejected() {
    let mut env = Env::new();
    let err = env
        .register_package("bloom/menu", [("a", "")])
        .unwrap_err()
        .to_string();
    assert!(err.contains("is not an identifier"), "{err}");
}

#[test]
fn a_package_module_keeps_its_state_across_importers() {
    // One spelling, two importers: the ordinary module dedupe, which the
    // package rules must not disturb. (Two *different* spellings of one file
    // — flat `motion` inside the library and `bloom/motion` outside it — are
    // still two modules; a module's identity is the path it was imported by.
    // See docs/module-system.md#packages.)
    let mut env = Env::new();
    env.register_package(
        "bloom",
        [
            (
                "menu",
                "import bloom/motion
export fn open()
  motion.bump()
end
",
            ),
            (
                "motion",
                "var n = 0
export fn bump()
  set n = get n + 1
  get n
end
",
            ),
        ],
    )
    .unwrap();
    assert_eq!(
        run(
            &mut env,
            "import bloom/menu\nimport bloom/motion\nprint(menu.open())\nprint(motion.bump())"
        ),
        vec!["1", "2"]
    );
}
