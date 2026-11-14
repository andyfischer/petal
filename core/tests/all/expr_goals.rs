//! Goals that write expressions (`petal::static_expr::StaticExpr`): a
//! reference, a member path, calls with named arguments, record keys that are
//! not identifiers, and reading them back.

use petal::goal_based_editing::{Goal, StaticExpr, StaticValue, modify_source_with_goals};
use petal::literal_edit::set_path_expr;
use petal::static_expr::get_static_expr;
use petal::static_value::get_static_value;

fn apply(source: &str, goals: &[Goal]) -> String {
    modify_source_with_goals(source, goals).unwrap_or_else(|e| panic!("{e}\n{source}"))
}

fn ink() -> StaticExpr {
    StaticExpr::path("tok.color.ink").unwrap()
}

// ── building and parsing ───────────────────────────────────────────────────

#[test]
fn a_path_renders_as_written() {
    assert_eq!(ink().to_source().unwrap(), "tok.color.ink");
    assert_eq!(ink().free_names(), vec!["tok".to_string()]);
}

#[test]
fn a_call_with_named_arguments_renders_them_named() {
    let modes = StaticExpr::call("modes")
        .named("light", StaticValue::color(0x0a, 0x0a, 0x0e))
        .named("dark", StaticValue::color(0xf2, 0xf2, 0xf7));
    assert_eq!(modes.to_source().unwrap(), "modes(light: :0a0a0e, dark: :f2f2f7)");
    let shadow = StaticExpr::call("shadow").arg(StaticExpr::name("c").unwrap()).named("radius", 4).named("y", 3);
    assert_eq!(shadow.to_source().unwrap(), "shadow(c, radius: 4, y: 3)");
    assert_eq!(shadow.free_names(), vec!["c".to_string(), "shadow".to_string()]);
}

#[test]
fn parse_reads_exactly_one_expression() {
    let e = StaticExpr::parse("shadow(c, radius: 4, y: 3)").unwrap();
    assert_eq!(e.to_source().unwrap(), "shadow(c, radius: 4, y: 3)");
    let e = StaticExpr::parse("per(ios: tok.a, other: alpha(C.ink, 0.6))").unwrap();
    assert_eq!(e.to_source().unwrap(), "per(ios: tok.a, other: alpha(C.ink, 0.6))");
    let e = StaticExpr::parse("m[\"label.color\"]").unwrap();
    assert_eq!(e, StaticExpr::name("m").unwrap().field("label.color"));
    assert_eq!(e.to_source().unwrap(), "m[\"label.color\"]");
    // A value stays a value.
    assert_eq!(StaticExpr::parse("[1, -2.5, :ff0000]").unwrap().as_value(), Some(StaticValue::list([
        StaticValue::Int(1),
        StaticValue::Float(-2.5),
        StaticValue::color(255, 0, 0),
    ])));
    for bad in [
        "",
        "let x = 1",
        "a\nb",
        "f(",
        "x + 1",
        "\"hi {name}\"",
        "fn(x) -> x",
        "if a then b else c end",
        "f(...xs)",
    ] {
        assert!(StaticExpr::parse(bad).is_err(), "`{bad}` should not parse as a written expression");
    }
}

#[test]
fn names_are_checked_when_built_by_hand() {
    assert!(StaticExpr::name("tok color").is_err());
    assert!(StaticExpr::name("end").is_err());
    assert!(StaticExpr::path("tok..ink").is_err());
    // A hand-built enum variant with a bad name cannot render.
    let bad = StaticExpr::Name("x); evil(".into());
    assert!(bad.to_source().is_err());
    let bad_arg = StaticExpr::call("f").named("not ok", 1);
    assert!(bad_arg.to_source().is_err());
}

#[test]
fn strings_are_always_quoted_never_code() {
    // A caller's string is a string literal, wherever it goes, even one that
    // spells an expression or the placeholder the edit uses inside.
    let e = StaticExpr::call("label").arg("tok.color.ink\" + evil(\"").arg("__petal_expr_hole.h0_");
    let src = apply("let x = 1\n", &[Goal::should_set_path_expr("x", e)]);
    assert_eq!(get_static_expr(&src, "x").unwrap().free_names(), vec!["label".to_string()]);
    let StaticExpr::Call { args, .. } = get_static_expr(&src, "x").unwrap() else { panic!() };
    assert_eq!(args[0], StaticExpr::Value(StaticValue::str("tok.color.ink\" + evil(\"")));
    assert_eq!(args[1], StaticExpr::Value(StaticValue::str("__petal_expr_hole.h0_")));
}

