//! Structural edits by outcome: reorder, move (within a list, between
//! parents), insert, remove and duplicate, resolved against list literals
//! (with comments and helper-call entries), element children, data lists
//! read by `map` and `for`, and concatenations with a library's list; the
//! `id=` written for new elements; and the reasons given when an edit is
//! refused.

mod common;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::{
    Document, DocumentRun, DocumentSession, EntrySource, OutputKey, Parent, Place, Resolution, ResolvedEdit, Runner,
    Step, StructureIntent, StructureOptions, Template, parent_of, resolve_structure, sibling_entries,
};
use programs_as_documents::output_map::PathSeg;

fn resolve_with(runner: &Runner, src: &str, opts: &StructureOptions, intent: impl Fn(&DocumentRun) -> StructureIntent) -> Resolution {
    let r = run(runner, src);
    let i = intent(&r);
    resolve_structure(runner, &r, &i, opts)
}

fn resolve_src(runner: &Runner, src: &str, intent: impl Fn(&DocumentRun) -> StructureIntent) -> Resolution {
    resolve_with(runner, src, &StructureOptions::default(), intent)
}

fn edit(res: Resolution) -> ResolvedEdit {
    match res {
        Resolution::Edit(e) => e,
        other => panic!("expected an edit, got {other:?}"),
    }
}

