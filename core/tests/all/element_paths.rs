//! Edit paths into an element's children (`layers[2]/group[0]/image[1].x`):
//! set, insert, remove and move a child element as a minimal splice, keeping
//! the comments, blank lines and hand formatting between the tags. The path
//! grammar is in `docs/features/edit-paths.md`.

use petal::goal_based_editing::{Goal, modify_source_with_goals};
use petal::literal_edit::{self, EditErrorKind};
use petal::static_value::{StaticValue, get_static_value};

fn apply(source: &str, goals: &[Goal]) -> String {
    modify_source_with_goals(source, goals)
        .unwrap_or_else(|e| panic!("edit failed: {e}\n--- source ---\n{source}"))
}

/// The edit gives exactly `after`.
fn check(before: &str, goals: &[Goal], after: &str) {
    let got = apply(before, goals);
    assert_eq!(got, after, "\n--- got ---\n{got}\n--- want ---\n{after}");
}

fn image(src: &str, x: i64) -> StaticValue {
    StaticValue::element(
        "image",
        [("src", StaticValue::str(src)), ("x", StaticValue::int(x))],
        [] as [StaticValue; 0],
    )
}

const DOC: &str = "\
let layers = [
  <image src=\"bg.png\" x={0} />,
  <text x={10}>Title — café</text>,
  <group name=\"cards\">
    // The left card.
    <image src=\"a.png\" x={40} y={80} />

    // The right card, nudged by hand.
    <image   src=\"b.png\"  x={520} y={180} />
    <text x={1}>Né → ok</text>
  </group>,
]
";

#[test]
fn sets_an_attribute_of_a_nested_child() {
    let want = DOC.replace("x={520}", "x={600}");
    for path in [
        "layers[2]/image[1].x",
        "layers[2]/*[1].x",
        "layers[2]/image[1]/@x",
    ] {
        check(DOC, &[Goal::should_set_path(path, 600)], &want);
    }
    // A tag that is there once needs no index.
    check(
        DOC,
        &[Goal::should_set_path("layers[2]/text.x", 2)],
        &DOC.replace("x={1}>Né", "x={2}>Né"),
    );
    // A missing attribute is added, like a missing record field.
    check(
        DOC,
        &[Goal::should_set_path("layers[2]/image[0].opacity", 0.5)],
        &DOC.replace("y={80} />", "y={80} opacity={0.5} />"),
    );
}

#[test]
fn old_paths_keep_their_meaning() {
    check(
        DOC,
        &[Goal::should_set_path("layers[0].x", 5)],
        &DOC.replace("x={0}", "x={5}"),
    );
    check(
        DOC,
        &[Goal::should_set_path("layers[2].name", "deck")],
        &DOC.replace("name=\"cards\"", "name=\"deck\""),
    );
    check(
        DOC,
        &[Goal::should_remove("layers[0]")],
        &DOC.replace("  <image src=\"bg.png\" x={0} />,\n", ""),
    );
}

#[test]
fn sets_a_whole_child() {
    // Same tag: only what differs is written.
    let a = StaticValue::element(
        "image",
        [
            ("src", StaticValue::str("a.png")),
            ("x", StaticValue::int(40)),
            ("y", StaticValue::int(90)),
        ],
        [] as [StaticValue; 0],
    );
    check(
        DOC,
        &[Goal::should_set_path("layers[2]/image[0]", a)],
        &DOC.replace("y={80}", "y={90}"),
    );
    // Another tag: the child is replaced, and nothing around it moves.
    let rect = StaticValue::element(
        "rect",
        [("w", StaticValue::int(4))],
        [] as [StaticValue; 0],
    );
    check(
        DOC,
        &[Goal::should_set_path("layers[2]/image[1]", rect)],
        &DOC.replace(
            "<image   src=\"b.png\"  x={520} y={180} />",
            "<rect w={4} />",
        ),
    );
    // A child can only be an element.
    let err = literal_edit::set_path(DOC, "layers[2]/image[1]", &StaticValue::str("hi")).unwrap_err();
    assert_eq!(err.kind, EditErrorKind::Invalid);
}

