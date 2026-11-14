//! Expressions an edit writes: a reference, a member path, a call with named
//! arguments, with literal [`StaticValue`]s as leaves.
//!
//! [`StaticValue`] is a value: what a config binding holds, rendered as a
//! literal. Some edits mean an expression instead: binding a design node's
//! fill to a token is writing `fill={tok.color.ink}`, not the color the token
//! has today, and a helper call reads best with its named arguments
//! (`shadow(c, radius: 4, y: 3)`, `modes(light: :0A0A0E, dark: :F2F2F7)`).
//! [`StaticExpr`] is that expression, as a typed tree:
//!
//! ```ignore
//! use petal::static_expr::StaticExpr;
//!
//! let ink = StaticExpr::path("tok.color.ink")?;                 // tok.color.ink
//! let modes = StaticExpr::call("modes")
//!     .named("light", StaticValue::color(0x0a, 0x0a, 0x0e))
//!     .named("dark", StaticValue::color(0xf2, 0xf2, 0xf7));     // modes(light: :0a0a0e, dark: :f2f2f7)
//! let parsed = StaticExpr::parse("shadow(c, radius: 4, y: 3)")?; // checked: one expression
//! ```
//!
//! There is still no raw-source escape hatch: a [`StaticExpr`] is built
//! structurally or parsed from text that must be exactly one expression of
//! the kinds below, and it is always rendered by this module, which quotes
//! every string and checks every name. A caller's string can become a string
//! literal or (after the identifier check) a name, never code.
//!
//! What an expression can be: a literal value (any [`StaticValue`]), a name
//! (`tok`), a field read (`tok.color`, or `m["label.color"]` for a key that is
//! not an identifier), an index (`xs[2]`), a call with positional and named
//! arguments, and list, record and element literals whose parts are
//! expressions. Arithmetic, interpolated strings, lambdas, `if` and `match`
//! are not: an edit that needs them is a hand edit.
//!
//! The goals that write one are the `*_expr` goals of
//! [`crate::goal_based_editing`]; [`get_static_expr`] reads a binding back as
//! one (the read half of named-argument calls, which [`StaticValue::Call`]
//! cannot hold).

use std::collections::BTreeSet;
use std::fmt;

use crate::ast::{Expr, ExprKind, JsxChild, RecordField, StmtKind};
use crate::rewrite::parse_ast;
use crate::static_value::{
    StaticValue, color_literal, eval, is_identifier, record_field, render_string_literal,
};

/// An expression an edit writes. See the [module docs](self).
#[derive(Debug, Clone, PartialEq)]
pub enum StaticExpr {
    /// A literal value.
    Value(StaticValue),
    /// A name in scope where the expression is written: a binding, a
    /// parameter, an import, a function.
    Name(String),
    /// `of.name`; rendered `of["name"]` when `name` is not an identifier.
    Field { of: Box<StaticExpr>, name: String },
    /// `of[index]`.
    Index { of: Box<StaticExpr>, index: Box<StaticExpr> },
    /// `callee(args..., name: value...)`: positional arguments first, then
    /// the named ones in order.
    Call {
        callee: Box<StaticExpr>,
        args: Vec<StaticExpr>,
        named: Vec<(String, StaticExpr)>,
    },
    /// A list literal of expressions.
    List(Vec<StaticExpr>),
    /// A record literal of expressions (a key that is not an identifier is
    /// written `["key"]: value`).
    Record(Vec<(String, StaticExpr)>),
    /// An element literal whose attributes and children are expressions.
    Element {
        tag: String,
        props: Vec<(String, StaticExpr)>,
        children: Vec<StaticExpr>,
    },
}

/// Why text is not an expression an edit can write, or a structurally built
/// one cannot be rendered (a name that is not an identifier).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub message: String,
}

