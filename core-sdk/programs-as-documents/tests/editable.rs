//! `editable_literal`: is a binding's literal the value the program uses?

use petal::static_value::StaticValue;
use programs_as_documents::{NotEditable, editable_literal};

#[test]
fn a_static_literal_is_editable() {
    let src = "let m = 4\nlet items = [<rect x={m} />, 2]\n";
    let v = editable_literal(src, "items").unwrap();
    let StaticValue::List(xs) = v else { panic!() };
    assert_eq!(xs[1], StaticValue::Int(2));
    assert_eq!(editable_literal("let w = tunable(3, 0, 10)\n", "w"), Ok(StaticValue::Int(3)));
    assert_eq!(editable_literal("let n = -2\n", "n"), Ok(StaticValue::Int(-2)));
}

#[test]
fn reasons_it_is_not() {
    assert_eq!(editable_literal("let a = 1\n", "b"), Err(NotEditable::NotFound));
    assert!(matches!(editable_literal("let a = 1 + 2\n", "a"), Err(NotEditable::Computed { .. })));
    assert!(matches!(
        editable_literal("fn hero(x) <h x={x} /> end\nlet a = [hero(1 + 2)]\n", "a"),
        Err(NotEditable::Computed { .. })
    ));
    let src = "let items = [1, 2]\nprint(items)\nitems = items ++ [3]\n";
    assert_eq!(editable_literal(src, "items"), Err(NotEditable::RebuiltLater { line: 3 }));
    let src = "let items = [1, 2]\nlet items = items.map(fn(x) -> x * 2)\n";
    assert_eq!(editable_literal(src, "items"), Err(NotEditable::RebuiltLater { line: 2 }));
    // A later literal replaces the earlier code.
    assert_eq!(editable_literal("let a = 1 + 2\nlet a = 5\n", "a"), Ok(StaticValue::Int(5)));
    assert!(NotEditable::RebuiltLater { line: 3 }.to_string().contains("line 3"));
    assert!(matches!(editable_literal("let = (", "a"), Err(NotEditable::Parse(_))));
}
