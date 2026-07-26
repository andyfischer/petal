//! Goal-based source editing — declarative, formatting-preserving edits.
//!
//! Instead of imperative "replace this span with that text" calls, a caller
//! describes **goals**: properties the edited source should satisfy. A goal is
//! order-independent in intent ("there should be a call to `set_color_scheme`
//! with these arguments", "`font_size` should be 14") and leaves *how* to
//! achieve it — insert a new statement, or update an existing one in place — to
//! this module. [`modify_source_with_goals`] applies a list of goals to a source
//! string and returns the rewritten source.
//!
//! This is more expressive than a single-purpose rewrite helper: goals compose
//! (apply several in one pass), and the [`Goal`] enum is the extension point for
//! richer intents later (ensure an import, remove a call, set a field on a
//! record literal, …).
//!
//! Values are **structured** ([`StaticValue`]), not pre-rendered source: the
//! caller passes `"dracula"` / `5` / `true` and this module renders each into a
//! valid Petal literal (strings are quoted and escaped, so interpolation `{`,
//! quotes, and backslashes can never leak). Composite values — nested calls
//! ([`StaticValue::call`]), lists ([`StaticValue::list`]), records
//! ([`StaticValue::record`]) — let an embedder express whole declarative trees,
//! e.g. Garden's `layout(row([editor("a.rs")], [1.0]))`; a list of composite
//! elements is pretty-printed one element per line so the generated source reads
//! like hand-written config. Every value is structured, so the rendered source
//! is always well-formed — there is no verbatim/raw-source escape hatch.
//!
//! [`StaticValue`] is the same type [`crate::static_value::get_static_value`]
//! returns when *reading* a config file, so a value round-trips: read it, adjust
//! it, write it back.
//!
//! Edits go through the lossless CST primitives in [`crate::rewrite`]
//! ([`parse_ast`], [`find_call`], [`find_binding`], [`splice_node`], [`splice`]),
//! so comments and surrounding layout survive and the caller is not required to
//! match any particular existing formatting.
//!
//! ```ignore
//! use petal::goal_based_editing::{modify_source_with_goals, Goal};
//!
//! // Ensure the config selects the "dracula" scheme and a 14pt font, whatever
//! // it selects now. The &str is auto-wrapped and rendered as "dracula".
//! let goals = [
//!     Goal::should_call("set_color_scheme", ["dracula"]),
//!     Goal::should_set_value("font_size", 14),
//! ];
//! let updated = modify_source_with_goals(&source, &goals)?;
//! ```

use crate::rewrite::{find_binding, find_call, parse_ast, splice, splice_node};
use crate::static_value::render_call_at;

pub use crate::static_value::StaticValue;

/// Why a goal batch could not be applied — the source didn't parse, or the
/// rewrite machinery rejected an edit. A distinct type (rather than a bare
/// `String`) so the result of [`modify_source_with_goals`] reads unambiguously:
/// `Ok` is the rewritten source, `Err` is this failure. Wraps a human-readable
/// message; `Display` recovers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalError {
    pub message: String,
}

impl std::fmt::Display for GoalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GoalError {}

impl From<String> for GoalError {
    fn from(message: String) -> Self {
        GoalError { message }
    }
}

/// A declarative editing goal: a property the rewritten source should satisfy.
///
/// Extend this enum (and [`apply_goal`]) with new intents as they are needed;
/// [`modify_source_with_goals`] applies each goal in turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Goal {
    /// The source should contain a top-level call `function(params...)`.
    ///
    /// If a top-level statement-position call to `function` already exists, its
    /// argument list is replaced with `params` (the rest of the call — and the
    /// rest of the file — is left untouched). If no such call exists, the call
    /// is appended as a new top-level statement.
    ShouldCall {
        function: String,
        params: Vec<StaticValue>,
    },
    /// Reading `name` out of the edited source should yield `value` — the
    /// write half of Petal-as-a-config-format.
    ///
    /// The **last** top-level binding of `name` is the one that decides the
    /// program's value, so that is the one edited: its right-hand side is
    /// replaced with `value`, whatever it was before (a literal, a call, or a
    /// whole `if` expression collapse to the literal). If `name` isn't bound at
    /// top level, `let name = value` is appended.
    ShouldSetValue { name: String, value: StaticValue },
}

