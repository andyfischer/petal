//! Commas are required between adjacent elements of every delimited,
//! comma-separated construct. See docs/syntax/commas.md.

use petal::ast::{BinOp, Expr, ExprKind, Literal, Pattern, Stmt, StmtKind, UnaryOp};
use petal::lexer::{Lexer, Token};
use petal::parse::Parser;

fn try_parse(src: &str) -> Result<Vec<Stmt>, String> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize()?;
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    parser.parse_program()
}

fn parse(src: &str) -> Vec<Stmt> {
    try_parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"))
}

fn tokens(src: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().expect("lex failed");
    lexer.tokens
}

/// The error message produced by parsing `src`, which must fail.
fn parse_err(src: &str) -> String {
    match try_parse(src) {
        Ok(_) => panic!("expected a parse error for {src:?}, but it parsed"),
        Err(e) => e,
    }
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

// ---- Lexer: `-` is a single, spacing-insensitive token ----

#[test]
fn lexer_emits_one_minus_token_regardless_of_spacing() {
    for src in ["1 -2", "1 - 2", "1-2", "1- 2"] {
        assert!(
            tokens(src).contains(&Token::Minus),
            "{src:?} should lex a Minus"
        );
    }
}

// ---- Juxtaposition is now a parse error, in every construct ----

#[test]
fn juxtaposed_list_elements_are_an_error() {
    let err = parse_err("[0 0 1 0]");
    assert!(
        err.contains("Expected ',' between list elements"),
        "unexpected message: {err}"
    );
}

#[test]
fn juxtaposed_arguments_are_an_error() {
    let err = parse_err("color(0 1 2)");
    assert!(
        err.contains("Expected ',' between arguments"),
        "unexpected message: {err}"
    );
}

#[test]
fn juxtaposed_parameters_are_an_error() {
    let err = parse_err("fn f(a b c)\n  a\nend");
    assert!(
        err.contains("Expected ',' between parameters"),
        "unexpected message: {err}"
    );
}

#[test]
fn juxtaposed_record_fields_are_an_error() {
    let err = parse_err("{ x: 1  y: 2 }");
    assert!(
        err.contains("Expected ',' between record fields"),
        "unexpected message: {err}"
    );
}

#[test]
fn juxtaposed_record_pattern_fields_are_an_error() {
    let err = parse_err("match p\n  when { x: a  y: b } -> a\nend");
    assert!(
        err.contains("Expected ',' between record pattern fields"),
        "unexpected message: {err}"
    );
}

#[test]
fn juxtaposed_list_pattern_elements_are_an_error() {
    let err = parse_err("match p\n  when [a b] -> a\nend");
    assert!(
        err.contains("Expected ',' between list pattern elements"),
        "unexpected message: {err}"
    );
}

#[test]
fn juxtaposed_variant_pattern_fields_are_an_error() {
    let err = parse_err("match p\n  when Point(x y) -> x\nend");
    assert!(
        err.contains("Expected ',' between variant fields"),
        "unexpected message: {err}"
    );
}

#[test]
fn juxtaposed_enum_variants_are_an_error() {
    let err = parse_err("enum E A B C end");
    assert!(
        err.contains("Expected ',' between enum variants"),
        "unexpected message: {err}"
    );
}

// ---- A newline is not a substitute for a comma ----

#[test]
fn newline_does_not_separate_list_elements() {
    let err = parse_err("[1\n2]");
    assert!(
        err.contains("Expected ',' between list elements"),
        "unexpected message: {err}"
    );
}

#[test]
fn newline_does_not_separate_record_fields() {
    let err = parse_err("{\n  x: 1\n  y: 2\n}");
    assert!(
        err.contains("Expected ',' between record fields"),
        "unexpected message: {err}"
    );
}

#[test]
fn newline_does_not_separate_enum_variants() {
    let err = parse_err("enum E\n  A\n  B\nend");
    assert!(
        err.contains("Expected ',' between enum variants"),
        "unexpected message: {err}"
    );
}

/// A class body is a delimited, comma-separated construct like any other — it
/// used to be the one exception, accepting a bare newline between fields.
#[test]
fn newline_does_not_separate_class_fields() {
    let err = parse_err("class P\n  x: int\n  y: int\nend");
    assert!(
        err.contains("Expected ',' between class fields"),
        "unexpected message: {err}"
    );
}

#[test]
fn juxtaposed_class_fields_are_an_error() {
    let err = parse_err("class P\n  x: int y: int\nend");
    assert!(
        err.contains("Expected ',' between class fields"),
        "unexpected message: {err}"
    );
}

/// A stray comma where an element belongs names the construct instead of
/// producing a bare "Unexpected token: Comma".
#[test]
fn a_comma_in_element_position_names_the_construct() {
    for (src, what) in [
        ("[,]", "a list element"),
        ("[\n ,1]", "a list element"),
        ("f(1,,2)", "an argument"),
        ("{,}", "a record field"),
    ] {
        let err = parse_err(src);
        assert!(
            err.contains(&format!("Expected {what}, got ','")),
            "{src:?}: unexpected message: {err}"
        );
    }
}

/// `[[1,2] [3,4]]` parses the second bracket as an *index*, so the failure
/// surfaces inside it. Blaming the closing bracket points at the wrong place.
#[test]
fn a_missing_comma_between_lists_blames_the_index_not_the_bracket() {
    let err = parse_err("print([[1,2] [3,4]])");
    assert!(
        err.contains("Expected ']' to close the index, got ','"),
        "unexpected message: {err}"
    );
}

// ---- Error position points at the element that needed a comma ----

#[test]
fn missing_comma_error_carries_source_position() {
    let err = parse_err("let xs = [1 2]");
    assert!(
        err.contains("[line 1, column 13]"),
        "expected the caret at the second element: {err}"
    );
}

#[test]
fn missing_comma_error_position_survives_newlines() {
    // The offending element is on line 3; the report points there, not at the
    // end of line 2.
    let err = parse_err("let xs = [\n  1,\n  2 3,\n]");
    assert!(
        err.contains("[line 3, column 5]"),
        "expected the caret at the juxtaposed element: {err}"
    );
}

// ---- Comma-separated and trailing-comma forms still parse ----

#[test]
fn comma_separated_list_parses() {
    match sole_expr("[1, 2, 3]").kind {
        ExprKind::List(elems) => assert_eq!(elems.len(), 3),
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn trailing_comma_is_allowed_everywhere() {
    match sole_expr("[1, 2, 3,]").kind {
        ExprKind::List(elems) => assert_eq!(elems.len(), 3),
        other => panic!("expected list, got {other:?}"),
    }
    match sole_expr("f(1, 2,)").kind {
        ExprKind::Call { args, .. } => assert_eq!(args.len(), 2),
        other => panic!("expected call, got {other:?}"),
    }
    match sole_expr("{ x: 1, y: 2, }").kind {
        ExprKind::Record(fields) => assert_eq!(fields.len(), 2),
        other => panic!("expected record, got {other:?}"),
    }
    parse("fn f(a, b,)\n  a\nend");
    parse("enum E\n  A,\n  B,\nend");
    parse("class P\n  x: int,\n  y: int,\nend");
    parse("class P\n  x: int, y: int\nend");
}

#[test]
fn commas_may_be_wrapped_across_lines() {
    match sole_expr("[\n  1,\n  2,\n  3\n]").kind {
        ExprKind::List(elems) => assert_eq!(elems.len(), 3),
        other => panic!("expected list, got {other:?}"),
    }
    // A comma at the start of the next line works too.
    match sole_expr("[\n  1\n  , 2\n]").kind {
        ExprKind::List(elems) => assert_eq!(elems.len(), 2),
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn patterns_accept_commas_and_trailing_commas() {
    let stmts = parse(
        "match p\n  when [a, b,] -> a\n  when { x: q, y: r, } -> q\n  when Point(x, y,) -> x\nend",
    );
    assert_eq!(stmts.len(), 1);
}

// ---- Unary minus, now decided by grammar position alone ----

#[test]
fn negation_after_a_comma_parses_as_negation() {
    match sole_expr("f(a, -b)").kind {
        ExprKind::Call { args, .. } => {
            assert_eq!(args.len(), 2);
            assert!(matches!(
                args[1].kind,
                ExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    ..
                }
            ));
        }
        other => panic!("expected call, got {other:?}"),
    }
    match sole_expr("[1, -2]").kind {
        ExprKind::List(elems) => {
            assert_eq!(elems.len(), 2);
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
fn minus_between_two_expressions_is_always_subtraction() {
    // Spacing no longer changes the meaning of `-`.
    for src in ["[1 - 2]", "[1-2]", "[1- 2]", "[1 -2]"] {
        match sole_expr(src).kind {
            ExprKind::List(elems) => {
                assert_eq!(elems.len(), 1, "{src:?} should be one element");
                assert!(
                    matches!(elems[0].kind, ExprKind::BinaryOp { op: BinOp::Sub, .. }),
                    "{src:?} should be subtraction"
                );
            }
            other => panic!("expected list, got {other:?}"),
        }
    }
}

#[test]
fn call_argument_minus_is_subtraction() {
    match sole_expr("f(1 -2)").kind {
        ExprKind::Call { args, .. } => {
            assert_eq!(args.len(), 1, "f(1 -2) is one subtracted argument");
            assert!(matches!(
                args[0].kind,
                ExprKind::BinaryOp { op: BinOp::Sub, .. }
            ));
        }
        other => panic!("expected call, got {other:?}"),
    }
}

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
fn index_minus_is_subtraction() {
    match sole_expr("xs[3 -1]").kind {
        ExprKind::IndexAccess { index, .. } => assert!(matches!(
            index.kind,
            ExprKind::BinaryOp { op: BinOp::Sub, .. }
        )),
        other => panic!("expected index access, got {other:?}"),
    }
}

#[test]
fn negative_literal_patterns_still_parse() {
    let mut stmts = parse("match n\n  when -1 -> 0\nend");
    assert_eq!(stmts.len(), 1);
    match stmts.remove(0).kind {
        StmtKind::Expr(Expr {
            kind: ExprKind::Match { arms, .. },
            ..
        }) => {
            assert!(matches!(
                arms[0].pattern,
                Pattern::Literal(Literal::Int(-1))
            ));
        }
        other => panic!("expected match, got {other:?}"),
    }
}
