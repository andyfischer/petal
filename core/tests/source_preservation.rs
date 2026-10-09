//! Style-preserving edits, as a table of before / edit / after cases
//! (docs/source-preservation.md). Each case is checked three ways: the edit
//! gives exactly the expected text, the result still parses, and applying the
//! same goals again changes nothing unless the goal is an insert (which adds
//! another element by nature).

use petal::goal_based_editing::{Goal, modify_source_with_goals};
use petal::static_value::{StaticValue, get_static_value};

fn rec(fields: &[(&str, StaticValue)]) -> StaticValue {
    StaticValue::record(fields.iter().cloned())
}

fn effect(name: &str, amount: f64) -> StaticValue {
    rec(&[("effect", name.into()), ("amount", amount.into())])
}

fn list(items: &[StaticValue]) -> StaticValue {
    StaticValue::list(items.iter().cloned())
}

fn hex(text: &str) -> StaticValue {
    StaticValue::color_hex(text).unwrap()
}

fn apply(source: &str, goals: &[Goal]) -> String {
    modify_source_with_goals(source, goals)
        .unwrap_or_else(|e| panic!("edit failed: {e}\n--- source ---\n{source}"))
}

struct Case {
    name: &'static str,
    before: &'static str,
    goals: Vec<Goal>,
    after: &'static str,
}

fn case(name: &'static str, before: &'static str, goals: Vec<Goal>, after: &'static str) -> Case {
    Case {
        name,
        before,
        goals,
        after,
    }
}

/// The Neon-style config the editor edits: a multi-line record with trailing
/// commas, comments on and between lines, aligned values, and a list of
/// compact one-line records.
const POST: &str = "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
";