impl Goal {
    /// Construct a [`Goal::ShouldCall`]. `params` are structured
    /// [`StaticValue`]s; bare `&str`/`String`/`i32`/`i64`/`f64`/`bool` are
    /// accepted directly via [`From`] (a `&str` becomes a quoted string
    /// literal).
    ///
    /// ```ignore
    /// Goal::should_call("set_color_scheme", ["dracula"]);       // set_color_scheme("dracula")
    /// Goal::should_call("resize", [800, 600]);                  // resize(800, 600)
    /// Goal::should_call("configure", vec![StaticValue::str("dark"), StaticValue::bool(true)]);
    /// ```
    ///
    /// Arguments of differing types can't share one array literal (arrays are
    /// homogeneous), so use a `Vec<StaticValue>` with the [`StaticValue`]
    /// constructors for mixed calls, as in the third line above.
    pub fn should_call<S, P, A>(function: S, params: P) -> Goal
    where
        S: Into<String>,
        P: IntoIterator<Item = A>,
        A: Into<StaticValue>,
    {
        Goal::ShouldCall {
            function: function.into(),
            params: params.into_iter().map(Into::into).collect(),
        }
    }

    /// Construct a [`Goal::ShouldSetValue`]: after the edit, `name` is bound to
    /// `value`. Scalars coerce via [`From`] just as call params do.
    ///
    /// ```ignore
    /// Goal::should_set_value("color_scheme", "dracula");   // let color_scheme = "dracula"
    /// Goal::should_set_value("font_size", 14);             // let font_size = 14
    /// Goal::should_set_value("editor", StaticValue::record(vec![("tab_width", 4)]));
    /// ```
    pub fn should_set_value<S, V>(name: S, value: V) -> Goal
    where
        S: Into<String>,
        V: Into<StaticValue>,
    {
        Goal::ShouldSetValue {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Apply `goals` to `source` in order, returning the rewritten source.
///
/// Goals are applied sequentially, each seeing the output of the previous one,
/// so later goals observe earlier insertions. An error from any goal aborts the
/// whole batch (the source is only returned on full success).
pub fn modify_source_with_goals(source: &str, goals: &[Goal]) -> Result<String, GoalError> {
    let mut current = source.to_string();
    for goal in goals {
        current = apply_goal(&current, goal)?;
    }
    Ok(current)
}

/// Rewrite `source` to satisfy a single `goal`.
fn apply_goal(source: &str, goal: &Goal) -> Result<String, GoalError> {
    match goal {
        Goal::ShouldCall { function, params } => ensure_call(source, function, params),
        Goal::ShouldSetValue { name, value } => ensure_binding(source, name, value),
    }
}

/// Render a top-level call `function(arg0, arg1, ...)` from structured values.
/// The call starts at column 0, so its arguments render at depth 1.
fn render_call(function: &str, params: &[StaticValue]) -> String {
    render_call_at(function, params, 1)
}

/// Append `statement` to `source` as a new top-level statement, separated by a
/// blank line (or as the whole content of an empty file).
fn append_statement(source: &str, statement: &str) -> String {
    let trimmed = source.trim_end_matches('\n');
    if trimmed.is_empty() {
        format!("{statement}\n")
    } else {
        format!("{trimmed}\n\n{statement}\n")
    }
}

/// Replace the node at `span` with `replacement`, preferring the lossless tree
/// splice and falling back to a string-level span splice.
///
/// The fallback exists because rendered output only parses when the caller
/// supplies valid Petal identifiers — record keys and fn names are rendered bare
/// and are NOT validated against the grammar, so a contract-violating structured
/// value (e.g. a record key with a space) reaches here and is string-spliced
/// as-is, producing possibly-invalid source returned as `Ok`. Well-formed
/// identifiers always take the tree-splice path.
fn replace_span(
    tree: &std::rc::Rc<crate::cst::GreenNode>,
    source: &str,
    span: crate::source_map::SourceSpan,
    replacement: &str,
) -> String {
    match splice_node(tree, span, replacement) {
        Some(edited) => edited.text(),
        None => splice(source, span, replacement),
    }
}

/// Ensure `source` has a top-level `function(params...)` call: update the first
/// existing one's whole call expression in place, or append a fresh call.
///
/// Only top-level statement-position calls with a bare-identifier callee are
/// matched (the shape of declarative config); a call nested in another
/// expression is ignored, so ensuring it appends a new statement rather than
/// editing the nested one.
fn ensure_call(source: &str, function: &str, params: &[StaticValue]) -> Result<String, GoalError> {
    let replacement = render_call(function, params);
    let (tree, stmts) = parse_ast(source)?;
    match find_call(&stmts, function) {
        Some(span) => Ok(replace_span(&tree, source, span, &replacement)),
        None => Ok(append_statement(source, &replacement)),
    }
}

/// Ensure reading `name` out of `source` yields `value`: replace the right-hand
/// side of its last top-level binding, or append `let name = value`.
///
/// Replacing the whole right-hand side (rather than patching inside it) is what
/// makes this a *static* change for non-trivial bindings too: a `name = if …`
/// or `name = compute(…)` collapses to the literal, which satisfies the goal by
/// construction. Everything around the value — the `let`, the name, comments,
/// indentation — is untouched.
fn ensure_binding(source: &str, name: &str, value: &StaticValue) -> Result<String, GoalError> {
    let (tree, stmts) = parse_ast(source)?;
    match find_binding(&stmts, name) {
        // The value sits after `let name = ` at depth 1, so a multi-line
        // composite indents its elements one level in.
        Some(span) => Ok(replace_span(&tree, source, span, &value.render(1))),
        None => Ok(append_statement(
            source,
            &format!("let {name} = {}", value.render(1)),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str, goals: &[Goal]) -> String {
        modify_source_with_goals(source, goals).unwrap()
    }

    #[test]
    fn unparseable_source_returns_a_goal_error() {
        // The source doesn't parse, so the batch fails with a typed GoalError
        // (not a bare String) whose message is recoverable.
        let err = modify_source_with_goals(
            "set_color_scheme(\n",
            &[Goal::should_call("set_color_scheme", ["dracula"])],
        )
        .unwrap_err();
        assert!(!err.message.is_empty());
        // Display surfaces the message.
        assert_eq!(err.to_string(), err.message);
    }

    #[test]
    fn invalid_record_key_string_splices_as_is() {
        // Record keys render bare and are not validated against Petal's grammar,
        // so a key with a space produces a call that can't reparse as a single
        // expression. The tree splice fails and the string-splice fallback runs,
        // returning the (syntactically invalid) rendered source as Ok — this
        // pins that documented fallback behavior for a valid-but-contract-
        // violating structured value.
        let out = apply(
            "foo({})\n",
            &[Goal::should_call(
                "foo",
                [StaticValue::record(vec![("bad key", StaticValue::int(1))])],
            )],
        );
        assert_eq!(out, "foo({ bad key: 1 })\n");
    }

    #[test]
    fn should_call_updates_existing_call_args() {
        let out = apply(
            "set_color_scheme(\"light\")\n",
            &[Goal::should_call("set_color_scheme", ["dracula"])],
        );
        assert_eq!(out, "set_color_scheme(\"dracula\")\n");
    }

    #[test]
    fn should_call_appends_when_missing() {
        let out = apply(
            "set_theme({})\n",
            &[Goal::should_call("set_color_scheme", ["dracula"])],
        );
        assert_eq!(out, "set_theme({})\n\nset_color_scheme(\"dracula\")\n");
    }

    #[test]
    fn should_call_appends_to_empty_source() {
        let out = apply("", &[Goal::should_call("set_color_scheme", ["dracula"])]);
        assert_eq!(out, "set_color_scheme(\"dracula\")\n");
    }

    #[test]
    fn should_call_preserves_surrounding_comments_and_layout() {
        // The existing call is flexible about layout: leading indentation and a
        // trailing comment are trivia around the call node and survive the edit.
        let out = apply(
            "// user config\nx = 1\n    set_color_scheme(\"light\") // was light\ny = 2\n",
            &[Goal::should_call("set_color_scheme", ["dracula"])],
        );
        assert_eq!(
            out,
            "// user config\nx = 1\n    set_color_scheme(\"dracula\") // was light\ny = 2\n"
        );
    }

    #[test]
    fn should_call_replaces_multiline_call_whole() {
        let src = "set_color_scheme(\n    \"light\",\n)\nx = 2\n";
        let out = apply(src, &[Goal::should_call("set_color_scheme", ["dracula"])]);
        assert_eq!(out, "set_color_scheme(\"dracula\")\nx = 2\n");
    }

    #[test]
    fn renders_int_params() {
        let out = apply("", &[Goal::should_call("resize", [800, 600])]);
        assert_eq!(out, "resize(800, 600)\n");
    }

    #[test]
    fn renders_float_with_decimal_point() {
        let out = apply("", &[Goal::should_call("set_scale", [1.0])]);
        assert_eq!(out, "set_scale(1.0)\n");
    }

    #[test]
    fn renders_bool_and_nil() {
        let out = apply(
            "",
            &[Goal::should_call(
                "configure",
                vec![StaticValue::bool(true), StaticValue::nil()],
            )],
        );
        assert_eq!(out, "configure(true, nil)\n");
    }

    #[test]
    fn renders_zero_params() {
        let out = apply("", &[Goal::should_call("clear", Vec::<StaticValue>::new())]);
        assert_eq!(out, "clear()\n");
    }

    #[test]
    fn renders_mixed_typed_params_via_vec() {
        let out = apply(
            "",
            &[Goal::should_call(
                "set",
                vec![StaticValue::str("size"), StaticValue::int(14)],
            )],
        );
        assert_eq!(out, "set(\"size\", 14)\n");
    }

    #[test]
    fn escapes_string_literals() {
        // Quote, backslash, and the interpolation opener `{` are escaped so the
        // rendered call is a single well-formed string that reparses (the tree
        // splice, not the string fallback, is taken).
        let out = apply(
            "name(\"x\")\n",
            &[Goal::should_call("name", ["a\"b\\c{d}"])],
        );
        assert_eq!(out, "name(\"a\\\"b\\\\c\\{d}\")\n");
        // And the result is valid, re-editable source.
        let again = apply(&out, &[Goal::should_call("name", ["plain"])]);
        assert_eq!(again, "name(\"plain\")\n");
    }

    #[test]
    fn renders_scalar_list_inline() {
        let out = apply(
            "",
            &[Goal::should_call("grid", [StaticValue::list([1, 2, 3])])],
        );
        assert_eq!(out, "grid([1, 2, 3])\n");
    }

    #[test]
    fn renders_empty_list_and_record() {
        let out = apply(
            "",
            &[Goal::should_call(
                "configure",
                vec![
                    StaticValue::list(Vec::<StaticValue>::new()),
                    StaticValue::record(Vec::<(String, StaticValue)>::new()),
                ],
            )],
        );
        assert_eq!(out, "configure([], {})\n");
    }

    #[test]
    fn renders_record_inline() {
        let out = apply(
            "",
            &[Goal::should_call(
                "editor_config",
                [StaticValue::record(vec![
                    ("line_numbers", StaticValue::bool(true)),
                    ("tab_width", StaticValue::int(4)),
                ])],
            )],
        );
        assert_eq!(out, "editor_config({ line_numbers: true, tab_width: 4 })\n");
    }

    #[test]
    fn renders_nested_call() {
        let out = apply(
            "",
            &[Goal::should_call(
                "layout",
                [StaticValue::call("editor", ["a.rs"])],
            )],
        );
        assert_eq!(out, "layout(editor(\"a.rs\"))\n");
    }

    #[test]
    fn f32_coerces_via_shortest_display() {
        let out = apply("", &[Goal::should_call("ratios", [0.7f32, 0.3f32])]);
        assert_eq!(out, "ratios(0.7, 0.3)\n");
    }

    #[test]
    fn list_of_calls_renders_multiline() {
        // A composite-element list is laid out one element per line, indented
        // relative to the call nesting — the shape of a declarative layout tree.
        let out = apply(
            "",
            &[Goal::should_call(
                "layout",
                [StaticValue::call(
                    "row",
                    vec![
                        StaticValue::list([
                            StaticValue::call(
                                "column",
                                vec![StaticValue::list([
                                    StaticValue::call("editor", ["a"]),
                                    StaticValue::call("editor", ["b"]),
                                ])],
                            ),
                            StaticValue::call("editor", ["c"]),
                        ]),
                        StaticValue::list([0.6f32, 0.4f32]),
                    ],
                )],
            )],
        );
        let expected = "\
layout(row([
    column([
      editor(\"a\"),
      editor(\"b\"),
    ]),
    editor(\"c\"),
  ], [0.6, 0.4]))\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn multiline_call_updates_in_place_and_reparses() {
        // The multi-line rendered call still takes the lossless tree-splice
        // path (it parses as one expression), so surrounding comments survive.
        let src = "// config\nlayout(editor())\n// end\n";
        let out = apply(
            src,
            &[Goal::should_call(
                "layout",
                [StaticValue::call(
                    "column",
                    vec![
                        StaticValue::list([
                            StaticValue::call("editor", ["x"]),
                            StaticValue::call("editor", ["y"]),
                        ]),
                        StaticValue::list([0.5f32, 0.5f32]),
                    ],
                )],
            )],
        );
        assert_eq!(
            out,
            "// config\nlayout(column([\n    editor(\"x\"),\n    editor(\"y\"),\n  ], [0.5, 0.5]))\n// end\n"
        );
        // And the result is valid, re-editable source.
        let again = apply(
            &out,
            &[Goal::should_call(
                "layout",
                [StaticValue::call("editor", Vec::<StaticValue>::new())],
            )],
        );
        assert!(again.contains("layout(editor())"), "got: {again}");
    }

    #[test]
    fn multiple_goals_apply_in_sequence() {
        let out = apply(
            "set_color_scheme(\"light\")\n",
            &[
                Goal::should_call("set_color_scheme", ["dracula"]),
                Goal::should_call("set_font_size", [14]),
            ],
        );
        assert_eq!(out, "set_color_scheme(\"dracula\")\n\nset_font_size(14)\n");
    }

    #[test]
    fn later_goal_updates_a_call_an_earlier_goal_inserted() {
        let out = apply(
            "",
            &[
                Goal::should_call("set_color_scheme", ["light"]),
                Goal::should_call("set_color_scheme", ["dracula"]),
            ],
        );
        assert_eq!(out, "set_color_scheme(\"dracula\")\n");
    }

    #[test]
    fn multibyte_source_survives_edit() {
        let out = apply(
            "// café ☕ theme\nset_color_scheme(\"light\")\n",
            &[Goal::should_call("set_color_scheme", ["dracula"])],
        );
        assert_eq!(out, "// café ☕ theme\nset_color_scheme(\"dracula\")\n");
    }

    #[test]
    fn should_call_constructor_accepts_owned_and_borrowed() {
        // &str, String, ints, floats, bools all coerce into StaticValue.
        let _ = Goal::should_call("f", ["a"]);
        let _ = Goal::should_call(String::from("f"), vec![String::from("a")]);
        let _ = Goal::should_call("f", [1, 2, 3]);
        let _ = Goal::should_call("f", vec![StaticValue::float(1.5), StaticValue::bool(false)]);
    }

    // ── ShouldSetValue ───────────────────────────────────────────────────

    #[test]
    fn should_set_value_updates_an_existing_let() {
        let out = apply(
            "let color_scheme = \"light\"\n",
            &[Goal::should_set_value("color_scheme", "dracula")],
        );
        assert_eq!(out, "let color_scheme = \"dracula\"\n");
    }

    #[test]
    fn should_set_value_updates_a_bare_assignment() {
        let out = apply(
            "font_size = 12\n",
            &[Goal::should_set_value("font_size", 14)],
        );
        assert_eq!(out, "font_size = 14\n");
    }

    #[test]
    fn should_set_value_appends_when_the_name_is_unbound() {
        let out = apply(
            "let color_scheme = \"light\"\n",
            &[Goal::should_set_value("font_size", 14)],
        );
        assert_eq!(out, "let color_scheme = \"light\"\n\nlet font_size = 14\n");
    }

    #[test]
    fn should_set_value_appends_to_empty_source() {
        let out = apply("", &[Goal::should_set_value("font_size", 14)]);
        assert_eq!(out, "let font_size = 14\n");
    }

    #[test]
    fn should_set_value_edits_the_last_binding() {
        // The last binding decides the program's value, so that is the one that
        // has to change — editing the first would leave the goal unmet.
        let out = apply(
            "let size = 12\nlet size = 13\nsize = 14\n",
            &[Goal::should_set_value("size", 20)],
        );
        assert_eq!(out, "let size = 12\nlet size = 13\nsize = 20\n");
    }

    #[test]
    fn should_set_value_preserves_comments_and_layout() {
        let out = apply(
            "// user config\n\nlet font_size = 12 // points\nlet other = 1\n",
            &[Goal::should_set_value("font_size", 14)],
        );
        assert_eq!(
            out,
            "// user config\n\nlet font_size = 14 // points\nlet other = 1\n"
        );
    }

    #[test]
    fn should_set_value_collapses_a_computed_binding_to_the_literal() {
        // The whole right-hand side is replaced, so a conditional or a call
        // becomes the literal — the static change that makes the goal hold.
        let out = apply(
            "let font_size = if wide_screen then 16 else 12 end\n",
            &[Goal::should_set_value("font_size", 14)],
        );
        assert_eq!(out, "let font_size = 14\n");
        let out = apply(
            "let accent = rgb(255, 0, 0)\n",
            &[Goal::should_set_value("accent", "red")],
        );
        assert_eq!(out, "let accent = \"red\"\n");
    }

    #[test]
    fn should_set_value_writes_composites() {
        let out = apply(
            "let editor = {}\n",
            &[Goal::should_set_value(
                "editor",
                StaticValue::record(vec![
                    ("line_numbers", StaticValue::bool(true)),
                    ("tab_width", StaticValue::int(4)),
                ]),
            )],
        );
        assert_eq!(out, "let editor = { line_numbers: true, tab_width: 4 }\n");

        let out = apply(
            "let panes = []\n",
            &[Goal::should_set_value(
                "panes",
                StaticValue::list([
                    StaticValue::call("editor", ["a.rs"]),
                    StaticValue::call("editor", ["b.rs"]),
                ]),
            )],
        );
        // A composite list indents its elements one level in from the binding.
        assert_eq!(
            out,
            "let panes = [\n    editor(\"a.rs\"),\n    editor(\"b.rs\"),\n  ]\n"
        );
    }

    #[test]
    fn should_set_value_ignores_a_binding_inside_a_function() {
        // `size` inside `f` is that body's scope; the goal is about the
        // top-level name, so a new top-level binding is appended.
        let out = apply(
            "fn f() let size = 1 end\n",
            &[Goal::should_set_value("size", 14)],
        );
        assert_eq!(out, "fn f() let size = 1 end\n\nlet size = 14\n");
    }

    #[test]
    fn should_set_value_result_reads_back_as_the_value() {
        // The round-trip property that makes read-modify-write safe: what the
        // goal writes is what `get_static_value` reads.
        use crate::static_value::get_static_value;
        let values = [
            StaticValue::str("a\"b\\c{d}"),
            StaticValue::int(-3),
            StaticValue::float(1.5),
            StaticValue::bool(false),
            StaticValue::nil(),
            StaticValue::list([1, 2]),
            StaticValue::record(vec![("k", StaticValue::str("v"))]),
            StaticValue::call("rgb", [1, 2, 3]),
        ];
        for value in values {
            let out = apply(
                "// config\nlet setting = 0 // note\n",
                &[Goal::should_set_value("setting", value.clone())],
            );
            assert_eq!(get_static_value(&out, "setting").unwrap(), value);
            // And the comments survived.
            assert!(out.starts_with("// config\n"), "got: {out}");
        }
    }

    #[test]
    fn set_value_and_call_goals_compose() {
        let out = apply(
            "// config\nlet font_size = 12\n",
            &[
                Goal::should_set_value("font_size", 14),
                Goal::should_call("set_color_scheme", ["dracula"]),
                Goal::should_set_value("wrap", true),
            ],
        );
        assert_eq!(
            out,
            "// config\nlet font_size = 14\n\nset_color_scheme(\"dracula\")\n\nlet wrap = true\n"
        );
    }

    #[test]
    fn later_set_value_goal_updates_one_an_earlier_goal_inserted() {
        let out = apply(
            "",
            &[
                Goal::should_set_value("font_size", 12),
                Goal::should_set_value("font_size", 14),
            ],
        );
        assert_eq!(out, "let font_size = 14\n");
    }

    #[test]
    fn should_set_value_survives_multibyte_source() {
        let out = apply(
            "// café ☕ theme\nlet font_size = 12\n",
            &[Goal::should_set_value("font_size", 14)],
        );
        assert_eq!(out, "// café ☕ theme\nlet font_size = 14\n");
    }

    #[test]
    fn should_set_value_reports_unparseable_source() {
        let err =
            modify_source_with_goals("let x = \n", &[Goal::should_set_value("x", 1)]).unwrap_err();
        assert!(!err.message.is_empty());
    }
}
