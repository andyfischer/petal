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
    let src = "[<text id=\"t\">Hi <span>there</span></text>]\n";
    let why = reason(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Bye")));
    assert!(why.contains("child elements"), "{why}");
}


// petal-movies' text and captions items: lines of text with `<br />`
// between them.

#[test]
fn line_breaks_are_part_of_the_text() {
    let src = "[<text id=\"t\">Hi<br />there</text>]\n";
    let r = run(&value_runner(), src);
    let n = r.map.find(&OutputKey::Id("t".into())).unwrap();
    assert_eq!(programs_as_documents::text_of(&r.observed_node(n)).as_deref(), Some("Hi\nthere"));
    assert_eq!(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Hi\nthere")), Resolution::AlreadyHolds);
    // One line replaces the lines, and the `<br />` goes.
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Bye")));
    assert_eq!(e.step, Step::DirectLiteral);
    assert_eq!(e.source, "[<text id=\"t\">Bye</text>]\n");
    assert!(e.also_changes.is_empty(), "{:?}", e.also_changes);
}

#[test]
fn a_line_break_in_the_new_text_writes_a_br() {
    // On one line, the text stays on it.
    let src = "[<text id=\"t\" size={12}>Hello</text>]\n";
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "One\ntwo\n\nthree")));
    assert_eq!(e.source, "[<text id=\"t\" size={12}>One<br />two<br /><br />three</text>]\n");
    assert!(e.description.contains("One\\ntwo"), "{}", e.description);
    // An element with no text yet.
    let src = "[<text id=\"t\" />]\n";
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "A\nB")));
    assert_eq!(e.step, Step::SingleSite);
    let r = run(&value_runner(), &e.source);
    let n = r.map.find(&OutputKey::Id("t".into())).unwrap();
    assert_eq!(programs_as_documents::text_of(&r.observed_node(n)).as_deref(), Some("A\nB"));
}

#[test]
fn lines_written_one_per_line_keep_that_layout() {
    // petal-movies' title-card.ptl.
    let src = "[\n  <text\n    id=\"t\"\n    size={72}\n  >\n    Every movie<br />\n    is a Petal script.<br />\n    <br />\n    Write it as JSX.\n  </text>,\n]\n";
    let e = edit(resolve_src(&value_runner(), src, |_| {
        text(OutputKey::Id("t".into()), "Every movie\nis a Petal script.\n\nWrite it in Petal.")
    }));
    assert_eq!(e.source, src.replace("Write it as JSX.", "Write it in Petal."));
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Every movie\nis a script.")));
    assert_eq!(
        e.source,
        "[\n  <text\n    id=\"t\"\n    size={72}\n  >\n    Every movie<br />\n    is a script.\n  </text>,\n]\n"
    );
    assert!(e.also_changes.is_empty(), "{:?}", e.also_changes);
}

#[test]
fn a_line_break_through_a_hole_is_written_in_the_string() {
    // The string the hole reads gets a `\n`, which the element's text
    // reads back as the line break.
    let src = "fn hint(s)\n  <text>{s}</text>\nend\n[hint(\"Swipe\")]\n";
    let e = edit(resolve_src(&value_runner(), src, |r| text(key_of(r, "text", 0), "Swipe\nleft")));
    assert_eq!(e.step, Step::DirectLiteral);
    assert_eq!(e.source, src.replace("\"Swipe\"", "\"Swipe\\nleft\""));
    assert!(e.also_changes.is_empty(), "{:?}", e.also_changes);
}

#[test]
fn a_hole_beside_lines_is_still_refused() {
    let src = "let n = 3\n[<text id=\"t\">{n}<br />items</text>]\n";
    let why = reason(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "4\nitems")));
    assert!(why.contains("mixes text"), "{why}");
}

#[test]
fn text_written_with_string_holes_among_lines_is_text() {
    // petal-movies: a line with `{` or `<` in it is written as a `{"…"}`
    // string; the next edit reads all of it as the text's own.
    let src = "[\n  <text\n    id=\"t\"\n  >\n    One<br />\n    {\"Two \\{and} <three>\"}\n  </text>,\n]\n";
    let e = edit(resolve_src(&value_runner(), src, |_| text(OutputKey::Id("t".into()), "Short")));
    assert!(e.source.contains("  >Short</text>,"), "{}", e.source);
}

/// A package whose `<text>` has a child element: its literal is in the
/// package, so editing its text is refused as another file's, not left
/// Unresolved with a span and line number in the package.
#[test]
fn a_package_literal_with_child_elements_is_refused_as_another_files() {
    const PKG: &str = "pub fn kids(t)\n  <text>{t}<b /></text>\nend\n";
    let runner = Runner::new("doc.ptl", |env: &mut petal::env::Env| {
        setup(env)?;
        let files = [("pkg", PKG), (petal::package::MANIFEST_FILE, "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n")];
        env.register_package("pkg", files).map_err(|e| e.to_string())?;
        env.provides_package("pkg")
    });
    let src = "import pkg\n[kids(\"Hi\")]\n";
    match resolve_src(&runner, src, |r| text(key_of(r, "text", 0), "Yo")) {
        Resolution::Refused { reason } => assert!(reason.contains("pkg"), "{reason}"),
        other => panic!("expected Refused, got {other:?}"),
    }
    // With other files allowed, it is left to the host (Unresolved), still
    // naming the package and with no span that could be read as the
    // document's.
    let r = run(&runner, src);
    let opts = ResolveOptions { allow_other_files: true, ..ResolveOptions::default() };
    match resolve(&runner, &r, &text(key_of(&r, "text", 0), "Yo"), &opts) {
        Resolution::Unresolved { reason, at } => {
            assert!(reason.contains("pkg"), "{reason}");
            assert!(at.is_none_or(|s| s.file != petal::source_map::FileId(0)), "{at:?}");
        }
        other => panic!("expected Unresolved, got {other:?}"),
    }
}
