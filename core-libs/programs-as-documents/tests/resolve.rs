//! Resolving intents: steps 1 and 2 of the ladder, the cases that need a
//! choice or a hand edit, verify-by-rerun, and committing through a
//! `Document`.

mod common;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::{
    Document, DocumentSession, Intent, OutputKey, Resolution, ResolveOptions, ResolvedEdit, Runner, Step, resolve,
    verify, Verified,
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
fn a_top_level_literal_is_edited_in_place_keeping_comments_and_layout() {
    let src = "// A poster.
let items = [
  <rect id=\"a\"   x={10}  y={5} />,   // the first
  // between
  <rect id=\"b\" x={20} />,
]
items
";
    let e = edit(resolve_src(&value_runner(), src, |_| set(OutputKey::Id("a".into()), "x", StaticValue::Int(300))));
    assert_eq!(e.step, Step::DirectLiteral);
    assert_eq!(e.source, src.replace("x={10}", "x={300}"));
    assert!(e.description.contains("line 3"), "{}", e.description);
}

#[test]
fn a_float_keeps_its_spelling_and_a_string_its_quotes() {
    let src = "[<text size={3.50} label=\"hi\" />]";
    let r = value_runner();
    let k = |r: &programs_as_documents::DocumentRun| key_of(r, "text", 0);
    let e = edit(resolve_src(&r, src, |r| set(k(r), "size", StaticValue::Float(4.25))));
    assert_eq!(e.source, "[<text size={4.25} label=\"hi\" />]");
    let e = edit(resolve_src(&r, src, |r| set(k(r), "label", StaticValue::str("yo"))));
    assert_eq!(e.source, "[<text size={3.50} label=\"yo\" />]");
}

#[test]
fn a_helper_called_once_is_edited_at_its_argument_or_in_its_body() {
    let src = "import shapes
fn card(x, title)
  <card x={x} w={40} title={title} />
end
doc([card(100, title: \"one\")])
";
    let r = emit_runner();
    let k = |r: &programs_as_documents::DocumentRun| key_of(r, "card", 0);
    // Step 1 through a parameter: the call's argument.
    let e = edit(resolve_src(&r, src, |r| set(k(r), "x", StaticValue::Int(150))));
    assert_eq!(e.source, src.replace("card(100,", "card(150,"));
    let e = edit(resolve_src(&r, src, |r| set(k(r), "title", StaticValue::str("uno"))));
    assert_eq!(e.source, src.replace("\"one\"", "\"uno\""));
    // Step 1 in the body: one call, so the body's literal is this card's.
    let e = edit(resolve_src(&r, src, |r| set(k(r), "w", StaticValue::Int(50))));
    assert_eq!(e.source, src.replace("w={40}", "w={50}"));
    // Step 2: a field the literal does not write is added to it.
    let e = edit(resolve_src(&r, src, |r| set(k(r), "h", StaticValue::Int(9))));
    assert_eq!(e.step, Step::SingleSite);
    assert!(e.source.contains("<card x={x} w={40} title={title} h={9} />"), "{}", e.source);
}

#[test]
fn a_helper_called_twice_edits_each_calls_argument_but_asks_about_its_body() {
    let src = "import shapes
fn card(x)
  <card x={x} w={40} />
end
doc([card(1), card(2)])
";
    let r = emit_runner();
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "card", 1), "x", StaticValue::Int(5))));
    assert_eq!(e.source, src.replace("card(2)", "card(5)"));
    match resolve_src(&r, src, |r| set(key_of(r, "card", 1), "w", StaticValue::Int(50))) {
        Resolution::NeedsChoice { reason, options } => {
            assert!(reason.contains("2 outputs"), "{reason}");
            assert_eq!(options[0].edit.as_ref().unwrap().source, src.replace("w={40}", "w={50}"));
        }
        other => panic!("{other:?}"),
    }
    // Adding an attribute to the shared literal is a choice too.
    assert!(matches!(
        resolve_src(&r, src, |r| set(key_of(r, "card", 0), "h", StaticValue::Int(1))),
        Resolution::NeedsChoice { .. }
    ));
}

