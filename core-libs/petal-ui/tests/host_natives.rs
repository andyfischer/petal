//! `petal check` cannot see this crate's registrations (the core CLI does not
//! depend on petal-ui). What it reads instead is the `ui` package's
//! `native fn` declarations (`prelude/ui.ptl`, `prelude/input.ptl`), which
//! the `petal` binary bundles. These tests hold the declarations to the real
//! registrations (`Env::verify_interface`): a native added here without
//! being declared there cannot be reached from a script at all, and one
//! declared there that no host registers fails every script that uses it.

use std::collections::BTreeSet;

use petal::env::Env;
use petal::native_fn::NativeSignature;

/// The names `register` adds to a bare `Env`, beyond the core builtins.
fn added_by(register: impl FnOnce(&mut Env)) -> BTreeSet<String> {
    let core: BTreeSet<String> = Env::new().native_fn_names().into_iter().collect();
    let mut env = Env::new();
    register(&mut env);
    env.native_fn_names()
        .into_iter()
        .filter(|n| !core.contains(n))
        .collect()
}

/// The drift test: what `register_all` registers is exactly what the `ui`
/// package declares.
#[test]
fn register_all_agrees_with_the_ui_interface_package() {
    let mut env = Env::new();
    petal_ui::register_all(&mut env);
    if let Err(mismatch) = env.verify_interface(petal_ui::INTERFACE_PACKAGES) {
        panic!("the ui package and register_all disagree: {mismatch}");
    }
}

/// The copy of `ui` bundled in the `petal` binary is this crate's: an `Env`
/// that registers the natives but not the package verifies against it too.
#[test]
fn the_bundled_ui_package_is_this_crates() {
    let mut env = Env::new();
    petal_ui::input::register_input(&mut env);
    petal_ui::draw::register_draw(&mut env);
    petal_ui::host_data::register_host_data(&mut env);
    if let Err(mismatch) = env.verify_interface(&["ui"]) {
        panic!("the bundled ui package and the natives disagree: {mismatch}");
    }
    let bundled = petal::bundled::package("ui").expect("ui is bundled");
    for (name, source) in petal_ui::package_modules() {
        let file = bundled
            .files
            .iter()
            .find(|(path, _)| *path == format!("{name}.ptl"))
            .unwrap_or_else(|| panic!("{name}.ptl is not bundled"));
        assert_eq!(file.1, source, "{name}.ptl");
    }
}

/// A host that registers only the input half answers for `ui/input` alone:
/// a script that imports that module loads and runs with no draw native.
#[test]
fn an_input_only_host_runs_scripts_that_import_ui_input() {
    let mut env = Env::new();
    petal_ui::input::register_input(&mut env);
    petal_ui::register_prelude(&mut env);
    petal_ui::input::bind_frame_info(&mut env, 0.0, 0);
    env.run_source("import ui/input\nprint(frame_count(), input.mouse_x())")
        .expect("an input-only script runs");
    let err = env.load_program("import ui\nclear(0, 0, 0)").unwrap_err();
    assert!(err.contains("needs native fns"), "{err}");
}

/// `Env::declare_natives` agrees: handed the modules, a full petal-ui host is
/// missing nothing, and a bare `Env` is missing all of it.
#[test]
fn declare_natives_reports_what_a_host_lacks() {
    let mut env = Env::new();
    petal_ui::register_all(&mut env);
    let mut bare = Env::new();
    let mut missing = Vec::new();
    for (_, source) in petal_ui::package_modules() {
        assert_eq!(env.declare_natives(source).unwrap(), Vec::<String>::new());
        missing.extend(bare.declare_natives(source).unwrap());
    }
    missing.sort();
    assert_eq!(missing, petal_ui::interface().host_names());
}

