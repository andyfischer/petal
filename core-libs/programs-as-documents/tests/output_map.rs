//! The output source map: sites, chains, fan-out, field provenance, keys.

mod common;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::{FieldProvenance, OutputKey, PathSeg};

fn literal(p: &FieldProvenance) -> &programs_as_documents::LiteralSource {
    match p {
        FieldProvenance::Literal(l) => l,
        other => panic!("expected a literal, got {other:?}"),
    }
}

#[test]
fn a_top_level_list_maps_each_element_to_its_literal() {
    let src = "let items = [
  <rect id=\"a\" x={10} />,
  <rect x={20} />,
]
items";
    let r = run(&value_runner(), src);
    let rects = tagged(&r, "rect");
    assert_eq!(rects.len(), 2);
    assert_eq!(rects[0].key, OutputKey::Id("a".into()));
    assert_eq!(rects[0].path, vec![PathSeg::Index(0), PathSeg::Index(0)]);
    let site = rects[1].site.as_ref().unwrap();
    assert_eq!(site.fan_out, 1);
    assert!(site.chain.is_empty());
    assert_eq!(site.span.unwrap().start.line, 3);
    assert_eq!(
        rects[1].key,
        OutputKey::Site { site: "/<rect>#1".into(), chain: vec![], occurrence: 0 }
    );
    let x = literal(&rects[1].field("x").unwrap().provenance);
    assert_eq!(x.constant, Some(StaticValue::Int(20)));
    assert_eq!(x.shared, 1);
    assert_eq!(x.span.start.line, 3);
}

#[test]
fn a_helper_called_twice_shares_its_site_and_body_literals() {
    let src = "import shapes
fn card(x)
  <card x={x} w={40} />
end
doc([card(1), card(2)])";
    let r = run(&emit_runner(), src);
    let cards = tagged(&r, "card");
    let (a, b) = (cards[0], cards[1]);
    assert_eq!(a.site.as_ref().unwrap().fan_out, 2);
    assert_eq!(a.site.as_ref().unwrap().chain_keys, vec!["/card()#0".to_string()]);
    assert_eq!(b.site.as_ref().unwrap().chain_keys, vec!["/card()#1".to_string()]);
    assert_ne!(a.key, b.key);
    // `x` comes from each call's own argument; `w` from the shared body.
    assert_eq!(literal(&a.field("x").unwrap().provenance).shared, 1);
    assert_eq!(literal(&a.field("w").unwrap().provenance).shared, 2);
}

#[test]
fn loop_elements_are_told_apart_by_occurrence_and_computed_fields_say_why() {
    let src = "let items = for i in range(3) do
  <dot x={i * 10} y={7} />
end
items";
    let r = run(&value_runner(), src);
    let dots = tagged(&r, "dot");
    assert_eq!(dots.len(), 3);
    let occ: Vec<usize> = dots
        .iter()
        .map(|d| match &d.key {
            OutputKey::Site { occurrence, .. } => *occurrence,
            k => panic!("{k:?}"),
        })
        .collect();
    assert_eq!(occ, [0, 1, 2]);
    assert_eq!(dots[0].site.as_ref().unwrap().fan_out, 3);
    match &dots[1].field("x").unwrap().provenance {
        FieldProvenance::Computed { reason, .. } => assert!(reason.contains("i * 10"), "{reason}"),
        p => panic!("{p:?}"),
    }
    assert_eq!(literal(&dots[1].field("y").unwrap().provenance).shared, 3);
}

#[test]
fn values_read_out_of_records_report_the_path_inside_the_literal() {
    let src = "let theme = {colors: {ink: \"#111\"}, pad: 4}
[<box fill={theme.colors.ink} style={{w: 2}} />]";
    let r = run(&value_runner(), src);
    let b = tagged(&r, "box")[0];
    let fill = literal(&b.field("fill").unwrap().provenance);
    let within = fill.within.as_ref().unwrap();
    assert_eq!(within.path, ".colors.ink");
    assert_eq!(within.span.unwrap().start.line, 1);
    let style = literal(&b.field("style").unwrap().provenance);
    assert_eq!(style.constant, None, "a record literal");
}

#[test]
fn children_and_nested_elements_are_nodes_with_paths() {
    let src = "[<group>
  <a />
  text
  <b id={7} />
</group>]";
    let r = run(&value_runner(), src);
    let b = tagged(&r, "b")[0];
    assert_eq!(b.key, OutputKey::Id("7".into()));
    assert_eq!(b.path.first(), Some(&PathSeg::Index(0)));
    assert!(matches!(b.path.last(), Some(PathSeg::Child(_))));
    assert_eq!(r.map.find(&OutputKey::Path(b.path.clone())).unwrap().tag, "b");
}

#[test]
fn an_element_built_in_a_package_has_its_site_in_another_file() {
    let r = run(&emit_runner(), "import shapes\ndoc([badge(\"hi\", 3)])");
    let b = tagged(&r, "badge")[0];
    let site = b.site.as_ref().unwrap();
    assert_ne!(site.file(), Some(petal::source_map::FileId(0)));
    // The argument is written in the document.
    assert_eq!(literal(&b.field("x").unwrap().provenance).file(), petal::source_map::FileId(0));
}
