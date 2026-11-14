//! `Intent::SetText`: an element's text, written as literal text between its
//! tags, added to an element with none, in the string a `{param}` hole
//! reads, or in its data entry; and the reasons when it has no one place.

mod common;

use common::*;
use programs_as_documents::{
    DocumentRun, Intent, OutputKey, Resolution, ResolveOptions, ResolvedEdit, Runner, Scope, Step, resolve,
};

fn text(target: OutputKey, t: &str) -> Intent {
    Intent::SetText { target, text: t.into() }
}

fn resolve_src(runner: &Runner, src: &str, intent: impl Fn(&DocumentRun) -> Intent) -> Resolution {
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

fn reason(res: Resolution) -> String {
    match res {
        Resolution::Refused { reason } | Resolution::Unresolved { reason, .. } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn literal_text_is_replaced_between_the_tags() {
    let src = "[\n  <text id=\"t\" size={f(2)}>Hello</text>, # greeting\n]\nfn f(x)\n  x * 2\nend\n";
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Hello, world")));
    assert_eq!(e.step, Step::DirectLiteral);
    assert_eq!(e.source, src.replace(">Hello<", ">Hello, world<"));
    // Text that would not read back as bare text is a quoted hole.
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "a {b} <c>")));
    assert!(e.source.contains("<text id=\"t\" size={f(2)}>{\"a \\{b} <c>\"}</text>, # greeting"), "{}", e.source);
    assert_eq!(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Hello")), Resolution::AlreadyHolds);
}

#[test]
fn an_element_with_no_text_gets_some() {
    let src = "[<text id=\"t\" />]\n";
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Hi")));
    assert_eq!(e.step, Step::SingleSite);
    assert_eq!(e.source, "[<text id=\"t\">Hi</text>]\n");
}

#[test]
fn a_param_hole_is_traced_to_the_string_it_reads() {
    let src = "fn hint(s)\n  <text>{s}</text>\nend\n[hint(\"Swipe\"), hint(\"Tap\")]\n";
    let e = edit(resolve_src(&value_runner(), src, |r| text(key_of(r, "text", 1), "Tap twice")));
    assert_eq!(e.step, Step::DirectLiteral);
    assert_eq!(e.source, "fn hint(s)\n  <text>{s}</text>\nend\n[hint(\"Swipe\"), hint(\"Tap twice\")]\n");
    assert!(e.also_changes.is_empty());
}

#[test]
fn a_shared_text_literal_asks() {
    let src = "fn label()\n  <text>Same</text>\nend\n[label(), label()]\n";
    match resolve_src(&value_runner(), src, |r| text(key_of(r, "text", 0), "Other")) {
        Resolution::NeedsChoice { options, .. } => {
            assert_eq!(options[0].scope, Scope::EditShared);
            assert!(options[0].edit.as_ref().unwrap().source.contains("<text>Other</text>"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn text_read_from_a_data_list_is_edited_in_its_entry() {
    let src = "fn row(r)\n  <text id={r.id}>{r.title}</text>\nend\nlet rows = [\n  {id: \"a\", title: \"Plan\"},\n  {id: \"b\", title: \"Build\"},\n]\nrows.map(row)\n";
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("b".into()), "Ship")));
    assert_eq!(e.step, Step::Scope(Scope::EditData));
    assert!(e.source.contains("{id: \"b\", title: \"Ship\"},"), "{}", e.source);
}

#[test]
fn text_with_no_one_place_is_refused_with_the_reason() {
    let src = "let n = 3\n[<text id=\"t\">{\"{n} items\"}</text>]\n";
    let why = reason(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "4 items")));
    assert!(why.contains("computed") && why.contains("line 2"), "{why}");
    let src = "[<text id=\"t\">Hi<br />there</text>]\n";
    let why = reason(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Bye")));
    assert!(why.contains("child elements"), "{why}");
}
