//! Tests holding the bundled `core` package (`core/packages/core`) to the
//! natives [`super::register_builtins`] registers.
//!
//! The package is where each builtin's parameter names, arity, parameter
//! types and result type are written (`native fn …`); the natives read their
//! arguments by index and know none of it. These tests are what keeps the
//! two from drifting: every registered native is declared exactly once,
//! nothing is declared that is not registered, and every declared form is one
//! the native really takes.

use std::collections::{BTreeMap, BTreeSet};

use crate::env::Env;
use crate::native_decl::{NativeDecl, builtin_decls, builtin_package_of, core_decls};
use crate::native_fn::NativeFnTable;

/// The core table alone: what `register_builtins` registers, no host.
fn core_table() -> NativeFnTable {
    let mut table = NativeFnTable::new();
    super::register_builtins(&mut table);
    table
}

fn registered_names() -> BTreeSet<String> {
    let table = core_table();
    (0..table.count())
        .map(|i| table.get_name(crate::native_fn::NativeFnId(i as u32)).to_string())
        .collect()
}

/// Every core-registered native has exactly one declaration in the `core`
/// package or in one of the domain builtin packages
/// (`bundled::BUILTIN_PACKAGES`), and those declare nothing else.
#[test]
fn every_core_native_is_declared_exactly_once_and_vice_versa() {
    let registered = registered_names();
    // Host name → the names declared for it.
    let mut declared: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, decl) in builtin_decls().iter() {
        declared
            .entry(decl.host.clone())
            .or_default()
            .push(name.to_string());
    }
    let undeclared: Vec<&String> = registered
        .iter()
        .filter(|n| !declared.contains_key(*n))
        .collect();
    assert!(
        undeclared.is_empty(),
        "registered by register_builtins but not declared in core/packages/: {undeclared:?}"
    );
    let unregistered: Vec<&String> = declared
        .keys()
        .filter(|n| !registered.contains(*n))
        .collect();
    assert!(
        unregistered.is_empty(),
        "declared in core/packages/ but not registered by register_builtins: {unregistered:?}"
    );
    for (host, names) in &declared {
        assert_eq!(
            names.len(),
            1,
            "core native `{host}` is declared under more than one name: {names:?}"
        );
        // A core builtin is declared under the name it is registered as: the
        // global scope binds the registered name.
        assert_eq!(&names[0], host, "core native `{host}` is declared as `{}`", names[0]);
    }
    assert_eq!(builtin_decls().len(), registered.len());
}

/// No native is declared in two files of the package (the loader panics on
/// that), and each file declares at least one.
#[test]
fn every_core_module_declares_something() {
    let package = crate::bundled::package(crate::bundled::CORE_PACKAGE).unwrap();
    for (path, source) in package.sources() {
        let decls = crate::native_decl::NativeDecls::parse(source)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        // `core.ptl` is the package's own module: it re-exports the others
        // (`import core`) and declares nothing itself.
        if path == "src/core.ptl" {
            assert!(decls.is_empty(), "{path} is the facade; declarations go in a module");
            assert_eq!(
                source.lines().filter(|l| l.starts_with("pub import ./")).count(),
                package.sources().count() - 1,
                "{path} re-exports every other module of the package"
            );
            continue;
        }
        assert!(!decls.is_empty(), "{path} declares no native fn");
    }
}

/// Whether `err` is a complaint about how many arguments arrived, as
/// opposed to what they were. Every arity message in `builtins/` says
/// `expects <count…>`; a type message says `expects a list` and the like.
fn is_arity_error(err: &str) -> bool {
    err.split("expects ").skip(1).any(|rest| {
        let rest = rest.trim_start_matches("at least ");
        rest.starts_with(|c: char| c.is_ascii_digit()) || rest.starts_with("no arguments")
    })
}