impl ExprError {
    fn new(message: impl Into<String>) -> ExprError {
        ExprError { message: message.into() }
    }
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExprError {}

impl From<StaticValue> for StaticExpr {
    fn from(v: StaticValue) -> StaticExpr {
        StaticExpr::Value(v)
    }
}

macro_rules! from_scalar {
    ($($t:ty),*) => {$(
        impl From<$t> for StaticExpr {
            fn from(v: $t) -> StaticExpr {
                StaticExpr::Value(v.into())
            }
        }
    )*};
}
from_scalar!(&str, String, i64, i32, f64, f32, bool);

impl StaticExpr {
    /// A name, checked to be an identifier.
    pub fn name(name: impl Into<String>) -> Result<StaticExpr, ExprError> {
        let name = name.into();
        check_name(&name, "a name")?;
        Ok(StaticExpr::Name(name))
    }

    /// A member path `a.b.c`: a name, then field reads. Each part must be an
    /// identifier.
    pub fn path(path: &str) -> Result<StaticExpr, ExprError> {
        let mut parts = path.split('.');
        let mut out = StaticExpr::name(parts.next().unwrap_or_default().trim())?;
        for part in parts {
            let part = part.trim();
            check_name(part, "a field name in a path")?;
            out = out.field(part);
        }
        Ok(out)
    }

    /// `self.name` (any key: one that is not an identifier is written
    /// `self["name"]`).
    pub fn field(self, name: impl Into<String>) -> StaticExpr {
        StaticExpr::Field { of: Box::new(self), name: name.into() }
    }

    /// `self[index]`.
    pub fn index(self, index: impl Into<StaticExpr>) -> StaticExpr {
        StaticExpr::Index { of: Box::new(self), index: Box::new(index.into()) }
    }

    /// A call of the function `function` (a name, or a member path such as
    /// `design.modes`) with no arguments yet: add them with [`arg`](Self::arg)
    /// and [`named`](Self::named). A `function` that is not a path renders as
    /// an error.
    pub fn call(function: &str) -> StaticExpr {
        let callee = StaticExpr::path(function).unwrap_or_else(|_| StaticExpr::Name(function.to_string()));
        StaticExpr::Call { callee: Box::new(callee), args: Vec::new(), named: Vec::new() }
    }

    /// This call with one more positional argument. On anything but a call
    /// it does nothing.
    pub fn arg(mut self, value: impl Into<StaticExpr>) -> StaticExpr {
        if let StaticExpr::Call { args, .. } = &mut self {
            args.push(value.into());
        }
        self
    }

    /// This call with one more named argument (`name: value`). On anything
    /// but a call it does nothing.
    pub fn named(mut self, name: impl Into<String>, value: impl Into<StaticExpr>) -> StaticExpr {
        if let StaticExpr::Call { named, .. } = &mut self {
            named.push((name.into(), value.into()));
        }
        self
    }

    /// Parse `text` as an expression. It must be exactly one expression (not
    /// a statement, not two expressions) of the kinds the [module docs](self)
    /// list; anything else is an error that says what was found.
    pub fn parse(text: &str) -> Result<StaticExpr, ExprError> {
        if text.trim().is_empty() {
            return Err(ExprError::new("an empty text is not an expression"));
        }
        let (_, stmts) = parse_ast(text).map_err(|e| ExprError::new(format!("`{}` does not parse: {e}", text.trim())))?;
        match stmts.as_slice() {
            [stmt] => match &stmt.kind {
                StmtKind::Expr(e) => StaticExpr::from_ast(e),
                _ => Err(ExprError::new(format!("`{}` is a statement, not an expression", text.trim()))),
            },
            [] => Err(ExprError::new("the text holds no expression")),
            more => Err(ExprError::new(format!(
                "`{}` is {} statements, not one expression",
                text.trim(),
                more.len()
            ))),
        }
    }

