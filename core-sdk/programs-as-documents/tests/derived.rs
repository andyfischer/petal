//! Derived result values (`Runner::with_derived`): an index the host's
//! vocabulary makes from the document's elements, keyed by where each one
//! is (petal-design's instance expansions, keyed by each instance's path).
//! Their elements are in the output map, but an edit's rerun does not
//! compare them; and the indentation a lifted literal lays new lines out
//! with is the document's step.

mod common;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::output_map::PathSeg;
use programs_as_documents::{
    Intent, OutputKey, OutputSource, Parent, Place, Resolution, ResolveOptions, Runner, StructureIntent,
    StructureOptions, Template, resolve, resolve_structure,
};

/// A document whose second result value lists where each `<card>` is: the
/// value is the list, then the index.
const INDEXED: &str = r#"native fn doc_emit(items, index)
let items = [
  <card id="a" w={1} />,
  <box id="b">
    <card id="c" w={2} />
  </box>,
]
fn index(xs)
  var out = []
  for i in range(len(xs)) do
    if xs[i].tag == "card" then
      set out = append(out, {at: i, el: <seen of={xs[i].props.id} w={xs[i].props.w} />})
    end
  end
  out
end
doc_emit(items, index(items))
"#;

fn runner(derived: bool) -> Runner {
    let r = Runner::new("doc.ptl", setup).with_output(OutputSource::Buffer { symbol: "doc_emit".into(), last: Some(2) });
    if derived { r.with_derived(vec![1]) } else { r }
}

#[test]
fn a_derived_value_is_mapped_but_not_compared() {
    let r = runner(true);
    let before = run(&r, INDEXED);
    assert_eq!(r.derived(), [1]);
    // Its elements are outputs like any other.
    assert_eq!(tagged(&before, "seen").len(), 1);
    assert!(before.is_derived(&tagged(&before, "seen")[0].path));
    // Moving the box in front of the card shifts the card's index: a
    // change in the derived value, which the rerun doesn't count.
    let intent = StructureIntent::Move { target: OutputKey::Id("b".into()), to: Place { parent: Parent::List(vec![PathSeg::Index(0)]), index: 0 } };
    let res = resolve_structure(&r, &before, &intent, &StructureOptions::default());
    let Resolution::Edit(e) = res else { panic!("{res:?}") };
    assert!(e.source.contains("let items = [\n  <box id=\"b\">"), "{}", e.source);
    // Without it, the same edit is refused: the index changed.
    let plain = runner(false);
    let res = resolve_structure(&plain, &run(&plain, INDEXED), &intent, &StructureOptions::default());
    assert!(matches!(res, Resolution::Refused { .. }), "{res:?}");
}

#[test]
fn a_derived_value_does_not_make_an_edit_a_choice() {
    // The index copies each card's `w` into its <seen>: setting it
    // changes the derived <seen> too, which is not another output the edit
    // changes.
    let r = runner(true);
    let before = run(&r, INDEXED);
    let intent = Intent::SetField { target: OutputKey::Id("a".into()), field: "w".into(), value: StaticValue::Int(5) };
    let res = resolve(&r, &before, &intent, &ResolveOptions::default());
    assert!(matches!(res, Resolution::Edit(_)), "{res:?}");
    // Without it, the <seen> counts: the edit changes another output.
    let plain = runner(false);
    let res = resolve(&plain, &run(&plain, INDEXED), &intent, &ResolveOptions::default());
    assert!(matches!(res, Resolution::NeedsChoice { .. }), "{res:?}");
}

#[test]
fn a_first_child_is_indented_by_the_documents_step() {
    // An empty element deep in a list: its new first child goes one step
    // (two spaces, as the document indents) in, not one column-width.
    let src = "import shapes\nlet items = [\n  <box id=\"b\">\n    <box id=\"inner\" />\n  </box>,\n]\ndoc(items)\n";
    let r = emit_runner();
    let before = run(&r, src);
    let at = Place { parent: Parent::Children(OutputKey::Id("inner".into())), index: 0 };
    let intent = StructureIntent::Insert { at, template: Template::Source("<dot />".into()) };
    let Resolution::Edit(e) = resolve_structure(&r, &before, &intent, &StructureOptions::default()) else { panic!() };
    assert!(e.source.contains("    <box id=\"inner\">\n      <dot />\n    </box>\n"), "{}", e.source);
    assert_eq!(programs_as_documents::splice::indent_step(src), "  ");
    assert_eq!(programs_as_documents::splice::indent_step("a\n\tb\n\t\tc\n"), "\t");
}
