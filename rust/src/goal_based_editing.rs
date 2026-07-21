//! Goal-based source editing — declarative, formatting-preserving edits.
//!
//! Instead of imperative "replace this span with that text" calls, a caller
//! describes **goals**: properties the edited source should satisfy. A goal is
//! order-independent in intent ("there should be a call to `set_color_scheme`
//! with these arguments") and leaves *how* to achieve it — insert a new call,
//! or update an existing one in place — to this module. [`modify_source_with_goals`]
//! applies a list of goals to a source string and returns the rewritten source.
//!
//! This is more expressive than a single-purpose rewrite helper: goals compose
//! (apply several in one pass), and the [`Goal`] enum is the extension point for
//! richer intents later (ensure an import, remove a call, set a field on a
//! record literal, …). Today the only variant is [`Goal::ShouldCall`].
//!
//! Call arguments are **structured values** ([`Arg`]), not pre-rendered source:
//! the caller passes `"dracula"` / `5` / `true` and this module renders each into
//! a valid Petal literal (strings are quoted and escaped, so interpolation `{`,
//! quotes, and backslashes can never leak). Composite arguments — nested calls
//! ([`Arg::call`]), lists ([`Arg::list`]), records ([`Arg::record`]) — let an
//! embedder express whole declarative trees, e.g. Garden's
//! `layout(row([editor("a.rs")], [1.0]))`; a list of composite elements is
//! pretty-printed one element per line so the generated source reads like
//! hand-written config. Every argument is a structured value, so the rendered
//! source is always well-formed — there is no verbatim/raw-source escape hatch.
//!
//! Edits go through the lossless CST primitives in [`crate::rewrite`]
//! ([`parse_ast`], [`find_call`], [`splice_node`], [`splice`]), so comments and
//! surrounding layout survive and the caller is not required to match any
//! particular existing formatting.
//!
//! ```ignore
//! use petal::goal_based_editing::{modify_source_with_goals, Goal};
//!
//! // Ensure the config selects the "dracula" scheme, whatever it selects now.
//! // The &str is auto-wrapped as a string Arg and rendered as "dracula".
//! let goals = [Goal::should_call("set_color_scheme", ["dracula"])];
//! let updated = modify_source_with_goals(&source, &goals)?;
//! ```

use crate::rewrite::{find_call, parse_ast, splice, splice_node};

/// Why a goal batch could not be applied — the source didn't parse, or the
/// rewrite machinery rejected an edit. A distinct type (rather than a bare
/// `String`) so the result of [`modify_source_with_goals`] reads unambiguously:
/// `Ok` is the rewritten source, `Err` is this failure. Wraps a human-readable
/// message; `Display`/`From<GoalError> for String` recover it.
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

impl From<&str> for GoalError {
    fn from(message: &str) -> Self {
        GoalError {
            message: message.to_string(),
        }
    }
}

impl From<GoalError> for String {
    fn from(err: GoalError) -> Self {
        err.message
    }
}

/// A structured call argument. Rendered into a Petal literal at edit time.
///
/// Use the typed variants (via the [`From`] impls or the constructors); strings
/// are quoted/escaped for you and every variant renders to well-formed Petal.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// A string, rendered as a quoted-and-escaped Petal string literal.
    Str(String),
    /// An integer literal.
    Int(i64),
    /// A float literal (always rendered with a decimal point).
    Float(f64),
    /// A `true` / `false` literal.
    Bool(bool),
    /// The `nil` literal.
    Nil,
    /// A list literal `[a, b, c]`. Renders inline when every element is a
    /// scalar; a list containing composite elements (calls, lists, records)
    /// renders one element per line, indented — the shape of a declarative
    /// layout tree.
    List(Vec<Arg>),
    /// A record literal `{ key: value, ... }`, rendered inline. Keys are
    /// rendered bare, so they must be valid Petal identifiers.
    Record(Vec<(String, Arg)>),
    /// A nested call `function(args...)` — building block for declarative
    /// call trees like `layout(row([editor("a")], [1.0]))`.
    Call { function: String, args: Vec<Arg> },
}

