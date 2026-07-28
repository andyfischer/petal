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
use crate::static_value::{get_static_value, render_call_at};

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
        /// Where a *newly inserted* call goes. Ignored when the call already
        /// exists, since then nothing is inserted.
        placement: Placement,
    },
    /// Reading `name` out of the edited source should yield `value` — the
    /// write half of Petal-as-a-config-format.
    ///
    /// The **last** top-level binding of `name` is the one that decides the
    /// program's value, so that is the one edited: its right-hand side is
    /// replaced with `value`, whatever it was before (a literal, a call, or a
    /// whole `if` expression collapse to the literal). If `name` isn't bound at
    /// top level, `let name = value` is appended — at the end of the file, or
    /// wherever `placement` says.
    ///
    /// A goal that already holds is a **no-op**: if reading `name` out of the
    /// source already yields `value`, the source comes back byte-identical.
    /// That is what keeps a save that rewrites every field from turning
    /// `let drag = 0.020000` into `let drag = 0.02` — the same `f64`, spelled
    /// differently, and a diff somebody has to read and dismiss.
    ShouldSetValue {
        name: String,
        value: StaticValue,
        /// Where the binding goes when it has to be inserted. Ignored when
        /// `name` is already bound, since then the existing binding is edited
        /// where it sits.
        placement: Placement,
    },
}

/// Where a goal's new statement goes when one has to be inserted.
///
/// A config file generated from a table has a shape — grouped, each binding
/// under its own doc comment — and a new field appended at the bottom lands
/// outside it. A host that knows its own ordering says so here, and keeps it
/// without regenerating the file (which would throw away the user's comments
/// and layout, the thing goal-based editing exists to protect).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Placement {
    /// Append to the end of the file. The default.
    #[default]
    End,
    /// Insert directly below the top-level binding (or statement-position call)
    /// of this name, keeping the blank-line spacing around it. Falls back to
    /// [`Placement::End`] if the anchor isn't in the file.
    After(String),
    /// Insert directly above the named anchor — above its doc comment, so the
    /// comment stays with the binding it describes. Falls back to
    /// [`Placement::End`] if the anchor isn't in the file.
    Before(String),
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
            placement: Placement::End,
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
            placement: Placement::End,
        }
    }

    /// Place this goal's statement directly below `anchor` if it has to be
    /// inserted (see [`Placement::After`]).
    ///
    /// ```ignore
    /// Goal::should_set_value("tether_slack_m", 0.5).after("tether_max_m")
    /// ```
    pub fn after(self, anchor: impl Into<String>) -> Goal {
        self.with_placement(Placement::After(anchor.into()))
    }

    /// Place this goal's statement directly above `anchor` — and above
    /// `anchor`'s doc comment — if it has to be inserted (see
    /// [`Placement::Before`]).
    pub fn before(self, anchor: impl Into<String>) -> Goal {
        self.with_placement(Placement::Before(anchor.into()))
    }

    /// This goal with its insertion placement replaced.
    pub fn with_placement(self, new: Placement) -> Goal {
        match self {
            Goal::ShouldCall {
                function, params, ..
            } => Goal::ShouldCall {
                function,
                params,
                placement: new,
            },
            Goal::ShouldSetValue { name, value, .. } => Goal::ShouldSetValue {
                name,
                value,
                placement: new,
            },
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
        Goal::ShouldCall {
            function,
            params,
            placement,
        } => ensure_call(source, function, params, placement),
        Goal::ShouldSetValue {
            name,
            value,
            placement,
        } => ensure_binding(source, name, value, placement),
    }
}

/// Render a top-level call `function(arg0, arg1, ...)` from structured values.
/// The call starts at column 0, so its arguments render at depth 1.
fn render_call(function: &str, params: &[StaticValue]) -> String {
    render_call_at(function, params, 1)
}

