//! Adversarial cases from an independent review: places where an edit
//! could land on the wrong output, the wrong literal or the wrong text.

mod common;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::{
    Intent, OutputKey, Resolution, ResolveOptions, ResolvedEdit, Runner, resolve,
};

fn set(target: OutputKey, field: &str, value: StaticValue) -> Intent {
    Intent::SetField { target, field: field.into(), value }
}

fn resolve_src(runner: &Runner, src: &str, intent: impl Fn(&programs_as_documents::DocumentRun) -> Intent) -> Resolution {
    let r = run(runner, src);
    let i = intent(&r);
    resolve(runner, &r, &i, &ResolveOptions::default())
}

fn edit(res: Resolution) -> ResolvedEdit {
    match res {
        Resolution::Edit(e) => e,
        other => panic!("expected an edit, got {other:?}"),
    }
}

#[test]
fn duplicate_ids_give_each_output_its_own_key() {
    let src = "[<rect id=\"a\" x={1} />, <rect id=\"a\" x={2} />]";
    let r = run(&value_runner(), src);
    let keys: Vec<OutputKey> = r.map.nodes.iter().map(|n| n.key.clone()).collect();
    assert_eq!(keys.len(), 2);
    assert_ne!(keys[0], keys[1], "two outputs share the key {:?}", keys[0]);
    // Editing the second one edits the second literal.
    let e = edit(resolve(&value_runner(), &r, &set(keys[1].clone(), "x", StaticValue::Int(9)), &ResolveOptions::default()));
    assert_eq!(e.source, "[<rect id=\"a\" x={1} />, <rect id=\"a\" x={9} />]");
}

#[test]
fn an_id_on_one_loop_output_does_not_renumber_the_others() {
    // Giving the first card an id must not make the rerun think the cards
    // after it changed (their site occurrences stay put).
    let src = "let rows = [\n  {x: 1, id: nil},\n  {x: 2, id: \"k\"},\n  {x: 3, id: nil},\n]\nlet out = for r in rows do\n  <card x={r.x} id={r.id} />\nend\nout\n";
    let r = run(&value_runner(), src);
    let k = key_of(&r, "card", 0);
    let e = edit(resolve(&value_runner(), &r, &set(k, "id", StaticValue::str("first")), &ResolveOptions::default()));
    assert!(e.also_changes.is_empty(), "{:?}", e.also_changes);
    assert_eq!(e.source, src.replacen("id: nil", "id: \"first\"", 1));
}

#[test]
fn setting_an_id_another_output_has_is_not_verified_against_that_output() {
    let src = "[<rect id=\"a\" x={1} />, <rect id=\"b\" x={2} />]";
    let res = resolve_src(&value_runner(), src, |_| set(OutputKey::Id("b".into()), "id", StaticValue::str("a")));
    assert!(!matches!(res, Resolution::Edit(_) | Resolution::AlreadyHolds), "{res:?}");
}

#[test]
fn non_ascii_text_before_a_literal_keeps_the_edit_on_the_literal() {
    let src = "// café ☕ 🎨 poster\nlet title = \"naïve 🎨\"\n[<text label={title} x={10} />]\n";
    let r = value_runner();
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "text", 0), "x", StaticValue::Int(42))));
    assert_eq!(e.source, src.replace("x={10}", "x={42}"));
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "text", 0), "label", StaticValue::str("crème"))));
    assert_eq!(e.source, src.replace("\"naïve 🎨\"", "\"crème\""));
}

#[test]
fn a_recursive_helper_traces_its_parameter_to_the_outermost_argument() {
    let src = "fn tree(n, x)\n  if n == 0 then\n    <leaf x={x} />\n  else\n    <g>{tree(n - 1, x)}</g>\n  end\nend\n[tree(2, 5), tree(1, 7)]\n";
    let r = value_runner();
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "leaf", 1), "x", StaticValue::Int(70))));
    assert_eq!(e.source, src.replace("tree(1, 7)", "tree(1, 70)"));
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "leaf", 0), "x", StaticValue::Int(50))));
    assert_eq!(e.source, src.replace("tree(2, 5)", "tree(2, 50)"));
}