#[test]
fn inserts_a_child_on_its_own_line_like_its_siblings() {
    // Before the second image: directly below the first one's line, so the
    // blank line and the comment stay with the image they describe.
    check(
        DOC,
        &[Goal::should_insert("layers[2]/image[1]", image("c.png", 300))],
        &DOC.replace(
            "x={40} y={80} />\n",
            "x={40} y={80} />\n    <image src=\"c.png\" x={300} />\n",
        ),
    );
    // First: directly below the open tag.
    check(
        DOC,
        &[Goal::should_insert("layers[2]/*[0]", image("c.png", 300))],
        &DOC.replace(
            "<group name=\"cards\">\n",
            "<group name=\"cards\">\n    <image src=\"c.png\" x={300} />\n",
        ),
    );
    // One past the last `<image>`: just after it, before the `<text>`.
    check(
        DOC,
        &[Goal::should_insert("layers[2]/image[2]", image("c.png", 300))],
        &DOC.replace(
            "y={180} />\n",
            "y={180} />\n    <image src=\"c.png\" x={300} />\n",
        ),
    );
    // Appending to an element adds a last child.
    check(
        DOC,
        &[Goal::should_append("layers[2]", image("c.png", 300))],
        &DOC.replace(
            "ok</text>\n",
            "ok</text>\n    <image src=\"c.png\" x={300} />\n",
        ),
    );
    // And it reads back where the path said.
    let got = apply(DOC, &[Goal::should_insert("layers[2]/image[1]", image("c.png", 300))]);
    let StaticValue::List(layers) = get_static_value(&got, "layers").unwrap() else {
        panic!("a list");
    };
    let StaticValue::Element { children, .. } = &layers[2] else {
        panic!("a group");
    };
    assert_eq!(children[1], image("c.png", 300));
}

#[test]
fn removes_a_child_and_keeps_the_comments() {
    check(
        DOC,
        &[Goal::should_remove("layers[2]/image[1]")],
        &DOC.replace("    <image   src=\"b.png\"  x={520} y={180} />\n", ""),
    );
    check(
        DOC,
        &[Goal::should_remove("layers[2]/text")],
        &DOC.replace("    <text x={1}>Né → ok</text>\n", ""),
    );
    // A child that is not there is an error, as a list index past the end is.
    let err = literal_edit::remove_path(DOC, "layers[2]/image[2]").unwrap_err();
    assert_eq!(err.kind, EditErrorKind::NotFound);
}

#[test]
fn moves_a_child_keeping_its_text() {
    // Within the group: the hand-spaced image goes first, as written.
    check(
        DOC,
        &[Goal::should_move("layers[2]/image[1]", "layers[2]/*[0]")],
        &DOC
            .replace("    <image   src=\"b.png\"  x={520} y={180} />\n", "")
            .replace(
                "<group name=\"cards\">\n",
                "<group name=\"cards\">\n    <image   src=\"b.png\"  x={520} y={180} />\n",
            ),
    );
    // Out of the list and into the group (the group is `layers[1]` once the
    // first layer is gone).
    check(
        DOC,
        &[Goal::should_move("layers[0]", "layers[1]/*[0]")],
        &DOC.replace("  <image src=\"bg.png\" x={0} />,\n", "").replace(
            "<group name=\"cards\">\n",
            "<group name=\"cards\">\n    <image src=\"bg.png\" x={0} />\n",
        ),
    );
    // Out of the group into the list.
    check(
        DOC,
        &[Goal::should_move("layers[2]/text", "layers[3]")],
        &DOC.replace("    <text x={1}>Né → ok</text>\n", "")
            .replace("  </group>,\n", "  </group>,\n  <text x={1}>Né → ok</text>,\n"),
    );
    // A path moved onto itself changes nothing.
    check(DOC, &[Goal::should_move("layers[2]/image[0]", "layers[2]/image[0]")], DOC);
}