#[test]
fn a_named_constant_is_edited_where_it_is_bound() {
    let src = "let m = 96 // margin
[<box x={m} />, <box id=\"other\" x={4} />]
";
    let e = edit(resolve_src(&value_runner(), src, |r| set(key_of(r, "box", 0), "x", StaticValue::Int(120))));
    assert_eq!(e.source, src.replace("96", "120"));
}

#[test]
fn a_named_constant_feeding_other_outputs_needs_a_choice() {
    // Two outputs read `m` directly: the trace sees the sharing.
    let src = "let m = 96\n[<box x={m} />, <box x={m} />]\n";
    match resolve_src(&value_runner(), src, |r| set(key_of(r, "box", 0), "x", StaticValue::Int(1))) {
        Resolution::NeedsChoice { reason, .. } => assert!(reason.contains("2 outputs"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // `m` also feeds a computed field: only the rerun sees that.
    let src = "let m = 96\n[<box x={m} />, <line w={m + 1} />]\n";
    match resolve_src(&value_runner(), src, |r| set(key_of(r, "box", 0), "x", StaticValue::Int(1))) {
        Resolution::NeedsChoice { reason, options } => {
            assert!(reason.contains("<line>"), "{reason}");
            assert_eq!(options[0].edit.as_ref().unwrap().source, src.replace("96", "1"));
        }
        other => panic!("{other:?}"),
    }
    // ...including a computed field of the same element.
    let src = "let m = 96\n[<box x={m} w={m * 2} />]\n";
    match resolve_src(&value_runner(), src, |r| set(key_of(r, "box", 0), "x", StaticValue::Int(1))) {
        Resolution::NeedsChoice { reason, .. } => assert!(reason.contains("`w`"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn loop_built_elements_need_a_choice_or_are_computed() {
    let src = "let items = for i in range(3) do
  <dot x={i * 10} y={7} />
end
items
";
    let r = value_runner();
    match resolve_src(&r, src, |r| set(key_of(r, "dot", 1), "y", StaticValue::Int(8))) {
        Resolution::NeedsChoice { reason, options } => {
            assert!(reason.contains("3 outputs"), "{reason}");
            assert_eq!(options[0].edit.as_ref().unwrap().source, src.replace("y={7}", "y={8}"));
        }
        other => panic!("{other:?}"),
    }
    // With solving off, a computed field is a reason that names the line.
    let no_solve = ResolveOptions { solve: false, ..ResolveOptions::default() };
    let rn = run(&r, src);
    match resolve(&r, &rn, &set(key_of(&rn, "dot", 1), "x", StaticValue::Int(8)), &no_solve) {
        Resolution::Unresolved { reason, at } => {
            assert!(reason.contains("computed") && reason.contains("line 2"), "{reason}");
            assert_eq!(at.unwrap().start.line, 2);
        }
        other => panic!("{other:?}"),
    }
    // With it on (the default), the solver changes the loop's number, and
    // says which other outputs move with it.
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "dot", 1), "x", StaticValue::Int(8))));
    assert_eq!(e.step, Step::Solve);
    assert_eq!(e.source, src.replace("i * 10", "i * 8"));
    assert_eq!(e.description, "set `10` on line 2 to `8`");
    assert_eq!(e.also_changes.len(), 1, "{:?}", e.also_changes);
}

#[test]
fn dotted_fields_edit_inside_record_literals() {
    let src = "let theme = {ink: \"#111\", pad: 4}
[<box fill={theme.ink} style={{w: 2, h: 3}} />]
";
    let r = value_runner();
    let k = |r: &programs_as_documents::DocumentRun| key_of(r, "box", 0);
    let e = edit(resolve_src(&r, src, |r| set(k(r), "style.h", StaticValue::Int(30))));
    assert_eq!(e.source, src.replace("h: 3", "h: 30"));
    let e = edit(resolve_src(&r, src, |r| set(k(r), "style.d", StaticValue::Int(1))));
    assert!(e.source.contains("style={{w: 2, h: 3, d: 1}}"), "{}", e.source);
    let e = edit(resolve_src(&r, src, |r| set(k(r), "fill", StaticValue::str("#222"))));
    assert_eq!(e.source, src.replace("#111", "#222"));
    let e = edit(resolve_src(&r, src, |r| Intent::RemoveField { target: k(r), field: "style.w".into() }));
    assert!(e.source.contains("style={{h: 3}}"), "{}", e.source);
}

#[test]
fn removing_a_field_removes_the_attribute() {
    let src = "[<rect x={1} y={2} />]";
    let r = value_runner();
    let e = edit(resolve_src(&r, src, |r| Intent::RemoveField { target: key_of(r, "rect", 0), field: "y".into() }));
    assert_eq!(e.source, "[<rect x={1} />]");
    assert_eq!(
        resolve_src(&r, src, |r| Intent::RemoveField { target: key_of(r, "rect", 0), field: "z".into() }),
        Resolution::AlreadyHolds
    );
    assert_eq!(
        resolve_src(&r, src, |r| set(key_of(r, "rect", 0), "x", StaticValue::Float(1.0))),
        Resolution::AlreadyHolds
    );
}

#[test]
fn an_edit_that_would_land_in_another_file_is_refused_naming_it() {
    let src = "import shapes\ndoc([badge(\"hi\", 3)])\n";
    let r = emit_runner();
    match resolve_src(&r, src, |r| set(key_of(r, "badge", 0), "size", StaticValue::Int(20))) {
        Resolution::Refused { reason } => assert!(reason.contains("shapes"), "{reason}"),
        other => panic!("{other:?}"),
    }
    match resolve_src(&r, src, |r| set(key_of(r, "badge", 0), "tone", StaticValue::str("red"))) {
        Resolution::Refused { reason } => assert!(reason.contains("not in this document"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // The argument is in the document, so it can be edited.
    let e = edit(resolve_src(&r, src, |r| set(key_of(r, "badge", 0), "x", StaticValue::Int(4))));
    assert_eq!(e.source, src.replace(", 3)", ", 4)"));
}

#[test]
fn an_edit_whose_rerun_fails_is_refused() {
    // `m` is a direct literal of `x`, but making it a string breaks `m * 2`.
    let src = "let m = 96\n[<box x={m} />, <line w={m * 2} />]\n";
    match resolve_src(&value_runner(), src, |r| set(key_of(r, "box", 0), "x", StaticValue::str("wide"))) {
        Resolution::Refused { reason } => assert!(reason.contains("does not run"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // A source that does not make the intent hold fails verification.
    let runner = value_runner();
    let r = run(&runner, "[<box x={1} />]");
    let node = r.map.nodes[0].clone();
    let intent = set(node.key.clone(), "x", StaticValue::Int(2));
    match verify(&runner, &r, &node, &intent, "[<box x={3} />]") {
        Verified::Fails(reason) => assert!(reason.contains("is 3, not 2"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_document_commits_resolved_edits_as_undo_steps_and_leaves_others_alone() {
    let src = "let items = [<rect id=\"a\" x={1} />, <rect id=\"b\" x={1} />]\nitems\n";
    let mut doc = Document::new(DocumentSession::new("doc.ptl".into(), src.into()), value_runner());
    let a = OutputKey::Id("a".into());
    assert!(matches!(doc.apply(&set(a.clone(), "x", StaticValue::Int(5)), None), Resolution::Edit(_)));
    assert!(matches!(doc.apply(&set(a.clone(), "y", StaticValue::Int(6)), None), Resolution::Edit(_)));
    assert_eq!(doc.session.source(), src.replace("id=\"a\" x={1}", "id=\"a\" x={5} y={6}"));
    // A refusal leaves the source as it was.
    let before = doc.session.source().to_string();
    let bad = set(OutputKey::Id("nope".into()), "x", StaticValue::Int(1));
    assert!(matches!(doc.apply(&bad, None), Resolution::Unresolved { .. }));
    assert_eq!(doc.session.source(), before);
    assert!(doc.session.undo());
    assert_eq!(doc.session.source(), src.replace("x={1} />, <rect id=\"b\"", "x={5} />, <rect id=\"b\""));
    assert!(doc.session.undo());
    assert_eq!(doc.session.source(), src);
    assert!(doc.session.redo());
    // The run follows the source.
    let x = doc.run().unwrap().map.find(&a).unwrap().fields[1].clone();
    assert_eq!(x.name, "x");
    // A drag: keyed edits coalesce into one step.
    let mut doc = Document::new(DocumentSession::new("doc.ptl".into(), src.into()), value_runner());
    for v in [2, 3, 4] {
        assert!(matches!(doc.apply(&set(a.clone(), "x", StaticValue::Int(v)), Some("drag a")), Resolution::Edit(_)));
    }
    assert!(doc.session.undo());
    assert_eq!(doc.session.source(), src);
    assert!(!doc.session.can_undo());
}
