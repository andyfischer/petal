//! The example in docs/features/programs-as-documents.md, checked: each row
//! of its intent table resolves the way the page says.

mod common;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::output_map::PathSeg;
use programs_as_documents::{
    Intent, OutputKey, Parent, PasteOptions, PasteRef, Resolution, ResolveOptions, Scope, StructureIntent, StructureOptions, copy, prepare_paste, resolve,
    resolve_structure,
};

const DOC: &str = r#"let gutter = 24

fn card(row)
  <card id={row.id} x={row.x} y={300} title={row.title} />
end

let rows = [
  {id: "plan", x: 40, title: "Plan"},
  {id: "build", x: 260, title: "Build"},
]

[
  <logo id="logo" x={gutter} y={gutter} />,
  rows.map(card),
]
"#;

fn id(s: &str) -> OutputKey {
    OutputKey::Id(s.into())
}

fn set(target: &str, field: &str, value: StaticValue) -> Intent {
    Intent::SetField { target: id(target), field: field.into(), value }
}

#[test]
fn a_title_read_through_map_is_written_into_its_data_entry() {
    let runner = value_runner();
    let r = run(&runner, DOC);
    let res = resolve(&runner, &r, &set("build", "title", StaticValue::Str("Ship".into())), &ResolveOptions::default());
    let Resolution::Edit(e) = res else { panic!("expected an edit, got {res:?}") };
    assert!(e.source.contains(r#"{id: "build", x: 260, title: "Ship"},"#), "{}", e.source);
    assert!(e.source.contains(r#"{id: "plan", x: 40, title: "Plan"},"#), "{}", e.source);
    assert!(e.also_changes.is_empty(), "{:?}", e.also_changes);
}

#[test]
fn a_literal_shared_by_both_cards_asks_first() {
    let runner = value_runner();
    let r = run(&runner, DOC);
    let res = resolve(&runner, &r, &set("plan", "y", StaticValue::Int(320)), &ResolveOptions::default());
    let Resolution::NeedsChoice { options, .. } = res else { panic!("expected a choice, got {res:?}") };
    let shared = options.iter().find(|c| c.scope == Scope::EditShared).expect("an edit-all choice");
    let edit = shared.edit.as_ref().expect("the edit-all choice verifies");
    assert!(edit.source.contains("y={320}"), "{}", edit.source);
}

#[test]
fn a_constant_that_feeds_two_fields_asks_first() {
    let runner = value_runner();
    let r = run(&runner, DOC);
    let res = resolve(&runner, &r, &set("logo", "x", StaticValue::Int(30)), &ResolveOptions::default());
    let Resolution::NeedsChoice { options, .. } = res else { panic!("expected a choice, got {res:?}") };
    let edit = options.iter().find_map(|c| c.edit.as_ref()).expect("a verified choice");
    assert!(edit.source.contains("let gutter = 30"), "{}", edit.source);
}

#[test]
fn reordering_the_cards_reorders_the_data_list() {
    let runner = value_runner();
    let r = run(&runner, DOC);
    let order = vec![id("logo"), id("build"), id("plan")];
    let intent = StructureIntent::Reorder { parent: Parent::List(vec![PathSeg::Index(0)]), order };
    let res = resolve_structure(&runner, &r, &intent, &StructureOptions::default());
    let Resolution::Edit(e) = res else { panic!("expected an edit, got {res:?}") };
    let build = e.source.find(r#"id: "build""#).unwrap();
    let plan = e.source.find(r#"id: "plan""#).unwrap();
    assert!(build < plan, "{}", e.source);
    assert!(e.source.contains("rows.map(card)"), "{}", e.source);
}

#[test]
fn copying_the_logo_lists_the_constant_it_reads() {
    let runner = value_runner();
    let r = run(&runner, DOC);
    let clip = copy(&r, &id("logo")).expect("copy");
    assert_eq!(clip.text, r#"<logo id="logo" x={gutter} y={gutter} />"#);
    assert!(clip.reference("gutter").is_some(), "{:?}", clip.refs);
}

#[test]
fn pasting_the_logo_as_values_inlines_the_constant() {
    let runner = value_runner();
    let r = run(&runner, DOC);
    let clip = copy(&r, &id("logo")).expect("copy");
    let mut opts = PasteOptions::default();
    opts.refs.insert("gutter".into(), PasteRef::CopyAsValue);
    let prepared = prepare_paste("[\n  <title />,\n]\n", &clip, &opts).expect("prepare");
    assert_eq!(prepared.text, r#"<logo id="logo" x={24} y={24} />"#);
    assert!(prepared.preamble.is_empty(), "{:?}", prepared.preamble);
}