#[test]
fn one_element_value_listed_twice_asks_before_changing_both() {
    let src = "let a = <rect x={1} />\n[a, a]\n";
    let res = resolve_src(&value_runner(), src, |r| set(key_of(r, "rect", 1), "x", StaticValue::Int(2)));
    assert!(matches!(res, Resolution::NeedsChoice { .. }), "{res:?}");
}

#[test]
fn an_edit_that_adds_an_output_is_not_made_silently() {
    let src = "let w = 10\n[<a w={w} />, if w > 50 then <b /> else nil end]\n";
    let res = resolve_src(&value_runner(), src, |r| set(key_of(r, "a", 0), "w", StaticValue::Int(100)));
    assert!(!matches!(res, Resolution::Edit(ref e) if e.also_changes.is_empty()), "{res:?}");
}

#[test]
fn one_literal_feeding_two_props_of_one_element_asks() {
    let src = "let s = 10\n[<rect w={s} h={s} />]\n";
    let res = resolve_src(&value_runner(), src, |r| set(key_of(r, "rect", 0), "w", StaticValue::Int(20)));
    assert!(matches!(res, Resolution::NeedsChoice { .. }), "{res:?}");
}

#[test]
fn an_attribute_overwritten_by_an_assignment_is_not_edited_at_the_literal() {
    let src = "var e = <rect x={1} />\ne.x = 5\n[e]\n";
    let Ok(r) = value_runner().run(src) else { return };
    let res = resolve(&value_runner(), &r, &set(key_of(&r, "rect", 0), "x", StaticValue::Int(9)), &ResolveOptions::default());
    if let Resolution::Edit(e) = &res {
        // Whatever it wrote, the rerun must show x = 9.
        let after = run(&value_runner(), &e.source);
        let v = after.node_value(&after.map.nodes[0]).unwrap();
        assert_eq!(programs_as_documents::intent::field_of(&v, "x"), Some(&StaticValue::Int(9)), "{}", e.source);
    }
}

#[test]
fn a_change_to_an_output_that_holds_a_function_is_noticed() {
    // `<btn>` holds a function, so it has no literal form; the rerun must
    // still see that its `x` moved with `m`.
    let src = "let m = 5\n[<a x={m} />, <btn x={m + 1} on_click={fn() -> 1} />]\n";
    let res = resolve_src(&value_runner(), src, |r| set(key_of(r, "a", 0), "x", StaticValue::Int(50)));
    assert!(!matches!(res, Resolution::Edit(ref e) if e.also_changes.is_empty()), "{res:?}");
}

#[test]
fn an_output_that_holds_a_function_can_still_be_edited() {
    let src = "[<btn x={1} on_click={fn() -> 1} />]\n";
    let e = edit(resolve_src(&value_runner(), src, |r| set(key_of(r, "btn", 0), "x", StaticValue::Int(2))));
    assert_eq!(e.source, src.replace("x={1}", "x={2}"));
}

#[test]
fn a_structural_edit_of_outputs_that_hold_functions_is_still_checked() {
    use programs_as_documents::{Parent, PathSeg, StructureIntent, StructureOptions, resolve_structure};
    // Every output holds a function, so none has a literal form; the rerun
    // is still compared (functions compare as opaque) and the edit made.
    let src = "let items = [\n  <btn id=\"a\" on_click={fn() -> 1} />,\n  <btn id=\"b\" on_click={fn() -> 2} />,\n]\nitems\n";
    let r = run(&value_runner(), src);
    let res = resolve_structure(
        &value_runner(),
        &r,
        &StructureIntent::Reorder {
            parent: Parent::List(vec![PathSeg::Index(0)]),
            order: vec![OutputKey::Id("b".into()), OutputKey::Id("a".into())],
        },
        &StructureOptions::default(),
    );
    let e = edit(res);
    assert!(e.source.find("id=\"b\"").unwrap() < e.source.find("id=\"a\"").unwrap(), "{}", e.source);
}
