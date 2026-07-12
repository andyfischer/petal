//! Tests for comma-less list/argument juxtaposition and the spacing-aware
//! minus rule. See docs/syntax/optional-commas.md.

use petal::ast::{BinOp, Expr, ExprKind, Stmt, StmtKind, UnaryOp};
use petal::lexer::{Lexer, Token};
use petal::parse::Parser;

fn parse(src: &str) -> Vec<Stmt> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().expect("lex failed");
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    parser.parse_program().expect("parse failed")
}

fn tokens(src: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().expect("lex failed");
    lexer.tokens
}

/// Pull the single expression out of a one-statement program.
fn sole_expr(src: &str) -> Expr {
    let mut stmts = parse(src);
    assert_eq!(stmts.len(), 1, "expected one statement in {src:?}");
    match stmts.remove(0).kind {
        StmtKind::Expr(e) => e,
        other => panic!("expected expression statement, got {other:?}"),
    }
}

// ---- Lexer: spacing-aware minus ----

#[test]
fn lexer_minus_prefix_only_when_space_before_not_after() {
    // space before, no space after -> MinusPrefix
    assert!(tokens("1 -2").contains(&Token::MinusPrefix));
    // spaces both sides -> ordinary Minus
    assert!(tokens("1 - 2").contains(&Token::Minus));
    assert!(!tokens("1 - 2").contains(&Token::MinusPrefix));
    // no spaces -> ordinary Minus
    assert!(tokens("1-2").contains(&Token::Minus));
    assert!(!tokens("1-2").contains(&Token::MinusPrefix));
    // space after only -> ordinary Minus
    assert!(tokens("1- 2").contains(&Token::Minus));
    assert!(!tokens("1- 2").contains(&Token::MinusPrefix));
}

// ---- List literals ----

#[test]
fn list_juxtaposed_positive_numbers() {
    match sole_expr("[0 0 1 0 0 1 1 1]").kind {
        ExprKind::List(elems) => assert_eq!(elems.len(), 8),
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn list_space_minus_no_space_is_two_negated_elements() {
    match sole_expr("[1 -2]").kind {
        ExprKind::List(elems) => {
            assert_eq!(elems.len(), 2, "[1 -2] should be two elements");
            assert!(matches!(
                elems[1].kind,
                ExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    ..
                }
            ));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn list_spaced_minus_is_subtraction_single_element() {
    match sole_expr("[1 - 2]").kind {
        ExprKind::List(elems) => {
            assert_eq!(elems.len(), 1, "[1 - 2] should be one element");
            assert!(matches!(
                elems[0].kind,
                ExprKind::BinaryOp { op: BinOp::Sub, .. }
            ));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn list_chained_negatives() {
    match sole_expr("[10 -3 -1]").kind {
        ExprKind::List(elems) => assert_eq!(elems.len(), 3),
        other => panic!("expected list, got {other:?}"),
    }
}

// ---- Call arguments ----

#[test]
fn call_juxtaposed_args() {
    match sole_expr("color(0 1 2)").kind {
        ExprKind::Call { args, .. } => assert_eq!(args.len(), 3),
        other => panic!("expected call, got {other:?}"),
    }
}

#[test]
fn call_space_minus_is_two_args() {
    match sole_expr("f(1 -2)").kind {
        ExprKind::Call { args, .. } => assert_eq!(args.len(), 2),
        other => panic!("expected call, got {other:?}"),
    }
}

// ---- Subtraction is preserved outside juxtaposition contexts ----

#[test]
fn let_binding_space_minus_is_subtraction() {
    let mut stmts = parse("let x = a -b");
    assert_eq!(stmts.len(), 1);
    match stmts.remove(0).kind {
        StmtKind::Let { value, .. } => assert!(matches!(
            value.kind,
            ExprKind::BinaryOp { op: BinOp::Sub, .. }
        )),
        other => panic!("expected let, got {other:?}"),
    }
}

#[test]
fn grouping_resets_juxtaposition() {
    // The parens are a grouping, not a juxtaposition list, so `1 -2` is subtraction.
    match sole_expr("[(1 -2)]").kind {
        ExprKind::List(elems) => {
            assert_eq!(elems.len(), 1);
            assert!(matches!(
                elems[0].kind,
                ExprKind::BinaryOp { op: BinOp::Sub, .. }
            ));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn index_resets_juxtaposition() {
    // The index is a single expression, so `3 -1` is subtraction -> xs[2].
    match sole_expr("xs[3 -1]").kind {
        ExprKind::IndexAccess { index, .. } => assert!(matches!(
            index.kind,
            ExprKind::BinaryOp { op: BinOp::Sub, .. }
        )),
        other => panic!("expected index access, got {other:?}"),
    }
}