/// A script that uses the prelude does not load into an `Env` that lacks the
/// natives it reaches: the error comes before any line runs and names them.
#[test]
fn a_ui_script_does_not_load_without_the_natives() {
    let mut env = Env::new();
    petal_ui::register_prelude(&mut env);
    let src = "import ui\nprint(\"ran\")\ndraw_rect(0, 0, 4, 4, 255, 0, 0)";
    let err = env.load_program(src).unwrap_err();
    assert!(err.contains("needs native fns this host does not register: "), "{err}");
    assert!(err.contains(" more"), "{err}");
    assert!(err.contains("petal-ui runner"), "{err}");
    // Check-only compilation does not link.
    env.set_check_only(true);
    env.load_program(src).expect("checks without the natives");
}

/// Nothing is imported implicitly: a ui name with no `import ui` is unknown,
/// in a full petal-ui host as anywhere else.
#[test]
fn a_ui_name_is_reachable_only_through_the_import() {
    let mut env = Env::new();
    petal_ui::register_all(&mut env);
    let err = env.run_source("draw_rect(0, 0, 4, 4, 255, 0, 0)").unwrap_err();
    assert!(err.contains("Unknown function: draw_rect"), "{err}");
    env.run_source("import ui\ndraw_rect(0, 0, 4, 4, 255, 0, 0)")
        .expect("runs with the import");
}

/// The panel stubs stand in for Garden's panel natives: every stub is one
/// the `garden` interface package declares.
#[test]
fn panel_stubs_are_declared_by_the_garden_interface() {
    let registered = added_by(petal_ui::panel_stubs::register_panel_stubs);
    let declared: BTreeSet<String> =
        petal::native_decl::NativeDecls::parse(petal_ui::panel_stubs::GARDEN_INTERFACE)
            .expect("garden.ptl parses")
            .host_names()
            .into_iter()
            .collect();
    let undeclared: Vec<_> = registered.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "the garden interface package lacks panel stubs: {undeclared:?}"
    );
}

// ---------------------------------------------------------------------------
// Parameter names (named arguments)
// ---------------------------------------------------------------------------
//
// The `ui` module's `native fn` declarations are given to the natives as they
// register (`petal_ui::params`), so the module and the registry cannot
// disagree about a name. What can still go wrong is a declaration disagreeing
// with a native's *body* — a parameter the native never reads, an optional one it requires —
// which is what these hold.

/// The natives alone, without the `ui` prelude whose overloads shadow most of
/// the draw names.
fn natives_only() -> Env {
    let mut env = Env::new();
    petal_ui::input::register_input(&mut env);
    petal_ui::draw::register_draw(&mut env);
    petal_ui::host_data::register_host_data(&mut env);
    env
}

/// A declaration of every petal-ui native under its own host name, one line
/// per declared form: what lets a test call the raw natives, which a script
/// reaches only through a declaration. Ends with a newline.
fn raw_decls() -> String {
    let mut out = String::new();
    for (name, sigs) in declared_signatures() {
        for sig in sigs {
            let params: Vec<String> = sig
                .params()
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if i < sig.required() {
                        p.to_string()
                    } else {
                        format!("{p}?")
                    }
                })
                .collect();
            out.push_str(&format!("native fn {name}({})\n", params.join(", ")));
        }
    }
    out
}

/// Run `src` against the raw natives.
fn run_raw(src: &str) -> Result<petal::value::Value, String> {
    natives_only().run_source(&format!("{}{src}", raw_decls()))
}

/// Host native → the parameter lists `ui.ptl` declares for it, sorted.
fn declared_signatures() -> Vec<(String, Vec<NativeSignature>)> {
    let mut all: Vec<_> = petal_ui::interface()
        .signatures_by_host()
        .into_iter()
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all
}

/// Every petal-ui native is declared, and the registry holds exactly the
/// declared parameter lists.
#[test]
fn every_petal_ui_native_declares_its_parameters() {
    let env = natives_only();
    let declared = declared_signatures();
    for name in added_by(petal_ui::register_all) {
        let Some((_, sigs)) = declared.iter().find(|(n, _)| *n == name) else {
            panic!("the ui package has no native fn for {name}");
        };
        assert!(!sigs.is_empty(), "{name} declares no parameter list");
        let registered = env.native_signatures(&name).expect("registered");
        assert_eq!(registered, sigs.as_slice(), "{name}");
    }
}

