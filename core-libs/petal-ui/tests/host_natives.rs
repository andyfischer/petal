//! `petal check` cannot see this crate's registrations (the core CLI does not
//! depend on petal-ui). What it reads instead is the `ui` module's `native fn`
//! declarations (`prelude/ui.ptl`) — and, until every host declares its
//! interface that way, static lists of the natives each host adds
//! (`petal::typecheck::globals`). These tests hold both to the real
//! registrations: a native added here without being declared there would make
//! `check` report every call to it as an unknown function.

use std::collections::BTreeSet;

use petal::env::Env;
use petal::native_fn::NativeSignature;
use petal::typecheck::globals::{GARDEN_NATIVES, PETAL_UI_NATIVES};

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

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn petal_ui_list_matches_register_all() {
    let registered = added_by(petal_ui::register_all);
    let listed = set(PETAL_UI_NATIVES);
    let unlisted: Vec<_> = registered.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&registered).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "petal::typecheck::globals::PETAL_UI_NATIVES is out of date.\n  \
         registered but not listed: {unlisted:?}\n  listed but not registered: {stale:?}"
    );
}

/// The `ui` module declares exactly the natives `register_all` registers:
/// every registered native has a `native fn`, and nothing is declared that no
/// host provides.
#[test]
fn the_ui_interface_declares_exactly_what_register_all_registers() {
    let registered = added_by(petal_ui::register_all);
    let declared: BTreeSet<String> = petal_ui::interface().host_names().into_iter().collect();
    let undeclared: Vec<_> = registered.difference(&declared).collect();
    let unregistered: Vec<_> = declared.difference(&registered).collect();
    assert!(
        undeclared.is_empty() && unregistered.is_empty(),
        "prelude/ui.ptl's native fn declarations are out of date.\n  \
         registered but not declared: {undeclared:?}\n  declared but not registered: {unregistered:?}"
    );
}

/// `Env::declare_natives` agrees: handed the module, a full petal-ui host is
/// missing nothing, and a bare `Env` is missing all of it.
#[test]
fn declare_natives_reports_what_a_host_lacks() {
    let mut env = Env::new();
    petal_ui::register_all(&mut env);
    assert_eq!(
        env.declare_natives(petal_ui::prelude_source()).unwrap(),
        Vec::<String>::new()
    );
    let mut bare = Env::new();
    let mut missing = bare.declare_natives(petal_ui::prelude_source()).unwrap();
    missing.sort();
    assert_eq!(missing, petal_ui::interface().host_names());
}

/// A script that uses the prelude does not load into an `Env` that lacks the
/// natives it reaches: the error comes before any line runs and names them.
#[test]
fn a_ui_script_does_not_load_without_the_natives() {
    let mut env = Env::new();
    petal_ui::register_prelude(&mut env);
    let err = env
        .load_program("print(\"ran\")\ndraw_rect(0, 0, 4, 4, 255, 0, 0)")
        .unwrap_err();
    assert!(
        err.starts_with("module 'ui' needs native fns this host does not register: "),
        "{err}"
    );
    assert!(err.contains(" more"), "{err}");
    // Check-only compilation does not link.
    env.set_check_only(true);
    env.load_program("draw_rect(0, 0, 4, 4, 255, 0, 0)")
        .expect("checks without the natives");
}

/// The panel stubs stand in for Garden's panel natives, so every one of them
/// must be in Garden's list.
#[test]
fn panel_stubs_are_in_the_garden_list() {
    let registered = added_by(petal_ui::panel_stubs::register_panel_stubs);
    let listed = set(GARDEN_NATIVES);
    let unlisted: Vec<_> = registered.difference(&listed).collect();
    assert!(
        unlisted.is_empty(),
        "petal::typecheck::globals::GARDEN_NATIVES lacks panel stubs: {unlisted:?}"
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
    for name in PETAL_UI_NATIVES {
        let Some((_, sigs)) = declared.iter().find(|(n, _)| n == name) else {
            panic!("prelude/ui.ptl has no native fn for {name}");
        };
        assert!(!sigs.is_empty(), "{name} declares no parameter list");
        let registered = env.native_signatures(name).expect("registered");
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
                (natives_only().run_source(&src), src)
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
    let run = |src: &str| natives_only().run_source(src);
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
