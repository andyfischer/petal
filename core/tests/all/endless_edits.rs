//! Program manipulation on source whose blocks are closed by indentation
//! alone (docs/tasks/pythonic-syntax.md §6.6): goal edits, literal edits,
//! drops, tree splices and the direct-manipulation solver must read such a
//! file, change only what they are asked to, and leave a file that parses to
//! the program they meant. Each case also runs on the same source with its
//! `end`s (`petal::fmt::remove_block_ends` gives one from the other), and the
//! two results must be the same program.

use petal::drop_edit::{propose_drop, static_from_json};
use petal::goal_based_editing::{Goal, modify_source_with_goals};
use petal::rewrite;
use petal::static_value::{StaticValue, get_static_value};
use serde_json::json;

/// A program with every kind of block, each closed by an `end`.
const WITH_ENDS: &str = "\
# Settings first.
let SIZE = tunable(14)
let THEME = {
  ink: :202020,
  pad: [4, 8],
}

fn area(w, h)
  if w < 0 then
    return 0
  else if h < 0 then
    return 0
  end
  w * h
end

let STEPS = [
  \"fetch\",
  \"parse\",  # tolerant
  \"emit\",
]

let scale = fn(v)
  let k = 2
  v * k
end

for s in STEPS do
  match s
    when \"fetch\" -> print(1)
    when _ ->
      print(scale(SIZE))
  end
end

layout(area(SIZE, 2))

fn layout(n)
  print(n)
end
";

/// [`WITH_ENDS`] with every block closed by its indentation.
fn without_ends() -> String {
    let stripped = petal::fmt::remove_block_ends(WITH_ENDS).expect("remove ends");
    assert_eq!(stripped.kept, Vec::<u32>::new());
    assert!(!stripped.text.contains("end\n"), "{}", stripped.text);
    stripped.text
}

fn shape(source: &str) -> serde_json::Value {
    let (_tree, stmts) = rewrite::parse_ast(source)
        .unwrap_or_else(|e| panic!("does not parse: {e}\n--- source ---\n{source}"));
    rewrite::ast_shape(&stmts).unwrap()
}

/// Apply `goals` to the program with and without its `end`s: both results
/// parse, and they are the same program. Returns the one without. Goals that
/// state what should hold (`settles`) find nothing to do the second time.
fn edit_both(goals: &[Goal], settles: bool) -> String {
    let endless = without_ends();
    let a = modify_source_with_goals(WITH_ENDS, goals).expect("edit with ends");
    let b = modify_source_with_goals(&endless, goals)
        .unwrap_or_else(|e| panic!("edit without ends failed: {e}"));
    assert_eq!(shape(&a), shape(&b), "\n--- with ends ---\n{a}\n--- without ---\n{b}");
    if settles {
        assert_eq!(modify_source_with_goals(&b, goals).unwrap(), b, "not idempotent");
    }
    b
}

fn value(source: &str, name: &str) -> StaticValue {
    get_static_value(source, name).unwrap()
}

#[test]
fn the_fixture_is_the_same_program_both_ways() {
    assert_eq!(shape(WITH_ENDS), shape(&without_ends()));
}

#[test]
fn setting_values_and_paths_edits_only_the_literal() {
    let endless = without_ends();
    let out = edit_both(
        &[
            Goal::should_set_value("SIZE", 20),
            Goal::should_set_path("THEME.pad[1]", 12),
            Goal::should_set_path("THEME.ink", StaticValue::str("dark")),
        ],
        true,
    );
    assert_eq!(
        value(&out, "THEME"),
        static_from_json(&json!({"ink": "dark", "pad": [4, 12]})).unwrap()
    );
    // Every other line is byte for byte what it was.
    let changed: Vec<(&str, &str)> =
        endless.lines().zip(out.lines()).filter(|(a, b)| a != b).collect();
    assert_eq!(
        changed,
        [
            ("let SIZE = tunable(14)", "let SIZE = tunable(20)"),
            ("  ink: :202020,", "  ink: \"dark\","),
            ("  pad: [4, 8],", "  pad: [4, 12],"),
        ]
    );
}

#[test]
fn list_elements_are_inserted_removed_and_moved() {
    let out = edit_both(
        &[
            Goal::should_insert("STEPS[1]", "check"),
            Goal::should_append("STEPS", "done"),
            Goal::should_remove("STEPS[0]"),
            Goal::should_move("STEPS[0]", "STEPS[2]"),
        ],
        false,
    );
    assert_eq!(
        value(&out, "STEPS"),
        static_from_json(&json!(["parse", "emit", "check", "done"])).unwrap(),
        "{out}"
    );
    // The comment travelled with its element, and the loop under the list
    // (a block with no `end`) is untouched.
    assert!(out.contains("\"parse\",  # tolerant"), "{out}");
    assert!(out.contains("for s in STEPS do\n  match s\n    when \"fetch\" -> print(1)\n"), "{out}");
}

