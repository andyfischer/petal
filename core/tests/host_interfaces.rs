//! Bundled packages and host interfaces: what a host registers, what a script
//! must import to reach it, and the check that the two agree.
//!
//! - the `petal` library bundles `core`, `ui`, `bloom`, `text_layout` and
//!   `glsl`, so a check resolves them with no host and no search path;
//! - nothing is imported implicitly: a host's names come through `import`;
//! - a native is reachable only through a `native fn` declaration in scope;
//! - `Env::verify_interface` holds a host's registrations to the packages
//!   that declare them.

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::value::Value;

fn seven(cxt: &mut PetalCxt) -> NativeResult {
    cxt.push_int(7);
    Ok(1)
}

fn sub(cxt: &mut PetalCxt) -> NativeResult {
    let v = cxt.get_int(1)? - cxt.get_int(2)?;
    cxt.push_int(v);
    Ok(1)
}

fn run(env: &mut Env, src: &str) -> Result<Vec<String>, String> {
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output())
}

// ── Bundled packages ─────────────────────────────────────────────────────

#[test]
fn the_bundled_set_is_core_the_domain_builtins_ui_bloom_text_layout_and_glsl() {
    let names: Vec<&str> = petal::bundled::BUNDLED_PACKAGES.iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        [
            "core", "noise", "color", "vec2", "autodiff", "json", "io", "creative", "ui", "bloom",
            "text_layout", "glsl"
        ]
    );
    // The domain builtins are bundled packages, each importable by name.
    for name in petal::bundled::BUILTIN_PACKAGES {
        assert!(names.contains(name), "{name} is not bundled");
    }
    for package in petal::bundled::BUNDLED_PACKAGES {
        assert!(
            package.files.iter().any(|(path, _)| *path == "petal.toml"),
            "{} has no manifest",
            package.name
        );
        // Tests, examples and docs beside a package's modules are not part of it.
        for (path, _) in package.files {
            assert!(
                !path.starts_with("tests/") && !path.starts_with("examples/"),
                "{} bundles {path}",
                package.name
            );
        }
    }
}

