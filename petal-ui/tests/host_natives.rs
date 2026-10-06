//! `petal check` cannot see this crate's registrations (the core CLI does not
//! depend on petal-ui), so it carries static lists of the natives each host
//! adds (`petal::typecheck::globals`). These tests hold the lists to the real
//! registrations: a native added here without being listed there would make
//! `check` report every call to it as an unknown function.

use std::collections::BTreeSet;

use petal::env::Env;
use petal::native_fn::NativeSignature;
use petal::typecheck::globals::{GARDEN_NATIVES, PETAL_UI_NATIVE_PARAMS, PETAL_UI_NATIVES};

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
// `PETAL_UI_NATIVE_PARAMS` is declared onto the natives as they register
// (`petal_ui::params`), so the table and the registry cannot disagree about a
// name. What can still go wrong is the table disagreeing with a native's
// *body* — a parameter the native never reads, an optional one it requires —
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

fn signatures(specs: &[&str]) -> Vec<NativeSignature> {
    specs
        .iter()
        .map(|s| NativeSignature::parse(s).expect("spec parses"))
        .collect()
}

/// Every petal-ui native has a row, and the registry holds exactly that row.
#[test]
fn every_petal_ui_native_declares_its_parameters() {
    let env = natives_only();
    for name in PETAL_UI_NATIVES {
        let Some((_, specs)) = PETAL_UI_NATIVE_PARAMS.iter().find(|(n, _)| n == name) else {
            panic!("PETAL_UI_NATIVE_PARAMS has no row for {name}");
        };
        let declared = env.native_signatures(name).expect("registered");
        assert_eq!(declared, signatures(specs), "{name}");
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
    for (name, specs) in PETAL_UI_NATIVE_PARAMS {
        for sig in signatures(specs) {
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
            if sig.required() > 0 && !COUNTS_BACK.contains(name) {
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
