//! `#` line comments (docs/tasks/pythonic-syntax.md §3.3, §3.4), at the
//! language level and through the tools that read or move comments. The
//! lexer's own cases are in `core/src/lexer.rs`, the migration's in
//! `migrate_old_code.rs`; `comments.rs` covers `/* … */` and the deprecated
//! `//`.

use petal::env::Env;
use petal::lint::{LintOptions, lint_source};
use petal::literal_edit;
use petal::static_value::{StaticValue, get_static_value};
use petal::value::value_to_display_string;

fn show(src: &str) -> String {
    let mut env = Env::new();
    let v = env
        .run_source(src)
        .unwrap_or_else(|e| panic!("{src:?} failed: {e}"));
    value_to_display_string(&v, env.heap())
}

fn error(src: &str) -> String {
    Env::new()
        .run_source(src)
        .err()
        .unwrap_or_else(|| panic!("{src:?} should fail"))
        .to_string()
}

fn lint(src: &str) -> String {
    lint_source(src, &LintOptions::default()).unwrap().output
}

#[test]
fn hash_comments_are_ignored_whole_line_and_trailing() {
    assert_eq!(show("# header\nlet x = 1 # one\n#\n#x = 2\nx + 1 # two"), "2");
    assert_eq!(show("#!/usr/bin/env petal run\nlet x = [1, # first\n  2] # done\nx"), "[1, 2]");
    // Inside a block, a call's arguments, a record and a match.
    assert_eq!(
        show("fn f(a, # the first\n  b)\n  # the body\n  if a > b then # bigger\n    a\n  else\n    # not\n    b\n  end\nend\nf(1, 2)"),
        "2"
    );
    assert_eq!(show("let r = {\n  a: 1, # one\n  # two\n  b: 2,\n}\nr.a + r.b"), "3");
    assert_eq!(show("match 2\n  # which\n  when 1 -> \"a\" # one\n  when _ -> \"b\"\nend"), "b");
}

#[test]
fn a_comment_does_not_continue_or_end_an_expression() {
    // A trailing operator continues onto the next line past a comment.
    assert_eq!(show("let x = 1 + # more\n  2\nx"), "3");
    // And a comment is not an operand.
    assert!(error("let x = 1 + # nothing\n").contains("Unexpected"), "{}", error("let x = 1 + # nothing\n"));
}