#[test]
fn a_new_binding_lands_at_the_top_level_not_in_the_last_block() {
    // The file ends inside `layout`'s body, which nothing closes but the
    // column of what follows: an appended statement must start at column 1.
    let out = edit_both(&[Goal::should_set_value("GAP", 3)], true);
    assert!(out.ends_with("fn layout(n)\n  print(n)\n\nlet GAP = 3\n"), "{out}");
    assert_eq!(value(&out, "GAP"), StaticValue::int(3));

    // Placed after a block lambda and before a function: outside both.
    let out = edit_both(
        &[
            Goal::should_set_value("A", 1).after("scale"),
            Goal::should_set_value("B", StaticValue::list([StaticValue::int(1), StaticValue::int(2)]))
                .before("STEPS"),
        ],
        true,
    );
    assert!(out.contains("  v * k\n\nlet A = 1\n\nfor s in STEPS do"), "{out}");
    assert!(out.contains("  w * h\n\nlet B = [1, 2]\n\nlet STEPS = ["), "{out}");
    assert_eq!(value(&out, "A"), StaticValue::int(1));
}

/// Source held in an indented host string has a left margin (B12): a new
/// top-level statement starts at it, or the file stops parsing.
#[test]
fn a_new_statement_starts_at_the_files_left_margin() {
    let src = "    let a = 1\n    fn f(x)\n      x + a\n";
    let out = modify_source_with_goals(
        src,
        &[
            Goal::should_set_value("b", 2),
            Goal::should_set_value("c", StaticValue::list([StaticValue::call("f", [1]), StaticValue::call("f", [2])]))
                .after("a"),
            Goal::should_set_value("z", 0).before("a"),
        ],
    )
    .unwrap();
    assert_eq!(
        out,
        "    let z = 0\n\n    let a = 1\n    let c = [\n        f(1),\n        f(2),\n      ]\n    fn f(x)\n      x + a\n\n    let b = 2\n"
    );
    shape(&out);
}

#[test]
fn a_drop_on_a_list_is_written_back() {
    let endless = without_ends();
    let model = json!(["fetch", "parse", "emit"]);
    let drop = json!(["parse", "emit", "fetch"]);
    let with_ends = propose_drop(WITH_ENDS, &model, &drop).unwrap();
    let edit = propose_drop(&endless, &model, &drop).unwrap();
    assert_eq!(edit.binding, "STEPS");
    assert_eq!(edit.goals, with_ends.goals);
    assert_eq!(shape(&edit.source), shape(&with_ends.source));
    assert_eq!(value(&edit.source, "STEPS"), static_from_json(&drop).unwrap());
}

#[test]
fn tree_splices_find_blocks_by_their_spans() {
    let endless = without_ends();
    let (tree, stmts) = rewrite::parse_ast(&endless).unwrap();
    // A statement-position call after blocks with no `end`.
    let call = rewrite::find_call(&stmts, "layout").expect("the call");
    let edited = rewrite::splice_node(&tree, call, "layout(area(SIZE, 3), true)").expect("splice");
    let text = edited.text();
    assert!(text.contains("\nlayout(area(SIZE, 3), true)\n\nfn layout(n)\n  print(n)\n"), "{text}");
    shape(&text);
    // A binding whose value is a block lambda: its span is the whole block,
    // and replacing it leaves what follows outside.
    let scale = rewrite::find_binding(&stmts, "scale").expect("the binding");
    let src: Vec<char> = endless.chars().collect();
    let written: String = src[scale.start.offset as usize..scale.end.offset as usize].iter().collect();
    assert_eq!(written, "fn(v)\n  let k = 2\n  v * k");
    let text = rewrite::splice(&endless, scale, "fn(v) -> v * 3");
    assert!(text.contains("let scale = fn(v) -> v * 3\n\nfor s in STEPS do\n"), "{text}");
    shape(&text);
}

/// Direct manipulation v2: the solver reads a program with no `end`, finds
/// the literal behind an output and writes the new value in place.
#[test]
fn the_solver_edits_a_literal_in_a_program_with_no_end() {
    use petal::dm::NoHost;
    use petal::propose_edit::{opts_from_json, propose_with_natives};
    let with_ends = "\
import io
let x = tunable(10)
fn scale(v)
  if v > 100 then
    return v
  end
  let k = 2
  v * k
end
for i in [1] do
  push_output(symbol(\"shapes\"), scale(x) * i)
end
";
    let endless = petal::fmt::remove_block_ends(with_ends).unwrap().text;
    assert!(!endless.contains("end"), "{endless}");
    let opts = opts_from_json(&json!({"channel": "shapes", "emit": 0, "value": true, "to": 50})).unwrap();
    let mut answers = Vec::new();
    for source in [with_ends, endless.as_str()] {
        let report = propose_with_natives(source, None, &[], &opts, &mut NoHost, &mut |_| {}).expect("proposes");
        let best = &report["alternatives"][0];
        assert_eq!(best["verified"], json!(true), "{report:#}");
        assert_eq!(best["changes"][0]["new"].as_f64(), Some(25.0), "{report:#}");
        answers.push(report["alternatives"][0]["changes"].clone());
    }
    // The same literal, on the same line.
    assert_eq!(answers[0][0]["line"], answers[1][0]["line"], "{answers:#?}");
    // Written into the source, the edit moves nothing else.
    let edited = petal::literal_edit::set_path(&endless, "x", &StaticValue::int(25)).unwrap();
    assert_eq!(edited, endless.replace("tunable(10)", "tunable(25)"));
    shape(&edited);
}
