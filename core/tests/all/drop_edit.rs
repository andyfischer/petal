//! Structural direct manipulation (D2 P4): a drop on a literal model, diffed
//! into goal edits and written back with the file's comments and layout kept.

use petal::drop_edit::{literal_bindings, propose_drop, static_from_json};
use petal::goal_based_editing::Goal;
use petal::static_value::get_static_value;
use serde_json::json;

const PIPELINE: &str = "\
// The pipeline, in the order it runs.
let STEPS = [
  \"fetch\",
  \"parse\",   // tolerant
  \"check\",
  \"emit\",    // last
]

// Not the model: the same values in another order.
let OTHER = [\"emit\", \"check\"]

state picked = 0
";

/// Read `name` back out of `source` as the runtime would see it.
fn value(source: &str, name: &str) -> petal::static_value::StaticValue {
    get_static_value(source, name).unwrap()
}

#[test]
fn a_reorder_round_trips_and_keeps_comments() {
    let model = json!(["fetch", "parse", "check", "emit"]);
    let drop = json!(["parse", "check", "fetch", "emit"]);
    let edit = propose_drop(PIPELINE, &model, &drop).unwrap();
    assert_eq!(edit.binding, "STEPS");
    assert_eq!(
        edit.goals,
        vec![Goal::should_remove("STEPS[0]"), Goal::should_insert("STEPS[2]", "fetch")]
    );
    // The edited source reads back as the drop…
    assert_eq!(value(&edit.source, "STEPS"), static_from_json(&drop).unwrap());
    // …and everything not moved is where it was, comments included.
    for kept in [
        "// The pipeline, in the order it runs.",
        "\"parse\",   // tolerant",
        "\"emit\",    // last",
        "// Not the model: the same values in another order.",
        "let OTHER = [\"emit\", \"check\"]",
        "state picked = 0",
    ] {
        assert!(edit.source.contains(kept), "lost {kept:?}:\n{}", edit.source);
    }
    // One element per line, as the list was written.
    assert!(edit.source.contains("  \"check\",\n  \"fetch\",\n"), "{}", edit.source);
}

#[test]
fn a_cross_column_move_edits_only_the_two_columns() {
    let src = "\
// Board: columns of cards.
let BOARD = [
  {id: \"todo\", cards: [{id: 1, t: \"write\"}, {id: 2, t: \"test\"}]},  // backlog
  {id: \"done\", cards: [{id: 3, t: \"ship\"}]},
]
";
    let model = json!([
        {"id": "todo", "cards": [{"id": 1, "t": "write"}, {"id": 2, "t": "test"}]},
        {"id": "done", "cards": [{"id": 3, "t": "ship"}]}
    ]);
    let drop = json!([
        {"id": "todo", "cards": [{"id": 2, "t": "test"}]},
        {"id": "done", "cards": [{"id": 3, "t": "ship"}, {"id": 1, "t": "write"}]}
    ]);
    let edit = propose_drop(src, &model, &drop).unwrap();
    assert_eq!(
        edit.goals,
        vec![
            Goal::should_remove("BOARD[0].cards[0]"),
            Goal::should_insert(
                "BOARD[1].cards[1]",
                static_from_json(&json!({"id": 1, "t": "write"})).unwrap()
            ),
        ]
    );
    assert_eq!(value(&edit.source, "BOARD"), static_from_json(&drop).unwrap());
    assert!(edit.source.contains("// Board: columns of cards."));
    assert!(edit.source.contains("// backlog"), "{}", edit.source);
}

#[test]
fn a_scalar_drop_sets_the_path_and_colours_stay_literals() {
    let src = "let SWATCH = {name: \"warm\", tint: :ff8800, size: 3}\n";
    let model = json!({"name": "warm", "tint": {"r": 255, "g": 136, "b": 0}, "size": 3});
    let drop = json!({"name": "warm", "tint": {"r": 255, "g": 136, "b": 0}, "size": 5});
    let edit = propose_drop(src, &model, &drop).unwrap();
    assert_eq!(edit.goals, vec![Goal::should_set_path("SWATCH.size", 5)]);
    assert_eq!(edit.source, "let SWATCH = {name: \"warm\", tint: :ff8800, size: 5}\n");
}

#[test]
fn no_edit_without_exactly_one_literal_binding() {
    // A state initializer is not a target: editing it changes nothing on screen.
    let src = "state items = [1, 2]\nlet A = [3, 4]\nlet B = [3, 4]\n";
    let err = propose_drop(src, &json!([1, 2]), &json!([2, 1])).unwrap_err();
    assert!(err.message.contains("not a top-level `let` literal"), "{err}");
    let err = propose_drop(src, &json!([3, 4]), &json!([4, 3])).unwrap_err();
    assert!(err.message.contains("A, B"), "{err}");
    assert_eq!(
        literal_bindings(src, &static_from_json(&json!([3, 4])).unwrap()).unwrap(),
        vec!["A".to_string(), "B".to_string()]
    );
}

#[test]
fn a_drop_that_changes_nothing_leaves_the_source_byte_identical() {
    let model = json!(["fetch", "parse", "check", "emit"]);
    let edit = propose_drop(PIPELINE, &model, &model).unwrap();
    assert!(edit.goals.is_empty());
    assert_eq!(edit.source, PIPELINE);
}
