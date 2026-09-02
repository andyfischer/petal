//! Statically-known Petal values — the data type a `.ptl` file carries when it
//! is used as a **configuration format**, in both directions.
//!
//! A config-style Petal file is mostly `let` bindings:
//!
//! ```text
//! let color_scheme = "dracula"
//! let font_size    = 14
//! let editor       = { line_numbers: true, tab_width: 4 }
//! ```
//!
//! [`StaticValue`] is the Rust mirror of one of those right-hand sides.
//! [`get_static_value`] reads one out of source **without running the program**
//! (hence *static*: no `Env`, no heap, no side effects), and the same type is
//! what an edit writes back — [`crate::goal_based_editing`] renders it into
//! well-formed Petal for `Goal::should_set_value` / `Goal::should_call`. Reading
//! and writing share one type, so a value round-trips:
//!
//! ```ignore
//! use petal::static_value::get_static_value;
//! use petal::goal_based_editing::{modify_source_with_goals, Goal};
//!
//! let scheme = get_static_value(&source, "color_scheme")?;      // StaticValue::Str("dracula")
//! let source = modify_source_with_goals(&source, &[
//!     Goal::should_set_value("color_scheme", "nord"),
//! ])?;                                                          // let color_scheme = "nord"
//! ```
//!
//! # What counts as static
//!
//! Literals (int, float, string, bool, `nil`), negation and `not` applied to
//! them, list and record literals of static elements, and references to a name
//! bound to a static value **earlier in the file**. A **call** is also static:
//! it is captured unevaluated as [`StaticValue::Call`], because in a config file
//! a call is a constructor the host interprets (`rgb(255, 0, 0)`,
//! `editor("a.rs")`) rather than a computation to run.
//!
//! Everything else — arithmetic, string interpolation, `if`/`match`, lambdas,
//! field access, `state` declarations, `fn` declarations — is **not** static and
//! is reported as [`StaticValueError::NotStatic`] rather than silently skipped.

use std::collections::BTreeMap;

use crate::ast::{AssignTarget, Expr, ExprKind, Literal, RecordField, StmtKind, UnaryOp};
use crate::cst::parse_source;
use crate::source_map::ENTRY_FILE;

/// A statically-known Petal value: what a config binding holds, and what an
/// edit writes back.
///
/// Use the typed variants (via the [`From`] impls or the constructors); strings
/// are quoted/escaped for you and every variant renders to well-formed Petal.
#[derive(Debug, Clone, PartialEq)]
pub enum StaticValue {
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
    List(Vec<StaticValue>),
    /// A record literal `{ key: value, ... }`, rendered inline. Keys are
    /// rendered bare, so they must be valid Petal identifiers.
    Record(Vec<(String, StaticValue)>),
    /// A call `function(args...)`, held unevaluated — the building block for
    /// declarative call trees like `layout(row([editor("a")], [1.0]))`, and how
    /// a config file names a host-interpreted constructor.
    Call {
        function: String,
        args: Vec<StaticValue>,
    },
}