fn refused(res: Resolution) -> String {
    match res {
        Resolution::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn id(s: &str) -> OutputKey {
    OutputKey::Id(s.into())
}

/// The root list of a document whose value is the list.
fn root() -> Parent {
    Parent::List(vec![PathSeg::Index(0)])
}

/// Labels of the elements of `parent` in `src`'s run: the id, else the
/// `title`, else the tag.
fn labels(runner: &Runner, src: &str, parent: &Parent) -> Vec<String> {
    let r = run(runner, src);
    programs_as_documents::siblings(&r, parent)
        .unwrap()
        .iter()
        .map(|n| {
            let v = r.node_value(n).unwrap();
            let f = |k: &str| programs_as_documents::intent::field_of(&v, k).cloned();
            match (f("id"), f("title")) {
                (Some(StaticValue::Str(s)), _) | (None, Some(StaticValue::Str(s))) => s,
                _ => n.tag.clone(),
            }
        })
        .collect()
}

// ── a list literal ─────────────────────────────────────────────────────────

const LIST: &str = "fn hero(t)
  <hero title={t} />
end

[
  <a id=\"a\" />, // first
  hero(\"h\"),
  // the last one
  <c id=\"c\" x={1} />,
]
";

fn hero_key(r: &DocumentRun) -> OutputKey {
    tagged(r, "hero")[0].key.clone()
}

#[test]
fn reorder_a_literal_keeps_entry_text_and_comments() {
    let e = edit(resolve_src(&value_runner(), LIST, |r| StructureIntent::Reorder {
        parent: root(),
        order: vec![id("c"), id("a"), hero_key(r)],
    }));
    assert_eq!(e.step, Step::Entries);
    assert_eq!(
        e.source,
        "fn hero(t)
  <hero title={t} />
end

[
  <c id=\"c\" x={1} />,
  <a id=\"a\" />, // first
  hero(\"h\"),
  // the last one
]
"
    );
    assert_eq!(labels(&value_runner(), &e.source, &root()), ["c", "a", "h"]);
}

#[test]
fn move_within_a_list() {
    let e = edit(resolve_src(&value_runner(), LIST, |_| StructureIntent::Move {
        target: id("a"),
        to: Place { parent: root(), index: 2 },
    }));
    assert_eq!(labels(&value_runner(), &e.source, &root()), ["h", "c", "a"]);
    assert!(e.source.contains("<a id=\"a\" />, // first\n]"), "{}", e.source);
    // A helper call moves as written.
    let e = edit(resolve_src(&value_runner(), LIST, |r| StructureIntent::Move {
        target: hero_key(r),
        to: Place { parent: root(), index: 0 },
    }));
    assert!(e.source.contains("[\n  hero(\"h\"),\n  <a id=\"a\" />, // first\n"), "{}", e.source);
}

#[test]
fn remove_takes_the_entry_and_its_line() {
    let e = edit(resolve_src(&value_runner(), LIST, |_| StructureIntent::Remove { target: id("a") }));
    assert!(!e.source.contains("first"), "{}", e.source);
    assert!(e.source.contains("[\n  hero(\"h\"),\n  // the last one\n"), "{}", e.source);
    let e = edit(resolve_src(&value_runner(), LIST, |r| StructureIntent::Remove { target: hero_key(r) }));
    assert_eq!(labels(&value_runner(), &e.source, &root()), ["a", "c"]);
}

#[test]
fn insert_a_value_and_source_text() {
    let el = StaticValue::Element { tag: "b".into(), props: vec![("x".into(), StaticValue::Int(5))], children: vec![] };
    let e = edit(resolve_src(&value_runner(), LIST, |_| StructureIntent::Insert {
        at: Place { parent: root(), index: 1 },
        template: Template::Value(el.clone()),
    }));
    assert!(e.source.contains("<a id=\"a\" />, // first\n  <b x={5} />,\n  hero(\"h\"),"), "{}", e.source);
    // Source text keeps its own layout, indented to its new place.
    let text = "<group>\n  <b />\n</group>";
    let e = edit(resolve_src(&value_runner(), LIST, |_| StructureIntent::Insert {
        at: Place { parent: root(), index: 3 },
        template: Template::Source(text.into()),
    }));
    assert!(e.source.contains("  <c id=\"c\" x={1} />,\n  <group>\n    <b />\n  </group>,\n]"), "{}", e.source);
    // With write_ids, the new element gets an id of its own.
    let opts = StructureOptions { write_ids: true };
    let e = edit(resolve_with(&value_runner(), LIST, &opts, |_| StructureIntent::Insert {
        at: Place { parent: root(), index: 0 },
        template: Template::Value(el.clone()),
    }));
    assert!(e.source.contains("<b x={5} id=\"b-1\" />"), "{}", e.source);
    let r = run(&value_runner(), &e.source);
    assert!(r.node(&id("b-1")).is_some());
    let e = edit(resolve_with(&value_runner(), LIST, &opts, |_| StructureIntent::Insert {
        at: Place { parent: root(), index: 0 },
        template: Template::Source("<b x={7} />".into()),
    }));
    assert!(e.source.contains("<b x={7} id=\"b-1\" />"), "{}", e.source);
}

#[test]
fn duplicate_gives_the_copy_a_fresh_id() {
    let e = edit(resolve_src(&value_runner(), LIST, |_| StructureIntent::Duplicate { target: id("c") }));
    assert!(e.source.contains("<c id=\"c\" x={1} />,\n  <c id=\"c-1\" x={1} />,\n]"), "{}", e.source);
    // A call is copied as the call.
    let e = edit(resolve_src(&value_runner(), LIST, |r| StructureIntent::Duplicate { target: hero_key(r) }));
    assert!(e.source.contains("  hero(\"h\"),\n  hero(\"h\"),\n"), "{}", e.source);
    assert_eq!(labels(&value_runner(), &e.source, &root()), ["a", "h", "h", "c"]);
}

#[test]
fn an_inline_list_stays_inline() {
    let src = "[<a id=\"a\" />, <b id=\"b\" />, <c id=\"c\" />]\n";
    let e = edit(resolve_src(&value_runner(), src, |_| StructureIntent::Reorder {
        parent: root(),
        order: vec![id("b"), id("c"), id("a")],
    }));
    assert_eq!(e.source, "[<b id=\"b\" />, <c id=\"c\" />, <a id=\"a\" />]\n");
    let e = edit(resolve_src(&value_runner(), src, |_| StructureIntent::Remove { target: id("b") }));
    assert_eq!(e.source, "[<a id=\"a\" />, <c id=\"c\" />]\n");
}

#[test]
fn a_list_bound_to_a_name_and_handed_to_the_host() {
    let src = "import shapes\n\nlet items = [\n  <a id=\"a\" />,\n  <b id=\"b\" />,\n]\n\ndoc(items)\n";
    let e = edit(resolve_src(&emit_runner(), src, |_| StructureIntent::Move {
        target: id("b"),
        to: Place { parent: root(), index: 0 },
    }));
    assert_eq!(e.source, "import shapes\n\nlet items = [\n  <b id=\"b\" />,\n  <a id=\"a\" />,\n]\n\ndoc(items)\n");
}

#[test]
fn an_if_entry_that_may_give_nil() {
    let src = "let show = true\n[\n  <a id=\"a\" />,\n  if show then <b id=\"b\" /> else nil,\n  <c id=\"c\" />,\n]\n";
    let e = edit(resolve_src(&value_runner(), src, |_| StructureIntent::Move {
        target: id("b"),
        to: Place { parent: root(), index: 2 },
    }));
    assert!(e.source.contains("  <c id=\"c\" />,\n  if show then <b id=\"b\" /> else nil,\n]"), "{}", e.source);
}

// ── element children ──────────────────────────────────────────────────────

const GROUPS: &str = "[
  <group id=\"g1\">
    <a id=\"a\" />
    <b id=\"b\" /> {/* bee */}
  </group>,
  <group id=\"g2\">
    <c id=\"c\" />
  </group>,
  <d id=\"d\" />,
  <group id=\"empty\" />,
]
";

fn kids(s: &str) -> Parent {
    Parent::Children(id(s))
}

#[test]
fn reorder_children() {
    let e = edit(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Reorder {
        parent: kids("g1"),
        order: vec![id("b"), id("a")],
    }));
    assert_eq!(labels(&value_runner(), &e.source, &kids("g1")), ["b", "a"]);
    assert!(e.source.contains("bee"), "{}", e.source);
    let r = run(&value_runner(), GROUPS);
    assert_eq!(parent_of(&r, &id("b")), Some((kids("g1"), 1)));
}

#[test]
fn move_between_parents() {
    // Into another group.
    let e = edit(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Move {
        target: id("a"),
        to: Place { parent: kids("g2"), index: 1 },
    }));
    assert_eq!(labels(&value_runner(), &e.source, &kids("g1")), ["b"]);
    assert_eq!(labels(&value_runner(), &e.source, &kids("g2")), ["c", "a"]);
    assert!(e.source.contains("<group id=\"g2\">\n    <c id=\"c\" />\n    <a id=\"a\" />\n  </group>"), "{}", e.source);
    // Out of a group into the list around it.
    let e = edit(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Move {
        target: id("b"),
        to: Place { parent: root(), index: 0 },
    }));
    assert_eq!(labels(&value_runner(), &e.source, &root()), ["b", "g1", "g2", "d", "empty"]);
    assert_eq!(labels(&value_runner(), &e.source, &kids("g1")), ["a"]);
    // From the list into a group that comes later.
    let e = edit(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Move {
        target: id("d"),
        to: Place { parent: kids("g1"), index: 0 },
    }));
    assert_eq!(labels(&value_runner(), &e.source, &root()), ["g1", "g2", "empty"]);
    assert_eq!(labels(&value_runner(), &e.source, &kids("g1")), ["d", "a", "b"]);
    // Into an empty group.
    let e = edit(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Move {
        target: id("d"),
        to: Place { parent: kids("empty"), index: 0 },
    }));
    assert_eq!(labels(&value_runner(), &e.source, &kids("empty")), ["d"]);
    assert!(e.source.contains("<group id=\"empty\">\n    <d id=\"d\" />\n  </group>,"), "{}", e.source);
    // Not inside itself.
    let reason = refused(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Move {
        target: id("g1"),
        to: Place { parent: kids("g1"), index: 0 },
    }));
    assert!(reason.contains("inside itself"), "{reason}");
}