    /// The expression `expr` of a parsed source is, or why it is not one an
    /// edit can write.
    pub fn from_ast(expr: &Expr) -> Result<StaticExpr, ExprError> {
        if let Some(color) = color_literal(expr) {
            return Ok(StaticExpr::Value(color));
        }
        Ok(match &expr.kind {
            ExprKind::Ident(name) => StaticExpr::Name(name.clone()),
            ExprKind::FieldAccess { object, field } => StaticExpr::from_ast(object)?.field(field.clone()),
            // `m["label.color"]` is the field `label.color`, however spelled.
            ExprKind::IndexAccess { object, index } => match &index.kind {
                ExprKind::Literal(crate::ast::Literal::String(key)) => StaticExpr::from_ast(object)?.field(key.clone()),
                _ => StaticExpr::from_ast(object)?.index(StaticExpr::from_ast(index)?),
            },
            ExprKind::Call { function, args, arg_names } => {
                let mut positional = Vec::new();
                let mut named = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    match arg_names.get(i).and_then(|n| n.as_deref()) {
                        Some(crate::ast::SPREAD_ARG) => return Err(ExprError::new("a spread argument (`...`)")),
                        Some(name) => named.push((name.to_string(), StaticExpr::from_ast(arg)?)),
                        None if !named.is_empty() => {
                            return Err(ExprError::new("a positional argument after a named one"));
                        }
                        None => positional.push(StaticExpr::from_ast(arg)?),
                    }
                }
                StaticExpr::Call { callee: Box::new(StaticExpr::from_ast(function)?), args: positional, named }
            }
            ExprKind::List(items) => StaticExpr::List(items.iter().map(StaticExpr::from_ast).collect::<Result<_, _>>()?),
            ExprKind::Record(fields) => StaticExpr::Record(
                fields
                    .iter()
                    .map(|f| match record_field(f) {
                        Some((key, value)) => Ok((key.to_string(), StaticExpr::from_ast(value)?)),
                        None if matches!(f, RecordField::Spread(_)) => {
                            Err(ExprError::new("a record with a `...` spread or a computed key that is not a string"))
                        }
                        None => Err(ExprError::new("a record field it cannot read")),
                    })
                    .collect::<Result<_, _>>()?,
            ),
            ExprKind::Element { tag, props, children } => StaticExpr::Element {
                tag: tag.clone(),
                props: props
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), StaticExpr::from_ast(v)?)))
                    .collect::<Result<_, ExprError>>()?,
                children: children
                    .iter()
                    .map(|c| match c {
                        JsxChild::Text(t) => Ok(StaticExpr::Value(StaticValue::Str(t.clone()))),
                        JsxChild::Expr(e) => StaticExpr::from_ast(e),
                    })
                    .collect::<Result<_, _>>()?,
            },
            _ => match eval(expr, &[]) {
                Ok(v) => StaticExpr::Value(v),
                Err(why) => return Err(ExprError::new(format!("not an expression an edit can write: {why}"))),
            },
        }
        .simplified())
    }

    /// The same expression with literal parts folded into
    /// [`StaticExpr::Value`]s: a list, record or element whose parts are all
    /// values is a value. A call stays a call (its callee is a name in scope).
    fn simplified(self) -> StaticExpr {
        match self {
            StaticExpr::List(items) if items.iter().all(|i| matches!(i, StaticExpr::Value(_))) => {
                StaticExpr::Value(StaticValue::List(items.into_iter().map(|i| i.into_value()).collect()))
            }
            StaticExpr::Record(fields) if fields.iter().all(|(_, v)| matches!(v, StaticExpr::Value(_))) => {
                StaticExpr::Value(StaticValue::Record(fields.into_iter().map(|(k, v)| (k, v.into_value())).collect()))
            }
            StaticExpr::Element { tag, props, children }
                if props.iter().all(|(_, v)| matches!(v, StaticExpr::Value(_)))
                    && children.iter().all(|c| matches!(c, StaticExpr::Value(_))) =>
            {
                StaticExpr::Value(StaticValue::Element {
                    tag,
                    props: props.into_iter().map(|(k, v)| (k, v.into_value())).collect(),
                    children: children.into_iter().map(|c| c.into_value()).collect(),
                })
            }
            other => other,
        }
    }

    fn into_value(self) -> StaticValue {
        match self {
            StaticExpr::Value(v) => v,
            _ => unreachable!("checked by the caller"),
        }
    }

    /// This expression as a value, when it is one: a literal, or a call of a
    /// plain name with positional value arguments (a [`StaticValue::Call`]),
    /// or a list, record or element of those. `None` when it reads a name,
    /// a field or an index, or passes a named argument.
    pub fn as_value(&self) -> Option<StaticValue> {
        Some(match self {
            StaticExpr::Value(v) => v.clone(),
            StaticExpr::Call { callee, args, named } if named.is_empty() => match callee.as_ref() {
                StaticExpr::Name(f) => StaticValue::Call {
                    function: f.clone(),
                    args: args.iter().map(StaticExpr::as_value).collect::<Option<_>>()?,
                },
                _ => return None,
            },
            StaticExpr::List(items) => StaticValue::List(items.iter().map(StaticExpr::as_value).collect::<Option<_>>()?),
            StaticExpr::Record(fields) => StaticValue::Record(
                fields.iter().map(|(k, v)| Some((k.clone(), v.as_value()?))).collect::<Option<_>>()?,
            ),
            StaticExpr::Element { tag, props, children } => StaticValue::Element {
                tag: tag.clone(),
                props: props.iter().map(|(k, v)| Some((k.clone(), v.as_value()?))).collect::<Option<_>>()?,
                children: children.iter().map(StaticExpr::as_value).collect::<Option<_>>()?,
            },
            StaticExpr::Name(_) | StaticExpr::Field { .. } | StaticExpr::Index { .. } | StaticExpr::Call { .. } => {
                return None;
            }
        })
    }

    /// The names the expression reads from the scope it is written in: each
    /// [`Name`](StaticExpr::Name) (the root of a member path, a callee), and
    /// the function of each [`StaticValue::Call`] and the enum of each
    /// [`StaticValue::Variant`] among its values. Sorted, each once.
    pub fn free_names(&self) -> Vec<String> {
        let mut out = BTreeSet::new();
        self.collect_names(&mut out);
        out.into_iter().collect()
    }

    fn collect_names(&self, out: &mut BTreeSet<String>) {
        match self {
            StaticExpr::Value(v) => value_names(v, out),
            StaticExpr::Name(n) => {
                out.insert(n.clone());
            }
            StaticExpr::Field { of, .. } => of.collect_names(out),
            StaticExpr::Index { of, index } => {
                of.collect_names(out);
                index.collect_names(out);
            }
            StaticExpr::Call { callee, args, named } => {
                callee.collect_names(out);
                args.iter().for_each(|a| a.collect_names(out));
                named.iter().for_each(|(_, a)| a.collect_names(out));
            }
            StaticExpr::List(items) => items.iter().for_each(|a| a.collect_names(out)),
            StaticExpr::Record(fields) => fields.iter().for_each(|(_, a)| a.collect_names(out)),
            StaticExpr::Element { props, children, .. } => {
                props.iter().for_each(|(_, a)| a.collect_names(out));
                children.iter().for_each(|a| a.collect_names(out));
            }
        }
    }

    /// Check every name, field, argument name, attribute and tag can be
    /// written (identifiers where the syntax needs one).
    pub fn check(&self) -> Result<(), ExprError> {
        match self {
            StaticExpr::Value(_) => Ok(()),
            StaticExpr::Name(n) => check_name(n, "a name"),
            StaticExpr::Field { of, .. } => of.check(),
            StaticExpr::Index { of, index } => {
                of.check()?;
                index.check()
            }
            StaticExpr::Call { callee, args, named } => {
                callee.check()?;
                if !matches!(callee.as_ref(), StaticExpr::Name(_) | StaticExpr::Field { .. } | StaticExpr::Index { .. }) {
                    return Err(ExprError::new("only a name or a member path can be called"));
                }
                args.iter().try_for_each(StaticExpr::check)?;
                for (name, arg) in named {
                    check_name(name, "an argument name")?;
                    arg.check()?;
                }
                Ok(())
            }
            StaticExpr::List(items) => items.iter().try_for_each(StaticExpr::check),
            StaticExpr::Record(fields) => fields.iter().try_for_each(|(_, v)| v.check()),
            StaticExpr::Element { tag, props, children } => {
                check_tag(tag)?;
                for (k, v) in props {
                    check_tag(k)?;
                    v.check()?;
                }
                children.iter().try_for_each(StaticExpr::check)
            }
        }
    }

    /// Render as Petal source starting at column 0, after [`check`](Self::check).
    pub fn to_source(&self) -> Result<String, ExprError> {
        self.render(0)
    }

    /// Render at `depth` (two-space indent units, for multi-line lists).
    pub(crate) fn render(&self, depth: usize) -> Result<String, ExprError> {
        self.check()?;
        let mut holes = Holes::new("", self);
        let value = holes.lower(self)?;
        Ok(holes.fill(&value.render(depth)))
    }

    /// The text of an expression that is not a value (a name, a field, an
    /// index, a call), rendered inline.
    fn render_leaf(&self) -> Result<String, ExprError> {
        Ok(match self {
            StaticExpr::Name(n) => n.clone(),
            StaticExpr::Field { of, name } if is_identifier(name) => format!("{}.{name}", of.render_postfix()?),
            StaticExpr::Field { of, name } => format!("{}[{}]", of.render_postfix()?, render_string_literal(name)),
            StaticExpr::Index { of, index } => format!("{}[{}]", of.render_postfix()?, index.render(0)?),
            StaticExpr::Call { callee, args, named } => {
                let mut parts = Vec::new();
                for a in args {
                    parts.push(a.render(0)?);
                }
                for (n, a) in named {
                    parts.push(format!("{n}: {}", a.render(0)?));
                }
                format!("{}({})", callee.render_postfix()?, parts.join(", "))
            }
            other => other.render(0)?,
        })
    }

    /// The text of the expression before a `.`, `[` or `(`: parenthesized
    /// when it is a literal.
    fn render_postfix(&self) -> Result<String, ExprError> {
        Ok(match self {
            StaticExpr::Name(_) | StaticExpr::Field { .. } | StaticExpr::Index { .. } | StaticExpr::Call { .. } => {
                self.render_leaf()?
            }
            other => format!("({})", other.render(0)?),
        })
    }
}