#[test]
fn a_moved_multi_line_child_is_reindented() {
    let source = "\
let doc = <frame>
  <group>
    <image
      src=\"a.png\"
      x={1}
    />
  </group>
  <image src=\"b.png\" />
</frame>
";
    check(
        source,
        &[Goal::should_move("doc/group/image", "doc/*[2]")],
        "\
let doc = <frame>
  <group>
  </group>
  <image src=\"b.png\" />
  <image
    src=\"a.png\"
    x={1}
  />
</frame>
",
    );
}

#[test]
fn setting_the_whole_list_edits_group_children_in_place() {
    // What a GUI does after reordering inside a group: it writes the whole
    // `layers` value back. The children are lined up one by one, so the
    // comments and the hand spacing survive.
    let StaticValue::List(mut layers) = get_static_value(DOC, "layers").unwrap() else {
        panic!("a list");
    };
    let StaticValue::Element { children, .. } = &mut layers[2] else {
        panic!("a group");
    };
    children.swap(0, 1);
    let want = StaticValue::List(layers);
    let got = apply(DOC, &[Goal::should_set_path("layers", want.clone())]);
    assert_eq!(get_static_value(&got, "layers").unwrap(), want);
    for kept in [
        "    // The left card.\n",
        "\n\n    // The right card, nudged by hand.\n",
        "<image   src=\"b.png\"  x={520} y={180} />",
        "<text x={1}>Né → ok</text>",
        "<text x={10}>Title — café</text>",
    ] {
        assert!(got.contains(kept), "lost {kept:?}:\n{got}");
    }
    // Changing one nested attribute through the whole value is one token.
    let StaticValue::List(mut layers) = get_static_value(DOC, "layers").unwrap() else {
        panic!("a list");
    };
    let StaticValue::Element { children, .. } = &mut layers[2] else {
        panic!("a group");
    };
    let StaticValue::Element { props, .. } = &mut children[1] else {
        panic!("an image");
    };
    props[1].1 = StaticValue::int(521);
    check(
        DOC,
        &[Goal::should_set_path("layers", StaticValue::List(layers.clone()))],
        &DOC.replace("x={520}", "x={521}"),
    );
    // Removing a child through the whole value takes only its line.
    let StaticValue::Element { children, .. } = &mut layers[2] else {
        panic!("a group");
    };
    children.remove(0);
    let got = apply(DOC, &[Goal::should_set_path("layers", StaticValue::List(layers))]);
    assert_eq!(
        got,
        DOC.replace("    <image src=\"a.png\" x={40} y={80} />\n", "")
            .replace("x={520}", "x={521}")
    );
}

#[test]
fn one_line_children_stay_on_one_line() {
    let source = "let g = <group><a x={1} /> <b /></group>\n";
    check(
        source,
        &[Goal::should_insert(
            "g/*[1]",
            StaticValue::element("c", [] as [(&str, StaticValue); 0], [] as [StaticValue; 0]),
        )],
        "let g = <group><a x={1} /> <c /> <b /></group>\n",
    );
    check(
        source,
        &[Goal::should_remove("g/a")],
        "let g = <group><b /></group>\n",
    );
    check(
        source,
        &[Goal::should_remove("g/b")],
        "let g = <group><a x={1} /></group>\n",
    );
    // Once `<b />` is out, `<a>` is alone and shows no spacing to copy.
    check(
        source,
        &[Goal::should_move("g/b", "g/*[0]")],
        "let g = <group><b /><a x={1} /></group>\n",
    );
}