#[test]
fn insert_and_remove_children() {
    let el = StaticValue::Element { tag: "z".into(), props: vec![], children: vec![] };
    let e = edit(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Insert {
        at: Place { parent: kids("empty"), index: 0 },
        template: Template::Value(el.clone()),
    }));
    assert!(e.source.contains("<group id=\"empty\">\n    <z />\n  </group>,"), "{}", e.source);
    let e = edit(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Remove { target: id("b") }));
    assert!(!e.source.contains("<b "), "{}", e.source);
    assert_eq!(labels(&value_runner(), &e.source, &kids("g1")), ["a"]);
    // Only an element can be a child.
    let reason = refused(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Insert {
        at: Place { parent: kids("g1"), index: 0 },
        template: Template::Value(StaticValue::Int(3)),
    }));
    assert!(reason.contains("only an element"), "{reason}");
}

// ── data lists ────────────────────────────────────────────────────────────

const MAPPED: &str = "import shapes

let rows = [
  {title: \"one\"}, // 1
  {title: \"two\"},
  {title: \"three\"},
]

fn card(r)
  <card title={r.title} />
end

doc(rows.map(card))
";

fn card(r: &DocumentRun, i: usize) -> OutputKey {
    key_of(r, "card", i)
}

#[test]
fn reorder_a_mapped_list_reorders_the_data() {
    let e = edit(resolve_src(&emit_runner(), MAPPED, |r| StructureIntent::Reorder {
        parent: root(),
        order: vec![card(r, 2), card(r, 0), card(r, 1)],
    }));
    assert_eq!(e.step, Step::DataList);
    assert!(e.source.contains("let rows = [\n  {title: \"three\"},\n  {title: \"one\"}, // 1\n  {title: \"two\"},\n]"), "{}", e.source);
    let r = run(&emit_runner(), MAPPED);
    let entries = sibling_entries(&r, &root()).unwrap();
    assert!(matches!(&entries[1].source, EntrySource::Data { entry: 1, .. }), "{entries:?}");
}

#[test]
fn insert_remove_and_duplicate_in_data() {
    let rec = StaticValue::Record(vec![("title".into(), StaticValue::Str("new".into()))]);
    let e = edit(resolve_src(&emit_runner(), MAPPED, |_| StructureIntent::Insert {
        at: Place { parent: root(), index: 1 },
        template: Template::Value(rec.clone()),
    }));
    assert!(e.source.contains("  {title: \"one\"}, // 1\n  {title: \"new\"},\n  {title: \"two\"},"), "{}", e.source);
    assert_eq!(labels(&emit_runner(), &e.source, &root()), ["one", "new", "two", "three"]);
    let e = edit(resolve_src(&emit_runner(), MAPPED, |r| StructureIntent::Remove { target: card(r, 1) }));
    assert_eq!(labels(&emit_runner(), &e.source, &root()), ["one", "three"]);
    let e = edit(resolve_src(&emit_runner(), MAPPED, |r| StructureIntent::Duplicate { target: card(r, 2) }));
    assert_eq!(labels(&emit_runner(), &e.source, &root()), ["one", "two", "three", "three"]);
    // An element is not a data entry.
    let el = StaticValue::Element { tag: "card".into(), props: vec![], children: vec![] };
    let reason = refused(resolve_src(&emit_runner(), MAPPED, |_| StructureIntent::Insert {
        at: Place { parent: root(), index: 0 },
        template: Template::Value(el.clone()),
    }));
    assert!(reason.contains("data list on line 3"), "{reason}");
}

#[test]
fn a_for_loop_and_a_lambda_over_data() {
    let src = "let rows = [\"x\", \"y\", \"z\"]\nlet items = for t in rows do\n  <item title={t} />\nend\nitems\n";
    let e = edit(resolve_src(&value_runner(), src, |r| StructureIntent::Move {
        target: key_of(r, "item", 0),
        to: Place { parent: root(), index: 2 },
    }));
    assert!(e.source.starts_with("let rows = [\"y\", \"z\", \"x\"]\n"), "{}", e.source);
    // A group's children made by a `map` in a hole: the list the hole
    // gives is flattened into the children.
    let src = "let rows = [\"x\", \"y\"]\n<group>{rows.map(fn(t) <item title={t} /> end)}</group>\n";
    let r = run(&value_runner(), src);
    let g = Parent::Children(r.map.at_path(&[PathSeg::Index(0)]).unwrap().key.clone());
    assert_eq!(parent_of(&r, &key_of(&r, "item", 1)), Some((g.clone(), 1)));
    let e = edit(resolve_src(&value_runner(), src, |r| StructureIntent::Reorder {
        parent: g.clone(),
        order: vec![key_of(r, "item", 1), key_of(r, "item", 0)],
    }));
    assert!(e.source.starts_with("let rows = [\"y\", \"x\"]\n"), "{}", e.source);
}

#[test]
fn a_computed_data_list_is_refused_with_its_reason() {
    let src = "let rows = [\"x\", \"y\", \"z\"]\nrows.filter(fn(t) t != \"y\" end).map(fn(t) <item title={t} /> end)\n";
    let reason = refused(resolve_src(&value_runner(), src, |r| StructureIntent::Move {
        target: key_of(r, "item", 0),
        to: Place { parent: root(), index: 1 },
    }));
    assert!(reason.contains("is computed"), "{reason}");
}

// ── concatenation ─────────────────────────────────────────────────────────

const CONCAT: &str = "import shapes

let mine = [
  <mine id=\"m1\" />,
  <mine id=\"m2\" />,
]

doc(shapes.LIBRARY ++ mine)
";

#[test]
fn a_concatenation_edits_only_the_documents_segment() {
    let e = edit(resolve_src(&emit_runner(), CONCAT, |_| StructureIntent::Reorder {
        parent: root(),
        order: vec![id("lib-a"), id("lib-b"), id("m2"), id("m1")],
    }));
    assert!(e.source.contains("  <mine id=\"m2\" />,\n  <mine id=\"m1\" />,\n"), "{}", e.source);
    let r = run(&emit_runner(), CONCAT);
    let entries = sibling_entries(&r, &root()).unwrap();
    let EntrySource::Locked { reason } = &entries[0].source else { panic!("{entries:?}") };
    assert!(reason.contains("comes from `shapes"), "{reason}");
    // Moving a library element, or moving past one, is refused.
    let reason = refused(resolve_src(&emit_runner(), CONCAT, |_| StructureIntent::Remove { target: id("lib-a") }));
    assert!(reason.contains("comes from"), "{reason}");
    let reason = refused(resolve_src(&emit_runner(), CONCAT, |_| StructureIntent::Move {
        target: id("m2"),
        to: Place { parent: root(), index: 0 },
    }));
    assert!(reason.contains("cannot move"), "{reason}");
    // Inserting next to the document's own elements works.
    let el = StaticValue::Element { tag: "mine".into(), props: vec![("id".into(), StaticValue::Str("m3".into()))], children: vec![] };
    let e = edit(resolve_src(&emit_runner(), CONCAT, |_| StructureIntent::Insert {
        at: Place { parent: root(), index: 4 },
        template: Template::Value(el.clone()),
    }));
    assert!(e.source.contains("  <mine id=\"m2\" />,\n  <mine id=\"m3\" />,\n]"), "{}", e.source);
}

// ── verification and the session ──────────────────────────────────────────

#[test]
fn an_edit_that_does_not_give_the_order_is_refused() {
    // Two separate lists joined: an element cannot move into the other's
    // segment by reordering alone.
    let src = "let xs = [<a id=\"a\" />, <b id=\"b\" />]\nlet ys = [<c id=\"c\" />]\nxs ++ ys\n";
    let reason = refused(resolve_src(&value_runner(), src, |_| StructureIntent::Reorder {
        parent: root(),
        order: vec![id("a"), id("c"), id("b")],
    }));
    assert!(reason.contains("separate lists (the list on line 1, the list on line 2)"), "{reason}");
    // A move puts it in the other list.
    let e = edit(resolve_src(&value_runner(), src, |_| StructureIntent::Move {
        target: id("b"),
        to: Place { parent: root(), index: 2 },
    }));
    assert_eq!(e.source, "let xs = [<a id=\"a\" />]\nlet ys = [<c id=\"c\" />, <b id=\"b\" />]\nxs ++ ys\n");
    // A rerun that does not give what was asked is refused: the boxes'
    // children are written once, in the helper, so reordering one box's
    // reorders both.
    let src = "fn box() <box>\n  <p />\n  <q />\n</box> end\n[box(), box()]\n";
    let reason = refused(resolve_src(&value_runner(), src, |r| StructureIntent::Reorder {
        parent: Parent::Children(key_of(r, "box", 0)),
        order: vec![key_of(r, "q", 0), key_of(r, "p", 0)],
    }));
    assert!(reason.contains("does not do what was asked"), "{reason}");
    assert!(reason.contains("nothing was changed"), "{reason}");
    // The same literal reached twice through one entry moves as a group.
    let src = "fn two() [<p />, <q />] end\n[<a id=\"a\" />] ++ two() ++ two()\n";
    let reason = refused(resolve_src(&value_runner(), src, |r| StructureIntent::Remove { target: key_of(r, "p", 1) }));
    assert!(reason.contains("builds 2 of these elements"), "{reason}");
}

#[test]
fn document_apply_structure_is_one_undo_step() {
    let mut doc = Document::new(DocumentSession::new("doc.ptl".into(), LIST.into()), value_runner());
    let res = doc.apply_structure(&StructureIntent::Remove { target: id("c") }, &StructureOptions::default(), None);
    assert!(matches!(res, Resolution::Edit(_)), "{res:?}");
    assert!(!doc.session.source().contains("<c "));
    assert!(doc.session.undo());
    assert_eq!(doc.session.source(), LIST);
}

#[test]
fn duplicate_a_child_and_a_data_entry_with_ids() {
    let opts = StructureOptions { write_ids: true };
    let src = "[\n  <group id=\"g\">\n    <dot x={1} />\n  </group>,\n]\n";
    let e = edit(resolve_with(&value_runner(), src, &opts, |r| StructureIntent::Duplicate { target: key_of(r, "dot", 0) }));
    assert!(e.source.contains("    <dot x={1} />\n    <dot x={1} id=\"dot-1\" />\n  </group>"), "{}", e.source);
    // A data entry with an id gets a fresh one.
    let src = "let rows = [\n  {id: \"a\", t: 1},\n]\nrows.map(fn(r) <dot id={r.id} t={r.t} /> end)\n";
    let e = edit(resolve_with(&value_runner(), src, &opts, |_| StructureIntent::Duplicate { target: id("a") }));
    assert!(e.source.contains("  {id: \"a\", t: 1},\n  {id: \"a-1\", t: 1},\n]"), "{}", e.source);
    let r = run(&value_runner(), &e.source);
    assert!(r.node(&id("a-1")).is_some());
}

#[test]
fn insert_source_text_among_children_and_into_a_flattened_list() {
    let text = "<g2>\n  <x />\n</g2>";
    let e = edit(resolve_src(&value_runner(), GROUPS, |_| StructureIntent::Insert {
        at: Place { parent: kids("g2"), index: 0 },
        template: Template::Source(text.into()),
    }));
    assert!(e.source.contains("<group id=\"g2\">\n    <g2>\n      <x />\n    </g2>\n    <c id=\"c\" />"), "{}", e.source);
    // A data list mapped inside a list literal: the host sees one flat list.
    let src = "let rows = [1, 2]\n[\n  <head />,\n  rows.map(fn(n) <dot n={n} /> end),\n]\n";
    let r = run(&value_runner(), src);
    assert_eq!(parent_of(&r, &key_of(&r, "dot", 1)), Some((root(), 2)));
    let e = edit(resolve_src(&value_runner(), src, |_| StructureIntent::Insert {
        at: Place { parent: root(), index: 3 },
        template: Template::Value(StaticValue::Int(3)),
    }));
    assert!(e.source.starts_with("let rows = [1, 2, 3]\n"), "{}", e.source);
    let e = edit(resolve_src(&value_runner(), src, |r| StructureIntent::Reorder {
        parent: root(),
        order: vec![key_of(r, "head", 0), key_of(r, "dot", 1), key_of(r, "dot", 0)],
    }));
    assert!(e.source.starts_with("let rows = [2, 1]\n"), "{}", e.source);
}

#[test]
fn duplicate_keeps_a_multi_line_entry_as_written() {
    let src = "[\n  # first\n  <a x={1}\n    y={2} />,\n  <b />,\n]\n";
    let e = edit(resolve_src(&value_runner(), src, |r| StructureIntent::Duplicate { target: key_of(r, "a", 0) }));
    assert_eq!(e.source, "[\n  # first\n  <a x={1}\n    y={2} />,\n  <a x={1}\n    y={2} />,\n  <b />,\n]\n");
}


#[test]
fn text_between_children_goes_with_the_element_before_it() {
    // In JSX, `// note` after a tag is text, written after that entry: an
    // insert or a move in front of it leaves it after its element.
    let src = "<g>\n  <a />\n  <b /> // note\n  <c />\n</g>\n";
    let at = |r: &DocumentRun, index| Place { parent: Parent::Children(key_of(r, "g", 0)), index };
    let e = edit(resolve_src(&value_runner(), src, |r| StructureIntent::Insert {
        at: at(r, 1),
        template: Template::Source("<n />".into()),
    }));
    assert_eq!(e.source, "<g>\n  <a />\n  <n />\n  <b /> // note\n  <c />\n</g>\n");
    let e = edit(resolve_src(&value_runner(), src, |r| StructureIntent::Move { target: key_of(r, "c", 0), to: at(r, 0) }));
    assert!(e.source.contains("<b /> // note"), "{}", e.source);
}