#[test]
fn a_hash_in_a_string_or_a_raw_string_is_text() {
    assert_eq!(show(r##""a # b // c""##), "a # b // c");
    assert_eq!(show("\"#ff8800\""), "#ff8800");
    assert_eq!(show("let n = 3\n\"{n} # of items\""), "3 # of items");
    assert_eq!(show("\"\"\"\n#version 300 es\n#define K 1\n\"\"\".length"), "29");
}

#[test]
fn hash_then_a_color_shaped_word_is_an_error_everywhere_code_is_read() {
    for src in ["let c = #fff", "let c = nil ?? #fff\nc", "f(#ff8800)", "let r = <rect fill=#f80 />", "\"{#fff}\""] {
        let e = error(src);
        assert!(e.contains("is no longer a color literal"), "{src}: {e}");
        assert!(e.contains("petal migrate-old-code"), "{src}: {e}");
    }
    // With a space, or when the word goes on, it is a comment.
    assert_eq!(show("let c = 1 # fff\nc"), "1");
    assert_eq!(show("let c = 1 #fade_in the title\nc"), "1");
    // `fade` is four hex digits: color-shaped, so not a comment.
    assert!(error("let c = 1 #fade the title in\nc").contains("write `:fade` (or `# fade` for a comment)"));
    assert_eq!(show("let c = 1 #12345 is five digits\nc"), "1");
}

#[test]
fn in_element_children_a_hash_line_is_text_and_a_hole_is_the_comment() {
    assert_eq!(show("<p>\n  # of items\n</p>"), "<p># of items</p>");
    assert_eq!(show("<p>#fff</p>"), "<p>#fff</p>");
    assert_eq!(show("<g>\n  {/* the photo */}\n  <image src=\"a.png\" />\n</g>"), r#"<g><image src="a.png" /></g>"#);
    // In the tag and in a hole, where code is read, `#` is a comment.
    assert_eq!(show("<rect # the box\n  w={1} # wide\n/>"), "<rect w={1} />");
    assert_eq!(show("(<g>{\n  # pick\n  1 + 1\n}</g>).children[0]"), "2");
    // The deprecated `//` line still reads as a comment.
    assert_eq!(show("<g>\n  // the photo\n  <a />\n</g>"), "<g><a /></g>");
}

#[test]
fn lint_ignore_directives_work_with_either_marker() {
    for marker in ["#", "//"] {
        let src = format!("fn f()\n  {marker} petal-lint-ignore prefer-let\n  var n = 0\n  n\nend\n");
        assert!(lint(&src).contains("var n = 0"), "{marker}");
        let src = format!("fn f()\n  var n = 0 {marker} petal-lint-ignore prefer-let -- shared\n  n\nend\n");
        assert!(lint(&src).contains("var n = 0"), "{marker}");
        let src = format!("{marker} petal-lint-ignore-file prefer-let\nfn f()\n  var n = 0\n  n\nend\n");
        assert!(lint(&src).contains("var n = 0"), "{marker}");
    }
    assert!(lint("fn f()\n  # a note about n\n  var n = 0\n  n\nend\n").contains("let n = 0"));
}

/// Lint fixes that re-render a span refuse one with a comment inside, and
/// carry a comment outside it across: the same with `#` as with `//`.
#[test]
fn lint_fixes_keep_hash_comments() {
    assert_eq!(
        lint("let n = 5\nlet m = int(n) # keep me\nprint(m)\n"),
        "let n = 5\nlet m = n # keep me\nprint(m)\n"
    );
    let src = "let n = 5\nlet m = int(n # why\n)\nprint(m)\n";
    assert_eq!(lint(src), src);
    let src = "fn f(c)\n  if c == \"a\" then 1 # first\n  elsif c == \"b\" then 2\n  elsif c == \"c\" then 3\n  else 4\n  end\nend\n";
    assert_eq!(lint(src), src);
    let out = lint("fn f(c)\n  if c == \"a\" then 1\n  elsif c == \"b\" then 2\n  # fall through to c\n  elsif c == \"c\" then 3\n  else 4\n  end\nend\n");
    assert!(out.contains("elsif") && out.contains("# fall through to c"), "{out}");
    // A `#` in a string is not a comment and blocks nothing.
    assert_eq!(
        lint("let b = \"[\"\nfor i in range(3) do b ++= \"#\" end\nprint(b)\n"),
        "let b = \"[\"\nb ++= repeat(\"#\", 3)\nprint(b)\n"
    );
    let src = "let b = \"[\"\nfor i in range(3) do b ++= \".\" # dots\nend\nprint(b)\n";
    assert_eq!(lint(src), src);
}

#[test]
fn literal_edits_keep_and_move_hash_comments() {
    let src = "# the steps\nlet l = [\n  \"a\", # first\n  # about b\n  \"b\",\n  \"c\",\n]\n";
    // A removed element takes its trailing comment; the rest stay.
    assert_eq!(
        literal_edit::remove_path(src, "l[0]").unwrap(),
        "# the steps\nlet l = [\n  # about b\n  \"b\",\n  \"c\",\n]\n"
    );
    // A moved element takes its trailing comment along.
    assert_eq!(
        literal_edit::move_path(src, "l[0]", "l[2]").unwrap(),
        "# the steps\nlet l = [\n  # about b\n  \"b\",\n  \"c\",\n  \"a\", # first\n]\n"
    );
    // A whole-value set that swaps elements moves the comment lines above
    // each with it.
    let src = "let l = [\n  # A\n  {a: 1},\n  # B\n  {b: 2},\n]\n";
    let StaticValue::List(mut items) = get_static_value(src, "l").unwrap() else {
        panic!("a list");
    };
    items.swap(0, 1);
    assert_eq!(
        literal_edit::set_path(src, "l", &StaticValue::List(items)).unwrap(),
        "let l = [\n  # B\n  {b: 2},\n  # A\n  {a: 1},\n]\n"
    );
}

#[test]
fn a_swap_of_children_moves_their_comment_holes_with_them() {
    let src = "let g = <group>\n  {/* A comment */}\n  <a n={1} />\n  {/* B comment */}\n  <b n={2} />\n</group>\n";
    let StaticValue::Element { tag, props, mut children } = get_static_value(src, "g").unwrap() else {
        panic!("an element");
    };
    children.swap(0, 1);
    let swapped = StaticValue::Element { tag, props, children };
    assert_eq!(
        literal_edit::set_path(src, "g", &swapped).unwrap(),
        "let g = <group>\n  {/* B comment */}\n  <b n={2} />\n  {/* A comment */}\n  <a n={1} />\n</group>\n"
    );
}

#[test]
fn a_static_binding_reads_its_hash_comment() {
    let src = "# How far the hull drifts.\nlet DRIFT = 0.02 # per frame\n";
    let bindings = petal::static_value::static_bindings(src).unwrap();
    assert_eq!(bindings[0].comment.as_deref(), Some("How far the hull drifts."));
    assert_eq!(
        literal_edit::set_path(src, "DRIFT", &StaticValue::float(0.5)).unwrap(),
        "# How far the hull drifts.\nlet DRIFT = 0.5 # per frame\n"
    );
}