#[test]
fn mixed_one_line_and_multi_line_children() {
    let source = "\
let g = <group>
  <a /> <b />
  <c
    x={1}
  />
</group>
";
    // `<c>` has its lines to itself: a new child after it gets its own line,
    // its attributes one per line like `<c>`'s.
    check(
        source,
        &[Goal::should_append("g", image("d.png", 2))],
        "\
let g = <group>
  <a /> <b />
  <c
    x={1}
  />
  <image
    src=\"d.png\"
    x={2}
  />
</group>
",
    );
    // `<b>` shares its line: a new child after it joins that line.
    check(
        source,
        &[Goal::should_insert("g/*[2]", image("d.png", 2))],
        "\
let g = <group>
  <a /> <b /> <image src=\"d.png\" x={2} />
  <c
    x={1}
  />
</group>
",
    );
    check(
        source,
        &[Goal::should_remove("g/c")],
        "let g = <group>\n  <a /> <b />\n</group>\n",
    );
}

#[test]
fn an_empty_element_gains_its_first_child() {
    check(
        "let g = <group name=\"x\" />\n",
        &[Goal::should_append("g", image("a.png", 1))],
        "let g = <group name=\"x\">\n  <image src=\"a.png\" x={1} />\n</group>\n",
    );
    check(
        "let g = <group></group>\n",
        &[Goal::should_insert("g/image[0]", image("a.png", 1))],
        "let g = <group>\n  <image src=\"a.png\" x={1} />\n</group>\n",
    );
    // A comment already inside stays, above the new child.
    check(
        "let g = <group>\n  // nothing yet\n</group>\n",
        &[Goal::should_append("g", image("a.png", 1))],
        "let g = <group>\n  // nothing yet\n  <image src=\"a.png\" x={1} />\n</group>\n",
    );
}

#[test]
fn comment_holes_and_line_breaks_are_not_children() {
    let source = "\
let t = <text>
  {/* the headline */}
  Big<br />
  <b weight={7} />
</text>
";
    // `*[0]` skips the `<br />`; `br[0]` names it.
    check(
        source,
        &[Goal::should_set_path("t/*[0].weight", 8)],
        &source.replace("{7}", "{8}"),
    );
    assert_eq!(
        literal_edit::set_path(source, "t/br[0].x", &StaticValue::int(1))
            .map(|s| s.contains("<br x={1} />")),
        Ok(true)
    );
}

#[test]
fn a_bad_path_says_which_step_failed_and_what_is_there() {
    let err = |path: &str| literal_edit::set_path(DOC, path, &StaticValue::int(1)).unwrap_err();
    let e = err("layers[2]/image[5].x");
    assert_eq!(e.kind, EditErrorKind::NotFound);
    assert!(
        e.message.contains("step `/image[5]`")
            && e.message.contains("the `<group>` element at `layers[2]` has 2 `<image>` children"),
        "{}",
        e.message
    );
    let e = err("layers[2]/image.x");
    assert!(e.message.contains("ambiguous") && e.message.contains("`image[0]`"), "{}", e.message);
    let e = err("layers[0]/rect[0]");
    assert!(e.message.contains("has no `<rect>` children"), "{}", e.message);
    let e = err("layers[2][0]");
    assert!(e.message.contains("`layers[2]/*[0]`"), "{}", e.message);
    let e = err("layers[2]/image[0].x/b");
    assert!(e.message.contains("is a number, not an element"), "{}", e.message);
    let e = err("layers[7]");
    assert!(e.message.contains("has no element 7"), "{}", e.message);
    let e = err("layers[2]/");
    assert_eq!(e.kind, EditErrorKind::Invalid);
    assert!(e.message.contains("expected a tag name"), "{}", e.message);
    // Inserting needs an index, and only an element is a child.
    let e = literal_edit::insert_path(DOC, "layers[2]/text", &image("c", 1), &Default::default())
        .unwrap_err();
    assert!(e.message.contains("needs an index"), "{}", e.message);
    let e = literal_edit::insert_path(DOC, "layers[2]/*[0]", &StaticValue::int(1), &Default::default())
        .unwrap_err();
    assert!(e.message.contains("only an element"), "{}", e.message);
    // The C bridge's reader finds the same values.
    let (_, stmts) = petal::rewrite::parse_ast(DOC).unwrap();
    let (name, segs) = petal::rewrite::parse_binding_path("layers[2]/image[1].x").unwrap();
    let span = petal::rewrite::find_binding_path(&stmts, &name, &segs).unwrap();
    let text: String = DOC
        .chars()
        .skip(span.start.offset as usize)
        .take((span.end.offset - span.start.offset) as usize)
        .collect();
    assert_eq!(text, "520");
}