fn cases() -> Vec<Case> {
    let set = Goal::should_set_path::<&str, StaticValue>;
    let insert = Goal::should_insert::<&str, StaticValue>;
    let remove = |path: &str| Goal::should_remove(path);
    vec![
        // ── Scalars keep their neighbours' trivia ────────────────────────
        case(
            "a scalar in a spaced one-line record",
            "let editor = { line_numbers: true,  tab_width: 4 } // prefs\n",
            vec![set("editor.tab_width", 8.into())],
            "let editor = { line_numbers: true,  tab_width: 8 } // prefs\n",
        ),
        case(
            "a scalar in a compact one-line record",
            "let editor = {line_numbers:true,tab_width:4}\n",
            vec![set("editor.line_numbers", false.into())],
            "let editor = {line_numbers:false,tab_width:4}\n",
        ),
        case(
            "a scalar on a commented, aligned line",
            POST,
            vec![
                set("POST.exposure", 1.2.into()),
                set("POST.bloom", 0.5.into()),
            ],
            "\
// The night look.
export config let POST = {
  exposure: 1.2,   // the night is bright
  bloom:    0.5,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
",
        ),
        case(
            "a scalar in a nested list of records",
            POST,
            vec![set("POST.effects[1].amount", 0.45.into())],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.45},
  ],
}
",
        ),
        // ── Numbers ──────────────────────────────────────────────────────
        case(
            "an int stays an int, a float a float",
            "let n = 3\nlet f = 3.0\n",
            vec![set("n", 4.into()), set("f", 4.0.into())],
            "let n = 4\nlet f = 4.0\n",
        ),
        case(
            "the value's type decides int against float",
            "let n = 3\n",
            vec![set("n", 3.5.into())],
            "let n = 3.5\n",
        ),
        case(
            "negative numbers",
            "let dir = vec3(0.35, -1.0, 0.55)\nlet lo = -3\n",
            vec![
                set("dir[1]", (-0.5).into()),
                set("dir[2]", (-0.25).into()),
                set("lo", 3.into()),
            ],
            "let dir = vec3(0.35, -0.5, -0.25)\nlet lo = 3\n",
        ),
        case(
            "a zero-padded float keeps its decimals",
            "let drag  = 0.020000\nlet speed = 3.50\n",
            vec![set("drag", 0.03.into()), set("speed", 4.0.into())],
            "let drag  = 0.030000\nlet speed = 4.00\n",
        ),
        case(
            "padding gives way when the value needs more digits",
            "let speed = 3.50\nlet gain = 1.45\n",
            vec![set("speed", 3.125.into()), set("gain", 1.2.into())],
            "let speed = 3.125\nlet gain = 1.2\n",
        ),
        // ── Colors ───────────────────────────────────────────────────────
        case(
            "a color is written back as a color",
            POST,
            vec![set("POST.tint", hex("#29d9ff"))],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #29d9ff,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
",
        ),
        case(
            "a color keeps its case and its short or long form",
            "let a = #FF2E88\nlet b = #f80\nlet c = #F80\nlet d = #ff2e88cc\n",
            vec![
                set("a", hex("#29d9ff")),
                set("b", hex("#00ff88")),
                set("c", hex("#29d9ff")),
                set("d", StaticValue::color_alpha(1, 2, 3, 4)),
            ],
            "let a = #29D9FF\nlet b = #0f8\nlet c = #29D9FF\nlet d = #01020304\n",
        ),
        case(
            "a new color beside an uppercase one is uppercase",
            "let palette = [#FF2E88, #101018]\n",
            vec![insert("palette[2]", hex("#29d9ff"))],
            "let palette = [#FF2E88, #101018, #29d9ff]\n",
        ),
        // ── Inserting list elements ──────────────────────────────────────
        case(
            "insert at the start, middle and end of a one-line list",
            "let xs = [1, 2, 3] // counts\n",
            vec![
                insert("xs[0]", 0.into()),
                insert("xs[2]", 15.into()),
                insert("xs[5]", 4.into()),
            ],
            "let xs = [0, 1, 15, 2, 3, 4] // counts\n",
        ),
        case(
            "insert into a tight list keeps it tight",
            "let xs = [1,2,3]\n",
            vec![insert("xs[1]", 9.into()), Goal::should_append("xs", 4)],
            "let xs = [1,9,2,3,4]\n",
        ),
        case(
            "insert at the start of a multi-line list",
            POST,
            vec![insert("POST.effects[0]", effect("crt", 0.2))],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"crt\", amount: 0.2},
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
",
        ),
        case(
            "insert in the middle leaves the comment with its element",
            POST,
            vec![insert("POST.effects[1]", effect("crt", 0.2))],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    {effect: \"crt\", amount: 0.2},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
",
        ),
        case(
            "append to a multi-line list, in the siblings' style",
            POST,
            vec![Goal::should_append(
                "POST.effects",
                rec(&[
                    ("effect", "halftone".into()),
                    ("amount", 0.7.into()),
                    ("ink", hex("#101018")),
                ]),
            )],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
    {effect: \"halftone\", amount: 0.7, ink: #101018},
  ],
}
",
        ),
        case(
            "append where the last element has no trailing comma",
            "let xs = [\n    \"a\",\n    \"b\" // last\n]\n",
            vec![Goal::should_append("xs", "c")],
            "let xs = [\n    \"a\",\n    \"b\", // last\n    \"c\"\n]\n",
        ),
        case(
            "insert into an empty list: scalars inline, records one per line",
            "let xs = []\nlet fx = {\n    effects: [],\n}\n",
            vec![
                Goal::should_append("xs", 1),
                Goal::should_append("xs", 2),
                Goal::should_append("fx.effects", effect("crt", 0.2)),
            ],
            "let xs = [1, 2]\nlet fx = {\n    effects: [\n        { effect: \"crt\", amount: 0.2 },\n    ],\n}\n",
        ),
        case(
            "an empty list open across lines is filled one per line",
            "let xs = [\n  // nothing yet\n]\n",
            vec![Goal::should_append("xs", 1)],
            "let xs = [\n  1,\n  // nothing yet\n]\n",
        ),
        // ── Inserting record fields ──────────────────────────────────────
        case(
            "a new field in a spaced one-line record",
            "let editor = { line_numbers: true, tab_width: 4 }\n",
            vec![insert("editor.wrap", false.into())],
            "let editor = { line_numbers: true, tab_width: 4, wrap: false }\n",
        ),
        case(
            "a new field in a compact one-line record",
            "let editor = {line_numbers:true,tab_width:4}\n",
            vec![insert("editor.wrap", false.into()).before("tab_width")],
            "let editor = {line_numbers:true,wrap:false,tab_width:4}\n",
        ),
        case(
            "a new field in a multi-line record, placed after a sibling",
            POST,
            vec![insert("POST.vignette", 0.3.into()).after("exposure")],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  vignette: 0.3,
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
",
        ),
        case(
            "a new field lines up with a column of values",
            "let gen = {\n  half:   800.0,  // metres\n  lane_w: 3.4,\n}\n",
            vec![
                insert("gen.n", 12.into()),
                insert("gen.block_size", 40.0.into()).before("half"),
            ],
            "let gen = {\n  block_size: 40.0,\n  half:   800.0,  // metres\n  lane_w: 3.4,\n  n:      12,\n}\n",
        ),
        case(
            "setting a field that is not there adds it",
            "let editor = {}\nlet other = { }\n",
            vec![
                set("editor.wrap", true.into()),
                set("other.wrap", true.into()),
            ],
            "let editor = { wrap: true }\nlet other = { wrap: true }\n",
        ),
        case(
            "a compact file gets compact new records",
            "let a = {x: 1}\nlet b = {}\n",
            vec![set("b.y", 2.into())],
            "let a = {x: 1}\nlet b = {y: 2}\n",
        ),
        // ── Removing ─────────────────────────────────────────────────────
        case(
            "remove the first, a middle and the last of a one-line list",
            "let xs = [1, 2, 3, 4, 5]\n",
            vec![remove("xs[0]"), remove("xs[1]"), remove("xs[2]")],
            "let xs = [2, 4]\n",
        ),
        case(
            "remove the only element, keeping the brackets' padding",
            "let xs = [1]\nlet r = { a: 1 }\nlet t = [ 1, ]\n",
            vec![remove("xs[0]"), remove("r.a"), remove("t[0]")],
            "let xs = []\nlet r = { }\nlet t = [ ]\n",
        ),
        case(
            "remove the first of a multi-line list",
            POST,
            vec![remove("POST.effects[0]")],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
",
        ),
        case(
            "remove the last of a multi-line list; comments on other lines stay",
            POST,
            vec![remove("POST.effects[1]")],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
  ],
}
",
        ),
        case(
            "remove the last where there was no trailing comma",
            "let xs = [\n    \"a\",\n    \"b\",\n    \"c\"\n]\n",
            vec![remove("xs[2]")],
            "let xs = [\n    \"a\",\n    \"b\"\n]\n",
        ),
        case(
            "remove the only element of a multi-line list leaves it open",
            "let xs = [\n    \"a\",\n]\n",
            vec![remove("xs[0]")],
            "let xs = [\n]\n",
        ),
        case(
            "remove a field with a trailing comment, and one that is absent",
            POST,
            vec![remove("POST.exposure"), remove("POST.nope")],
            "\
// The night look.
export config let POST = {
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
",
        ),
        case(
            "remove a field of a one-line record",
            "let r = {a: 1, b: 2, c: 3}\n",
            vec![remove("r.b"), remove("r.c")],
            "let r = {a: 1}\n",
        ),
        // ── Whole values: only what differs is written ───────────────────
        case(
            "a whole record written back with one field changed",
            POST,
            vec![Goal::should_set_value(
                "POST",
                rec(&[
                    ("exposure", 1.45.into()),
                    ("bloom", 0.6.into()),
                    ("tint", hex("#ff2e88")),
                    (
                        "effects",
                        list(&[
                            rec(&[
                                ("effect", "lens_dirt".into()),
                                ("amount", 0.5.into()),
                                ("scale", 3.0.into()),
                            ]),
                            effect("grain", 0.3),
                        ]),
                    ),
                ]),
            )],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.6,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"lens_dirt\", amount: 0.5, scale: 3.0},
    // grain goes last
    {effect: \"grain\", amount: 0.30},
  ],
}
",
        ),
        case(
            "a list whose shape changes is edited in place",
            POST,
            vec![set(
                "POST.effects",
                list(&[
                    effect("crt", 0.2),
                    effect("grain", 0.35),
                    effect("halftone", 0.7),
                ]),
            )],
            "\
// The night look.
export config let POST = {
  exposure: 1.45,   // the night is bright
  bloom:    0.42,
  tint: #ff2e88,
  // the stack, in the order it runs
  effects: [
    {effect: \"crt\", amount: 0.2},
    // grain goes last
    {effect: \"grain\", amount: 0.35},
    {effect: \"halftone\", amount: 0.7},
  ],
}
",
        ),
        case(
            "a reordered list moves its elements' own text",
            "let fx = [\n  {effect: \"a\", amount: 0.50},   // first\n  {effect: \"b\",   amount: 1.0},\n  {effect: \"c\", amount: 2.0},\n]\n",
            vec![set(
                "fx",
                list(&[effect("b", 1.0), effect("a", 0.5), effect("c", 2.0)]),
            )],
            "let fx = [\n  {effect: \"b\",   amount: 1.0},\n  {effect: \"a\", amount: 0.50},   // first\n  {effect: \"c\", amount: 2.0},\n]\n",
        ),
        case(
            "a record that gains and loses fields keeps the rest",
            "let r = {\n    a: 1,   // one\n    b: 2,\n    c: 3,   // three\n}\n",
            vec![set(
                "r",
                rec(&[("a", 1.into()), ("c", 30.into()), ("d", 4.into())]),
            )],
            "let r = {\n    a: 1,   // one\n    c: 30,   // three\n    d: 4,\n}\n",
        ),
        case(
            "a call's arguments are a list too",
            "let dir = vec3(0.35,  -1.0, 0.55)   // sun\n",
            vec![set("dir", StaticValue::call("vec3", [0.35, -1.0, 0.6]))],
            "let dir = vec3(0.35,  -1.0, 0.6)   // sun\n",
        ),
        // ── Elements that span lines ─────────────────────────────────────
        case(
            "a new record beside multi-line records is multi-line",
            "let fx = [\n    {\n        effect: \"grain\",\n        amount: 0.3, // subtle\n    },\n]\n",
            vec![Goal::should_append("fx", effect("crt", 0.2))],
            "let fx = [\n    {\n        effect: \"grain\",\n        amount: 0.3, // subtle\n    },\n    {\n        effect: \"crt\",\n        amount: 0.2,\n    },\n]\n",
        ),
        case(
            "removing a multi-line element takes all its lines",
            "let fx = [\n    {\n        effect: \"grain\",\n    },\n    // kept\n    {\n        effect: \"crt\",\n    }, // gone\n]\n",
            vec![remove("fx[1]")],
            "let fx = [\n    {\n        effect: \"grain\",\n    },\n    // kept\n]\n",
        ),
        case(
            "a composite value for a new field is indented from its line",
            "let cfg = {\n    name: \"neon\",\n}\n",
            vec![insert(
                "cfg.effects",
                list(&[effect("crt", 0.2), effect("grain", 0.3)]),
            )],
            "let cfg = {\n    name: \"neon\",\n    effects: [\n        { effect: \"crt\", amount: 0.2 },\n        { effect: \"grain\", amount: 0.3 },\n    ],\n}\n",
        ),
        // ── Paths through calls, spreads and awkward text ────────────────
        case(
            "a path through a call's arguments, and inserting one",
            "let sun = light(vec3(0.3, -1.0, 0.5), { color: #fff })\n",
            vec![
                set("sun[0][2]", 0.75.into()),
                set("sun[1].color", hex("#ffeedd")),
                insert("sun[0][3]", 1.0.into()),
            ],
            "let sun = light(vec3(0.3, -1.0, 0.75, 1.0), { color: #fed })\n",
        ),
        case(
            "a record with a spread is still edited field by field by path",
            "let base = { a: 1 }\nlet r = { ...base, b: 2 }\n",
            vec![set("r.b", 3.into()), insert("r.c", 4.into())],
            "let base = { a: 1 }\nlet r = { ...base, b: 3, c: 4 }\n",
        ),
        case(
            "strings holding commas, brackets and comment markers",
            "let xs = [\n  \"a, b\", // one, two\n  \"// not a comment ]\",\n  \"c\",\n]\n",
            vec![remove("xs[1]"), set("xs[1]", "d//e".into())],
            "let xs = [\n  \"a, b\", // one, two\n  \"d//e\",\n]\n",
        ),
        case(
            "a comment on the opening line stays above a new first element",
            "let xs = [ // counts\n  1,\n]\n",
            vec![insert("xs[0]", 0.into())],
            "let xs = [ // counts\n  0,\n  1,\n]\n",
        ),
        case(
            "a wrapped list keeps its rows",
            "let m = [1, 2, 3,\n         4, 5, 6]\n",
            vec![remove("m[2]"), remove("m[2]"), insert("m[4]", 7.into())],
            "let m = [1, 2,\n         5, 6, 7]\n",
        ),
        case(
            "the last binding of a name is the one edited",
            "let v = [1]\nlet v = [1] // this one\n",
            vec![Goal::should_append("v", 2)],
            "let v = [1]\nlet v = [1, 2] // this one\n",
        ),
        case(
            "a value of another shape replaces the old one whole",
            "let a = { x: 1 } // was a record\nlet b = 3\n",
            vec![set("a", 5.into()), set("b", list(&[1.into(), 2.into()]))],
            "let a = 5 // was a record\nlet b = [1, 2]\n",
        ),
    ]
}

#[test]
fn table_of_edits() {
    for case in cases() {
        let out = apply(case.before, &case.goals);
        assert_eq!(out, case.after, "case: {}", case.name);
        assert!(
            petal::rewrite::parse_ast(&out).is_ok(),
            "case `{}` left source that does not parse",
            case.name
        );
    }
}

#[test]
fn set_and_remove_goals_are_idempotent() {
    // A goal that already holds writes nothing — so applying the goals of any
    // case a second time leaves its result byte-identical. Inserting into a
    // list is the exception: it is an action, not a state.
    for case in cases() {
        let repeats = case.goals.iter().all(|goal| match goal {
            Goal::ShouldAppend { .. } => false,
            Goal::ShouldInsert { path, .. } => !path.ends_with(']'),
            // A second `remove xs[0]` would remove the next element.
            Goal::ShouldRemove { path } => !path.ends_with(']'),
            _ => true,
        });
        if repeats {
            let once = apply(case.before, &case.goals);
            assert_eq!(apply(&once, &case.goals), once, "case: {}", case.name);
        }
    }
}

/// Every top-level binding of `source`, written back as the value it reads.
fn write_back_everything(source: &str) -> String {
    let goals: Vec<Goal> = petal::static_value::static_values(source)
        .unwrap()
        .into_iter()
        .map(|(name, value)| Goal::should_set_value(name, value))
        .collect();
    apply(source, &goals)
}

#[test]
fn a_no_op_edit_leaves_the_file_byte_identical() {
    // Reading every value and writing it straight back must not move a byte,
    // however the file is formatted — in every `before` and `after` above.
    for case in cases() {
        for text in [case.before, case.after] {
            assert_eq!(write_back_everything(text), text, "case: {}", case.name);
        }
    }
    // And path by path.
    let goals = [
        Goal::should_set_path("POST.exposure", 1.45),
        Goal::should_set_path("POST.tint", hex("#FF2E88")),
        Goal::should_set_path("POST.effects[1].amount", 0.3),
        Goal::should_set_path("POST.effects[0]", {
            rec(&[
                ("effect", "lens_dirt".into()),
                ("amount", 0.5.into()),
                ("scale", 3.0.into()),
            ])
        }),
        Goal::should_remove("POST.absent"),
    ];
    assert_eq!(apply(POST, &goals), POST);
}

#[test]
fn an_edit_and_its_inverse_restore_the_original_bytes() {
    let set = Goal::should_set_path::<&str, StaticValue>;
    let insert = Goal::should_insert::<&str, StaticValue>;
    let remove = |path: &str| Goal::should_remove(path);
    let lens = rec(&[
        ("effect", "lens_dirt".into()),
        ("amount", 0.5.into()),
        ("scale", 3.0.into()),
    ]);
    let compact = "let r = {a:1,b:2}\nlet xs = [1,2,3]\nlet one = [1]\n";
    let spaced = "let r = { a: 1, b: 2 }\nlet xs = [ 1, 2, 3 ]\nlet lone = { a: 1 }\n";
    let lines = "let xs = [\n    1,\n    2,\n    3\n]\nlet ys = [\n\t1,\n\t2,\n]\n";
    let round_trips: Vec<(&str, Vec<Goal>, Vec<Goal>)> = vec![
        // Scalars, there and back.
        (
            POST,
            vec![
                set("POST.exposure", 2.0.into()),
                set("POST.tint", hex("#000")),
            ],
            vec![
                set("POST.exposure", 1.45.into()),
                set("POST.tint", hex("#ff2e88")),
            ],
        ),
        (
            "let drag = 0.020000 // m/s\n",
            vec![set("drag", 0.5.into())],
            vec![set("drag", 0.02.into())],
        ),
        // Insert then remove, at the start, middle and end.
        (
            POST,
            vec![insert("POST.effects[0]", effect("crt", 0.2))],
            vec![remove("POST.effects[0]")],
        ),
        (
            POST,
            vec![insert("POST.effects[1]", effect("crt", 0.2))],
            vec![remove("POST.effects[1]")],
        ),
        (
            POST,
            vec![insert("POST.effects[2]", effect("crt", 0.2))],
            vec![remove("POST.effects[2]")],
        ),
        (
            POST,
            vec![insert("POST.gamma", 2.2.into())],
            vec![remove("POST.gamma")],
        ),
        (
            compact,
            vec![insert("xs[0]", 0.into())],
            vec![remove("xs[0]")],
        ),
        (
            compact,
            vec![insert("xs[3]", 4.into())],
            vec![remove("xs[3]")],
        ),
        (compact, vec![insert("r.c", 3.into())], vec![remove("r.c")]),
        (
            spaced,
            vec![insert("xs[1]", 9.into())],
            vec![remove("xs[1]")],
        ),
        (
            spaced,
            vec![insert("r.c", 3.into()).before("a")],
            vec![remove("r.c")],
        ),
        (
            lines,
            vec![insert("xs[3]", 4.into())],
            vec![remove("xs[3]")],
        ),
        (
            lines,
            vec![insert("ys[0]", 0.into())],
            vec![remove("ys[0]")],
        ),
        (
            lines,
            vec![insert("ys[2]", 3.into())],
            vec![remove("ys[2]")],
        ),
        // Remove then insert: first, last, only.
        (
            compact,
            vec![remove("xs[0]")],
            vec![insert("xs[0]", 1.into())],
        ),
        (
            compact,
            vec![remove("xs[2]")],
            vec![insert("xs[2]", 3.into())],
        ),
        (compact, vec![remove("r.b")], vec![insert("r.b", 2.into())]),
        (
            spaced,
            vec![remove("xs[0]")],
            vec![insert("xs[0]", 1.into())],
        ),
        (
            spaced,
            vec![remove("xs[2]")],
            vec![insert("xs[2]", 3.into())],
        ),
        (
            compact,
            vec![remove("one[0]")],
            vec![insert("one[0]", 1.into())],
        ),
        (
            spaced,
            vec![remove("lone.a")],
            vec![insert("lone.a", 1.into())],
        ),
        (
            lines,
            vec![remove("xs[0]")],
            vec![insert("xs[0]", 1.into())],
        ),
        (
            lines,
            vec![remove("xs[2]")],
            vec![insert("xs[2]", 3.into())],
        ),
        (
            lines,
            vec![remove("ys[1]")],
            vec![insert("ys[1]", 2.into())],
        ),
        (
            // Emptied, the list still says it is open across lines; the tab
            // comes from the rest of the file.
            "let zs = [\n\t9,\n]\nlet ys = [\n\t1,\n]\n",
            vec![remove("ys[0]")],
            vec![insert("ys[0]", 1.into())],
        ),
        (
            POST,
            vec![remove("POST.effects[0]")],
            vec![insert("POST.effects[0]", lens.clone())],
        ),
        (
            POST,
            vec![remove("POST.tint")],
            vec![insert("POST.tint", hex("#ff2e88")).after("bloom")],
        ),
        (
            "let gen = {\n  half:   800.0,  // metres\n  lane_w: 3.4,\n  n:      12,\n}\n",
            vec![remove("gen.n")],
            vec![insert("gen.n", 12.into())],
        ),
        // A whole-value write and the write of the value it replaced.
        (
            compact,
            vec![set("xs", list(&[3.into(), 1.into()]))],
            vec![set("xs", list(&[1.into(), 2.into(), 3.into()]))],
        ),
    ];
    for (source, edit, inverse) in round_trips {
        let edited = apply(source, &edit);
        assert_ne!(edited, source, "the edit {edit:?} should change something");
        assert_eq!(
            apply(&edited, &inverse),
            source,
            "edit {edit:?} then {inverse:?}\n--- edited ---\n{edited}"
        );
    }
}

#[test]
fn what_an_edit_writes_is_what_a_read_returns() {
    let value = rec(&[
        ("exposure", 0.9.into()),
        ("tint", hex("#0af")),
        ("effects", list(&[effect("crt", 0.2)])),
        ("dir", StaticValue::call("vec3", [0.0, -1.0, 0.0])),
        ("on", true.into()),
    ]);
    let out = apply(POST, &[Goal::should_set_value("POST", value.clone())]);
    assert_eq!(get_static_value(&out, "POST").unwrap(), value, "{out}");
    assert!(out.starts_with("// The night look.\nexport config let POST = {\n"));
}

#[test]
fn edits_that_cannot_be_made_are_errors_and_change_nothing() {
    let fails = |goal: Goal| modify_source_with_goals(POST, &[goal]).unwrap_err().message;
    assert!(fails(Goal::should_set_path("NOPE.x", 1)).contains("no top-level binding"));
    assert!(fails(Goal::should_set_path("POST.effects[7].amount", 1)).contains("names no literal"));
    assert!(fails(Goal::should_set_path("POST..x", 1)).contains("not a binding path"));
    // A color is one token: there is nothing inside it to name.
    assert!(fails(Goal::should_set_path("POST.tint.r", 1)).contains("names no literal"));
    assert!(fails(Goal::should_insert("POST.effects[9]", 1)).contains("past the end"));
    assert!(fails(Goal::should_insert("POST.exposure[0]", 1)).contains("cannot insert"));
    assert!(fails(Goal::should_remove("POST.effects[2]")).contains("names no literal"));
    assert!(fails(Goal::should_remove("POST")).contains("whole binding"));
    assert!(fails(Goal::should_append("POST.exposure", 1)).contains("not a list"));
    // A key that is not an identifier would break the file: refused.
    assert!(
        fails(Goal::should_insert(
            "POST.effects[0]",
            rec(&[("bad key", 1.into())])
        ))
        .contains("does not parse")
    );
}

#[test]
fn multibyte_source_survives() {
    let src = "// café ☕\nlet xs = [\n  \"α\", // alpha\n  \"β\",\n]\n";
    let out = apply(
        src,
        &[
            Goal::should_set_path("xs[1]", "γ"),
            Goal::should_insert("xs[1]", "δ"),
        ],
    );
    assert_eq!(
        out,
        "// café ☕\nlet xs = [\n  \"α\", // alpha\n  \"δ\",\n  \"γ\",\n]\n"
    );
}

/// A small deterministic generator of values, for the property test below.
struct Gen(u64);

impl Gen {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }

    fn value(&mut self, depth: u32) -> StaticValue {
        let kinds = if depth == 0 { 6 } else { 9 };
        match self.next(kinds) {
            0 => StaticValue::int(self.next(7) as i64 - 3),
            1 => StaticValue::float((self.next(41) as f64 - 20.0) / 8.0),
            2 => StaticValue::str(["a", "grain", "x y", "q\"{"][self.next(4) as usize]),
            3 => StaticValue::bool(self.next(2) == 0),
            4 => StaticValue::nil(),
            5 => {
                let c = |g: &mut Gen| [0x00, 0x11, 0x2e, 0xff][g.next(4) as usize];
                match self.next(2) {
                    0 => StaticValue::color(c(self), c(self), c(self)),
                    _ => StaticValue::color_alpha(c(self), c(self), c(self), c(self)),
                }
            }
            6 => {
                let n = self.next(4);
                StaticValue::list((0..n).map(|_| self.value(depth - 1)).collect::<Vec<_>>())
            }
            7 => {
                let keys = ["effect", "amount", "scale", "ink", "on"];
                let first = self.next(3) as usize;
                let n = self.next(4) as usize;
                StaticValue::record(
                    keys.iter()
                        .cycle()
                        .skip(first)
                        .take(n)
                        .map(|k| (*k, self.value(depth - 1)))
                        .collect::<Vec<_>>(),
                )
            }
            _ => {
                let n = self.next(3);
                StaticValue::call(
                    ["vec3", "rgb"][self.next(2) as usize],
                    (0..n).map(|_| self.value(depth - 1)).collect::<Vec<_>>(),
                )
            }
        }
    }
}