// ── goals ──────────────────────────────────────────────────────────────────

#[test]
fn set_path_expr_writes_a_reference_on_an_attribute() {
    let src = "let card = <box fill={:0A0A0E} w={10} /> # the card\n";
    let out = apply(src, &[Goal::should_set_path_expr("card.fill", ink())]);
    assert_eq!(out, "let card = <box fill={tok.color.ink} w={10} /> # the card\n");
    // Already written that way: nothing changes.
    assert_eq!(apply(&out, &[Goal::should_set_path_expr("card.fill", ink())]), out);
    // A quoted attribute becomes a braced one; a missing one is added.
    let src = "let card = <box fill=\"red\" />\n";
    assert_eq!(
        apply(src, &[Goal::should_set_path_expr("card.fill", ink()), Goal::should_set_path_expr("card.stroke", ink())]),
        "let card = <box fill={tok.color.ink} stroke={tok.color.ink} />\n"
    );
}

#[test]
fn set_path_expr_inside_records_and_lists_keeps_the_layout() {
    let src = "let theme = {\n  # surfaces\n  bg: :ffffff,\n  fg: :000000, # text\n}\n";
    let out = apply(src, &[Goal::should_set_path_expr("theme.fg", ink())]);
    assert_eq!(out, "let theme = {\n  # surfaces\n  bg: :ffffff,\n  fg: tok.color.ink, # text\n}\n");
    // A value with references inside: only what differs is written.
    let rec = StaticExpr::Record(vec![
        ("bg".into(), StaticValue::color(0xff, 0xff, 0xff).into()),
        ("fg".into(), StaticExpr::path("tok.ink").unwrap()),
    ]);
    let out = apply(src, &[Goal::should_set_value_expr("theme", rec)]);
    assert_eq!(out, "let theme = {\n  # surfaces\n  bg: :ffffff,\n  fg: tok.ink, # text\n}\n");
    let out = apply("let xs = [1, 2]\n", &[Goal::should_append_expr("xs", StaticExpr::path("gutter").unwrap())]);
    assert_eq!(out, "let xs = [1, 2, gutter]\n");
    let out = apply("let xs = [1, 2]\n", &[Goal::should_insert_expr("xs[0]", StaticExpr::path("a.b").unwrap())]);
    assert_eq!(out, "let xs = [a.b, 1, 2]\n");
}

#[test]
fn set_value_and_call_goals_with_expressions() {
    let shadow = StaticExpr::parse("shadow(c, radius: 4, y: 3)").unwrap();
    let out = apply("let s = shadow(c, 4, nil, 0, 3)\n", &[Goal::should_set_value_expr("s", shadow.clone())]);
    assert_eq!(out, "let s = shadow(c, radius: 4, y: 3)\n");
    let out = apply("let a = 1\n", &[Goal::should_set_value_expr("s", shadow)]);
    assert_eq!(out, "let a = 1\n\nlet s = shadow(c, radius: 4, y: 3)\n");
    let out = apply(
        "theme(\"light\")\n",
        &[Goal::should_call_expr("theme", [ink()], [("scale", StaticExpr::from(2))])],
    );
    assert_eq!(out, "theme(tok.color.ink, scale: 2)\n");
}

#[test]
fn an_element_of_expressions_is_inserted_as_a_child() {
    let el = StaticExpr::Element {
        tag: "text".into(),
        props: vec![("color".into(), ink()), ("size".into(), 12.into())],
        children: vec![StaticExpr::path("copy.title").unwrap()],
    };
    let src = "let page = <vstack>\n  <rect />\n</vstack>\n";
    let out = apply(src, &[Goal::should_append_expr("page", el)]);
    assert_eq!(out, "let page = <vstack>\n  <rect />\n  <text color={tok.color.ink} size={12}>{copy.title}</text>\n</vstack>\n");
}

