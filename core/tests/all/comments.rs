//! Comments: `//` lines, `/* … */` blocks, and both inside element children.
//!
//! In element children a line whose first non-blank characters are `//` or
//! `/*` is a comment, not text (docs/language-guide.md, "JSX-like Elements"),
//! and `{/* … */}` is a comment anywhere in them.

use petal::env::Env;
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

#[test]
fn a_block_comment_is_ignored_inline_and_over_lines() {
    assert_eq!(show("/* a\n   b */\nlet x = 1 /* one */ + 2\nx"), "3");
    assert_eq!(show("let x = [1, /* skip\n */ 2]\nx"), "[1, 2]");
}

#[test]
fn block_comments_do_not_nest() {
    // The first `*/` closes the comment, as in JS and C.
    assert_eq!(show("/* a /* b */ 7"), "7");
}

#[test]
fn an_unterminated_block_comment_is_an_error() {
    let e = error("let x = 1\n/* never closed\nlet y = 2\n");
    assert!(e.contains("Unterminated block comment"), "{e}");
    assert!(e.contains("line 2"), "{e}");
}

#[test]
fn a_comment_looking_string_is_still_a_string() {
    assert_eq!(show(r#""a /* b */ c // d""#), "a /* b */ c // d");
}

#[test]
fn comment_lines_in_element_children_are_not_children() {
    let src = "<group>\n  // the photo\n  <image src=\"a.png\" />\n  /* two\n     lines */\n</group>";
    assert_eq!(show(src), r#"<group><image src="a.png" /></group>"#);
}

#[test]
fn a_comment_line_inside_text_drops_out_and_the_lines_still_join() {
    let src = "<text>Hello\n  // not shown\n  world</text>";
    assert_eq!(show(src), "<text>Hello world</text>");
}

#[test]
fn an_empty_or_comment_only_hole_is_no_child() {
    assert_eq!(show("<a>x{/* note */}</a>"), "<a>x</a>");
    assert_eq!(show("<a>{}</a>"), "<a />");
    assert_eq!(show("(<a>{/* n */}<b /></a>).children.len()"), "1");
}

#[test]
fn slashes_that_do_not_start_a_line_stay_text() {
    // Not first on its line: text, as before. A glob in text keeps its `/*`.
    assert_eq!(show("<a>see // here</a>"), "<a>see // here</a>");
    assert_eq!(show("<a>src/*.ptl</a>"), "<a>src/*.ptl</a>");
    // Text that should start a line with `//` goes in a hole.
    assert_eq!(show("<a>\n  {\"//\"} x\n</a>"), "<a>// x</a>");
}

#[test]
fn a_bare_record_key_names_the_fix() {
    let e = error("let m = {type: \"rect\", invert}");
    assert!(e.contains("Record field `invert` needs a value"), "{e}");
    assert!(e.contains("`invert: true`"), "{e}");
    assert!(e.contains("column 24"), "{e}");
    let e = error("let m = {invert, type: \"rect\"}");
    assert!(e.contains("Record field `invert` needs a value"), "{e}");
}