/// Call `name` with `count` arguments and return the error, if any. A
/// method (`Rect.inset`) is called on a receiver, which is its first
/// argument.
fn call_with(name: &str, count: usize) -> Option<String> {
    let args = |n: usize| vec!["1"; n].join(", ");
    let src = match name.split_once('.') {
        Some((class, method)) => {
            format!("{class}(0, 0, 4, 4).{method}({})", args(count - 1))
        }
        None => format!("{name}({})", args(count)),
    };
    // A domain builtin is in scope only where its package is imported.
    let src = match builtin_package_of(name) {
        Some(package) => format!("import {package}\n{src}"),
        None => src,
    };
    Env::new().run_source(&src).err()
}

/// The declarations with a fixed parameter list: everything but the variadic
/// natives and the `__` internals.
fn fixed_decls() -> Vec<(&'static str, &'static NativeDecl)> {
    let mut decls: Vec<_> = builtin_decls()
        .iter()
        .filter(|(_, d)| d.forms.iter().all(|f| !f.rest))
        .collect();
    decls.sort_by_key(|(name, _)| *name);
    decls
}

/// Every declared form is one the native really takes: called with any
/// argument count the form allows, it does not complain about the count —
/// and one argument past the longest form, or one short of the shortest,
/// it does. (The arguments are all `1`, so a type error is expected and
/// fine; only an arity error is evidence against the declaration.)
#[test]
fn declared_forms_agree_with_the_natives_arity() {
    // Natives that read the arguments they want and never count them, so
    // a wrong count is not refused and cannot be checked from outside.
    const UNCOUNTED: &[&str] = &["is_valid"];
    for (name, decl) in fixed_decls() {
        let longest = decl.forms.iter().map(|f| f.params.len()).max().unwrap();
        // A method call always supplies its receiver.
        let fewest = usize::from(name.contains('.'));
        for count in fewest..=longest + 1 {
            let err = call_with(name, count);
            let arity = err.as_deref().is_some_and(is_arity_error);
            if decl.accepts_count(count) {
                assert!(
                    !arity,
                    "{name}: declared to take {count} argument(s), but the native refuses \
                     that count: {err:?}"
                );
            } else if !UNCOUNTED.contains(&name) {
                assert!(
                    arity,
                    "{name}: not declared to take {count} argument(s), but the native did \
                     not refuse that count: {err:?}"
                );
            }
        }
    }
}

/// The declarations reach the registry, and every parameter name is one a
/// call site can write as an argument label (a keyword included: `end:`).
#[test]
fn every_declaration_reaches_the_registry_with_writable_names() {
    let env = Env::new();
    for (name, decl) in fixed_decls() {
        let sigs = env.native_signatures(name).expect("registered");
        assert_eq!(sigs, decl.signatures().as_slice(), "{name}");
        assert_eq!(sigs.len(), decl.forms.len(), "{name}");
        for sig in sigs {
            for param in sig.params() {
                let src = format!("f({param}: 1)\n");
                let mut lexer = crate::lexer::Lexer::new(&src);
                lexer.tokenize().expect("lexes");
                let parsed =
                    crate::parse::Parser::new(lexer.tokens, lexer.token_spans).parse_program();
                assert!(
                    parsed.is_ok(),
                    "{name}: `{param}: …` is not a writable argument label: {parsed:?}"
                );
            }
        }
    }
}

/// What takes no names is left so deliberately: a builtin declared with a
/// rest parameter shows up in this list and has to be decided.
#[test]
fn the_natives_that_refuse_names_are_the_variadic_and_internal_ones() {
    let env = Env::new();
    let mut undeclared: Vec<String> = env
        .native_fn_names()
        .into_iter()
        .filter(|n| env.native_signatures(n).is_some_and(|s| s.is_empty()))
        .collect();
    undeclared.sort();
    assert_eq!(
        undeclared,
        [
            "__declare_method",
            "__enum_all",
            "__enum_data",
            "__enum_from",
            "__pending",
            "__record_of",
            "__reject",
            "__resolve",
            "format",
            "print"
        ]
    );
}