/// Insert `statement` into `source` as a new top-level statement, where
/// `placement` says. An anchor that isn't in the file falls back to appending,
/// so a placement can never lose the statement.
fn insert_statement(
    source: &str,
    statement: &str,
    placement: &Placement,
    stmts: &[crate::ast::Stmt],
) -> String {
    let anchored = match placement {
        Placement::End => None,
        Placement::After(anchor) => insert_after(source, statement, anchor, stmts),
        Placement::Before(anchor) => insert_before(source, statement, anchor, stmts),
    };
    anchored.unwrap_or_else(|| append_statement(source, statement))
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

/// Insert `statement` on its own line just below the anchor's statement,
/// matching the spacing already there: a blank line after the anchor means the
/// file separates its bindings that way, so the new one is separated too.
fn insert_after(
    source: &str,
    statement: &str,
    anchor: &str,
    stmts: &[crate::ast::Stmt],
) -> Option<String> {
    let chars: Vec<char> = source.chars().collect();
    let span = anchor_span(stmts, anchor)?;
    // Past the end of the anchor's own line, so a trailing comment stays with it.
    let at = line_end_after(&chars, span.end.offset as usize);
    let separated = starts_blank_line(&chars, at);
    let inserted = if separated {
        format!("\n{statement}\n")
    } else {
        format!("{statement}\n")
    };
    Some(splice_text(&chars, at, at, &inserted))
}

/// Insert `statement` above the anchor — above its leading comment block, so
/// the comment stays attached to the binding it documents.
fn insert_before(
    source: &str,
    statement: &str,
    anchor: &str,
    stmts: &[crate::ast::Stmt],
) -> Option<String> {
    let chars: Vec<char> = source.chars().collect();
    let span = anchor_span(stmts, anchor)?;
    let at = comment_block_start(&chars, span.start.offset as usize);
    // Mirror the spacing on the other side: a blank line (or the top of the
    // file) above the anchor means bindings here are separated by one.
    let separated = at == 0 || ends_blank_line(&chars, at);
    let inserted = if separated {
        format!("{statement}\n\n")
    } else {
        format!("{statement}\n")
    };
    Some(splice_text(&chars, at, at, &inserted))
}

/// The span of the anchor's top-level statement: its binding if it has one,
/// else a statement-position call of that name.
fn anchor_span(stmts: &[crate::ast::Stmt], anchor: &str) -> Option<crate::source_map::SourceSpan> {
    binding_stmt_span(stmts, anchor).or_else(|| find_call(stmts, anchor))
}

/// The span of the whole `let name = …` / `name = …` statement (not just its
/// value), for the last top-level binding of `name`.
fn binding_stmt_span(
    stmts: &[crate::ast::Stmt],
    name: &str,
) -> Option<crate::source_map::SourceSpan> {
    use crate::ast::{AssignTarget, StmtKind};
    stmts.iter().rev().find_map(|stmt| match &stmt.kind {
        StmtKind::Let { name: bound, .. } if bound == name => Some(stmt.span),
        StmtKind::Assign {
            target: AssignTarget::Name(bound),
            ..
        } if bound == name => Some(stmt.span),
        _ => None,
    })
}

/// Offset just past the newline ending the line that `offset` sits on (or the
/// end of the source, when the last line has no newline — in which case one is
/// implied by the caller writing a whole line).
fn line_end_after(chars: &[char], offset: usize) -> usize {
    chars[offset.min(chars.len())..]
        .iter()
        .position(|&c| c == '\n')
        .map(|i| offset + i + 1)
        .unwrap_or(chars.len())
}

/// Start of the comment block above the statement beginning at `offset` — or
/// the start of that statement's own line when nothing documents it.
fn comment_block_start(chars: &[char], offset: usize) -> usize {
    let mut start = line_start_at(chars, offset.min(chars.len()));
    while start > 0 {
        let prev_start = line_start_at(chars, start - 1);
        let line: String = chars[prev_start..start - 1].iter().collect();
        if !line.trim_start().starts_with("//") {
            break;
        }
        start = prev_start;
    }
    start
}

/// Offset of the start of the line containing `offset`.
fn line_start_at(chars: &[char], offset: usize) -> usize {
    chars[..offset]
        .iter()
        .rposition(|&c| c == '\n')
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Whether the line starting at `at` is blank (or the source ends there).
fn starts_blank_line(chars: &[char], at: usize) -> bool {
    chars[at.min(chars.len())..]
        .iter()
        .take_while(|&&c| c != '\n')
        .all(|c| c.is_whitespace())
}

/// Whether the line *ending* at `at` (exclusive) is blank.
fn ends_blank_line(chars: &[char], at: usize) -> bool {
    let start = line_start_at(chars, at.saturating_sub(1));
    chars[start..at.saturating_sub(1).max(start)]
        .iter()
        .all(|c| c.is_whitespace())
}

/// Replace `chars[start..end]` with `text`, rebuilding the source.
fn splice_text(chars: &[char], start: usize, end: usize, text: &str) -> String {
    let mut out: String = chars[..start].iter().collect();
    out.push_str(text);
    out.extend(chars[end..].iter());
    out
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
fn ensure_call(
    source: &str,
    function: &str,
    params: &[StaticValue],
    placement: &Placement,
) -> Result<String, GoalError> {
    let replacement = render_call(function, params);
    let (tree, stmts) = parse_ast(source)?;
    match find_call(&stmts, function) {
        Some(span) => Ok(replace_span(&tree, source, span, &replacement)),
        None => Ok(insert_statement(source, &replacement, placement, &stmts)),
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
///
/// A goal that already holds writes nothing at all: the source is returned as
/// it came in, down to the byte. A caller that rewrites every field of a config
/// file on every save (the honest way to do it — "differs from the default" and
/// "differs from what the file says" are different questions) therefore only
/// touches the lines that actually moved.
fn ensure_binding(
    source: &str,
    name: &str,
    value: &StaticValue,
    placement: &Placement,
) -> Result<String, GoalError> {
    if get_static_value(source, name).as_ref() == Ok(value) {
        return Ok(source.to_string());
    }
    let (tree, stmts) = parse_ast(source)?;
    match find_binding(&stmts, name) {
        // The value sits after `let name = ` at depth 1, so a multi-line
        // composite indents its elements one level in.
        Some(span) => Ok(replace_span(&tree, source, span, &value.render(1))),
        None => Ok(insert_statement(
            source,
            &format!("let {name} = {}", value.render(1)),
            placement,
            &stmts,
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
                "configure",
                vec![StaticValue::str("size"), StaticValue::int(14)],
            )],
        );
        assert_eq!(out, "configure(\"size\", 14)\n");
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

    // ── A goal that already holds ────────────────────────────────────────

    #[test]
    fn a_goal_that_already_holds_leaves_the_source_byte_identical() {
        // The spelling of a float is not recoverable from its f64, so a save
        // that rewrites every field would renormalize lines nobody touched.
        // A goal is a statement about the outcome, and this outcome already
        // holds, so there is nothing to write.
        let src = "let drag_axial = 0.020000\n";
        let out = apply(src, &[Goal::should_set_value("drag_axial", 0.02)]);
        assert_eq!(out, src);

        // Same for the layout of a composite and the spelling of a string.
        let src = "let editor = {   line_numbers: true }\n";
        let out = apply(
            src,
            &[Goal::should_set_value(
                "editor",
                StaticValue::record(vec![("line_numbers", StaticValue::bool(true))]),
            )],
        );
        assert_eq!(out, src);
    }

    #[test]
    fn a_goal_that_does_not_hold_still_writes() {
        // The no-op is exact, not approximate: a value that differs at all is
        // written, and a binding that can't be read statically is collapsed.
        let out = apply(
            "let drag_axial = 0.020001\n",
            &[Goal::should_set_value("drag_axial", 0.02)],
        );
        assert_eq!(out, "let drag_axial = 0.02\n");

        let out = apply("let size = 12 + 2\n", &[Goal::should_set_value("size", 14)]);
        assert_eq!(out, "let size = 14\n");

        // An int is not a float, even at the same numeric value: the file says
        // which kind of number a field holds and a save must not change that.
        let out = apply("let scale = 2\n", &[Goal::should_set_value("scale", 2.0)]);
        assert_eq!(out, "let scale = 2.0\n");
    }

    #[test]
    fn every_goal_holding_leaves_a_whole_file_untouched() {
        // The property a config-file save depends on: writing back exactly what
        // was read changes nothing, comments, spacing and spelling included.
        let src = "\
// movement — tuning
//
// generated

// How much the hull drags.
let drag_axial = 0.020000

// Walking speed.
let walk_speed  = 3.50
";
        let out = apply(
            src,
            &[
                Goal::should_set_value("drag_axial", 0.02),
                Goal::should_set_value("walk_speed", 3.5),
            ],
        );
        assert_eq!(out, src);
    }

    // ── Placement ────────────────────────────────────────────────────────

    #[test]
    fn after_inserts_below_the_anchor_keeping_the_blank_line_style() {
        let src = "\
// The longest the tether pays out.
let tether_max_m = 12.0

// Something else.
let other = 1
";
        let out = apply(
            src,
            &[Goal::should_set_value("tether_slack_m", 0.5).after("tether_max_m")],
        );
        assert_eq!(
            out,
            "\
// The longest the tether pays out.
let tether_max_m = 12.0

let tether_slack_m = 0.5

// Something else.
let other = 1
"
        );
    }

    #[test]
    fn after_a_tightly_packed_anchor_does_not_invent_a_blank_line() {
        let out = apply(
            "let a = 1\nlet b = 2\n",
            &[Goal::should_set_value("c", 3).after("a")],
        );
        assert_eq!(out, "let a = 1\nlet c = 3\nlet b = 2\n");
    }

    #[test]
    fn after_keeps_a_trailing_comment_with_its_own_binding() {
        let out = apply(
            "let a = 1 // note\nlet b = 2\n",
            &[Goal::should_set_value("c", 3).after("a")],
        );
        assert_eq!(out, "let a = 1 // note\nlet c = 3\nlet b = 2\n");
    }

    #[test]
    fn before_inserts_above_the_anchors_doc_comment() {
        // Above the comment, not between it and its binding — otherwise the new
        // binding steals the documentation of the one it was placed against.
        let src = "\
let first = 1

// The longest the tether pays out.
let tether_max_m = 12.0
";
        let out = apply(
            src,
            &[Goal::should_set_value("tether_slack_m", 0.5).before("tether_max_m")],
        );
        assert_eq!(
            out,
            "\
let first = 1

let tether_slack_m = 0.5

// The longest the tether pays out.
let tether_max_m = 12.0
"
        );
    }

    #[test]
    fn before_the_first_binding_inserts_at_the_top() {
        let out = apply("let a = 1\n", &[Goal::should_set_value("b", 2).before("a")]);
        assert_eq!(out, "let b = 2\n\nlet a = 1\n");
    }

    #[test]
    fn a_missing_anchor_falls_back_to_appending() {
        // A placement can misplace a statement; it must never lose one.
        let out = apply(
            "let a = 1\n",
            &[Goal::should_set_value("b", 2).after("nonexistent")],
        );
        assert_eq!(out, "let a = 1\n\nlet b = 2\n");
    }

    #[test]
    fn placement_is_ignored_when_the_binding_already_exists() {
        // Nothing is inserted, so there is nowhere to place it: the existing
        // binding is edited where the author put it.
        let out = apply(
            "let b = 1\nlet a = 2\n",
            &[Goal::should_set_value("b", 5).after("a")],
        );
        assert_eq!(out, "let b = 5\nlet a = 2\n");
    }

    #[test]
    fn placement_works_for_calls_and_against_a_call_anchor() {
        let out = apply(
            "setup()\n\nteardown()\n",
            &[Goal::should_call("middle", [1]).after("setup")],
        );
        assert_eq!(out, "setup()\n\nmiddle(1)\n\nteardown()\n");
    }

    #[test]
    fn goals_placed_after_each_other_build_up_in_order() {
        // How a host keeps a generated file's ordering: each field anchors on
        // the one before it, including ones an earlier goal just inserted.
        // Inserting past the end of the file separates with a blank line, the
        // same as appending does.
        let out = apply(
            "let a = 1\n",
            &[
                Goal::should_set_value("b", 2).after("a"),
                Goal::should_set_value("c", 3).after("b"),
            ],
        );
        assert_eq!(out, "let a = 1\n\nlet b = 2\n\nlet c = 3\n");

        // And in the middle of a file the anchor's own spacing decides.
        let out = apply(
            "let a = 1\nlet z = 9\n",
            &[
                Goal::should_set_value("b", 2).after("a"),
                Goal::should_set_value("c", 3).after("b"),
            ],
        );
        assert_eq!(out, "let a = 1\nlet b = 2\nlet c = 3\nlet z = 9\n");
    }

    #[test]
    fn placement_survives_multibyte_source() {
        let out = apply(
            "// café ☕\nlet a = 1\n\nlet z = 2\n",
            &[Goal::should_set_value("b", 2).after("a")],
        );
        assert_eq!(out, "// café ☕\nlet a = 1\n\nlet b = 2\n\nlet z = 2\n");
    }

    #[test]
    fn should_set_value_reports_unparseable_source() {
        let err =
            modify_source_with_goals("let x = \n", &[Goal::should_set_value("x", 1)]).unwrap_err();
        assert!(!err.message.is_empty());
    }
}
