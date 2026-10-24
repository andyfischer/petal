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
//! them, list and record literals of static elements, element literals
//! (`<text y={400}>Hi</text>`, [`StaticValue::Element`]) whose attributes and
//! children are static, and references to a name bound to a static value
//! **earlier in the file**. A **call** is also static:
//! it is captured unevaluated as [`StaticValue::Call`], because in a config file
//! a call is a constructor the host interprets (`rgb(255, 0, 0)`,
//! `editor("a.rs")`) rather than a computation to run. So is a variant of an
//! `enum` the file itself declares — `Mode.dark`, `Shape.circle(2)` — which is
//! [`StaticValue::Variant`]: the declaration is right there, wherever in the
//! file it sits, and says what the variant is without running anything.
//!
//! Everything else — arithmetic, string interpolation, `if`/`match`, lambdas,
//! field access, `state` declarations, `fn` declarations, a variant of an enum
//! another module declares — is **not** static and is reported as
//! [`StaticValueError::NotStatic`] rather than silently skipped.

use std::collections::BTreeMap;

use crate::ast::{
    AssignTarget, Expr, ExprKind, JsxChild, Literal, RecordField, Stmt, StmtKind, UnaryOp,
};
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
    /// A color literal `#rrggbb` (or `#rgb`, `#rgba`, `#rrggbbaa`): channels
    /// `0..=255`, with `a` present only when the literal spells an alpha.
    /// Renders as `#rrggbb` / `#rrggbbaa` in lowercase; an edit of an existing
    /// literal keeps that literal's case and short/long form (see
    /// [`crate::literal_edit`]).
    ///
    /// At run time a color literal *is* the record `{r, g, b}` (plus `a`), but
    /// a config file that says `#ff2e88` should get `#29d9ff` written back,
    /// not `{ r: 41, g: 217, b: 255 }` — hence its own variant.
    Color { r: u8, g: u8, b: u8, a: Option<u8> },
    /// A call `function(args...)`, held unevaluated — the building block for
    /// declarative call trees like `layout(row([editor("a")], [1.0]))`, and how
    /// a config file names a host-interpreted constructor.
    Call {
        function: String,
        args: Vec<StaticValue>,
    },
    /// A variant of an enum: `Mode.dark` (`of` is `Mode`, `name` is `dark`),
    /// or with its payload, `Shape.circle(2)`. `args` is empty for a variant
    /// that has none. Read out of a file only for an enum that file declares.
    Variant {
        of: String,
        name: String,
        args: Vec<StaticValue>,
    },
    /// An element `<tag a="x" b={1}>children</tag>`, the JSX-style syntax a
    /// host walks as a tree. `props` are its attributes in source order (a
    /// bare `<video muted />` is `muted: true`); `children` its content, where
    /// a run of text is a `Str` and a `{hole}` or nested tag is whatever value
    /// it holds. Renders with each `<br />` ending a line, so text broken into
    /// lines reads as it will show.
    Element {
        tag: String,
        props: Vec<(String, StaticValue)>,
        children: Vec<StaticValue>,
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
    /// An opaque color value, rendered `#rrggbb`.
    pub fn color(r: u8, g: u8, b: u8) -> StaticValue {
        StaticValue::Color { r, g, b, a: None }
    }
    /// A color value with an alpha channel, rendered `#rrggbbaa`.
    pub fn color_alpha(r: u8, g: u8, b: u8, a: u8) -> StaticValue {
        StaticValue::Color {
            r,
            g,
            b,
            a: Some(a),
        }
    }
    /// The color a hex literal spells — `"#ff2e88"`, `"#f80"`, `"#ff2e88cc"`,
    /// with or without the `#`. `None` when the text is not 3, 4, 6 or 8 hex
    /// digits.
    pub fn color_hex(hex: &str) -> Option<StaticValue> {
        let hex = hex.trim();
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let channels = crate::parse::parse_color_hex(hex);
        let get = |name: &str| {
            channels
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| *value as u8)
        };
        Some(StaticValue::Color {
            r: get("r")?,
            g: get("g")?,
            b: get("b")?,
            a: get("a"),
        })
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

    /// A variant value: `StaticValue::variant("Mode", "dark", [] as [i64; 0])`
    /// renders as `Mode.dark`, and with arguments as `Shape.circle(2)`.
    pub fn variant<P, A>(of: impl Into<String>, name: impl Into<String>, args: P) -> StaticValue
    where
        P: IntoIterator<Item = A>,
        A: Into<StaticValue>,
    {
        StaticValue::Variant {
            of: of.into(),
            name: name.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// An element value: `StaticValue::element("text", [("y", 400)], ["Hi"])`
    /// renders as `<text y={400}>Hi</text>`.
    pub fn element<P, K, A, C, B>(tag: impl Into<String>, props: P, children: C) -> StaticValue
    where
        P: IntoIterator<Item = (K, A)>,
        K: Into<String>,
        A: Into<StaticValue>,
        C: IntoIterator<Item = B>,
        B: Into<StaticValue>,
    {
        StaticValue::Element {
            tag: tag.into(),
            props: props
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            children: children.into_iter().map(Into::into).collect(),
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
            StaticValue::Color { r, g, b, a } => match a {
                Some(a) => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
                None => format!("#{r:02x}{g:02x}{b:02x}"),
            },
            StaticValue::List(items) => render_list(items, depth),
            StaticValue::Record(fields) => render_record(fields, depth),
            StaticValue::Call { function, args } => render_call_at(function, args, depth),
            StaticValue::Variant { of, name, args } if args.is_empty() => format!("{of}.{name}"),
            StaticValue::Variant { of, name, args } => {
                render_call_at(&format!("{of}.{name}"), args, depth)
            }
            StaticValue::Element {
                tag,
                props,
                children,
            } => {
                let attrs: String = props
                    .iter()
                    .map(|(key, value)| format!(" {}", render_attr(key, value, depth)))
                    .collect();
                let open = format!("<{tag}{attrs}");
                let close = format!("</{tag}>");
                wrap_children(
                    &open,
                    &close,
                    &element_child_lines(children, depth + 1),
                    depth,
                )
            }
        }
    }

    /// True for the composite variants whose rendering can span lines; a list
    /// containing any of these is laid out one element per line.
    pub(crate) fn is_composite(&self) -> bool {
        matches!(
            self,
            StaticValue::List(_)
                | StaticValue::Record(_)
                | StaticValue::Call { .. }
                | StaticValue::Element { .. }
        ) || matches!(self, StaticValue::Variant { args, .. } if !args.is_empty())
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

/// One attribute of an element: `key="text"` for a string, a bare `key` for
/// `true`, and `key={value}` for anything else.
pub(crate) fn render_attr(key: &str, value: &StaticValue, depth: usize) -> String {
    match value {
        StaticValue::Str(s) => format!("{key}={}", render_string_literal(s)),
        StaticValue::Bool(true) => key.to_string(),
        other => format!("{key}={{{}}}", other.render(depth)),
    }
}

/// Whether `s` can be written as bare text between tags and read back the
/// same: nothing a tag or a hole starts with, and no whitespace at its edges,
/// which the line-break collapsing would eat.
fn is_plain_text(s: &str) -> bool {
    !s.is_empty() && s.trim() == s && !s.contains(['{', '}', '<', '>', '\n', '\r', '\t'])
}

/// An element's children as source lines, each at `depth`: text and `{hole}`s
/// run along one line, a `<br />` ends the line it is on, and any other tag
/// takes a line of its own. Text that would not read back as written (two runs
/// side by side, or space at its edges) is written as a `{"string"}` hole.
pub(crate) fn element_child_lines(children: &[StaticValue], depth: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut after_text = false;
    for child in children {
        match child {
            StaticValue::Str(s) if !after_text && is_plain_text(s) => {
                line.push_str(s);
                after_text = true;
                continue;
            }
            StaticValue::Element {
                tag,
                props,
                children,
            } if tag == "br" && props.is_empty() && children.is_empty() => {
                line.push_str("<br />");
                lines.push(std::mem::take(&mut line));
            }
            StaticValue::Element { .. } => {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                lines.push(child.render(depth));
            }
            other => line.push_str(&format!("{{{}}}", other.render(depth))),
        }
        after_text = false;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// An element from its open tag (`<tag attrs`, without the `>`), its close
/// tag and its child lines: self-closing with no children, on one line when
/// the children are one line of text, and otherwise one child line per line,
/// a step deeper than the tags.
pub(crate) fn wrap_children(open: &str, close: &str, lines: &[String], depth: usize) -> String {
    match lines {
        [] => format!("{open} />"),
        [only] if !only.starts_with('<') && !only.contains('\n') => {
            format!("{open}>{only}{close}")
        }
        _ => {
            let inner = indent(depth + 1);
            let body: String = lines.iter().map(|l| format!("{inner}{l}\n")).collect();
            format!("{open}>\n{body}{}{close}", indent(depth))
        }
    }
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
    /// in this sense. For `config let X = tunable(v, …)` it is `v`'s text, as
    /// `value` is `v`'s value (the bounds are in [`range`](Self::range)).
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
    /// Written `config let …`: the author declared the binding a tuning knob
    /// (see docs/features/direct-manipulation.md). A host showing a file's settings
    /// can offer exactly these for editing. Always `false` for a bare
    /// `name = …` rebinding, a `fn` and a `state`.
    pub is_config: bool,
    /// Written with `pub` (or `export`): other modules can import it.
    pub exported: bool,
    /// The range a `config let X = tunable(v, min, max, step)` declares:
    /// the bounds written as number literals, by position or by name.
    /// `None` when the binding is not such a `config let`, or names no
    /// bound. A host showing a slider can use it for the slider's ends; it
    /// is advice, as it is to direct manipulation, and nothing enforces it.
    pub range: Option<StaticRange>,
    /// The 1-based line the binding's statement starts on.
    pub line: u32,
    /// The declaration, when the binding is an `enum`'s name: what makes
    /// `Name.variant` below (or above) it a static value.
    pub(crate) enum_decl: Option<crate::program::EnumDef>,
}

/// The bounds of a `tunable(value, min?, max?, step?)` a `config let` is
/// initialized with (see [`StaticBinding::range`]). Each is the number its
/// literal says, or `None` when it is not written (or written `nil`).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
pub struct StaticRange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

/// The arguments of a `tunable(value, min?, max?, step?)` call, sorted into
/// their parameters: by position, or by name (`tunable(4.0, min: 1)`).
pub struct TunableArgs<'a> {
    pub value: &'a Expr,
    pub min: Option<&'a Expr>,
    pub max: Option<&'a Expr>,
    pub step: Option<&'a Expr>,
}

/// `e`'s arguments when `e` is a well-formed call of `tunable` by that name:
/// one to four arguments, each parameter given once, a named argument only
/// after the positional ones, no spread. `None` for anything else.
///
/// This goes by the name alone, so it is only asked where the syntax has
/// already said "this is a knob": the initializer of a `config let`
/// ([`config_value_expr`]). A file that declares its own `fn tunable` still
/// has its `config let X = tunable(…)` read this way.
pub fn tunable_args(e: &Expr) -> Option<TunableArgs<'_>> {
    const PARAMS: [&str; 4] = ["value", "min", "max", "step"];
    let ExprKind::Call {
        function,
        args,
        arg_names,
    } = &e.kind
    else {
        return None;
    };
    let ExprKind::Ident(f) = &function.kind else {
        return None;
    };
    if f != "tunable" || args.is_empty() || args.len() > 4 {
        return None;
    }
    let mut slots: [Option<&Expr>; 4] = [None; 4];
    let mut named_seen = false;
    for (i, arg) in args.iter().enumerate() {
        let slot = match arg_names.get(i).and_then(|n| n.as_deref()) {
            Some(name) => {
                named_seen = true;
                PARAMS.iter().position(|p| *p == name)?
            }
            None if named_seen => return None,
            None => i,
        };
        if slots[slot].replace(arg).is_some() {
            return None;
        }
    }
    Some(TunableArgs {
        value: slots[0]?,
        min: slots[1],
        max: slots[2],
        step: slots[3],
    })
}

/// Where a top-level binding's value is written, for a reader that goes by
/// paths: its initializer, except that in `config let X = tunable(v, …)` it
/// is `v`. The `tunable` there gives the setting a range; the setting is the
/// value, so `X` names `v` to [`static_bindings`], to path edits
/// ([`crate::literal_edit`], [`crate::rewrite::find_binding_path`]) and so to
/// `Env::set_config_value`.
pub fn config_value_expr(value: &Expr, is_config: bool) -> &Expr {
    match is_config.then(|| tunable_args(value)).flatten() {
        Some(args) => args.value,
        None => value,
    }
}

/// The range a `config let`'s initializer declares, when it is a `tunable`
/// call with at least one bound written as a number literal.
fn config_range(value: &Expr, is_config: bool) -> Option<StaticRange> {
    let args = is_config.then(|| tunable_args(value)).flatten()?;
    let number = |e: Option<&Expr>| match e.and_then(crate::source_diff::signed_number) {
        Some(Literal::Int(n)) => Some(n as f64),
        Some(Literal::Float(f)) => Some(f),
        _ => None,
    };
    let range = StaticRange {
        min: number(args.min),
        max: number(args.max),
        step: number(args.step),
    };
    (range != StaticRange::default()).then_some(range)
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
    Ok(bindings_of(&stmts, &chars))
}

/// The bindings the top-level statements `stmts` establish, in source order —
/// pass a prefix of a file's statements to get the names in scope at that
/// point, which is what [`eval`] resolves a reference against.
///
/// An `enum` is hoisted: its variants are static values on every line of the
/// file, above its declaration as well as below. Its name is listed where it
/// is declared, like any other.
pub(crate) fn bindings_of(stmts: &[Stmt], chars: &[char]) -> Vec<StaticBinding> {
    let enum_binding = |stmt: &Stmt| {
        let StmtKind::EnumDecl { name, variants } = &stmt.kind else {
            return None;
        };
        Some(StaticBinding {
            name: name.clone(),
            value: Err("an enum declaration".into()),
            text: None,
            comment: leading_comment(chars, stmt.span.start.offset as usize),
            is_config: false,
            exported: stmt.exported,
            range: None,
            line: stmt.span.start.line,
            enum_decl: Some(crate::program::EnumDef::declared(variants)),
        })
    };
    let hoisted: Vec<StaticBinding> = stmts.iter().filter_map(enum_binding).collect();
    let mut bindings: Vec<StaticBinding> = Vec::new();
    let eval = |expr: &Expr, bindings: &[StaticBinding]| {
        eval_in(
            expr,
            &Scope {
                bindings,
                hoisted: &hoisted,
            },
        )
    };
    for stmt in stmts {
        let mut range = None;
        let (name, value, text, is_config) = match &stmt.kind {
            StmtKind::Let {
                name,
                value,
                is_config,
                ..
            } => {
                // `config let X = tunable(v, …)`: the setting is `v`, with a
                // range.
                range = config_range(value, *is_config);
                let value = config_value_expr(value, *is_config);
                (
                    name.clone(),
                    eval(value, &bindings),
                    Some(span_text(chars, value.span)),
                    *is_config,
                )
            }
            StmtKind::Assign {
                target: AssignTarget::Name(name),
                value,
            } => (
                name.clone(),
                eval(value, &bindings),
                Some(span_text(chars, value.span)),
                false,
            ),
            // A `fn` declares a name too, so report it as bound-but-not-static
            // rather than letting a lookup say "not found" about a name that is
            // plainly there in the file.
            StmtKind::FnDecl { name, .. } => (
                name.clone(),
                Err("a function declaration".into()),
                None,
                false,
            ),
            // `state` is runtime state, not configuration: its value only exists
            // once the program runs, and it changes as the program runs.
            StmtKind::State { name, .. } => (
                name.clone(),
                Err("a `state` declaration, whose value only exists at run time".into()),
                None,
                false,
            ),
            StmtKind::Memo { name, .. } => (
                name.clone(),
                Err("a `memo` declaration, whose value only exists at run time".into()),
                None,
                false,
            ),
            StmtKind::EnumDecl { .. } => {
                if let Some(binding) = enum_binding(stmt) {
                    set_binding(&mut bindings, binding);
                }
                continue;
            }
            _ => continue,
        };
        let comment = leading_comment(chars, stmt.span.start.offset as usize);
        set_binding(
            &mut bindings,
            StaticBinding {
                name,
                value,
                text,
                comment,
                is_config,
                exported: stmt.exported,
                range,
                line: stmt.span.start.line,
                enum_decl: None,
            },
        );
    }
    bindings
}

/// What a static evaluation can see: the names bound above the expression,
/// and the file's enums, which are in scope on every line.
#[derive(Clone, Copy)]
struct Scope<'a> {
    bindings: &'a [StaticBinding],
    hoisted: &'a [StaticBinding],
}

/// The enum `expr` names and its declaration, when `expr` is the bare name
/// of an enum the file declares. A name bound above to something else is
/// that something else.
fn enum_named<'a>(
    expr: &'a Expr,
    scope: &Scope<'a>,
) -> Option<(&'a str, &'a crate::program::EnumDef)> {
    let ExprKind::Ident(name) = &expr.kind else {
        return None;
    };
    let named = |binding: &&StaticBinding| &binding.name == name;
    let binding = match scope.bindings.iter().find(named) {
        Some(bound) => bound,
        None => scope.hoisted.iter().find(named)?,
    };
    Some((name.as_str(), binding.enum_decl.as_ref()?))
}

/// `Enum.member` with `args` (`None`: not called) as a static value.
fn eval_variant(
    enum_name: &str,
    def: &crate::program::EnumDef,
    member: &str,
    args: Option<&[Expr]>,
    scope: &Scope,
) -> Result<StaticValue, String> {
    let Some(variant) = def.variant(member) else {
        return Err(match member {
            "all" | "from" => format!("`{enum_name}.{member}`, which needs evaluating"),
            _ => format!("`{enum_name}.{member}`, which is not a variant of enum `{enum_name}`"),
        });
    };
    let args = args.unwrap_or_default();
    if args.len() != variant.fields.len() {
        return Err(match (args.len(), variant.fields.len()) {
            (0, _) => format!("the constructor `{enum_name}.{member}`, which is a function"),
            (got, takes) => format!(
                "`{enum_name}.{member}` with {got} arguments where the variant takes {takes}"
            ),
        });
    }
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        out.push(eval_in(arg, scope)?);
    }
    Ok(StaticValue::Variant {
        of: enum_name.to_string(),
        name: member.to_string(),
        args: out,
    })
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
pub(crate) fn eval(expr: &Expr, bindings: &[StaticBinding]) -> Result<StaticValue, String> {
    eval_in(
        expr,
        &Scope {
            bindings,
            hoisted: &[],
        },
    )
}

fn eval_in(expr: &Expr, scope: &Scope) -> Result<StaticValue, String> {
    let bindings = scope.bindings;
    if let Some(color) = color_literal(expr) {
        return Ok(color);
    }
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
        ExprKind::UnaryOp { op, operand } => match (op, eval_in(operand, scope)?) {
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
                out.push(eval_in(item, scope)?);
            }
            Ok(StaticValue::List(out))
        }
        ExprKind::Record(fields) => {
            let mut out = Vec::with_capacity(fields.len());
            for field in fields {
                match field {
                    RecordField::Named(key, value) => {
                        out.push((key.clone(), eval_in(value, scope)?))
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
            // `Shape.circle(2)`: a variant with its payload.
            if let ExprKind::FieldAccess { object, field } = &function.kind
                && let Some((enum_name, def)) = enum_named(object, scope)
            {
                if !arg_names.is_empty() {
                    return Err("a named argument".into());
                }
                return eval_variant(enum_name, def, field, Some(args), scope);
            }
            let ExprKind::Ident(name) = &function.kind else {
                return Err("a call through an expression rather than a plain name".into());
            };
            if !arg_names.is_empty() {
                return Err("a named argument".into());
            }
            let mut out = Vec::with_capacity(args.len());
            for arg in args {
                out.push(eval_in(arg, scope)?);
            }
            Ok(StaticValue::Call {
                function: name.clone(),
                args: out,
            })
        }
        // An element is a tree of data: static when its attributes and its
        // children are.
        ExprKind::Element {
            tag,
            props,
            children,
        } => {
            let mut attrs = Vec::with_capacity(props.len());
            for (key, value) in props {
                attrs.push((key.clone(), eval_in(value, scope)?));
            }
            let mut content = Vec::with_capacity(children.len());
            for child in children {
                content.push(match child {
                    JsxChild::Text(text) => StaticValue::Str(text.clone()),
                    JsxChild::Expr(expr) => eval_in(expr, scope)?,
                });
            }
            Ok(StaticValue::Element {
                tag: tag.clone(),
                props: attrs,
                children: content,
            })
        }
        // `Mode.dark`: a variant of an enum this file declares.
        ExprKind::FieldAccess { object, field } if enum_named(object, scope).is_some() => {
            let (enum_name, def) = enum_named(object, scope).expect("checked");
            eval_variant(enum_name, def, field, None, scope)
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

/// The color `expr` spells, if it is a color literal (`#ff2e88`).
///
/// The parser lowers a color literal to the record of its channels, every
/// field carrying the span of the one color token — which is what tells it
/// apart from a record somebody wrote out (`{ r: 255, g: 46, b: 136 }`), whose
/// values sit strictly inside its braces.
pub(crate) fn color_literal(expr: &Expr) -> Option<StaticValue> {
    let ExprKind::Record(fields) = &expr.kind else {
        return None;
    };
    let mut channels = [None::<u8>; 4];
    for field in fields {
        let RecordField::Named(key, value) = field else {
            return None;
        };
        let ExprKind::Literal(Literal::Int(n)) = &value.kind else {
            return None;
        };
        if value.span != expr.span {
            return None;
        }
        let slot = ["r", "g", "b", "a"].iter().position(|name| name == key)?;
        channels[slot] = Some(u8::try_from(*n).ok()?);
    }
    Some(StaticValue::Color {
        r: channels[0]?,
        g: channels[1]?,
        b: channels[2]?,
        a: channels[3],
    })
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
        ExprKind::FieldAccess { .. } => "a field access",
        ExprKind::IndexAccess { .. } => "an index access",
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
        StaticValue::Color { .. } => "a color",
        StaticValue::List(_) => "a list",
        StaticValue::Record(_) => "a record",
        StaticValue::Call { .. } => "a call",
        StaticValue::Variant { .. } => "an enum variant",
        StaticValue::Element { .. } => "an element",
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
    fn a_color_literal_reads_as_a_color_not_a_record() {
        let src = "let tint = #ff2e88\nlet short = #F80\nlet glass = #10203040\nlet rec = { r: 255, g: 46, b: 136 }\n";
        assert_eq!(get(src, "tint"), StaticValue::color(0xff, 0x2e, 0x88));
        assert_eq!(get(src, "short"), StaticValue::color(0xff, 0x88, 0x00));
        assert_eq!(
            get(src, "glass"),
            StaticValue::color_alpha(0x10, 0x20, 0x30, 0x40)
        );
        // A record somebody wrote out stays a record.
        assert!(matches!(get(src, "rec"), StaticValue::Record(_)));
        // Rendering is the canonical long lowercase form, and reads back.
        assert_eq!(get(src, "short").to_source(), "#ff8800");
        assert_eq!(get(src, "glass").to_source(), "#10203040");
        assert_eq!(StaticValue::color_hex("#F80"), Some(get(src, "short")));
        assert_eq!(StaticValue::color_hex("ff2e88"), Some(get(src, "tint")));
        assert_eq!(StaticValue::color_hex("#ff2e8"), None);
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
        // The example file in docs/usage/config-files.md, pinned so the doc can't
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
    const MODES: &str = "let before = Mode.dark\n\
        // how the editor is lit\n\
        pub enum Mode\n  dark(bg: #101010),\n  light(bg: #fafafa),\nend\n\
        enum Shape\n  dot,\n  circle(r),\n  box(w, h),\nend\n";

    #[test]
    fn a_variant_of_an_enum_the_file_declares_is_static() {
        let src = format!(
            "{MODES}let mode = Mode.light\nlet shapes = [Shape.dot, Shape.circle(2), Shape.box(3, mode)]\n"
        );
        assert_eq!(get(&src, "mode"), StaticValue::variant("Mode", "light", [] as [i64; 0]));
        // An enum is hoisted: a variant is static above its declaration too.
        assert_eq!(get(&src, "before"), StaticValue::variant("Mode", "dark", [] as [i64; 0]));
        // A payload is static when its arguments are.
        let shapes = get(&src, "shapes");
        assert_eq!(
            shapes,
            StaticValue::list([
                StaticValue::variant("Shape", "dot", [] as [i64; 0]),
                StaticValue::variant("Shape", "circle", [2]),
                StaticValue::variant(
                    "Shape",
                    "box",
                    [StaticValue::int(3), StaticValue::variant("Mode", "light", [] as [i64; 0])]
                ),
            ])
        );
        // And renders back as it was written.
        assert_eq!(get(&src, "mode").to_source(), "Mode.light");
        assert_eq!(
            shapes.to_source(),
            "[\n  Shape.dot,\n  Shape.circle(2),\n  Shape.box(3, Mode.light),\n]"
        );
    }

    #[test]
    fn what_an_enum_has_that_is_not_a_variant_value_is_not_static() {
        let reason = |rhs: &str| {
            let src = format!("{MODES}let other = {{Mode: 1}}\nlet x = {rhs}\n");
            binding(&src, "x").value.unwrap_err()
        };
        assert_eq!(reason("Mode.all"), "`Mode.all`, which needs evaluating");
        assert_eq!(reason("Mode.from(\"dark\")"), "`Mode.from`, which needs evaluating");
        assert_eq!(reason("Mode.dim"), "`Mode.dim`, which is not a variant of enum `Mode`");
        assert_eq!(reason("Shape.circle"), "the constructor `Shape.circle`, which is a function");
        assert_eq!(
            reason("Shape.box(1)"),
            "`Shape.box` with 1 arguments where the variant takes 2"
        );
        assert_eq!(reason("Shape.circle(1 + 1)"), "an arithmetic or comparison expression, which needs evaluating");
        // Constant data is read off a value, which takes a run.
        assert_eq!(reason("Mode.dark.bg"), "a field access, which needs evaluating");
        // An enum another module declares is not known here.
        assert_eq!(reason("theme.Mode.dark"), "a field access, which needs evaluating");
        assert_eq!(reason("other.Mode"), "a field access, which needs evaluating");
    }

    #[test]
    fn an_enum_is_a_bound_name_that_is_not_itself_a_value() {
        let mode = binding(MODES, "Mode");
        assert_eq!(mode.value, Err("an enum declaration".to_string()));
        assert_eq!(mode.comment.as_deref(), Some("how the editor is lit"));
        assert!(mode.exported);
        assert_eq!((mode.line, mode.text), (3, None));
        assert!(!static_values(MODES).unwrap().contains_key("Mode"));
    }

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
    fn static_bindings_reports_config_and_line() {
        let src = "// knobs\nconfig let gain = 0.5\nlet scale = 2\n\nfn f() 1 end\n";
        let all = static_bindings(src).unwrap();
        let flags: Vec<(&str, bool, u32)> = all
            .iter()
            .map(|b| (b.name.as_str(), b.is_config, b.line))
            .collect();
        assert_eq!(
            flags,
            vec![("gain", true, 2), ("scale", false, 3), ("f", false, 5)]
        );
        // The entry file never exports; `exported` is a module's `pub`.
        assert!(all.iter().all(|b| !b.exported));
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

    #[test]
    fn a_tunable_config_reads_as_its_value_with_a_range() {
        let src = "config let SPEED = tunable(4.0, 0, 10)\n\
                   config let NAMED = tunable(12.0, min: 4, max: 40, step: 2)\n\
                   config let NEG = tunable(-0.5)\n\
                   config let LOW = tunable(3, nil, 9)\n\
                   config let BYNAME = tunable(value: 2.5, max: 5)\n\
                   let PLAIN = tunable(1.0, 0, 2)\n";
        let speed = binding(src, "SPEED");
        assert!(speed.is_config);
        assert_eq!(speed.value, Ok(StaticValue::float(4.0)));
        assert_eq!(speed.text.as_deref(), Some("4.0"));
        let range = |min, max, step| Some(StaticRange { min, max, step });
        assert_eq!(speed.range, range(Some(0.0), Some(10.0), None));
        // Named bounds, which make a call non-static anywhere else.
        let named = binding(src, "NAMED");
        assert_eq!(named.value, Ok(StaticValue::float(12.0)));
        assert_eq!(named.range, range(Some(4.0), Some(40.0), Some(2.0)));
        // A negative value; no bound written is no range.
        let neg = binding(src, "NEG");
        assert_eq!((neg.value, neg.range), (Ok(StaticValue::float(-0.5)), None));
        assert_eq!(binding(src, "LOW").range, range(None, Some(9.0), None));
        let by_name = binding(src, "BYNAME");
        assert_eq!(by_name.value, Ok(StaticValue::float(2.5)));
        assert_eq!(by_name.range, range(None, Some(5.0), None));
        // A plain `let` is a plain call, as before.
        let plain = binding(src, "PLAIN");
        assert!(matches!(plain.value, Ok(StaticValue::Call { ref function, .. }) if function == "tunable"));
        assert_eq!(plain.range, None);
    }

    #[test]
    fn a_tunable_config_is_unwrapped_even_when_the_file_shadows_tunable() {
        // `config` already says this is a knob, so a user `fn tunable` does
        // not hide it from hosts.
        let src = "fn tunable(v, lo, hi) v end\nconfig let X = tunable(4, 1, 8)\n";
        let x = binding(src, "X");
        assert_eq!(x.value, Ok(StaticValue::int(4)));
        assert_eq!(x.range, Some(StaticRange { min: Some(1.0), max: Some(8.0), step: None }));
    }

    #[test]
    fn a_malformed_tunable_config_is_left_as_a_call() {
        // Too many arguments, a repeated parameter, an unknown name: not the
        // hint's shape, so not unwrapped.
        for init in ["tunable(1, 2, 3, 4, 5)", "tunable(1, value: 2)", "tunable(1, lo: 0)"] {
            let src = format!("config let X = {init}\n");
            let x = binding(&src, "X");
            assert_eq!(x.range, None, "{init}");
            assert_eq!(x.text.as_deref(), Some(init), "{init}");
        }
    }
}