impl fmt::Display for StaticExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_source() {
            Ok(s) => f.write_str(&s),
            Err(e) => write!(f, "<invalid expression: {e}>"),
        }
    }
}

fn check_name(name: &str, what: &str) -> Result<(), ExprError> {
    if is_identifier(name) {
        Ok(())
    } else {
        Err(ExprError::new(format!("`{name}` is not {what}: it must be an identifier that is not a keyword")))
    }
}

/// A tag or attribute name: letters, digits, `_` and `-`, starting with a
/// letter or `_`.
fn check_tag(name: &str) -> Result<(), ExprError> {
    let mut chars = name.chars();
    let ok = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    if ok { Ok(()) } else { Err(ExprError::new(format!("`{name}` is not a tag or attribute name"))) }
}

fn value_names(v: &StaticValue, out: &mut BTreeSet<String>) {
    match v {
        StaticValue::Call { function, args } => {
            if let Some(root) = function.split('.').next() {
                out.insert(root.to_string());
            }
            args.iter().for_each(|a| value_names(a, out));
        }
        StaticValue::Variant { of, args, .. } => {
            out.insert(of.clone());
            args.iter().for_each(|a| value_names(a, out));
        }
        StaticValue::List(items) => items.iter().for_each(|a| value_names(a, out)),
        StaticValue::Record(fields) => fields.iter().for_each(|(_, a)| value_names(a, out)),
        StaticValue::Element { props, children, .. } => {
            props.iter().for_each(|(_, a)| value_names(a, out));
            children.iter().for_each(|a| value_names(a, out));
        }
        _ => {}
    }
}

