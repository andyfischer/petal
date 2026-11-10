//! Reading and editing element literals (`<text y={400}>Hi</text>`) as static
//! values: an attribute is a path step like a record field, the children are
//! rewritten whole, and everything the edit does not touch keeps its bytes.

use petal::goal_based_editing::{Goal, modify_source_with_goals};
use petal::static_value::{StaticValue, get_static_value};

fn apply(source: &str, goals: &[Goal]) -> String {
    modify_source_with_goals(source, goals)
        .unwrap_or_else(|e| panic!("edit failed: {e}\n--- source ---\n{source}"))
}

/// The edit gives exactly `after`, and making it again changes nothing.
fn check(before: &str, goals: &[Goal], after: &str) {
    let got = apply(before, goals);
    assert_eq!(got, after, "\n--- got ---\n{got}\n--- want ---\n{after}");
    assert_eq!(apply(&got, goals), got, "the edit is not idempotent");
}

fn br() -> StaticValue {
    StaticValue::element("br", [] as [(&str, StaticValue); 0], [] as [StaticValue; 0])
}

const REEL: &str = "\
let items = [
  // The clip under everything.
  <video src=\"clip.mov\" start={0.0} muted />,
  <text
    start={0.5}
    y={362}
    color={#000000}
    box_lines
  >
    Time to group up.<br />
    <br />
    Join now.
  </text>,
  <image src=\"logo.png\" x={804} y={1785} />,
]
";

#[test]
fn reads_elements_as_static_values() {
    let StaticValue::List(items) = get_static_value(REEL, "items").unwrap() else {
        panic!("items is a list");
    };
    assert_eq!(
        items[0],
        StaticValue::element(
            "video",
            [
                ("src", StaticValue::str("clip.mov")),
                ("start", StaticValue::float(0.0)),
                ("muted", StaticValue::bool(true)),
            ],
            [] as [StaticValue; 0],
        )
    );
    let StaticValue::Element {
        props, children, ..
    } = &items[1]
    else {
        panic!("a text element");
    };
    assert_eq!(props[2], ("color".into(), StaticValue::color(0, 0, 0)));
    assert_eq!(
        children,
        &vec![
            StaticValue::str("Time to group up."),
            br(),
            br(),
            StaticValue::str("Join now."),
        ]
    );
}

#[test]
fn sets_an_attribute_in_place() {
    check(
        REEL,
        &[Goal::should_set_path("items[1].y", StaticValue::int(400))],
        &REEL.replace("y={362}", "y={400}"),
    );
    check(
        REEL,
        &[Goal::should_set_path(
            "items[0].src",
            StaticValue::str("b.mov"),
        )],
        &REEL.replace("clip.mov", "b.mov"),
    );
}

#[test]
fn adds_a_missing_attribute_like_its_siblings() {
    // One attribute per line: the new one gets a line.
    check(
        REEL,
        &[Goal::should_set_path("items[1].size", StaticValue::int(57))],
        &REEL.replace("    box_lines\n", "    box_lines\n    size={57}\n"),
    );
    // On the tag's line: a space.
    check(
        REEL,
        &[Goal::should_set_path(
            "items[2].scale",
            StaticValue::float(0.2),
        )],
        &REEL.replace("y={1785} />", "y={1785} scale={0.2} />"),
    );
}

#[test]
fn removes_an_attribute_with_its_space_or_line() {
    check(
        REEL,
        &[Goal::should_remove("items[1].y")],
        &REEL.replace("    y={362}\n", ""),
    );
    check(
        REEL,
        &[Goal::should_remove("items[0].start")],
        &REEL.replace(" start={0.0}", ""),
    );
    // Not there: already removed.
    check(REEL, &[Goal::should_remove("items[0].nope")], REEL);
}

#[test]
fn rewrites_an_attribute_its_spelling_cannot_hold() {
    check(
        REEL,
        &[Goal::should_set_path(
            "items[0].muted",
            StaticValue::bool(false),
        )],
        &REEL.replace("muted />", "muted={false} />"),
    );
    check(
        REEL,
        &[Goal::should_set_path("items[0].src", StaticValue::int(3))],
        &REEL.replace("src=\"clip.mov\"", "src={3}"),
    );
}

