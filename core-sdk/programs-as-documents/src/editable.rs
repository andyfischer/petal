//! Whether a top-level binding is a literal an application can write into.
//!
//! Applications that keep a document's content in a named binding (`let
//! items = [...]`, `let project = {...}`) need to know that the binding's
//! literal is the value the program uses: written as a literal, static, and
//! not rebuilt by a later statement. [`editable_literal`] answers with the
//! value, or a reason the application can show next to a read-only part.

use petal::ast::{AssignTarget, Expr, ExprKind, StmtKind};
use petal::static_value::{StaticValue, StaticValueError, get_static_value, setting_value_expr};

/// Why a binding is not an editable literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotEditable {
    /// The source does not parse.
    Parse(String),
    /// Nothing at the top level binds the name.
    NotFound,
    /// The binding is written as code, or as a literal with code in it
    /// (`[hero(x), <slide />]`); `reason` says what.
    Computed { reason: String },
    /// The binding is a literal, but a later top-level statement on line
    /// `line` rebinds or changes it, so the literal is not the value the
    /// program ends up with.
    RebuiltLater { line: u32 },
}

impl std::fmt::Display for NotEditable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotEditable::Parse(e) => write!(f, "the script does not parse: {e}"),
            NotEditable::NotFound => write!(f, "the script does not define it at the top level"),
            NotEditable::Computed { reason } => write!(f, "it is built by code ({reason}); edit the script by hand"),
            NotEditable::RebuiltLater { line } => {
                write!(f, "it is rebuilt by code on line {line}, after its literal; edit the script by hand")
            }
        }
    }
}

impl std::error::Error for NotEditable {}

/// The value of top-level binding `name`, when its literal is the value the
/// program uses: the last top-level statement that binds or changes `name`
/// is a `let` (or `name = …`) whose right-hand side is a static literal.
pub fn editable_literal(source: &str, name: &str) -> Result<StaticValue, NotEditable> {
    let (_, stmts) = petal::rewrite::parse_ast(source).map_err(NotEditable::Parse)?;
    let writes: Vec<(u32, Option<&Expr>)> = stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::Let { name: n, value, .. } if n == name => Some((s.span.start.line, Some(value))),
            StmtKind::Assign { target: AssignTarget::Name(n), value } if n == name => {
                Some((s.span.start.line, Some(value)))
            }
            StmtKind::Assign { target, .. } | StmtKind::Set { target, .. } if target_root(target) == Some(name) => {
                Some((s.span.start.line, None))
            }
            _ => None,
        })
        .collect();
    if writes.is_empty() {
        return Err(NotEditable::NotFound);
    }
    let Some(lit) = writes.iter().rposition(|(_, v)| v.is_some_and(is_literal)) else {
        let reason = match get_static_value(source, name) {
            Err(StaticValueError::NotStatic { reason, .. }) => reason,
            _ => "not written as a literal".to_string(),
        };
        return Err(NotEditable::Computed { reason });
    };
    if let Some((line, _)) = writes.get(lit + 1) {
        return Err(NotEditable::RebuiltLater { line: *line });
    }
    match get_static_value(source, name) {
        Ok(v) => Ok(v),
        Err(StaticValueError::NotStatic { reason, .. }) => Err(NotEditable::Computed { reason }),
        Err(StaticValueError::NotFound { .. }) => Err(NotEditable::NotFound),
        Err(StaticValueError::Parse(e)) => Err(NotEditable::Parse(e)),
    }
}

/// The name an assignment target writes into (`items` in `items[0].x = 1`).
fn target_root(t: &AssignTarget) -> Option<&str> {
    let mut e = match t {
        AssignTarget::Name(n) => return Some(n),
        AssignTarget::Field(e, _) | AssignTarget::Index(e, _) => e.as_ref(),
    };
    loop {
        match &e.kind {
            ExprKind::Ident(n) => return Some(n),
            ExprKind::FieldAccess { .. } | ExprKind::IndexAccess { .. } => {
                e = match &e.kind {
                    ExprKind::FieldAccess { object, .. } => object,
                    ExprKind::IndexAccess { object, .. } => object,
                    _ => unreachable!(),
                }
            }
            _ => return None,
        }
    }
}

/// Written as a literal: a scalar, list, record or element literal (or a
/// setting, `tunable(v, …)`, whose value is one), whatever is inside it.
fn is_literal(e: &Expr) -> bool {
    let e = setting_value_expr(e);
    match &e.kind {
        ExprKind::Literal(_) | ExprKind::List(_) | ExprKind::Record(_) | ExprKind::Element { .. } => true,
        ExprKind::UnaryOp { operand, .. } => matches!(operand.kind, ExprKind::Literal(_)),
        _ => false,
    }
}
