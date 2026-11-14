//! Copy and paste: an element copied as its literal or the call that builds
//! it, or as its value when no code builds it alone; the free-variable
//! analysis of the copied text; pasting into the same or another document
//! with each reference copied as source, copied as a value or rebound; and
//! the reasons when a reference needs a choice.

mod common;

use std::collections::HashMap;

use common::*;
use petal::static_value::StaticValue;
use programs_as_documents::output_map::PathSeg;
use programs_as_documents::{
    ClipSource, Document, DocumentSession, OutputKey, Parent, PasteOptions, PasteRef, Place, RefKind, Resolution,
    ResolvedEdit, StructureOptions, copy, copy_text, paste, prepare_paste,
};

const SOURCE: &str = "let m = 96
let tok = {ink: :112233, size: 4}
let base = 10
let wide = base * 2

fn hero(t)
  <hero title={t} />
end

fn card(x)
  <card x={x} />
end

let rows = [1, 2]

[
  <rect id=\"r\" x={m} fill={tok.ink} />,
  hero(\"h\"),
  <wide id=\"w\" w={wide} />,
  card(5),
  rows.map(fn(n) <dot n={n} /> end),
]
";

const TARGET: &str = "[\n  <b id=\"b\" />,\n]\n";

fn id(s: &str) -> OutputKey {
    OutputKey::Id(s.into())
}

fn end_of_target() -> Place {
    Place { parent: Parent::List(vec![PathSeg::Index(0)]), index: 1 }
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

fn with_refs(refs: &[(&str, PasteRef)]) -> PasteOptions {
    PasteOptions {
        refs: refs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<HashMap<_, _>>(),
        structure: StructureOptions::default(),
    }
}

fn paste_into(target: &str, clip: &programs_as_documents::Clip, opts: &PasteOptions) -> Resolution {
    let runner = value_runner();
    let r = run(&runner, target);
    paste(&runner, &r, clip, end_of_target(), opts)
}

#[test]
fn copy_finds_the_names_the_text_reads() {
    let r = run(&value_runner(), SOURCE);
    let clip = copy(&r, &id("r")).unwrap();
    assert_eq!(clip.source, ClipSource::Literal);
    assert_eq!(clip.text, "<rect id=\"r\" x={m} fill={tok.ink} />");
    let m = clip.reference("m").unwrap();
    assert_eq!(m.kind, RefKind::Constant);
    assert_eq!(m.uses[0].value, Some(StaticValue::Int(96)));
    let tok = clip.reference("tok").unwrap();
    assert_eq!(tok.uses[0].path, ".ink");
    assert!(matches!(tok.uses[0].value, Some(StaticValue::Color { r: 0x11, g: 0x22, b: 0x33, .. })));
    assert_eq!(&clip.text[tok.uses[0].range.clone()], "tok.ink");
    // A call is copied as the call; its function is a reference.
    let hero = r.map.nodes.iter().find(|n| n.tag == "hero").unwrap().key.clone();
    let clip = copy(&r, &hero).unwrap();
    assert_eq!((clip.source, clip.text.as_str()), (ClipSource::Call, "hero(\"h\")"));
    assert_eq!(clip.reference("hero").unwrap().kind, RefKind::Function);
    // An element literal that reads its function's parameter is copied as
    // the call that gives it.
    let card = r.map.nodes.iter().find(|n| n.tag == "card").unwrap().key.clone();
    assert_eq!(copy(&r, &card).unwrap().text, "card(5)");
    // An element a loop built has no code of its own: its value is copied.
    let dot = r.map.nodes.iter().find(|n| n.tag == "dot").unwrap().key.clone();
    let clip = copy(&r, &dot).unwrap();
    assert_eq!((clip.source, clip.text.as_str()), (ClipSource::Value, "<dot n={1} />"));
    // A computed binding, and the bindings it reads.
    let clip = copy(&r, &id("w")).unwrap();
    assert_eq!(clip.reference("wide").unwrap().kind, RefKind::Computed);
    let names: Vec<&str> = clip.bindings.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["base", "wide"]);
}

#[test]
fn paste_copies_constants_as_values_by_default() {
    let r = run(&value_runner(), SOURCE);
    let clip = copy(&r, &id("r")).unwrap();
    let e = edit(paste_into(TARGET, &clip, &PasteOptions::default()));
    assert_eq!(e.source, "[\n  <b id=\"b\" />,\n  <rect id=\"r\" x={96} fill={:112233} />,\n]\n");
}

#[test]
fn paste_as_source_brings_the_binding() {
    let r = run(&value_runner(), SOURCE);
    let clip = copy(&r, &id("r")).unwrap();
    let e = edit(paste_into(TARGET, &clip, &with_refs(&[("m", PasteRef::CopyAsSource)])));
    assert_eq!(e.source, "let m = 96\n\n[\n  <b id=\"b\" />,\n  <rect id=\"r\" x={m} fill={:112233} />,\n]\n");
    // A function, and a computed binding with what it reads, in order.
    let hero = r.map.nodes.iter().find(|n| n.tag == "hero").unwrap().key.clone();
    let clip = copy(&r, &hero).unwrap();
    let e = edit(paste_into(TARGET, &clip, &with_refs(&[("hero", PasteRef::CopyAsSource)])));
    assert!(e.source.starts_with("fn hero(t)\n  <hero title={t} />\nend\n\n["), "{}", e.source);
    let clip = copy(&r, &id("w")).unwrap();
    let e = edit(paste_into(TARGET, &clip, &with_refs(&[("wide", PasteRef::CopyAsSource)])));
    assert!(e.source.starts_with("let base = 10\nlet wide = base * 2\n\n["), "{}", e.source);
}

#[test]
fn paste_rebinds_to_the_targets_names() {
    let r = run(&value_runner(), SOURCE);
    let clip = copy(&r, &id("r")).unwrap();
    let target = "let tok = {ink: :ffffff}\nlet ink2 = 3\n[\n  <b id=\"b\" />,\n]\n";
    let opts = with_refs(&[("tok", PasteRef::Rebind("tok".into()))]);
    let e = edit(paste_into(target, &clip, &opts));
    assert!(e.source.contains("<rect id=\"r\" x={96} fill={tok.ink} />"), "{}", e.source);
    // To another name.
    let opts = with_refs(&[("m", PasteRef::Rebind("ink2".into()))]);
    let e = edit(paste_into(target, &clip, &opts));
    assert!(e.source.contains("x={ink2}"), "{}", e.source);
    // The target must have it.
    let reason = refused(paste_into(TARGET, &clip, &with_refs(&[("m", PasteRef::Rebind("nope".into()))])));
    assert!(reason.contains("no top-level `nope`"), "{reason}");
}

#[test]
fn paste_refuses_a_reference_that_needs_a_choice() {
    let r = run(&value_runner(), SOURCE);
    let hero = r.map.nodes.iter().find(|n| n.tag == "hero").unwrap().key.clone();
    let clip = copy(&r, &hero).unwrap();
    let reason = refused(paste_into(TARGET, &clip, &PasteOptions::default()));
    assert!(reason.contains("`hero` is a function in the copied document"), "{reason}");
    assert!(reason.contains("CopyAsSource"), "{reason}");
    let clip = copy(&r, &id("w")).unwrap();
    let reason = refused(paste_into(TARGET, &clip, &PasteOptions::default()));
    assert!(reason.contains("`wide` is computed"), "{reason}");
    // Bringing a binding the target has under the same name, differently.
    let clip = copy(&r, &id("r")).unwrap();
    let reason = refused(paste_into("let m = 1\n[<b id=\"b\" />]\n", &clip, &with_refs(&[("m", PasteRef::CopyAsSource)])));
    assert!(reason.contains("already has a different `m`"), "{reason}");
    // Copying a computed reference as a value.
    let clip = copy(&r, &id("w")).unwrap();
    let reason = refused(paste_into(TARGET, &clip, &with_refs(&[("wide", PasteRef::CopyAsValue)])));
    assert!(reason.contains("has no static value"), "{reason}");
}

#[test]
fn paste_into_the_same_document_keeps_its_names_and_gets_a_fresh_id() {
    let mut doc = Document::new(DocumentSession::new("doc.ptl".into(), SOURCE.into()), value_runner());
    let clip = doc.copy(&id("r")).unwrap();
    let at = Place { parent: Parent::List(vec![PathSeg::Index(0)]), index: 1 };
    let res = doc.paste(&clip, at, &PasteOptions::default());
    assert!(matches!(res, Resolution::Edit(_)), "{res:?}");
    assert!(
        doc.session.source().contains("  <rect id=\"r\" x={m} fill={tok.ink} />,\n  <rect id=\"r-1\" x={m} fill={tok.ink} />,\n"),
        "{}",
        doc.session.source()
    );
    assert!(doc.session.undo());
    assert_eq!(doc.session.source(), SOURCE);
}

#[test]
fn copy_text_and_prepare_paste() {
    let clip = copy_text(SOURCE, "<group>\n  <rect x={m} />\n  {hero(\"x\")}\n</group>").unwrap();
    let p = prepare_paste(TARGET, &clip, &with_refs(&[("hero", PasteRef::CopyAsSource)])).unwrap();
    assert_eq!(p.text, "<group>\n  <rect x={96} />\n  {hero(\"x\")}\n</group>");
    assert_eq!(p.preamble.len(), 1);
    // A lambda's own parameter is not a reference.
    let clip = copy_text(SOURCE, "rows.map(fn(m) <dot n={m} /> end)").unwrap();
    assert_eq!(clip.refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["rows"]);
    // Multi-line text is indented to where it lands.
    let clip = copy_text(SOURCE, "<group>\n  <rect x={m} />\n</group>").unwrap();
    let e = edit(paste_into(TARGET, &clip, &PasteOptions::default()));
    assert_eq!(e.source, "[\n  <b id=\"b\" />,\n  <group>\n    <rect x={96} />\n  </group>,\n]\n");
}