impl StaticValue {
    /// A string value (quoted and escaped on render).
    pub fn str(s: impl Into<String>) -> StaticValue {
        StaticValue::Str(s.into())
    }
    /// An integer value.
    pub fn int(n: impl Into<i64>) -> StaticValue {
        StaticValue::Int(n.into())
    }
    /// A float value.
    pub fn float(f: impl Into<f64>) -> StaticValue {
        StaticValue::Float(f.into())
    }
    /// A boolean value.
    pub fn bool(b: bool) -> StaticValue {
        StaticValue::Bool(b)
    }
    /// The `nil` value.
    pub fn nil() -> StaticValue {
        StaticValue::Nil
    }
    /// A list value. Elements coerce like scalars do.
    pub fn list<P, A>(items: P) -> StaticValue
    where
        P: IntoIterator<Item = A>,
        A: Into<StaticValue>,
    {
        StaticValue::List(items.into_iter().map(Into::into).collect())
    }
    /// A record value. Keys must be valid Petal identifiers (they render
    /// bare); values coerce like scalars do.
    pub fn record<P, K, A>(fields: P) -> StaticValue
    where
        P: IntoIterator<Item = (K, A)>,
        K: Into<String>,
        A: Into<StaticValue>,
    {
        StaticValue::Record(
            fields
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
    /// A call value: `StaticValue::call("editor", ["a.rs"])` renders as
    /// `editor("a.rs")`.
    pub fn call<S, P, A>(function: S, args: P) -> StaticValue
    where
        S: Into<String>,
        P: IntoIterator<Item = A>,
        A: Into<StaticValue>,
    {
        StaticValue::Call {
            function: function.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// Render this value as Petal source, starting at column 0.
    ///
    /// The inverse of [`get_static_value`] for everything that module can read,
    /// so a value read out of a file renders back to equivalent source.
    pub fn to_source(&self) -> String {
        self.render(0)
    }

    /// Render this value as Petal source. `depth` is the current indent level
    /// in two-space units; it only matters for multi-line lists (see
    /// [`StaticValue::List`]) — scalars ignore it.
    pub(crate) fn render(&self, depth: usize) -> String {
        match self {
            StaticValue::Str(s) => render_string_literal(s),
            StaticValue::Int(n) => n.to_string(),
            // `{:?}` on f64 always emits a decimal point (`1.0`, not `1`), so the
            // result parses as a float rather than an int.
            StaticValue::Float(f) => format!("{f:?}"),
            StaticValue::Bool(true) => "true".to_string(),
            StaticValue::Bool(false) => "false".to_string(),
            StaticValue::Nil => "nil".to_string(),
            StaticValue::List(items) => render_list(items, depth),
            StaticValue::Record(fields) => render_record(fields, depth),
            StaticValue::Call { function, args } => render_call_at(function, args, depth),
        }
    }

    /// True for the composite variants whose rendering can span lines; a list
    /// containing any of these is laid out one element per line.
    fn is_composite(&self) -> bool {
        matches!(
            self,
            StaticValue::List(_) | StaticValue::Record(_) | StaticValue::Call { .. }
        )
    }
}

/// Render a list literal at `depth`. All-scalar lists stay inline
/// (`[0.7, 0.3]`); a list with composite elements puts each element on its own
/// line at `depth + 1`, with the closing bracket back at `depth` — the layout a
/// user would write for a tree of nested calls.
fn render_list(items: &[StaticValue], depth: usize) -> String {
    if !items.iter().any(StaticValue::is_composite) {
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
fn render_record(fields: &[(String, StaticValue)], depth: usize) -> String {
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
pub(crate) fn render_call_at(function: &str, args: &[StaticValue], depth: usize) -> String {
    let rendered: Vec<String> = args.iter().map(|a| a.render(depth)).collect();
    format!("{function}({})", rendered.join(", "))
}

/// Two spaces per `depth` level.
fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

impl From<&str> for StaticValue {
    fn from(s: &str) -> StaticValue {
        StaticValue::Str(s.to_string())
    }
}
impl From<String> for StaticValue {
    fn from(s: String) -> StaticValue {
        StaticValue::Str(s)
    }
}
impl From<i64> for StaticValue {
    fn from(n: i64) -> StaticValue {
        StaticValue::Int(n)
    }
}
impl From<i32> for StaticValue {
    fn from(n: i32) -> StaticValue {
        StaticValue::Int(n as i64)
    }
}
impl From<f64> for StaticValue {
    fn from(f: f64) -> StaticValue {
        StaticValue::Float(f)
    }
}
impl From<f32> for StaticValue {
    fn from(f: f32) -> StaticValue {
        // A plain `as f64` widening drags in garbage digits (0.7f32 becomes
        // 0.7000000298023224); round-tripping through the shortest display
        // form keeps the literal the user actually meant.
        StaticValue::Float(format!("{f}").parse().unwrap_or(f as f64))
    }
}
impl From<bool> for StaticValue {
    fn from(b: bool) -> StaticValue {
        StaticValue::Bool(b)
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

// ── Reading values out of source ─────────────────────────────────────────

/// Why [`get_static_value`] could not produce a value. The three cases are kept
/// distinct because a caller reacts differently to each: a parse failure is a
/// broken file, a missing name usually means "fall back to the default", and a
/// non-static binding is a real value the caller simply can't read without
/// running the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticValueError {
    /// The source didn't parse; wraps the parse error message.
    Parse(String),
    /// No top-level binding of this name exists in the source.
    NotFound { name: String },
    /// The name is bound, but its value isn't statically known.
    NotStatic { name: String, reason: String },
}

impl std::fmt::Display for StaticValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StaticValueError::Parse(msg) => write!(f, "source did not parse: {msg}"),
            StaticValueError::NotFound { name } => {
                write!(f, "no top-level binding for `{name}`")
            }
            StaticValueError::NotStatic { name, reason } => {
                write!(f, "`{name}` is not a static value: {reason}")
            }
        }
    }
}

impl std::error::Error for StaticValueError {}

/// Read the value bound to top-level `name` in `source`, without running it.
///
/// The **last** top-level binding of `name` wins, matching what the program
/// would end up with (and matching what `Goal::should_set_value` edits). Both
/// binding forms count — `let name = …` and a bare `name = …` rebinding.
///
/// ```ignore
/// let scheme = get_static_value(&source, "color_scheme")?;  // StaticValue::Str("dracula")
/// ```
///
/// Errors distinguish "didn't parse", "not bound", and "bound but not static";
/// see [`StaticValueError`] and the module docs for what counts as static.
pub fn get_static_value(source: &str, name: &str) -> Result<StaticValue, StaticValueError> {
    let bindings = eval_top_level(source)?;
    match lookup(&bindings, name) {
        Some(Ok(value)) => Ok(value.clone()),
        Some(Err(reason)) => Err(StaticValueError::NotStatic {
            name: name.to_string(),
            reason: reason.clone(),
        }),
        None => Err(StaticValueError::NotFound {
            name: name.to_string(),
        }),
    }
}

/// Read **every** statically-known top-level binding in `source` — the whole
/// config file in one pass, for a host that wants the values rather than a
/// specific name.
///
/// Names whose final binding isn't static are omitted rather than failing the
/// call: a config file that also declares functions or `state` still yields its
/// readable settings. Use [`get_static_value`] when a specific name's
/// unreadability is itself an error worth reporting, or [`static_bindings`] when
/// the *omitted* names matter too — telling "you wrote this in a form I can't
/// read" apart from "you didn't write it" needs both halves.
pub fn static_values(source: &str) -> Result<BTreeMap<String, StaticValue>, StaticValueError> {
    let bindings = eval_top_level(source)?;
    Ok(bindings
        .into_iter()
        .filter_map(|binding| {
            binding
                .value
                .ok()
                .map(|value| (binding.name.clone(), value))
        })
        .collect())
}

/// One top-level binding, with everything the source says about it: its value
/// *or* why it has none, the right-hand side as written, and the comment block
/// above it.
///
/// This is what [`static_bindings`] returns. It exists because a host reading a
/// config file needs more than the readable values: it has to report the
/// unreadable ones (a binding that is silently skipped looks exactly like a
/// binding that was never written), it may want to show the author's note next
/// to the value in its own UI, and it may want to re-render a number the way the
/// author spelled it.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticBinding {
    /// The bound name.
    pub name: String,
    /// The static value, or the reason there isn't one — the same noun phrase
    /// [`StaticValueError::NotStatic`] carries ("an arithmetic or comparison
    /// expression, which needs evaluating").
    pub value: Result<StaticValue, String>,
    /// The right-hand side exactly as it was written, whitespace and all.
    /// `None` for a `fn` or `state` declaration, which has no right-hand side
    /// in this sense.
    ///
    /// Keeping the source text is what lets a host write a value back in the
    /// author's own spelling: `0.020000` and `0.02` are the same `f64`, so
    /// without the text a writer cannot tell them apart.
    pub text: Option<String>,
    /// The comment block immediately above the binding — the lines' `//`
    /// markers and one following space stripped, joined with newlines. A blank
    /// line or any code between the comment and the binding ends the block, so
    /// a file header does not attach itself to the first binding. `None` when
    /// there is no such comment.
    pub comment: Option<String>,
}

/// Every top-level name `source` binds, in source order, each carrying either
/// its static value or the reason it has none — plus the source text and
/// leading comment of the binding (see [`StaticBinding`]).
///
/// The richer counterpart to [`static_values`]: same parse, same
/// static-evaluation rules, nothing dropped on the way out. A name bound more
/// than once appears once, at its first position, carrying its **last**
/// binding — the value the program ends up with.
///
/// ```ignore
/// for binding in static_bindings(&source)? {
///     match binding.value {
///         Ok(value) => apply(&binding.name, value),
///         // Reportable, rather than indistinguishable from "not mentioned".
///         Err(reason) => warn!("`{}` is not static: {reason}", binding.name),
///     }
/// }
/// ```
pub fn static_bindings(source: &str) -> Result<Vec<StaticBinding>, StaticValueError> {
    eval_top_level(source)
}

/// Every top-level name the source binds, in source order, each carrying either
/// its static value or the reason it doesn't have one. A name bound more than
/// once keeps only its last binding (later entries overwrite earlier ones), so
/// the result describes the program's end state.
fn eval_top_level(source: &str) -> Result<Vec<StaticBinding>, StaticValueError> {
    let (_tree, stmts) = parse_source(source, ENTRY_FILE).map_err(StaticValueError::Parse)?;
    let chars: Vec<char> = source.chars().collect();
    let mut bindings: Vec<StaticBinding> = Vec::new();
    for stmt in &stmts {
        let (name, value, text) = match &stmt.kind {
            StmtKind::Let { name, value, .. } => (
                name.clone(),
                eval(value, &bindings),
                Some(span_text(&chars, value.span)),
            ),
            StmtKind::Assign {
                target: AssignTarget::Name(name),
                value,
            } => (
                name.clone(),
                eval(value, &bindings),
                Some(span_text(&chars, value.span)),
            ),
            // A `fn` declares a name too, so report it as bound-but-not-static
            // rather than letting a lookup say "not found" about a name that is
            // plainly there in the file.
            StmtKind::FnDecl { name, .. } => {
                (name.clone(), Err("a function declaration".into()), None)
            }
            // `state` is runtime state, not configuration: its value only exists
            // once the program runs, and it changes as the program runs.
            StmtKind::State { name, .. } => (
                name.clone(),
                Err("a `state` declaration, whose value only exists at run time".into()),
                None,
            ),
            _ => continue,
        };
        let comment = leading_comment(&chars, stmt.span.start.offset as usize);
        set_binding(
            &mut bindings,
            StaticBinding {
                name,
                value,
                text,
                comment,
            },
        );
    }
    Ok(bindings)
}

/// The source text a span covers. Spans are **char** offsets (the lexer indexes
/// source as `Vec<char>`), so this slices chars, not bytes.
fn span_text(chars: &[char], span: crate::source_map::SourceSpan) -> String {
    let start = (span.start.offset as usize).min(chars.len());
    let end = (span.end.offset as usize).clamp(start, chars.len());
    chars[start..end].iter().collect()
}

/// The comment block directly above the statement starting at `offset`: the run
/// of whole-line `//` comments immediately preceding it, with markers stripped
/// and lines joined by newlines.
///
/// The run stops at the first line that isn't a comment — including a blank one,
/// so a file header separated by a blank line stays a file header rather than
/// becoming the first binding's doc comment. A comment trailing code on its own
/// line (`let a = 1 // note`) is not a whole-line comment and stops the run too.
fn leading_comment(chars: &[char], offset: usize) -> Option<String> {
    let mut line_start = line_start_at(chars, offset.min(chars.len()));
    let mut lines: Vec<String> = Vec::new();
    while line_start > 0 {
        let prev_end = line_start - 1; // the '\n' ending the previous line
        let prev_start = line_start_at(chars, prev_end);
        let line: String = chars[prev_start..prev_end].iter().collect();
        let trimmed = line.trim();
        let Some(body) = trimmed.strip_prefix("//") else {
            break;
        };
        lines.push(body.strip_prefix(' ').unwrap_or(body).to_string());
        line_start = prev_start;
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(lines.join("\n"))
}

/// Offset of the start of the line containing `offset`.
fn line_start_at(chars: &[char], offset: usize) -> usize {
    chars[..offset]
        .iter()
        .rposition(|&c| c == '\n')
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// The current binding for `name`, or `None` if it isn't bound.
fn lookup<'a>(
    bindings: &'a [StaticBinding],
    name: &str,
) -> Option<&'a Result<StaticValue, String>> {
    bindings
        .iter()
        .find(|binding| binding.name == name)
        .map(|binding| &binding.value)
}

/// Bind a name, replacing any earlier binding of it in place (so the entry
/// keeps its original source position but carries the latest value).
fn set_binding(bindings: &mut Vec<StaticBinding>, binding: StaticBinding) {
    match bindings.iter_mut().find(|entry| entry.name == binding.name) {
        Some(entry) => *entry = binding,
        None => bindings.push(binding),
    }
}

/// Statically evaluate `expr` against the names bound above it, or explain why
/// it has no static value. The `Err` string is a noun phrase that completes
/// "`x` is not a static value: …".
fn eval(expr: &Expr, bindings: &[StaticBinding]) -> Result<StaticValue, String> {
    match &expr.kind {
        ExprKind::Literal(lit) => Ok(match lit {
            Literal::Nil => StaticValue::Nil,
            Literal::Bool(b) => StaticValue::Bool(*b),
            Literal::Int(n) => StaticValue::Int(*n),
            Literal::Float(f) => StaticValue::Float(*f),
            Literal::String(s) => StaticValue::Str(s.clone()),
        }),
        // Negation and `not` fold into the literal so `-1` reads as Int(-1)
        // rather than as an unreadable expression.
        ExprKind::UnaryOp { op, operand } => match (op, eval(operand, bindings)?) {
            (UnaryOp::Neg, StaticValue::Int(n)) => Ok(StaticValue::Int(-n)),
            (UnaryOp::Neg, StaticValue::Float(f)) => Ok(StaticValue::Float(-f)),
            (UnaryOp::Not, StaticValue::Bool(b)) => Ok(StaticValue::Bool(!b)),
            (_, value) => Err(format!(
                "an operator applied to {}, which it can't fold",
                describe_value(&value)
            )),
        },
        ExprKind::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(eval(item, bindings)?);
            }
            Ok(StaticValue::List(out))
        }
        ExprKind::Record(fields) => {
            let mut out = Vec::with_capacity(fields.len());
            for field in fields {
                match field {
                    RecordField::Named(key, value) => {
                        out.push((key.clone(), eval(value, bindings)?))
                    }
                    RecordField::Spread(_) => {
                        return Err("a record with a `...` spread, which needs evaluating".into());
                    }
                }
            }
            Ok(StaticValue::Record(out))
        }
        // A call is captured unevaluated: in a config file it names a
        // host-interpreted constructor, not a computation to run.
        ExprKind::Call {
            function,
            args,
            arg_names,
        } => {
            let ExprKind::Ident(name) = &function.kind else {
                return Err("a call through an expression rather than a plain name".into());
            };
            if !arg_names.is_empty() {
                return Err("a named argument".into());
            }
            let mut out = Vec::with_capacity(args.len());
            for arg in args {
                out.push(eval(arg, bindings)?);
            }
            Ok(StaticValue::Call {
                function: name.clone(),
                args: out,
            })
        }
        // A reference resolves only against names bound *above* it — the same
        // order the program itself would see.
        ExprKind::Ident(name) => match lookup(bindings, name) {
            Some(Ok(value)) => Ok(value.clone()),
            Some(Err(reason)) => Err(format!("a reference to `{name}`, which is {reason}")),
            None => Err(format!(
                "a reference to `{name}`, which no binding above it defines"
            )),
        },
        other => Err(format!("{}, which needs evaluating", describe_expr(other))),
    }
}

/// A noun phrase naming the shape of a non-static expression, for the error.
fn describe_expr(kind: &ExprKind) -> &'static str {
    match kind {
        ExprKind::BinaryOp { .. } => "an arithmetic or comparison expression",
        ExprKind::StringInterp { .. } => "an interpolated string",
        ExprKind::If { .. } => "an `if` expression",
        ExprKind::Match { .. } => "a `match` expression",
        ExprKind::For { .. } => "a `for` expression",
        ExprKind::Lambda { .. } => "a lambda",
        ExprKind::Block(_) => "a block",
        ExprKind::FieldAccess { .. } => "a field access",
        ExprKind::IndexAccess { .. } => "an index access",
        ExprKind::Element { .. } => "an element literal",
        ExprKind::AtVar(_) => "an `@` in-out marker",
        _ => "an expression",
    }
}

/// A noun phrase naming a value's type, for the unary-operator error.
fn describe_value(value: &StaticValue) -> &'static str {
    match value {
        StaticValue::Str(_) => "a string",
        StaticValue::Int(_) => "an integer",
        StaticValue::Float(_) => "a float",
        StaticValue::Bool(_) => "a boolean",
        StaticValue::Nil => "nil",
        StaticValue::List(_) => "a list",
        StaticValue::Record(_) => "a record",
        StaticValue::Call { .. } => "a call",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(source: &str, name: &str) -> StaticValue {
        get_static_value(source, name).unwrap()
    }

    #[test]
    fn reads_scalar_lets() {
        let src = "let scheme = \"dracula\"\nlet size = 14\nlet scale = 1.5\nlet wrap = true\nlet limit = nil\n";
        assert_eq!(get(src, "scheme"), StaticValue::str("dracula"));
        assert_eq!(get(src, "size"), StaticValue::int(14));
        assert_eq!(get(src, "scale"), StaticValue::float(1.5));
        assert_eq!(get(src, "wrap"), StaticValue::bool(true));
        assert_eq!(get(src, "limit"), StaticValue::nil());
    }

    #[test]
    fn reads_bare_assignment_form() {
        // A config file may use `x = 1` rather than `let x = 1`; both bind.
        assert_eq!(get("size = 14\n", "size"), StaticValue::int(14));
    }

    #[test]
    fn last_binding_wins() {
        // The value the program ends up with is the one reported — the same
        // binding `Goal::should_set_value` edits.
        let src = "let size = 12\nlet size = 14\nsize = 16\n";
        assert_eq!(get(src, "size"), StaticValue::int(16));
    }

    #[test]
    fn a_later_non_static_rebinding_wins_too() {
        // `size` ends up non-static even though an earlier binding was readable;
        // reporting the stale 12 would be a lie about the program's end state.
        let err = get_static_value("let size = 12\nlet size = base + 1\n", "size").unwrap_err();
        assert!(
            matches!(&err, StaticValueError::NotStatic { name, .. } if name == "size"),
            "got: {err}"
        );
    }

    #[test]
    fn reads_negative_numbers() {
        let src = "let offset = -3\nlet bias = -0.5\n";
        assert_eq!(get(src, "offset"), StaticValue::int(-3));
        assert_eq!(get(src, "bias"), StaticValue::float(-0.5));
    }

    #[test]
    fn reads_lists_and_records() {
        let src = "let ratios = [0.6, 0.4]\nlet editor = { line_numbers: true, tab_width: 4 }\n";
        assert_eq!(get(src, "ratios"), StaticValue::list([0.6f64, 0.4f64]));
        assert_eq!(
            get(src, "editor"),
            StaticValue::record(vec![
                ("line_numbers", StaticValue::bool(true)),
                ("tab_width", StaticValue::int(4)),
            ])
        );
    }

    #[test]
    fn reads_nested_composites() {
        let src = "let panes = [{ file: \"a.rs\" }, { file: \"b.rs\" }]\n";
        assert_eq!(
            get(src, "panes"),
            StaticValue::list([
                StaticValue::record(vec![("file", StaticValue::str("a.rs"))]),
                StaticValue::record(vec![("file", StaticValue::str("b.rs"))]),
            ])
        );
    }

    #[test]
    fn a_call_is_static_and_held_unevaluated() {
        // In a config file a call names a host-interpreted constructor, so it
        // reads back as a Call rather than being rejected or run.
        assert_eq!(
            get("let accent = rgb(255, 0, 0)\n", "accent"),
            StaticValue::call("rgb", [255, 0, 0])
        );
    }

    #[test]
    fn resolves_a_reference_to_an_earlier_binding() {
        let src = "let base = 14\nlet heading = base\n";
        assert_eq!(get(src, "heading"), StaticValue::int(14));
    }

    #[test]
    fn a_reference_to_a_later_binding_is_not_static() {
        // Only names bound *above* resolve — the order the program itself sees.
        let err = get_static_value("let heading = base\nlet base = 14\n", "heading").unwrap_err();
        assert!(
            err.to_string().contains("no binding above it"),
            "got: {err}"
        );
    }

    #[test]
    fn comments_and_layout_do_not_affect_reading() {
        let src = "// user config\n\nlet size = 14 // points\n";
        assert_eq!(get(src, "size"), StaticValue::int(14));
    }

    #[test]
    fn missing_name_is_not_found() {
        let err = get_static_value("let size = 14\n", "scheme").unwrap_err();
        assert_eq!(
            err,
            StaticValueError::NotFound {
                name: "scheme".to_string()
            }
        );
        assert_eq!(err.to_string(), "no top-level binding for `scheme`");
    }

    #[test]
    fn computed_bindings_are_not_static() {
        for (src, name) in [
            ("let size = 12 + 2\n", "size"),
            ("let n = 1\nlet label = \"size {n}\"\n", "label"),
            ("let size = if wide_screen then 14 else 12 end\n", "size"),
            ("let f = fn(x) x end\n", "f"),
            ("let size = config.size\n", "size"),
            ("let size = sizes[0]\n", "size"),
            ("let opts = { ...defaults, tab_width: 4 }\n", "opts"),
        ] {
            let err = get_static_value(src, name).unwrap_err();
            assert!(
                matches!(err, StaticValueError::NotStatic { .. }),
                "expected NotStatic for {src:?}, got: {err}"
            );
        }
    }

    #[test]
    fn declarations_report_as_bound_but_not_static() {
        // A `fn` or `state` name is present in the file, so saying "not found"
        // would misdirect the caller — it's bound, just not readable statically.
        let err = get_static_value("fn size() 14 end\n", "size").unwrap_err();
        assert!(
            err.to_string().contains("function declaration"),
            "got: {err}"
        );
        let err = get_static_value("state count = 0\n", "count").unwrap_err();
        assert!(err.to_string().contains("run time"), "got: {err}");
    }

    #[test]
    fn nested_bindings_are_invisible() {
        // Only top-level bindings are configuration; one inside a function body
        // belongs to that body's scope.
        let err = get_static_value("fn f() let size = 14 end\n", "size").unwrap_err();
        assert!(
            matches!(err, StaticValueError::NotFound { .. }),
            "got: {err}"
        );
    }

    #[test]
    fn unparseable_source_is_a_parse_error() {
        let err = get_static_value("let size = \n", "size").unwrap_err();
        assert!(matches!(err, StaticValueError::Parse(_)), "got: {err}");
    }

    #[test]
    fn static_values_reads_the_whole_file() {
        let src =
            "let scheme = \"dracula\"\nlet size = 14\nstate count = 0\nlet computed = size + 1\n";
        let values = static_values(src).unwrap();
        // `count` (runtime state) and `computed` (needs evaluating) are omitted
        // rather than failing the whole read.
        assert_eq!(values.len(), 2);
        assert_eq!(values["scheme"], StaticValue::str("dracula"));
        assert_eq!(values["size"], StaticValue::int(14));
    }

    #[test]
    fn static_values_reports_a_parse_failure() {
        assert!(matches!(
            static_values("let size = \n").unwrap_err(),
            StaticValueError::Parse(_)
        ));
    }

    #[test]
    fn values_round_trip_through_source() {
        // Everything readable renders back to source that reads as the same
        // value — the property that makes read-modify-write safe.
        let src = "let a = \"x\"\nlet b = 14\nlet c = 1.5\nlet d = true\nlet e = nil\n\
                   let f = [1, 2]\nlet g = { k: \"v\" }\nlet h = rgb(1, 2, 3)\nlet i = -3\n";
        for name in ["a", "b", "c", "d", "e", "f", "g", "h", "i"] {
            let value = get(src, name);
            let rendered = format!("let {name} = {}\n", value.to_source());
            assert_eq!(get(&rendered, name), value, "round-tripping {name}");
        }
    }

    #[test]
    fn round_trips_a_string_needing_escapes() {
        let value = StaticValue::str("a\"b\\c{d}");
        let rendered = format!("let s = {}\n", value.to_source());
        assert_eq!(get(&rendered, "s"), value);
    }

    #[test]
    fn reads_the_documented_example_config() {
        // The example file in docs/config-files.md, pinned so the doc can't
        // drift away from what the reader actually does.
        let src = "\
// ~/.garden/config.ptl

let color_scheme = \"dracula\"
let font_size    = 14
let editor       = { line_numbers: true, tab_width: 4 }
let accent       = rgb(255, 128, 0)
let recent       = [\"a.rs\", \"b.rs\"]
";
        let values = static_values(src).unwrap();
        assert_eq!(values.len(), 5);
        assert_eq!(values["color_scheme"], StaticValue::str("dracula"));
        assert_eq!(values["font_size"], StaticValue::int(14));
        assert_eq!(
            values["editor"],
            StaticValue::record(vec![
                ("line_numbers", StaticValue::bool(true)),
                ("tab_width", StaticValue::int(4)),
            ])
        );
        assert_eq!(values["accent"], StaticValue::call("rgb", [255, 128, 0]));
        assert_eq!(
            values["recent"],
            StaticValue::list([StaticValue::str("a.rs"), StaticValue::str("b.rs")])
        );
    }

    // ── static_bindings ──────────────────────────────────────────────────

    /// The binding named `name`, for asserting on one entry of a read.
    fn binding(source: &str, name: &str) -> StaticBinding {
        static_bindings(source)
            .unwrap()
            .into_iter()
            .find(|b| b.name == name)
            .unwrap_or_else(|| panic!("`{name}` should be bound"))
    }

    #[test]
    fn static_bindings_reports_the_names_static_values_omits() {
        // The point of the richer read: a name bound to something unreadable is
        // *present*, with a reason, so a host can say "your edit isn't taking
        // effect and here is why" rather than "you never wrote it".
        let src = "let size = 12 + 2\nstate count = 0\nfn f() 1 end\nlet ok = 3\n";
        let names: Vec<String> = static_bindings(src)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names, vec!["size", "count", "f", "ok"]);
        assert_eq!(
            binding(src, "size").value,
            Err("an arithmetic or comparison expression, which needs evaluating".to_string())
        );
        assert!(binding(src, "count").value.is_err());
        assert_eq!(
            binding(src, "f").value,
            Err("a function declaration".into())
        );
        assert_eq!(binding(src, "ok").value, Ok(StaticValue::int(3)));
    }

    #[test]
    fn static_bindings_keeps_the_right_hand_side_as_written() {
        // `0.020000` and `0.02` are the same f64, so only the text can tell a
        // host what the author actually typed.
        let src = "let drag = 0.020000\nlet size = 14\nfn f() 1 end\n";
        assert_eq!(binding(src, "drag").text.as_deref(), Some("0.020000"));
        assert_eq!(binding(src, "drag").value, Ok(StaticValue::float(0.02)));
        assert_eq!(binding(src, "size").text.as_deref(), Some("14"));
        // A declaration has no right-hand side in this sense.
        assert_eq!(binding(src, "f").text, None);
    }

    #[test]
    fn static_bindings_reads_the_comment_above_a_binding() {
        let src = "\
// How much the hull drags.
// 0.0 to 1.0
let drag = 0.02
";
        assert_eq!(
            binding(src, "drag").comment.as_deref(),
            Some("How much the hull drags.\n0.0 to 1.0")
        );
    }

    #[test]
    fn a_file_header_is_not_the_first_bindings_comment() {
        // A blank line ends the comment block, which is what keeps a header
        // from attaching itself to whatever binding happens to come first.
        let src = "// movement config\n// generated\n\nlet drag = 0.02\n";
        assert_eq!(binding(src, "drag").comment, None);

        // And code on the line above ends it too, trailing comment or not.
        let src = "let a = 1 // note\nlet b = 2\n";
        assert_eq!(binding(src, "b").comment, None);
    }

    #[test]
    fn a_binding_without_a_comment_has_none() {
        assert_eq!(binding("let a = 1\n", "a").comment, None);
    }

    #[test]
    fn static_bindings_keeps_the_last_binding_at_the_first_position() {
        // Position is where the name first appears (a host regenerating the
        // file keeps its order); the value, text and comment are the last
        // binding's, since that is what the program ends up with.
        let src = "let size = 12\nlet other = 1\n// later\nlet size = 14\n";
        let names: Vec<String> = static_bindings(src)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names, vec!["size", "other"]);
        let size = binding(src, "size");
        assert_eq!(size.value, Ok(StaticValue::int(14)));
        assert_eq!(size.text.as_deref(), Some("14"));
        assert_eq!(size.comment.as_deref(), Some("later"));
    }

    #[test]
    fn static_bindings_survives_multibyte_source() {
        // Spans are char offsets, so a multi-byte comment must not shift the
        // text slice.
        let src = "// café ☕ setting\nlet size = 14\n";
        let size = binding(src, "size");
        assert_eq!(size.text.as_deref(), Some("14"));
        assert_eq!(size.comment.as_deref(), Some("café ☕ setting"));
    }

    #[test]
    fn static_bindings_reports_a_parse_failure() {
        assert!(matches!(
            static_bindings("let size = \n").unwrap_err(),
            StaticValueError::Parse(_)
        ));
    }

    #[test]
    fn to_source_renders_at_column_zero() {
        assert_eq!(StaticValue::int(14).to_source(), "14");
        assert_eq!(StaticValue::str("hi").to_source(), "\"hi\"");
        assert_eq!(StaticValue::list([1, 2]).to_source(), "[1, 2]");
    }
}