/// The split between what is global and what needs an import: `core` holds
/// what reads as language, each domain builtin package the natives of its
/// group, and no name is in both.
#[test]
fn the_domain_builtins_are_declared_by_their_packages_and_not_by_core() {
    for (name, package) in [
        ("noise", "noise"),
        ("noise_seed", "noise"),
        ("hsv", "color"),
        ("color_lerp", "color"),
        ("vec2", "vec2"),
        ("vec3", "vec2"),
        ("normalize", "vec2"),
        ("dual", "autodiff"),
        ("json_parse", "json"),
        ("json_stringify", "json"),
        ("symbol", "io"),
        ("push_output", "io"),
        ("binding", "io"),
        ("lerp", "creative"),
        ("map_range", "creative"),
        ("smoothstep", "creative"),
        ("choose", "creative"),
    ] {
        assert_eq!(builtin_package_of(name), Some(package), "{name}");
        assert!(core_decls().get(name).is_none(), "{name} is still global");
    }
    for name in [
        "print", "len", "str", "range", "abs", "sqrt", "sin", "min", "max", "clamp", "pow", "sign",
        "exp", "log", "fract", "radians", "degrees", "random", "random_int", "fn_info", "assert",
    ] {
        assert!(core_decls().get(name).is_some(), "{name} should stay global");
        assert_eq!(builtin_package_of(name), None, "{name}");
    }
    for package in crate::bundled::BUILTIN_PACKAGES {
        let bundled = crate::bundled::package(package).expect("bundled");
        assert!(
            bundled.sources().any(|(path, _)| path == format!("{package}.ptl")),
            "`import {package}` needs a module named after the package"
        );
    }
}

/// A moved builtin is unknown without its import and says which import
/// brings it back; bare, qualified and method syntax all work with it.
#[test]
fn a_domain_builtin_needs_its_import() {
    let err = Env::new().run_source("print(lerp(0, 10, 0.5))").unwrap_err();
    assert!(err.contains("Unknown function: lerp"), "{err}");
    assert!(err.contains("add `import creative`"), "{err}");
    let err = Env::new().run_source("let f = hsv\nprint(1)").unwrap_err();
    assert!(err.contains("Undefined variable: hsv"), "{err}");
    assert!(err.contains("add `import color`"), "{err}");
    let err = Env::new().run_source("let v = [3, 4]\nprint(v.mag())").unwrap_err();
    assert!(err.contains("No method 'mag' on type list"), "{err}");
    assert!(err.contains("add `import creative` to call it as a method"), "{err}");
    // A selective import brings only the names it lists; method syntax
    // follows what is in scope.
    let err = Env::new()
        .run_source("import vec2: vec2\nlet v = vec2(3, 4)\nprint(v.normalize())")
        .unwrap_err();
    assert!(err.contains("No method 'normalize' on type vec2"), "{err}");
    Env::new()
        .run_source("import vec2: vec2, normalize\nlet v = vec2(3, 4)\nassert_eq(v.normalize().x, 0.6)")
        .unwrap();
    // Values of a moved kind need no import to be used: fields, operators.
    let mut env = Env::new();
    env.register_module("geom", "import vec2\npub fn up()\n  vec2(0, 1)\nend");
    env.run_source("import geom\nlet v = up() * 2\nassert_eq(v.y, 2.0)")
        .unwrap();
    // … and the importing module's import does not leak into its importer.
    let err = env.run_source("import geom\nprint(vec2(1, 2))").unwrap_err();
    assert!(err.contains("Unknown function: vec2"), "{err}");

    Env::new()
        .run_source(
            "import creative\nimport vec2\nassert_eq(lerp(0, 10, 0.5), 5)\n\
             assert_eq(creative.lerp(0, 10, 0.5), 5)\nlet v = vec2(3, 4)\n\
             assert_eq(v.mag(), 5)\nassert_eq(vec2.dot(v, v), 25)\nassert_eq(clamp(5, 0, 1), 1)",
        )
        .unwrap();
}