#[test]
fn every_bundled_package_checks_with_no_host_and_no_search_path() {
    for import in ["ui", "ui/input", "bloom", "bloom/menu", "text_layout", "glsl", "core"] {
        let mut env = Env::new();
        env.set_check_only(true);
        let pid = env
            .load_program(&format!("import {import}\nprint(1)"))
            .unwrap_or_else(|e| panic!("import {import}: {e}"));
        let program = env.get_program(pid).unwrap();
        let unknown =
            petal::typecheck::globals::unresolved_globals(program, |n| env.native_signatures(n));
        assert!(
            unknown.is_empty(),
            "import {import} leaves names undefined: {:?}",
            unknown.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn bloom_reaches_ui_through_its_own_imports() {
    // bloom's modules import `ui` themselves; a script that imports only
    // bloom gets bloom's names and none of ui's.
    let mut env = Env::new();
    env.set_check_only(true);
    let pid = env.load_program("import bloom\nlet t = bloom.theme()\nclear(0, 0, 0)").unwrap();
    let program = env.get_program(pid).unwrap();
    let unknown: Vec<String> =
        petal::typecheck::globals::unresolved_globals(program, |n| env.native_signatures(n))
            .into_iter()
            .map(|d| d.message)
            .collect();
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert!(unknown[0].starts_with("unknown function `clear`"), "{unknown:?}");
}

#[test]
fn a_host_registration_wins_over_the_bundled_package_of_the_same_name() {
    let mut env = Env::new();
    env.register_package("ui", [("ui", "pub fn which()\n  \"the host's\"\nend\n")])
        .unwrap();
    assert_eq!(run(&mut env, "import ui\nprint(which())").unwrap(), ["the host's"]);
}

// ── Running needs the natives; checking does not ─────────────────────────

#[test]
fn a_ui_script_checks_anywhere_and_runs_only_where_the_natives_are() {
    let src = "import ui\nprint(\"ran\")\nclear(0, 0, 0)";
    let mut env = Env::new();
    let err = env.load_program(src).unwrap_err();
    assert!(
        err.contains("module 'ui' needs a native fn this host does not register: clear")
            || err.contains("module 'ui' needs native fns this host does not register"),
        "{err}"
    );
    assert!(err.contains("petal-ui runner"), "{err}");
    env.set_check_only(true);
    env.load_program(src).expect("a check does not link");
}

#[test]
fn the_runner_hint_is_only_for_ui() {
    let mut env = Env::new();
    env.register_package("game", [("game", "pub native fn spawn(kind)\n")]).unwrap();
    let err = env.load_program("import game\nspawn(1)").unwrap_err();
    assert!(
        err.contains("module 'game' needs a native fn this host does not register: spawn"),
        "{err}"
    );
    assert!(!err.contains("petal-ui"), "{err}");
}

// ── Nothing is implicit ──────────────────────────────────────────────────

#[test]
fn a_registered_native_is_reachable_only_through_a_declaration() {
    let mut env = Env::new();
    env.register_native("seven", seven, NativeEffects::PURE);
    // By call and by read: the name is not in scope.
    let err = run(&mut env, "print(seven())").unwrap_err();
    assert!(err.contains("Unknown function: seven"), "{err}");
    let err = run(&mut env, "let f = seven\nprint(f())").unwrap_err();
    assert!(err.contains("Undefined variable: seven"), "{err}");
    // A method call does not find it either…
    let err = run(&mut env, "print(1.seven())").unwrap_err();
    assert!(err.contains("No method 'seven'"), "{err}");
    // Declared in the file, or by an imported interface package, it is.
    assert_eq!(run(&mut env, "native fn seven()\nprint(seven())").unwrap(), ["7"]);
    env.register_package("host", [("host", "pub native fn seven() -> int\n")]).unwrap();
    assert_eq!(run(&mut env, "import host\nprint(seven(), host.seven())").unwrap(), ["7 7"]);
    assert_eq!(run(&mut env, "import host: seven\nprint(seven())").unwrap(), ["7"]);
}

fn sub_native(cxt: &mut PetalCxt) -> NativeResult {
    sub(cxt)
}

#[test]
fn method_syntax_reaches_a_host_native_where_its_declaration_is_in_scope() {
    let mut env = Env::new();
    env.register_native("minus", sub_native, NativeEffects::PURE);
    env.register_package("host", [("host", "pub native fn minus(a, b)\n")]).unwrap();
    // With the import, `x.minus(y)` is `minus(x, y)`, named arguments included.
    assert_eq!(
        run(&mut env, "import host\nprint(10.minus(3), 10.minus(b: 4))").unwrap(),
        ["7 6"]
    );
    // Without it the native is not a method of anything.
    let err = run(&mut env, "print(10.minus(3))").unwrap_err();
    assert!(err.contains("No method 'minus'"), "{err}");
    // The builtins stay methods of every value with no import.
    assert_eq!(run(&mut env, "print([1, 2, 3].len(), \"ab\".upper())").unwrap(), ["3 AB"]);
}

#[test]
fn the_builtins_need_no_import() {
    let mut env = Env::new();
    assert_eq!(
        run(&mut env, "print(len([1, 2]), abs(-3), sum([1, 2]))").unwrap(),
        ["2 3 3"]
    );
}

// ── Interface packages ───────────────────────────────────────────────────

#[test]
fn registering_an_interface_package_names_the_natives_parameters() {
    let mut env = Env::new();
    env.register_native("sub", sub, NativeEffects::PURE);
    assert!(env.native_signatures("sub").unwrap().is_empty());
    env.register_package("host", [("host", "pub native fn sub(a: int, b: int) -> int\n")])
        .unwrap();
    let sigs = env.native_signatures("sub").unwrap();
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].params(), ["a", "b"]);
    // Statically, and through a value the compiler cannot see through.
    assert_eq!(
        run(&mut env, "import host\nprint(sub(b: 1, a: 10))\nlet f = sub\nprint(f(b: 2, a: 10))")
            .unwrap(),
        ["9", "8"]
    );
}

#[test]
fn verify_interface_passes_when_registrations_and_declarations_agree() {
    let mut env = Env::new();
    env.register_native("seven", seven, NativeEffects::PURE);
    env.register_native("sub", sub, NativeEffects::PURE);
    env.register_package(
        "host",
        [
            ("host", "pub import ./math\npub native fn seven() -> int\n"),
            ("math", "pub native fn sub(a, b)\n"),
        ],
    )
    .unwrap();
    assert_eq!(env.verify_interface(&["host"]), Ok(()));
    // Two packages may answer for one host between them.
    let mut env = Env::new();
    env.register_native("seven", seven, NativeEffects::PURE);
    env.register_native("sub", sub, NativeEffects::PURE);
    env.register_package("a", [("a", "pub native fn seven()\n")]).unwrap();
    env.register_module("b", "pub native fn minus(a, b) = \"sub\"\n");
    assert_eq!(env.verify_interface(&["a", "b"]), Ok(()));
}

#[test]
fn verify_interface_reports_both_directions_and_what_it_could_not_read() {
    let mut env = Env::new();
    env.register_native("seven", seven, NativeEffects::PURE);
    env.register_package("host", [("host", "pub native fn eight()\n")]).unwrap();
    let mismatch = env.verify_interface(&["host", "nowhere"]).unwrap_err();
    assert_eq!(mismatch.unregistered, ["eight"]);
    assert_eq!(mismatch.undeclared, ["seven"]);
    assert_eq!(mismatch.problems.len(), 1);
    assert!(mismatch.problems[0].contains("'nowhere'"), "{:?}", mismatch.problems);
    let text = mismatch.to_string();
    assert!(text.contains("declared by the interface but not registered by the host: eight"));
    assert!(text.contains("registered by the host but declared by no interface package"));
    assert!(!mismatch.is_empty());
    // A host may replace a builtin under its own name and declare that.
    let mut env = Env::new();
    env.register_native("log", seven, NativeEffects::EFFECT);
    env.register_package("host", [("host", "pub native fn log(...args)\n")]).unwrap();
    assert_eq!(env.verify_interface(&["host"]), Ok(()));
    // A bare Env registers only the builtins, which no host answers for.
    assert_eq!(Env::new().verify_interface(&[]), Ok(()));
}

#[test]
fn verify_interface_takes_one_module_of_a_package() {
    // A host that registers half of an interface answers for that module.
    let mut env = Env::new();
    env.register_native("seven", seven, NativeEffects::PURE);
    env.register_package(
        "kit",
        [
            ("kit", "pub import ./input\npub native fn draw()\n"),
            ("input", "pub native fn seven()\n"),
        ],
    )
    .unwrap();
    assert_eq!(env.verify_interface(&["kit/input"]), Ok(()));
    assert_eq!(env.verify_interface(&["kit"]).unwrap_err().unregistered, ["draw"]);
    // A script that imports only that module needs only its natives.
    assert_eq!(run(&mut env, "import kit/input\nprint(seven())").unwrap(), ["7"]);
    assert!(env.load_program("import kit\ndraw()").is_err());
    let _ = Value::Nil;
}

// ── fix-imports ──────────────────────────────────────────────────────────

#[test]
fn fix_imports_adds_the_import_of_the_host_package_a_name_comes_from() {
    let mut fixer = petal::fix_imports::ImportFixer::new(&[]);
    let fixed = fixer.fix("clear(0, 0, 0)\nprint(mouse_x())\n", None).unwrap();
    assert_eq!(fixed.source, "import ui\n\nclear(0, 0, 0)\nprint(mouse_x())\n");
    // Already migrated: nothing to do.
    let again = fixer.fix(&fixed.source, None).unwrap();
    assert_eq!(again.source, fixed.source);
    assert!(again.changes.is_empty(), "{:?}", again.changes);
}

#[test]
fn fix_imports_picks_the_package_the_others_are_built_on() {
    // `button` is exported by ui and by bloom, which imports ui: a file that
    // imported neither meant the one underneath.
    let mut fixer = petal::fix_imports::ImportFixer::new(&[]);
    let src = "if button({x: 0, y: 0, w: 9, h: 9}, \"ok\") then print(1) end\n";
    let fixed = fixer.fix(src, None).unwrap();
    assert!(fixed.source.starts_with("import ui\n"), "{}", fixed.source);
    assert!(!fixed.source.contains("import bloom"), "{}", fixed.source);
}
