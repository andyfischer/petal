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
//! quotes, and backslashes can never leak). [`Arg::expr`] is the escape hatch for
//! arguments a literal can't express (identifiers, nested calls, records).
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

/// A structured call argument. Rendered into a Petal literal at edit time.
///
/// Prefer the typed variants (via the [`From`] impls or the constructors), so
/// strings are quoted/escaped for you; reach for [`Arg::Expr`] only when the
/// argument is something a literal can't express.
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
    /// A raw source expression, rendered **verbatim** — the escape hatch for
    /// arguments structured literals can't express (identifiers, nested calls,
    /// records, list literals): `Arg::expr("theme.dark")`, `Arg::expr("[1, 2]")`.
    Expr(String),
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
    /// A raw source expression, rendered verbatim.
    pub fn expr(src: impl Into<String>) -> Arg {
        Arg::Expr(src.into())
    }

    /// Render this argument as Petal source.
    fn render(&self) -> String {
        match self {
            Arg::Str(s) => render_string_literal(s),
            Arg::Int(n) => n.to_string(),
            // `{:?}` on f64 always emits a decimal point (`1.0`, not `1`), so the
            // result parses as a float rather than an int.
            Arg::Float(f) => format!("{f:?}"),
            Arg::Bool(true) => "true".to_string(),
            Arg::Bool(false) => "false".to_string(),
            Arg::Nil => "nil".to_string(),
            Arg::Expr(src) => src.clone(),
        }
    }
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
    ShouldCall {
        function: String,
        params: Vec<Arg>,
    },
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
pub fn modify_source_with_goals(source: &str, goals: &[Goal]) -> Result<String, String> {
    let mut current = source.to_string();
    for goal in goals {
        current = apply_goal(&current, goal)?;
    }
    Ok(current)
}

/// Rewrite `source` to satisfy a single `goal`.
fn apply_goal(source: &str, goal: &Goal) -> Result<String, String> {
    match goal {
        Goal::ShouldCall { function, params } => ensure_call(source, function, params),
    }
}

/// Render a call `function(arg0, arg1, ...)` from structured args.
fn render_call(function: &str, params: &[Arg]) -> String {
    let args: Vec<String> = params.iter().map(Arg::render).collect();
    format!("{function}({})", args.join(", "))
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
fn ensure_call(source: &str, function: &str, params: &[Arg]) -> Result<String, String> {
    let replacement = render_call(function, params);
    let (tree, stmts) = parse_ast(source)?;
    match find_call(&stmts, function) {
        Some(span) => Ok(match splice_node(&tree, span, &replacement) {
            Some(edited) => edited.text(),
            // Rendered call isn't a single parseable expression (only reachable
            // via a malformed `Arg::expr`): splice it in verbatim at the string
            // level.
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
            &[Goal::should_call("configure", vec![Arg::bool(true), Arg::nil()])],
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
    fn expr_arg_is_rendered_verbatim() {
        let out = apply(
            "theme({})\n",
            &[Goal::should_call("theme", [Arg::expr("palette.dark")])],
        );
        assert_eq!(out, "theme(palette.dark)\n");
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
    fn malformed_expr_arg_falls_back_to_string_splice() {
        // A raw expr that makes the rendered call unparseable: spliced verbatim
        // at the string level rather than failing.
        let out = apply(
            "set_color_scheme(\"light\")\n",
            &[Goal::should_call("set_color_scheme", [Arg::expr("<<broken")])],
        );
        assert_eq!(out, "set_color_scheme(<<broken)\n");
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
