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

#[test]
fn a_filtered_loop_edits_the_entry_that_built_the_output() {
    let src = "let rows = [\n  {x: 1, show: true},\n  {x: 2, show: false},\n  {x: 3, show: true},\n]\nlet out = for r in rows do\n  if r.show then <card x={r.x} /> else nil end\nend\nout\n";
    let res = resolve_src(&value_runner(), src, |r| set(key_of(r, "card", 1), "x", StaticValue::Int(30)));
    match res {
        Resolution::Edit(e) => assert_eq!(e.source, src.replace("{x: 3,", "{x: 30,"), "{}", e.description),
        r @ (Resolution::Unresolved { .. } | Resolution::Refused { .. } | Resolution::NeedsChoice { .. }) => eprintln!("OUTCOME {r:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_map_over_a_filtered_list_does_not_edit_a_hidden_entry() {
    let src = "let rows = [{x: 1, show: true}, {x: 2, show: false}, {x: 3, show: true}]\nrows.filter(fn(r) -> r.show).map(fn(r) -> <card x={r.x} />)\n";
    let res = resolve_src(&value_runner(), src, |r| set(key_of(r, "card", 1), "x", StaticValue::Int(30)));
    match res {
        Resolution::Edit(e) => assert_eq!(e.source, src.replace("{x: 3,", "{x: 30,"), "{}", e.description),
        r @ (Resolution::Unresolved { .. } | Resolution::Refused { .. } | Resolution::NeedsChoice { .. }) => eprintln!("OUTCOME {r:?}"),
        other => panic!("{other:?}"),
    }
}

mod structural {
    use super::*;
    use programs_as_documents::output_map::PathSeg;
    use programs_as_documents::{Parent, Place, StructureIntent, StructureOptions, Template, resolve_structure};

    fn go(src: &str, intent: impl Fn(&programs_as_documents::DocumentRun) -> StructureIntent) -> Resolution {
        let r = run(&value_runner(), src);
        resolve_structure(&value_runner(), &r, &intent(&r), &StructureOptions::default())
    }

    fn not_an_edit(res: &Resolution) -> bool {
        !matches!(res, Resolution::Edit(_))
    }

    #[test]
    fn a_list_literal_shown_twice_is_not_reordered_in_both_places() {
        let src = "let xs = [<a id=\"a\" />, <b id=\"b\" />]\n[xs, xs]\n";
        let res = go(src, |_| StructureIntent::Reorder {
            parent: Parent::List(vec![PathSeg::Index(0), PathSeg::Index(0)]),
            order: vec![OutputKey::Id("b".into()), OutputKey::Id("a".into())],
        });
        assert!(not_an_edit(&res), "{res:?}");
    }

    #[test]
    fn an_insert_into_a_helpers_list_called_twice_is_not_made_in_both() {
        let src = "fn row()\n  [<a />, <b />]\nend\n[row(), row()]\n";
        let res = go(src, |_| StructureIntent::Insert {
            at: Place { parent: Parent::List(vec![PathSeg::Index(0), PathSeg::Index(0)]), index: 1 },
            template: Template::Source("<new />".into()),
        });
        assert!(not_an_edit(&res), "{res:?}");
    }

    #[test]
    fn removing_an_element_a_helper_returns_twice_does_not_remove_both() {
        let src = "let a = <a />\n[a, <b />, a]\n";
        let res = go(src, |r| StructureIntent::Remove { target: key_of(r, "a", 1) });
        if let Resolution::Edit(e) = &res {
            let after = run(&value_runner(), &e.source);
            assert_eq!(tagged(&after, "a").len(), 1, "{}", e.source);
        }
    }

    #[test]
    fn removing_from_an_imported_list_is_refused() {
        let src = "import shapes\nlet mine = [<m id=\"m\" />]\nshapes.LIBRARY ++ mine\n";
        let res = go(src, |_| StructureIntent::Remove { target: OutputKey::Id("lib-a".into()) });
        assert!(not_an_edit(&res), "{res:?}");
        let r = run(&value_runner(), src);
        assert_eq!(r.source, src);
    }
}

mod files {
    use super::*;
    use programs_as_documents::output_map::PathSeg;
    use programs_as_documents::{Parent, StructureIntent, StructureOptions, resolve_structure};

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pad-review-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_constant_in_a_relative_module_is_never_written_into_the_document() {
        let d = dir("const");
        // The module's literal sits at the same offsets as text in the
        // document, so a wrong file check would edit the document there.
        std::fs::write(d.join("layout.ptl"), "pub let X = 10\npub fn card(t)\n  <card t={t} w={64} />\nend\n").unwrap();
        let r = Runner::new(d.join("doc.ptl"), setup);
        let src = "import ./layout: X, card\n[<dot x={X} />, card(\"a\")]\n";
        let res = resolve_src(&r, src, |r| set(key_of(r, "dot", 0), "x", StaticValue::Int(11)));
        assert!(matches!(res, Resolution::Refused { ref reason } if reason.contains("layout")), "{res:?}");
        let res = resolve_src(&r, src, |r| set(key_of(r, "card", 0), "w", StaticValue::Int(65)));
        assert!(!matches!(res, Resolution::Edit(_)), "{res:?}");
        let res = resolve_src(&r, src, |r| set(key_of(r, "card", 0), "h", StaticValue::Int(5)));
        assert!(!matches!(res, Resolution::Edit(_)), "{res:?}");
        // The argument is the document's own.
        let e = edit(resolve_src(&r, src, |r| set(key_of(r, "card", 0), "t", StaticValue::str("b"))));
        assert_eq!(e.source, src.replace("card(\"a\")", "card(\"b\")"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_relative_modules_list_is_locked_for_structural_edits() {
        let d = dir("list");
        std::fs::write(d.join("lib.ptl"), "pub let ITEMS = [<li id=\"x\" />, <li id=\"y\" />]\n").unwrap();
        let r = Runner::new(d.join("doc.ptl"), setup);
        let src = "import ./lib: ITEMS\nITEMS\n";
        let run_ = run(&r, src);
        let res = resolve_structure(
            &r,
            &run_,
            &StructureIntent::Reorder {
                parent: Parent::List(vec![PathSeg::Index(0)]),
                order: vec![OutputKey::Id("y".into()), OutputKey::Id("x".into())],
            },
            &StructureOptions::default(),
        );
        assert!(!matches!(res, Resolution::Edit(_)), "{res:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

mod clipboard {
    use super::*;
    use programs_as_documents::output_map::PathSeg;
    use programs_as_documents::{Parent, PasteOptions, Place, copy, paste};

    #[test]
    fn a_pasted_reference_the_target_binds_differently_keeps_the_copied_value() {
        let from = run(&value_runner(), "let accent = 1\n[<dot id=\"d\" x={accent} />]\n");
        let clip = copy(&from, &OutputKey::Id("d".into())).unwrap();
        let target = "let accent = 2\n[\n  <b id=\"b\" x={accent} />,\n]\n";
        let to = run(&value_runner(), target);
        let at = Place { parent: Parent::List(vec![PathSeg::Index(0)]), index: 1 };
        match paste(&value_runner(), &to, &clip, at, &PasteOptions::default()) {
            Resolution::Edit(e) => {
                let after = run(&value_runner(), &e.source);
                let v = after.node_value(after.map.find(&OutputKey::Id("d".into())).unwrap()).unwrap();
                assert_eq!(programs_as_documents::intent::field_of(&v, "x"), Some(&StaticValue::Int(1)), "{}", e.source);
            }
            other => assert!(!matches!(other, Resolution::AlreadyHolds), "{other:?}"),
        }
    }
}

mod session {
    use programs_as_documents::DocumentSession;

    fn s() -> DocumentSession {
        DocumentSession::new("/nonexistent/review-doc.ptl".into(), "0".into())
    }

    #[test]
    fn an_edit_after_an_undo_never_coalesces_into_the_undone_one() {
        let mut d = s();
        d.commit("1".into(), Some("k"));
        d.commit("2".into(), Some("k"));
        assert!(d.undo());
        assert_eq!(d.source(), "0");
        d.commit("3".into(), Some("k"));
        assert!(d.undo());
        assert_eq!(d.source(), "0");
        assert!(!d.can_redo() || d.redo());
    }

    #[test]
    fn interleaved_keys_are_separate_steps() {
        let mut d = s();
        d.commit("1".into(), Some("a"));
        d.commit("2".into(), Some("b"));
        d.commit("3".into(), Some("a"));
        assert!(d.undo());
        assert_eq!(d.source(), "2");
        assert!(d.undo());
        assert_eq!(d.source(), "1");
    }

    #[test]
    fn a_keyed_edit_after_a_gesture_is_its_own_step() {
        let mut d = s();
        d.commit("1".into(), Some("k"));
        d.begin_gesture();
        d.commit("2".into(), Some("k"));
        d.commit("3".into(), Some("k"));
        assert!(d.end_gesture());
        d.commit("4".into(), Some("k"));
        assert!(d.undo());
        assert_eq!(d.source(), "3");
        assert!(d.undo());
        assert_eq!(d.source(), "1");
        assert!(d.undo());
        assert_eq!(d.source(), "0");
    }

    #[test]
    fn a_cancelled_gesture_leaves_no_step() {
        let mut d = s();
        d.begin_gesture();
        d.commit("1".into(), None);
        assert!(d.cancel_gesture());
        assert_eq!(d.source(), "0");
        assert!(!d.can_undo());
    }

    #[test]
    fn a_cancelled_gesture_keeps_the_redo_steps() {
        let mut d = s();
        d.commit("1".into(), None);
        assert!(d.undo());
        d.begin_gesture();
        d.commit("2".into(), None);
        assert!(d.cancel_gesture());
        assert!(d.redo());
        assert_eq!(d.source(), "1");
    }
}

#[test]
fn a_lambda_called_through_a_user_helper_traces_to_the_outer_argument() {
    let src = "fn apply(f, v)\n  f(v)\nend\n[apply(fn(x) -> <a x={x} />, 5), apply(fn(x) -> <a x={x} />, 6)]\n";
    let e = edit(resolve_src(&value_runner(), src, |r| set(key_of(r, "a", 1), "x", StaticValue::Int(60))));
    assert_eq!(e.source, src.replace(", 6)", ", 60)"));
}
