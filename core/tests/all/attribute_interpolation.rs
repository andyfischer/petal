//! A quoted element attribute interpolates as any other double-quoted string
//! does: `id="{id}_box"` is `id={"{id}_box"}`. A quoted attribute without a
//! hole is still a string literal.
//!
//! Every source goes through [`petal::cst::parse_source`] (the tree must give
//! back the source byte for byte; its debug build also checks the AST it
//! projects against the parser's) and is then run.

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
fn shape(src: &str) -> String {
    let (_, stmts) = petal::cst::parse_source(src, ENTRY_FILE)
        .unwrap_or_else(|e| panic!("parse failed: {e}\n--- source ---\n{src}"));
    petal::rewrite::ast_shape(&stmts).unwrap().to_string()
}

#[test]
fn a_quoted_attribute_interpolates() {
    let out = run("let id = \"a\"
print(<box id=\"{id}_box\" />)");
    assert_eq!(out, ["<box id=\"a_box\" />"]);
}

#[test]
fn a_quoted_hole_is_the_braced_string() {
    assert_eq!(
        shape("let x = <box id=\"{id}_box\" />"),
        shape("let x = <box id={\"{id}_box\"} />"),
    );
}

#[test]
fn a_tag_closer_inside_a_quoted_hole_does_not_end_the_tag() {
    let out = run("let n = 2
print(<box k=\"{n > 1}\" t=\"{if n >= 2 then \"<b/>\" else \"x\"}/end\" />)");
    assert_eq!(out, ["<box k=\"true\" t=\"<b/>/end\" />"]);
}

#[test]
fn a_quoted_attribute_without_a_hole_is_a_literal() {
    let out = run("print(<box id=\"plain\" k=\"a\\{b}\" />)");
    assert_eq!(out, ["<box id=\"plain\" k=\"a{b}\" />"]);
}

#[test]
fn interpolated_attributes_work_across_lines_and_beside_others() {
    let out = run("let i = 3
let el = <row
    id=\"row_{i}\"
    label=\"Row {i} of {i + 1}\"
    selected
/>
print(el.props.id, el.props.label, el.props.selected)");
    assert_eq!(out, ["row_3 Row 3 of 4 true"]);
}