#[test]
fn an_expression_goal_that_fails_leaves_nothing_half_done() {
    let err = set_path_expr("let a = 1\n", "a.b", &ink()).unwrap_err();
    assert!(err.message.contains("names nothing"), "{}", err.message);
}

// ── record keys that are not identifiers ───────────────────────────────────

#[test]
fn quoted_keys_render_read_and_take_path_steps() {
    let src = "let o = {[\"label.color\"]: :ff0000, w: 4}\n";
    // Read back as the key.
    assert_eq!(
        get_static_value(src, "o").unwrap(),
        StaticValue::record([("label.color", StaticValue::color(255, 0, 0)), ("w", StaticValue::Int(4))])
    );
    // A path step names it.
    let out = apply(src, &[Goal::should_set_path_expr("o[\"label.color\"]", ink())]);
    assert_eq!(out, "let o = {[\"label.color\"]: tok.color.ink, w: 4}\n");
    // A new one is added quoted, and removed by the same step.
    let out = apply(&out, &[Goal::should_insert_expr("o[\"title.font\"]", StaticExpr::path("tok.font").unwrap())]);
    assert_eq!(out, "let o = {[\"label.color\"]: tok.color.ink, w: 4, [\"title.font\"]: tok.font}\n");
    let out = apply(&out, &[Goal::should_remove("o[\"label.color\"]")]);
    assert_eq!(out, "let o = {w: 4, [\"title.font\"]: tok.font}\n");
    // A whole record value with such keys round-trips.
    let v = StaticValue::record([("a.b", 1), ("end", 2), ("ok", 3)]);
    let out = apply("let r = {}\n", &[Goal::should_set_value("r", v.clone())]);
    assert_eq!(out, "let r = {[\"a.b\"]: 1, [\"end\"]: 2, ok: 3}\n");
    assert_eq!(get_static_value(&out, "r").unwrap(), v);
}

#[test]
fn a_quoted_key_step_prints_back_as_written() {
    let (name, segs) = petal::edit_path::parse_edit_path("o[\"a\\\"b\"].c[\"x.y\"]").unwrap();
    assert_eq!(name, "o");
    assert_eq!(petal::edit_path::path_text(&name, &segs), "o[\"a\\\"b\"].c[\"x.y\"]");
    assert!(petal::edit_path::parse_edit_path("o[\"open").is_err());
}

// ── reading expressions back ───────────────────────────────────────────────

#[test]
fn named_argument_calls_read_back() {
    let src = "let s = shadow(c, radius: 4, y: 3)\nlet m = modes(light: :0A0A0E, dark: tok.dark)\n";
    let s = get_static_expr(src, "s").unwrap();
    assert_eq!(s, StaticExpr::call("shadow").arg(StaticExpr::name("c").unwrap()).named("radius", 4).named("y", 3));
    let m = get_static_expr(src, "m").unwrap();
    let StaticExpr::Call { named, args, .. } = &m else { panic!() };
    assert!(args.is_empty());
    assert_eq!(named[0], ("light".into(), StaticExpr::Value(StaticValue::color(0x0a, 0x0a, 0x0e))));
    assert_eq!(named[1].1, StaticExpr::path("tok.dark").unwrap());
    // A value-only binding reads as a value.
    assert_eq!(get_static_expr("let x = [1, 2]\n", "x").unwrap().as_value(), Some(StaticValue::list([1, 2])));
}

// ── an element's children ──────────────────────────────────────────────────

#[test]
fn set_children_sets_the_text_of_an_element_whose_attributes_are_computed() {
    use petal::literal_edit::set_children;
    let src = "let t = <text size={f(2)}>Hi</text> # greeting\n";
    let out = set_children(src, "t", &[StaticValue::str("Hello there")]).unwrap();
    assert_eq!(out, "let t = <text size={f(2)}>Hello there</text> # greeting\n");
    assert_eq!(set_children(&out, "t", &[StaticValue::str("Hello there")]).unwrap(), out);
    let src = "let t = <text size={f(2)} />\n";
    assert_eq!(set_children(src, "t", &[StaticValue::str("Hi")]).unwrap(), "let t = <text size={f(2)}>Hi</text>\n");
    // Text that would not read back as written is a quoted hole.
    let out = set_children(src, "t", &[StaticValue::str("a {b} <c>")]).unwrap();
    assert_eq!(out, "let t = <text size={f(2)}>{\"a \\{b} <c>\"}</text>\n");
    assert!(set_children("let n = 1\n", "n", &[]).is_err());
}