// ── Review fixes (2026-11-11) ────────────────────────────────────────────

fn el(tag: &str, props: &[(&str, StaticValue)], children: Vec<StaticValue>) -> StaticValue {
    StaticValue::element(tag, props.iter().cloned(), children)
}

#[test]
fn an_empty_element_set_whole_keeps_the_comments_inside() {
    let src = "let g = <group>\n  // keep this note\n</group>\n";
    let want = "let g = <group>\n  // keep this note\n  <a />\n</group>\n";
    let a = el("a", &[], vec![]);
    let set = literal_edit::set_path(src, "g", &el("group", &[], vec![a.clone()])).unwrap();
    assert_eq!(set, want);
    assert_eq!(literal_edit::append_path(src, "g", &a).unwrap(), want);
}

#[test]
fn a_list_element_moved_into_an_element_keeps_its_trailing_comment() {
    let src = "let layers = [\n  <a />, // keep me\n  <g>\n    <b />\n  </g>,\n]\n";
    let got = literal_edit::move_path(src, "layers[0]", "layers[0]/*[1]").unwrap();
    assert_eq!(
        got,
        "let layers = [\n  <g>\n    <b />\n    // keep me\n    <a />\n  </g>,\n]\n"
    );
    // And back out again: the comment comes along.
    let back = literal_edit::move_path(&got, "layers[0]/a", "layers[0]").unwrap();
    assert!(back.contains("// keep me"), "{back}");
}

#[test]
fn a_move_to_the_same_place_spelled_another_way_changes_nothing() {
    let src = "let g = <group>\n  <image src=\"a\" />\n\n  // about b\n  <image src=\"b\" />\n  <text>t</text>\n</group>\n";
    for (from, to) in [
        ("g/image[1]", "g/*[1]"),
        ("g/image[1]", "g/image[1]"),
        ("g/*[0]", "g/image[0]"),
        ("g/text", "g/*[2]"),
    ] {
        assert_eq!(literal_edit::move_path(src, from, to).unwrap(), src, "{from} -> {to}");
    }
    let list = "let l = [\n  1,\n  // two\n  2,\n]\n";
    assert_eq!(literal_edit::move_path(list, "l[1]", "l[ 1]").unwrap(), list);
}

#[test]
fn a_whole_element_set_that_swaps_children_moves_their_comments_with_them() {
    let src = "let g = <group>\n  // A comment\n  <a n={1} />\n  // B comment\n  <b n={2} />\n</group>\n";
    let StaticValue::Element { tag, props, mut children } = get_static_value(src, "g").unwrap() else {
        panic!("an element");
    };
    children.swap(0, 1);
    let swapped = StaticValue::Element { tag, props, children };
    assert_eq!(
        literal_edit::set_path(src, "g", &swapped).unwrap(),
        "let g = <group>\n  // B comment\n  <b n={2} />\n  // A comment\n  <a n={1} />\n</group>\n"
    );
    // A list's elements do the same.
    let src = "let l = [\n  // A\n  { a: 1 },\n  // B\n  { b: 2 },\n]\n";
    let StaticValue::List(mut items) = get_static_value(src, "l").unwrap() else {
        panic!("a list");
    };
    items.swap(0, 1);
    assert_eq!(
        literal_edit::set_path(src, "l", &StaticValue::List(items)).unwrap(),
        "let l = [\n  // B\n  { b: 2 },\n  // A\n  { a: 1 },\n]\n"
    );
}