/// A plausible argument for a parameter, by its name: the natives check
/// types, so an all-integer call would fail before reading every argument.
fn sample(param: &str) -> &'static str {
    match param {
        "text" | "source" | "name" | "key" | "kind" | "arg" | "font" => "\"a\"",
        "where" => "\"tail\"",
        "style" => "{size: 12}",
        "rect" => "{x: 0, y: 0, w: 4, h: 4}",
        "points" => "[[0, 0], [4, 0], [4, 4]]",
        _ => "1",
    }
}

/// Each declared form is one the native really takes: it runs with every
/// argument count the form allows, and — where the form has a required
/// parameter — fails for want of an argument one short of that.
#[test]
fn declared_signatures_agree_with_what_the_natives_read() {
    // These find their last argument (a width, an offset) by counting back
    // from the end, so a call one short reads the size as that too and runs.
    const COUNTS_BACK: &[&str] = &["text_wrap", "text_ellipsize", "text_index_at"];
    for (name, sigs) in &declared_signatures() {
        let name = name.as_str();
        for sig in sigs {
            let params = sig.params();
            let call = |count: usize| {
                let args: Vec<&str> = params[..count].iter().map(|p| sample(p)).collect();
                let src = format!("{name}({})", args.join(", "));
                (run_raw(&src), src)
            };
            for count in sig.required()..=params.len() {
                let (result, src) = call(count);
                assert!(result.is_ok(), "`{src}` should run: {result:?}");
            }
            if sig.required() > 0 && !COUNTS_BACK.contains(&name) {
                let (result, src) = call(sig.required() - 1);
                let err = result.expect_err(&format!("`{src}` is one argument short"));
                assert!(err.contains("out of range"), "`{src}`: {err}");
            }
        }
    }
}

/// Named arguments reach the natives in the order they read them, in either
/// form a text native takes, and a wrong name is reported against the native.
#[test]
fn petal_ui_natives_take_named_arguments() {
    let run = |src: &str| run_raw(src);
    for src in [
        "clip(h: 4, w: 4, y: 0, x: 0)",
        "clip_push(x: 0, y: 0, w: 4, h: 4, radius: 2)",
        "mouse_down(button: 0)",
        "key_pressed(key: \"a\")",
        "draw_rect(x: 0, y: 0, w: 4, h: 4, r: 255, g: 0, b: 0, a: 128)",
        "draw_line(x1: 0, y1: 0, x2: 4, y2: 4, r: 0, g: 0, b: 0)",
        "draw_text(\"hi\", x: 0, y: 0, style: {size: 12})",
        "draw_text(\"hi\", x: 0, y: 0, size: 12, r: 0, g: 0, b: 0)",
        "text_width(text: \"hi\", size: 12)",
        "text_width(\"hi\", style: {size: 12})",
        "text_wrap(\"hi there\", size: 12, max_width: 40)",
    ] {
        let result = run(src);
        assert!(result.is_ok(), "`{src}`: {result:?}");
    }
    // The names put the arguments where a positional call would have.
    assert_eq!(
        format!("{:?}", run("text_width(size: 20, text: \"abc\")")),
        format!("{:?}", run("text_width(\"abc\", 20)"))
    );
    let err = run("draw_rect(x: 0, y: 0, w: 4, h: 4, r: 0, g: 0, b: 0, width: 2)").unwrap_err();
    assert!(
        err.contains("draw_rect() has no parameter named 'width'"),
        "{err}"
    );
    // An optional parameter cannot be skipped to reach a later one.
    let err = run("draw_line(0, 0, 4, 4, 0, 0, 0, width: 2)").unwrap_err();
    assert!(
        err.contains("draw_line() is missing a value for parameter 'a'"),
        "{err}"
    );
}