/// An expression lowered to a [`StaticValue`] for the value machinery (the
/// renderer, [`crate::literal_edit`]'s layout-preserving edits): each part
/// that is not a value is a placeholder variant `<prefix>.h<n>_`, whose text
/// [`Holes::fill`] puts back. The prefix is chosen so it does not occur in
/// the source being edited, so every occurrence after the edit is one of
/// ours.
pub(crate) struct Holes {
    prefix: String,
    texts: Vec<String>,
}

impl Holes {
    /// Holes for writing `expr` into `source`: the prefix occurs in neither
    /// (a string inside `expr` that spelled a placeholder would otherwise be
    /// filled too).
    pub(crate) fn new(source: &str, expr: &StaticExpr) -> Holes {
        let mut prefix = "__petal_expr_hole".to_string();
        let mut n = 0;
        let spelled = format!("{expr:?}");
        while source.contains(&prefix) || spelled.contains(&prefix) {
            n += 1;
            prefix = format!("__petal_expr_hole{n}");
        }
        Holes { prefix, texts: Vec::new() }
    }

    pub(crate) fn lower(&mut self, e: &StaticExpr) -> Result<StaticValue, ExprError> {
        if let Some(v) = e.as_value() {
            return Ok(v);
        }
        Ok(match e {
            StaticExpr::List(items) => StaticValue::List(items.iter().map(|i| self.lower(i)).collect::<Result<_, _>>()?),
            StaticExpr::Record(fields) => StaticValue::Record(
                fields.iter().map(|(k, v)| Ok((k.clone(), self.lower(v)?))).collect::<Result<_, ExprError>>()?,
            ),
            StaticExpr::Element { tag, props, children } => StaticValue::Element {
                tag: tag.clone(),
                props: props.iter().map(|(k, v)| Ok((k.clone(), self.lower(v)?))).collect::<Result<_, ExprError>>()?,
                children: children.iter().map(|c| self.lower(c)).collect::<Result<_, _>>()?,
            },
            leaf => {
                let text = leaf.render_leaf()?;
                self.texts.push(text);
                StaticValue::Variant {
                    of: self.prefix.clone(),
                    name: format!("h{}_", self.texts.len() - 1),
                    args: Vec::new(),
                }
            }
        })
    }