// ── no part of a value is written as code (review, 2026-11) ────────────────

/// A `StaticValue` inside an expression (or given to a value goal) renders
/// its call function, variant names, tags and attribute names as written,
/// so each is checked: a function `evil() + f` would otherwise be code.
#[test]
fn values_with_code_in_their_names_are_refused() {
    let src = "let cfg = {fill: :000000}\n";
    let call = |f: &str| StaticValue::Call { function: f.into(), args: vec![] };
    let evil = call("(launch(\"x\")) + f");
    // As an expression, through every expression goal.
    let e = StaticExpr::call("shadow").arg(StaticExpr::Value(evil.clone()));
    assert!(e.to_source().is_err());
    assert!(modify_source_with_goals(src, &[Goal::should_set_path_expr("cfg.fill", e.clone())]).is_err());
    assert!(modify_source_with_goals(src, &[Goal::should_set_value_expr("x", e)]).is_err());
    // As a value, through the value goals and literal_edit.
    for goal in [
        Goal::should_set_path("cfg.fill", evil.clone()),
        Goal::should_set_value("x", evil.clone()),
        Goal::should_insert("cfg.extra", evil.clone()),
    ] {
        assert!(modify_source_with_goals(src, &[goal.clone()]).is_err(), "{goal:?}");
    }
    let variant = StaticValue::Variant { of: "Mode".into(), name: "dark + launch()".into(), args: vec![] };
    assert!(modify_source_with_goals(src, &[Goal::should_set_path("cfg.fill", variant)]).is_err());
    let el = StaticValue::Element { tag: "box".into(), props: vec![("a={launch()} b".into(), StaticValue::Int(1))], children: vec![] };
    assert!(modify_source_with_goals(src, &[Goal::should_set_path("cfg.fill", el)]).is_err());
    let tag = StaticValue::Element { tag: "box>{launch()}</box".into(), props: vec![], children: vec![] };
    assert!(modify_source_with_goals(src, &[Goal::should_set_path("cfg.fill", tag)]).is_err());
    assert!(modify_source_with_goals(src, &[Goal::should_set_path("cfg.fill", f64::NAN)]).is_err());
    // A member-path function and an enum variant are fine.
    let ok = StaticValue::Call { function: "design.modes".into(), args: vec![StaticValue::Int(1)] };
    assert_eq!(apply(src, &[Goal::should_set_path("cfg.fill", ok)]), "let cfg = {fill: design.modes(1)}\n");
}

/// The binding name a set-value goal writes `let <name> = …` with, and the
/// function a call goal writes, are names, never code.
#[test]
fn binding_and_function_names_are_checked() {
    let src = "let a = 1\n";
    let name = "x = 1\nlaunch()\nlet y";
    assert!(modify_source_with_goals(src, &[Goal::should_set_value(name, 2)]).is_err());
    assert!(modify_source_with_goals(src, &[Goal::should_set_value_expr(name, ink())]).is_err());
    assert!(modify_source_with_goals(src, &[Goal::should_call("launch(); f", [1])]).is_err());
    assert!(apply(src, &[Goal::should_set_value_expr("b", ink())]).ends_with("let b = tok.color.ink\n"));
}

/// A key with a newline or tab is written `["a\nb"]` and its path step
/// reads back as the same key.
#[test]
fn a_quoted_key_with_escapes_round_trips_through_a_path() {
    let src = "let o = {[\"a\\nb\"]: 1, [\"a\\tb\"]: 2}\n";
    let out = apply(src, &[Goal::should_set_path("o[\"a\\nb\"]", 3), Goal::should_set_path("o[\"a\\tb\"]", 4)]);
    assert_eq!(out, "let o = {[\"a\\nb\"]: 3, [\"a\\tb\"]: 4}\n");
}
