//! A value may start on the line after `=` (docs/syntax/line-continuation.md,
//! "After `=`"; docs/proposals/equals-line-continuation.md).
//!
//! When `=` (or `op=`) ends its line and the next line of code is indented
//! further than the statement's line, the value starts there. It is one
//! expression, not a block. A next line that is not indented further is a new
//! statement, so the value is missing.
//!
//! Every source that parses goes through [`petal::cst::parse_source`]: the
//! tree must give back the source byte for byte, and its debug build checks
//! the AST it projects against the parser's.

use petal::env::Env;
use petal::goal_based_editing::{Goal, modify_source_with_goals};
use petal::source_map::ENTRY_FILE;
use petal::static_value::{StaticValue, get_static_value};

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

#[track_caller]
fn parse_err(src: &str) -> String {
    petal::cst::parse_source(src, ENTRY_FILE).expect_err("expected a parse error")
}

const MISSING: &str = "the value after `=` is missing";

// ---- every binding and assignment form --------------------------------------

#[test]
fn a_let_value_on_the_next_line_is_the_same_program() {
    assert_eq!(
        shape("let a =\n  <text>hi</text>\nprint(a)\n"),
        shape("let a = <text>hi</text>\nprint(a)\n"),
    );
    assert_eq!(run("let a =\n  <text>hi</text>\nprint(a)"), ["<text>hi</text>"]);
}

#[test]
fn var_set_and_compound_set() {
    let out = run("var x =
  1
set x =
  x + 4
set x +=
  1
print(get x)");
    assert_eq!(out, ["6"]);
}

#[test]
fn plain_and_compound_assignment() {
    let out = run("let r = {a: 1, s: \"x\"}
r.a =
  2
r.s ++=
  \"y\"
print(r)");
    assert_eq!(out, ["{ a: 2, s: \"xy\" }"]);
}

#[test]
fn state_and_memo() {
    let out = run("var n = 3
state var hits =
  0
memo(get n) sq =
  get n * get n
print(hits, sq)");
    assert_eq!(out, ["0 9"]);
}

#[test]
fn a_binding_in_a_fn_body_measures_against_its_own_line() {
    let out = run("fn f()
  let a =
    [1, 2]
  a
print(f())");
    assert_eq!(out, ["[1, 2]"]);
}

// ---- what the value may be ------------------------------------------------

#[test]
fn the_value_may_be_a_construct_with_an_indented_body() {
    let out = run("let n = 2
let v =
  if n > 1 then
    \"big\"
  else
    \"small\"
print(v)");
    assert_eq!(out, ["big"]);
}

#[test]
fn the_value_may_continue_after_an_operator() {
    let out = run("let total =
  1 + 2
    * 3
print(total)");
    assert_eq!(out, ["7"]);
}

#[test]
fn blank_and_comment_lines_after_the_equals_are_skipped() {
    let out = run("let a =

  # why
  1
print(a)");
    assert_eq!(out, ["1"]);
}

// ---- where it does not continue -------------------------------------------

#[test]
fn a_next_line_not_indented_further_is_a_new_statement() {
    // The live-edit case: an unfinished `let a =` above existing code stays
    // an error, not `let a = print(1)`.
    let err = parse_err("let a =\nprint(1)\n");
    assert!(err.contains(MISSING), "{err}");
    let err = parse_err("fn f()\n  let a =\n  print(1)\n");
    assert!(err.contains(MISSING), "{err}");
}

#[test]
fn an_equals_at_the_end_of_the_input_is_missing_its_value() {
    let err = parse_err("let a =");
    assert!(err.contains(MISSING), "{err}");
    let err = parse_err("let a =\n\n# nothing\n");
    assert!(err.contains(MISSING), "{err}");
}

#[test]
fn the_value_is_one_expression_not_a_block() {
    let err = parse_err("let a =\n  1\n  2\n");
    assert!(err.contains("unexpected indent"), "{err}");
    assert!(err.contains("one expression"), "{err}");
}

#[test]
fn an_unexpected_indent_elsewhere_gets_no_equals_hint() {
    let err = parse_err("let a = 1\n  2\nprint(a)\n");
    assert!(err.contains("unexpected indent"), "{err}");
    assert!(!err.contains("one expression"), "{err}");
}

#[test]
fn a_tab_in_the_continuation_line_is_refused() {
    let err = parse_err("let a =\n\t1\n");
    assert!(err.contains("tab"), "{err}");
}

// ---- tools that read and rewrite source -----------------------------------

#[test]
fn a_next_line_value_is_static_and_edits_in_place() {
    let src = "let speed =\n  5.0 # m/s\nlet cfg =\n  {a: 1, b: 2}\n";
    assert_eq!(get_static_value(src, "speed").unwrap(), StaticValue::from(5.0));

    let set = Goal::should_set_path::<&str, StaticValue>;
    let out = modify_source_with_goals(src, &[set("speed", 7.5.into())]).unwrap();
    assert_eq!(out, "let speed =\n  7.5 # m/s\nlet cfg =\n  {a: 1, b: 2}\n");
    let out = modify_source_with_goals(src, &[set("cfg.b", 9.into())]).unwrap();
    assert_eq!(out, "let speed =\n  5.0 # m/s\nlet cfg =\n  {a: 1, b: 9}\n");
}