    /// `text` with each placeholder replaced by its expression's text.
    pub(crate) fn fill(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (i, t) in self.texts.iter().enumerate().rev() {
            out = out.replace(&format!("{}.h{i}_", self.prefix), t);
        }
        out
    }
}

/// Read the value bound to top-level `name` in `source` as an expression,
/// without running it: like [`crate::static_value::get_static_value`], but
/// names, field reads and calls with named arguments are read as
/// [`StaticExpr`]s instead of making the binding unreadable. The last
/// top-level binding wins.
pub fn get_static_expr(source: &str, name: &str) -> Result<StaticExpr, ExprError> {
    let (_, stmts) = parse_ast(source).map_err(|e| ExprError::new(format!("source did not parse: {e}")))?;
    let found = stmts.iter().rev().find_map(|s| match &s.kind {
        StmtKind::Let { name: n, value, .. } if n == name => Some(value),
        StmtKind::Assign { target: crate::ast::AssignTarget::Name(n), value } if n == name => Some(value),
        _ => None,
    });
    let value = found.ok_or_else(|| ExprError::new(format!("no top-level binding for `{name}`")))?;
    StaticExpr::from_ast(crate::static_value::setting_value_expr(value))
}

/// Whether `old` (an expression of a source) already reads as `expr`.
pub(crate) fn same_expr(old: &Expr, expr: &StaticExpr) -> bool {
    StaticExpr::from_ast(old).is_ok_and(|o| o == *expr)
}
