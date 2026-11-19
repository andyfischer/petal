//! `{…}` holes among an element's children as structural entries: a
//! helper-made element moves into a group and out again, reorders and is
//! removed among the group's children, and a hole that builds several
//! elements is refused with the reason. And the comment that trails an
//! entry moves with it into a group's children.

mod common;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::output_map::PathSeg;
use programs_as_documents::{
    DocumentRun, EntrySource, OutputKey, Parent, Place, Resolution, ResolvedEdit, Runner, StructureIntent,
    StructureOptions, Template, resolve_structure, sibling_entries,
};

fn resolve_src(runner: &Runner, src: &str, intent: impl Fn(&DocumentRun) -> StructureIntent) -> Resolution {
    let r = run(runner, src);
    let i = intent(&r);
    resolve_structure(runner, &r, &i, &StructureOptions::default())
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

fn kids(s: &str) -> Parent {
    Parent::Children(id(s))
}

fn root() -> Parent {
    Parent::List(vec![PathSeg::Index(0)])
}

const DOC: &str = "fn badge(l)
  <badge id={l} />
end

[
  badge(\"x\"), # the x badge
  <group id=\"g\">
    {badge(\"y\")}
    <a id=\"a\" />
  </group>,
]
";

#[test]
fn a_hole_among_children_is_an_entry() {
    let r = run(&value_runner(), DOC);
    let es = sibling_entries(&r, &kids("g")).unwrap();
    assert_eq!(es.len(), 2);
    assert!(matches!(&es[0].source, EntrySource::Entry { container, entry: 0, .. } if container.holes == vec![true, false]));
}

#[test]
fn a_helper_made_element_reorders_and_is_removed_among_children() {
    let e = edit(resolve_src(&value_runner(), DOC, |_| StructureIntent::Reorder {
        parent: kids("g"),
        order: vec![id("a"), id("y")],
    }));
    assert!(e.source.contains("  <group id=\"g\">\n    <a id=\"a\" />\n    {badge(\"y\")}\n  </group>,"), "{}", e.source);
    let e = edit(resolve_src(&value_runner(), DOC, |_| StructureIntent::Remove { target: id("y") }));
    assert!(e.source.contains("  <group id=\"g\">\n    <a id=\"a\" />\n  </group>,"), "{}", e.source);
}

#[test]
fn a_helper_made_element_moves_into_a_group_and_out_again() {
    let e = edit(resolve_src(&value_runner(), DOC, |_| StructureIntent::Move {
        target: id("x"),
        to: Place { parent: kids("g"), index: 2 },
    }));
    assert!(
        e.source.contains("  <group id=\"g\">\n    {badge(\"y\")}\n    <a id=\"a\" />\n    {/* the x badge */}\n    {badge(\"x\")}\n  </group>,"),
        "{}",
        e.source
    );
    let e = edit(resolve_src(&value_runner(), DOC, |_| StructureIntent::Move {
        target: id("y"),
        to: Place { parent: root(), index: 0 },
    }));
    assert!(e.source.contains("[\n  badge(\"y\"),\n  badge(\"x\"), # the x badge\n  <group id=\"g\">\n    <a id=\"a\" />\n  </group>,\n]"), "{}", e.source);
}

#[test]
fn a_call_is_inserted_among_children_as_a_hole() {
    let e = edit(resolve_src(&value_runner(), DOC, |_| StructureIntent::Insert {
        at: Place { parent: kids("g"), index: 1 },
        template: Template::Value(StaticValue::call("badge", ["z"])),
    }));
    assert!(e.source.contains("    {badge(\"y\")}\n    {badge(\"z\")}\n    <a id=\"a\" />\n"), "{}", e.source);
    // Something that builds no element is refused when run.
    let why = refused(resolve_src(&value_runner(), DOC, |_| StructureIntent::Insert {
        at: Place { parent: kids("g"), index: 0 },
        template: Template::Source("len(\"q\")".into()),
    }));
    assert!(why.contains("does not do what was asked"), "{why}");
}

#[test]
fn a_hole_that_builds_several_elements_is_refused_with_the_reason() {
    let src = "fn pair()\n  let e = <p />\n  [e, e]\nend\n[<group id=\"g\">\n  {pair()}\n  <a id=\"a\" />\n</group>]\n";
    let why = refused(resolve_src(&value_runner(), src, |r| StructureIntent::Move {
        target: key_of(r, "p", 0),
        to: Place { parent: kids("g"), index: 2 },
    }));
    assert!(why.contains("`{pair()}` (line 6), which builds 2"), "{why}");
    // (Elements a hole builds as entries of a list literal in the document
    // are that list's entries, and move as such.)
    let r = run(&value_runner(), src);
    let es = sibling_entries(&r, &kids("g")).unwrap();
    assert!(matches!(es[0].source, EntrySource::Locked { .. }));
    assert!(es[2].source.is_movable());
}

#[test]
fn the_comment_trailing_a_list_entry_moves_into_a_groups_children() {
    let src = "[\n  <x id=\"x\" />, # keep me\n  <group id=\"g\">\n    <a id=\"a\" />\n  </group>,\n]\n";
    let e = edit(resolve_src(&value_runner(), src, |_| StructureIntent::Move {
        target: id("x"),
        to: Place { parent: kids("g"), index: 0 },
    }));
    assert_eq!(e.source, "[\n  <group id=\"g\">\n    {/* keep me */}\n    <x id=\"x\" />\n    <a id=\"a\" />\n  </group>,\n]\n");
    // And back out into the list, as a comment line above it.
    let e = edit(resolve_src(&value_runner(), &e.source, |_| StructureIntent::Move {
        target: id("x"),
        to: Place { parent: root(), index: 0 },
    }));
    assert!(e.source.contains("keep me"), "{}", e.source);
}
