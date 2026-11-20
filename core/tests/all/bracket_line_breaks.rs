//! Line breaks inside brackets, and element children inside `{…}` holes.
//!
//! - Inside `( )`, `[ ]` and `{ }` line breaks are free
//!   (docs/syntax/line-continuation.md): a grouping paren's expression and a
//!   record field's value may start on the line after the `(` or `:`.
//! - An element nested in a `{…}` hole lexes its children as element content,
//!   exactly as it would at the top level or in a `fn` body, so bare text and
//!   nested holes work there too.
//!
//! Every source goes through [`petal::cst::parse_source`] (the tree must give
//! back the source byte for byte) and is then run.

use petal::env::Env;
use petal::source_map::ENTRY_FILE;

#[track_caller]
fn run(src: &str) -> Vec<String> {
    let (tree, _) = petal::cst::parse_source(src, ENTRY_FILE)
        .unwrap_or_else(|e| panic!("parse failed: {e}\n--- source ---\n{src}"));
    assert_eq!(tree.text(), src, "the syntax tree lost text");
    let mut env = Env::new();
    env.run_source(src)
        .unwrap_or_else(|e| panic!("run failed: {e}\n--- source ---\n{src}"));
    env.take_output()
}

#[track_caller]
fn parse_err(src: &str) -> String {
    petal::cst::parse_source(src, ENTRY_FILE).expect_err("expected a parse error")
}

// ---- line breaks inside brackets ------------------------------------------

#[test]
fn a_grouping_paren_may_open_its_expression_on_the_next_line() {
    let out = run("let a = (
  1 + 2
)
print(a)");
    assert_eq!(out, ["3"]);
}

#[test]
fn a_grouping_paren_may_hold_an_element_on_the_next_line() {
    let out = run("let a = (
  <text>hi</text>
)
print(a)");
    assert_eq!(out, ["<text>hi</text>"]);
}

#[test]
fn a_grouping_paren_may_hold_an_indented_block_on_the_next_line() {
    let out = run("let n = 2
let v = (
  if n > 0 then
    \"pos\"
  else
    \"neg\"
)
print(v)");
    assert_eq!(out, ["pos"]);
}

#[test]
fn blank_and_comment_lines_after_a_paren_are_skipped() {
    let out = run("let w = (

  # the sum
  1 +
  2)
print(w)");
    assert_eq!(out, ["3"]);
}

#[test]
fn a_record_field_value_may_start_on_the_next_line() {
    let out = run("let r = {x:
  1, y:

  2}
print(r.x + r.y)");
    assert_eq!(out, ["3"]);
}

#[test]
fn a_computed_record_key_value_may_start_on_the_next_line() {
    let out = run("let k = \"z\"
let r = {[k]:
  5}
print(r.z)");
    assert_eq!(out, ["5"]);
}

#[test]
fn a_let_value_still_starts_on_the_line_of_its_equals() {
    // docs/syntax/line-continuation.md, "What does not continue". Changing
    // this is a language change, not a bug fix.
    let err = parse_err("let a =\n  1\n");
    assert!(err.contains("line break"), "{err}");
}

// ---- elements inside `{…}` holes --------------------------------------------

#[test]
fn bare_text_in_an_element_passed_as_an_argument_in_a_hole() {
    let out = run("fn pb(t, i)
  [t, i]
let x = <vstack>{pb(\"Go\", <text size={19}>G</text>)}</vstack>
print(x)");
    assert_eq!(out, ["<vstack>[\"Go\", <text size={19}>G</text>]</vstack>"]);
}

#[test]
fn bare_text_and_a_nested_hole_in_an_element_built_by_a_for_in_a_hole() {
    let out = run("let xs = [1, 2]
let x = <vstack>{for x in xs do <text>Item {x}</text> end}</vstack>
print(x)");
    assert_eq!(out, ["<vstack>[<text>Item 1</text>, <text>Item 2</text>]</vstack>"]);
}

#[test]
fn non_ascii_text_in_an_element_in_a_hole() {
    let out = run("let x = <vstack>{[<text>•••• 4242</text>]}</vstack>
print(x)");
    assert_eq!(out, ["<vstack>[<text>•••• 4242</text>]</vstack>"]);
}

#[test]
fn elements_nest_in_holes_at_any_depth() {
    let out = run("let n = 3
let x = <a>{[<b>count {n} of {n + 1}</b>, <c x={n}>hi <d>{\"deep\"}</d> there</c>]}</a>
print(x)");
    assert_eq!(out, ["<a>[<b>count 3 of 4</b>, <c x={3}>hi <d>deep</d> there</c>]</a>"]);
}

#[test]
fn bare_text_in_an_element_in_an_attribute_hole() {
    let out = run("let x = <a k={<b>text here</b>}>body</a>
print(x)");
    assert_eq!(out, ["<a k={<b>text here</b>}>body</a>"]);
}

#[test]
fn a_multi_line_element_in_a_hole_lexes_like_one_at_the_top_level() {
    let in_hole = run("let xs = [1]
let x = <ul>{
  for x in xs do
    <li>
      Item {x}
      done
    </li>
}</ul>
print(x)");
    let top = run("let x = 1
let y = <li>
  Item {x}
  done
</li>
print(y)");
    assert_eq!(in_hole, [format!("<ul>[{}]</ul>", top[0])]);
}

#[test]
fn braces_and_strings_in_a_hole_still_close_it_correctly() {
    let out = run("let x = <a>{ {k: 1}.k }</a>
print(x)
let y = <a>{ \"}\" }</a>
print(y)");
    assert_eq!(out, ["<a>1</a>", "<a>}</a>"]);
}