impl Arg {
    /// A string argument (quoted and escaped on render).
    pub fn str(s: impl Into<String>) -> Arg {
        Arg::Str(s.into())
    }
    /// An integer argument.
    pub fn int(n: impl Into<i64>) -> Arg {
        Arg::Int(n.into())
    }
    /// A float argument.
    pub fn float(f: impl Into<f64>) -> Arg {
        Arg::Float(f.into())
    }
    /// A boolean argument.
    pub fn bool(b: bool) -> Arg {
        Arg::Bool(b)
    }
    /// The `nil` argument.
    pub fn nil() -> Arg {
        Arg::Nil
    }
    /// A list argument. Elements coerce like call params do.
    pub fn list<P, A>(items: P) -> Arg
    where
        P: IntoIterator<Item = A>,
        A: Into<Arg>,
    {
        Arg::List(items.into_iter().map(Into::into).collect())
    }
    /// A record argument. Keys must be valid Petal identifiers (they render
    /// bare); values coerce like call params do.
    pub fn record<P, K, A>(fields: P) -> Arg
    where
        P: IntoIterator<Item = (K, A)>,
        K: Into<String>,
        A: Into<Arg>,
    {
        Arg::Record(
            fields
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
    /// A nested call argument: `Arg::call("editor", ["a.rs"])` renders as
    /// `editor("a.rs")`.
    pub fn call<S, P, A>(function: S, args: P) -> Arg
    where
        S: Into<String>,
        P: IntoIterator<Item = A>,
        A: Into<Arg>,
    {
        Arg::Call {
            function: function.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// Render this argument as Petal source. `depth` is the current indent
    /// level in two-space units; it only matters for multi-line lists (see
    /// [`Arg::List`]) — scalars ignore it.
    fn render(&self, depth: usize) -> String {
        match self {
            Arg::Str(s) => render_string_literal(s),
            Arg::Int(n) => n.to_string(),
            // `{:?}` on f64 always emits a decimal point (`1.0`, not `1`), so the
            // result parses as a float rather than an int.
            Arg::Float(f) => format!("{f:?}"),
            Arg::Bool(true) => "true".to_string(),
            Arg::Bool(false) => "false".to_string(),
            Arg::Nil => "nil".to_string(),
            Arg::List(items) => render_list(items, depth),
            Arg::Record(fields) => render_record(fields, depth),
            Arg::Call { function, args } => render_call_at(function, args, depth),
        }
    }

    /// True for the composite variants whose rendering can span lines; a list
    /// containing any of these is laid out one element per line.
    fn is_composite(&self) -> bool {
        matches!(self, Arg::List(_) | Arg::Record(_) | Arg::Call { .. })
    }
}

/// Render a list literal at `depth`. All-scalar lists stay inline
/// (`[0.7, 0.3]`); a list with composite elements puts each element on its own
/// line at `depth + 1`, with the closing bracket back at `depth` — the layout a
/// user would write for a tree of nested calls.
fn render_list(items: &[Arg], depth: usize) -> String {
    if !items.iter().any(Arg::is_composite) {
        let rendered: Vec<String> = items.iter().map(|a| a.render(depth)).collect();
        return format!("[{}]", rendered.join(", "));
    }
    let mut out = String::from("[\n");
    let inner = indent(depth + 1);
    for item in items {
        out.push_str(&inner);
        out.push_str(&item.render(depth + 1));
        out.push_str(",\n");
    }
    out.push_str(&indent(depth));
    out.push(']');
    out
}

/// Render a record literal inline: `{ key: value, ... }` (`{}` when empty).
fn render_record(fields: &[(String, Arg)], depth: usize) -> String {
    if fields.is_empty() {
        return "{}".to_string();
    }
    let rendered: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{k}: {}", v.render(depth)))
        .collect();
    format!("{{ {} }}", rendered.join(", "))
}

/// Render a call `function(arg0, arg1, ...)` with its arguments at `depth`
/// (multi-line lists among them indent their elements at `depth + 1`).
fn render_call_at(function: &str, args: &[Arg], depth: usize) -> String {
    let rendered: Vec<String> = args.iter().map(|a| a.render(depth)).collect();
    format!("{function}({})", rendered.join(", "))
}

/// Two spaces per `depth` level.
fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

impl From<&str> for Arg {
    fn from(s: &str) -> Arg {
        Arg::Str(s.to_string())
    }
}
impl From<String> for Arg {
    fn from(s: String) -> Arg {
        Arg::Str(s)
    }
}
impl From<i64> for Arg {
    fn from(n: i64) -> Arg {
        Arg::Int(n)
    }
}
impl From<i32> for Arg {
    fn from(n: i32) -> Arg {
        Arg::Int(n as i64)
    }
}
impl From<f64> for Arg {
    fn from(f: f64) -> Arg {
        Arg::Float(f)
    }
}
impl From<f32> for Arg {
    fn from(f: f32) -> Arg {
        // A plain `as f64` widening drags in garbage digits (0.7f32 becomes
        // 0.7000000298023224); round-tripping through the shortest display
        // form keeps the literal the user actually meant.
        Arg::Float(format!("{f}").parse().unwrap_or(f as f64))
    }
}
impl From<bool> for Arg {
    fn from(b: bool) -> Arg {
        Arg::Bool(b)
    }
}

/// Render `s` as a Petal string literal: double-quoted, with `\`, `"`, and the
/// interpolation opener `{` escaped (plus newlines/tabs), so no character of the
/// content can change how the literal parses.
fn render_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            // `{` starts interpolation in a Petal string; escape it so the
            // content is treated literally.
            '{' => out.push_str("\\{"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
    ShouldCall { function: String, params: Vec<Arg> },
}

impl Goal {
    /// Construct a [`Goal::ShouldCall`]. `params` are structured [`Arg`] values;
    /// bare `&str`/`String`/`i32`/`i64`/`f64`/`bool` are accepted directly via
    /// [`From`] (a `&str` becomes a quoted string literal).
    ///
    /// ```ignore
    /// Goal::should_call("set_color_scheme", ["dracula"]);       // set_color_scheme("dracula")
    /// Goal::should_call("resize", [800, 600]);                  // resize(800, 600)
    /// Goal::should_call("configure", vec![Arg::str("dark"), Arg::bool(true)]);
    /// ```
    ///
    /// Arguments of differing types can't share one array literal (arrays are
    /// homogeneous), so use a `Vec<Arg>` with the [`Arg`] constructors for mixed
    /// calls, as in the third line above.
    pub fn should_call<S, P, A>(function: S, params: P) -> Goal
    where
        S: Into<String>,
        P: IntoIterator<Item = A>,
        A: Into<Arg>,
    {
        Goal::ShouldCall {
            function: function.into(),
            params: params.into_iter().map(Into::into).collect(),
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
    }
}

/// Render a top-level call `function(arg0, arg1, ...)` from structured args.
/// The call starts at column 0, so its arguments render at depth 1.
fn render_call(function: &str, params: &[Arg]) -> String {
    render_call_at(function, params, 1)
}

/// Ensure `source` has a top-level `function(params...)` call: update the first
/// existing one's whole call expression in place, or append a fresh call.
///
/// The update prefers a lossless tree splice (comments and layout around the
/// call survive); if the rendered call doesn't parse as a single expression it
/// falls back to a string-level span splice. Only top-level statement-position
/// calls with a bare-identifier callee are matched (the shape of declarative
/// config); a call nested in another expression is ignored, so ensuring it
/// appends a new statement rather than editing the nested one.
fn ensure_call(source: &str, function: &str, params: &[Arg]) -> Result<String, GoalError> {
    let replacement = render_call(function, params);
    let (tree, stmts) = parse_ast(source)?;
    match find_call(&stmts, function) {
        Some(span) => Ok(match splice_node(&tree, span, &replacement) {
            Some(edited) => edited.text(),
            // Defensive: structured args always render to a parseable call, but
            // if a splice ever can't reparse, fall back to a string-level span
            // splice rather than dropping the edit.
            None => splice(source, span, &replacement),
        }),
        None => {
            let trimmed = source.trim_end_matches('\n');
            if trimmed.is_empty() {
                Ok(format!("{replacement}\n"))
            } else {
                Ok(format!("{trimmed}\n\n{replacement}\n"))
            }
        }
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
        // Display and the String conversion both surface the same message.
        assert_eq!(err.to_string(), String::from(err.clone()));
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
                vec![Arg::bool(true), Arg::nil()],
            )],
        );
        assert_eq!(out, "configure(true, nil)\n");
    }

    #[test]
    fn renders_zero_params() {
        let out = apply("", &[Goal::should_call("clear", Vec::<Arg>::new())]);
        assert_eq!(out, "clear()\n");
    }

    #[test]
    fn renders_mixed_typed_params_via_vec() {
        let out = apply(
            "",
            &[Goal::should_call(
                "set",
                vec![Arg::str("size"), Arg::int(14)],
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
        let out = apply("", &[Goal::should_call("grid", [Arg::list([1, 2, 3])])]);
        assert_eq!(out, "grid([1, 2, 3])\n");
    }

    #[test]
    fn renders_empty_list_and_record() {
        let out = apply(
            "",
            &[Goal::should_call(
                "configure",
                vec![
                    Arg::list(Vec::<Arg>::new()),
                    Arg::record(Vec::<(String, Arg)>::new()),
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
                [Arg::record(vec![
                    ("line_numbers", Arg::bool(true)),
                    ("tab_width", Arg::int(4)),
                ])],
            )],
        );
        assert_eq!(out, "editor_config({ line_numbers: true, tab_width: 4 })\n");
    }

    #[test]
    fn renders_nested_call() {
        let out = apply(
            "",
            &[Goal::should_call("layout", [Arg::call("editor", ["a.rs"])])],
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
                [Arg::call(
                    "row",
                    vec![
                        Arg::list([
                            Arg::call(
                                "column",
                                vec![Arg::list([
                                    Arg::call("editor", ["a"]),
                                    Arg::call("editor", ["b"]),
                                ])],
                            ),
                            Arg::call("editor", ["c"]),
                        ]),
                        Arg::list([0.6f32, 0.4f32]),
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
                [Arg::call(
                    "column",
                    vec![
                        Arg::list([Arg::call("editor", ["x"]), Arg::call("editor", ["y"])]),
                        Arg::list([0.5f32, 0.5f32]),
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
                [Arg::call("editor", Vec::<Arg>::new())],
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
        // &str, String, ints, floats, bools all coerce into Arg.
        let _ = Goal::should_call("f", ["a"]);
        let _ = Goal::should_call(String::from("f"), vec![String::from("a")]);
        let _ = Goal::should_call("f", [1, 2, 3]);
        let _ = Goal::should_call("f", vec![Arg::float(1.5), Arg::bool(false)]);
    }
}