#[test]
fn setting_the_whole_element_edits_only_what_differs() {
    let StaticValue::List(items) = get_static_value(REEL, "items").unwrap() else {
        panic!("items is a list");
    };
    let StaticValue::Element { tag, mut props, .. } = items[1].clone() else {
        panic!("a text element");
    };
    props[1].1 = StaticValue::int(300);
    let text = StaticValue::Element {
        tag,
        props,
        children: vec![StaticValue::str("One line now")],
    };
    let want = REEL.replace("y={362}", "y={300}").replace(
        "  >\n    Time to group up.<br />\n    <br />\n    Join now.\n  </text>",
        "  >One line now</text>",
    );
    check(REEL, &[Goal::should_set_path("items[1]", text)], &want);
}

#[test]
fn children_with_line_breaks_go_one_line_each() {
    let source = "let t = <text y={1}>Hi</text>\n";
    let lines = StaticValue::element(
        "text",
        [("y", StaticValue::int(1))],
        [
            StaticValue::str("One"),
            br(),
            br(),
            StaticValue::str("Two {braces}"),
        ],
    );
    check(
        source,
        &[Goal::should_set_value("t", lines.clone())],
        "let t = <text y={1}>\n  One<br />\n  <br />\n  {\"Two \\{braces}\"}\n</text>\n",
    );
    // And it reads back as the same value.
    let edited = apply(source, &[Goal::should_set_value("t", lines.clone())]);
    assert_eq!(get_static_value(&edited, "t").unwrap(), lines);
}

#[test]
fn a_self_closing_element_gains_children_and_loses_them() {
    let source = "let v = <video src=\"a.mov\" />\n";
    let with = StaticValue::element(
        "video",
        [("src", StaticValue::str("a.mov"))],
        [StaticValue::str("caption")],
    );
    check(
        source,
        &[Goal::should_set_value("v", with)],
        "let v = <video src=\"a.mov\">caption</video>\n",
    );
    let without = StaticValue::element(
        "video",
        [("src", StaticValue::str("a.mov"))],
        [] as [StaticValue; 0],
    );
    check(
        "let v = <video src=\"a.mov\">caption</video>\n",
        &[Goal::should_set_value("v", without)],
        source,
    );
}

#[test]
fn a_new_element_in_a_list_is_laid_out_like_its_neighbour() {
    let new = StaticValue::element(
        "text",
        [
            ("start", StaticValue::float(1.0)),
            ("y", StaticValue::int(800)),
        ],
        [StaticValue::str("Hello")],
    );
    let got = apply(REEL, &[Goal::should_insert("items[2]", new.clone())]);
    assert!(
        got.contains("  <text\n    start={1.0}\n    y={800}\n  >Hello</text>,\n  <image"),
        "{got}"
    );
    let StaticValue::List(items) = get_static_value(&got, "items").unwrap() else {
        panic!("items is a list");
    };
    assert_eq!(items[2], new);
    // Appended after a one-line element: one line.
    let got = apply(REEL, &[Goal::should_append("items", new)]);
    assert!(
        got.contains("  <text start={1.0} y={800}>Hello</text>,\n]"),
        "{got}"
    );
}

#[test]
fn removing_and_moving_elements_keeps_their_text() {
    let StaticValue::List(mut items) = get_static_value(REEL, "items").unwrap() else {
        panic!("items is a list");
    };
    items.swap(1, 2);
    let got = apply(
        REEL,
        &[Goal::should_set_value(
            "items",
            StaticValue::List(items.clone()),
        )],
    );
    let StaticValue::List(read) = get_static_value(&got, "items").unwrap() else {
        panic!("items is a list");
    };
    assert_eq!(read, items);
    assert!(
        got.contains("    box_lines\n  >\n    Time to group up.<br />"),
        "{got}"
    );
}

#[test]
fn to_source_renders_an_element_that_reads_back() {
    let value = StaticValue::element(
        "text",
        [
            ("font", StaticValue::str("Futura")),
            ("size", StaticValue::int(57)),
            ("italic", StaticValue::bool(true)),
            ("color", StaticValue::color(255, 255, 255)),
        ],
        [StaticValue::str("a"), br(), StaticValue::str("b")],
    );
    let source = value.to_source();
    assert_eq!(
        source,
        "<text font=\"Futura\" size={57} italic color={#ffffff}>\n  a<br />\n  b\n</text>"
    );
    let read = get_static_value(&format!("let x = {source}\n"), "x").unwrap();
    assert_eq!(read, value);
    let json = petal::static_json::to_json(&value);
    assert_eq!(petal::static_json::from_json(&json).unwrap(), value);
}