#[test]
fn a_child_in_a_parenthesised_hole_can_be_removed_moved_and_inserted_after() {
    let src = "let g = <group>\n  {<a />}\n  { <b x={1} /> }\n  {(<c />)}\n</group>\n";
    assert_eq!(
        literal_edit::remove_path(src, "g/c").unwrap(),
        "let g = <group>\n  {<a />}\n  { <b x={1} /> }\n</group>\n"
    );
    assert_eq!(
        literal_edit::move_path(src, "g/c", "g/*[0]").unwrap(),
        "let g = <group>\n  <c />\n  {<a />}\n  { <b x={1} /> }\n</group>\n"
    );
    assert_eq!(
        literal_edit::insert_path(src, "g/*[3]", &el("d", &[], vec![]), &Default::default())
            .unwrap(),
        "let g = <group>\n  {<a />}\n  { <b x={1} /> }\n  {(<c />)}\n  <d />\n</group>\n"
    );
}

#[test]
fn a_first_child_goes_after_the_text_before_it() {
    let src = "let g = <text>\n  Title\n  <b />\n</text>\n";
    assert_eq!(
        literal_edit::insert_path(src, "g/*[0]", &el("i", &[], vec![]), &Default::default())
            .unwrap(),
        "let g = <text>\n  Title\n  <i />\n  <b />\n</text>\n"
    );
    // Comments before the first child still stay above the new one.
    let src = "let g = <group>\n  // the layers\n  <b />\n</group>\n";
    assert_eq!(
        literal_edit::insert_path(src, "g/*[0]", &el("i", &[], vec![]), &Default::default())
            .unwrap(),
        "let g = <group>\n  <i />\n  // the layers\n  <b />\n</group>\n"
    );
}

#[test]
fn inserts_into_a_crlf_file_keep_crlf() {
    let src = "let g = <group>\r\n  <a />\r\n  <b />\r\n</group>\r\n";
    let got = literal_edit::insert_path(src, "g/*[1]", &el("c", &[], vec![]), &Default::default())
        .unwrap();
    assert_eq!(got, "let g = <group>\r\n  <a />\r\n  <c />\r\n  <b />\r\n</group>\r\n");
    let src = "let l = [\r\n  1,\r\n  2,\r\n]\r\n";
    let got = literal_edit::insert_path(src, "l[1]", &StaticValue::int(9), &Default::default())
        .unwrap();
    assert_eq!(got, "let l = [\r\n  1,\r\n  9,\r\n  2,\r\n]\r\n");
}

#[test]
fn a_note_after_a_tag_survives_a_whole_set_and_goes_with_its_element() {
    let src = "let g = <group>\n  <a x={1} /> // first\n  <b />\n</group>\n";
    let StaticValue::Element { tag, props, mut children } = get_static_value(src, "g").unwrap() else {
        panic!("an element");
    };
    let StaticValue::Element { props: b_props, .. } = &mut children[2] else {
        panic!("<b>: {children:?}");
    };
    b_props.push(("y".into(), StaticValue::int(2)));
    assert_eq!(
        literal_edit::set_path(src, "g", &StaticValue::Element { tag, props, children }).unwrap(),
        "let g = <group>\n  <a x={1} /> // first\n  <b y={2} />\n</group>\n"
    );
    assert_eq!(
        literal_edit::remove_path(src, "g/a").unwrap(),
        "let g = <group>\n  <b />\n</group>\n"
    );
}

#[test]
fn an_index_is_digits_only() {
    let src = "let g = <group>\n  <a />\n  <a />\n</group>\n";
    for path in ["g/a[+1]", "g/a[-1]", "g/*[+0]"] {
        let e = literal_edit::set_path(src, path, &StaticValue::int(1)).unwrap_err();
        assert_eq!(e.kind, EditErrorKind::Invalid, "{path}");
        assert!(e.message.contains("is not an edit path"), "{path}: {}", e.message);
    }
}