#[test]
fn any_value_written_over_any_other_reads_back() {
    // Whatever the old text and the new value, the edit must leave source that
    // parses and reads as the new value; writing it again must change nothing;
    // and everything outside the binding must be untouched.
    // (what stands before the binding, the binding, what stands after it)
    let layouts = [
        (
            "// head\n",
            "let v = 0 // tail\n",
            "let after = [1,\n  2]\n",
        ),
        (
            "",
            "let v = {effect: \"grain\", amount: 0.30, ink: #FFF}\n",
            "",
        ),
        (
            "",
            "let v = [\n    { effect: \"a\", amount: 1.0 },  // one\n    // two\n    { effect: \"b\", on: true }\n]\n",
            "",
        ),
        (
            "let before = {x: 1}\n",
            "let v = {\n\tscale: [ 1, 2 , 3 ],\n\tink:   vec3(0.5,-1.0, 2.0),\n\ton: nil,\n}\n",
            "let after = 1\n",
        ),
        (
            "",
            "let v = [[], {}, [\n], { },\n  rgb(), [1,], {a: 1,},]\n",
            "// end\n",
        ),
    ];
    let mut g = Gen(7);
    for (head, body, tail) in layouts {
        let mut text = format!("{head}{body}{tail}");
        for round in 0..60 {
            let value = g.value(3);
            let out =
                modify_source_with_goals(&text, &[Goal::should_set_value("v", value.clone())])
                    .unwrap_or_else(|e| {
                        panic!("round {round}: {e}\n--- value ---\n{value:?}\n--- text ---\n{text}")
                    });
            assert_eq!(
                get_static_value(&out, "v").as_ref(),
                Ok(&value),
                "round {round}\n--- before ---\n{text}\n--- after ---\n{out}"
            );
            assert_eq!(apply(&out, &[Goal::should_set_value("v", value)]), out);
            assert!(out.starts_with(head) && out.ends_with(tail), "{out}");
            text = out;
        }
    }
}
