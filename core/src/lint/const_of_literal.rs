//! Rule — `const-of-literal`: a `const` on literal data, where it checks
//! nothing a reader cannot see.
//!
//! ```text
//! let N = const(4)          // reported: `let N = 4` says the same
//! let T = const(map(…))     // not reported: `const` checks the call is static
//! ```
//!
//! Report only and opt-in. It reports the deprecated `const X = e` too, as
//! long as the parser still accepts it (until Phase 6 of
//! docs/tasks/lifecycle-annotation-functions-refactor.md); rewriting that
//! declaration is `petal migrate-old-code`'s ([`crate::migrate`]).

use crate::ast::{Expr, ExprKind, Literal, RecordField, Stmt, StmtKind, UnaryOp};
use crate::parse::CONST_ANNOTATION;
use crate::static_value::{color_literal, const_call_arg};

use super::Fix;

/// `const-of-literal`: every top-level `const X = e` or `let X = const(e)`
/// whose `e` is literal data. Report only.
pub(super) fn plan_const_of_literal_reports(stmts: &[Stmt]) -> Vec<Fix> {
    let mut out = Vec::new();
    for stmt in stmts {
        let StmtKind::Let {
            name,
            value,
            is_var: false,
            is_const,
            ..
        } = &stmt.kind
        else {
            continue;
        };
        let (data, anchor, how) = if *is_const {
            (value, stmt.span.start.offset as usize, format!("const {name} = …"))
        } else if let Some(arg) = const_call_arg(value) {
            let how = if crate::ast::is_annotation_call(value) {
                format!("@{CONST_ANNOTATION}")
            } else {
                format!("{CONST_ANNOTATION}(…)")
            };
            (arg, value.span.start.offset as usize, how)
        } else {
            continue;
        };
        if !is_literal_data(data) {
            continue;
        }
        out.push(Fix {
            anchor,
            message: format!(
                "`{how}` on literal data checks nothing a reader cannot see; \
                 `let {name} = …` says the same"
            ),
            splices: Vec::new(),
        });
    }
    out
}

/// Literal data: a literal (a negated number included), a colour, or a list
/// or record of literal data. A call is not, even of a constructor: `const`
/// is what checks that the call is static.
fn is_literal_data(e: &Expr) -> bool {
    if color_literal(e).is_some() {
        return true;
    }
    match &e.kind {
        ExprKind::Literal(_) => true,
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => matches!(
            operand.kind,
            ExprKind::Literal(Literal::Int(_) | Literal::Float(_))
        ),
        ExprKind::List(items) => items.iter().all(is_literal_data),
        ExprKind::Record(fields) => fields.iter().all(|f| match f {
            RecordField::Named(_, v) => is_literal_data(v),
            _ => false,
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal_reports(src: &str) -> Vec<String> {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).unwrap();
        plan_const_of_literal_reports(&stmts)
            .into_iter()
            .map(|f| f.message)
            .collect()
    }

    #[test]
    fn const_of_literal() {
        let src = "const A = 1\n\
                   let B = const(\"x\")\n\
                   let C = const([1, -2.5, {k: #ff0000}])\n\
                   const D = rgb(1, 2, 3)\n\
                   let E = const(A + 1)\n\
                   let F = const(\"{A}\")\n";
        let found = literal_reports(src);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[0].starts_with("`const A = …` on literal data"), "{found:?}");
        assert!(found[1].starts_with("`const(…)` on literal data"), "{found:?}");
    }
}
