//! Parse errors that name the fix for a habit carried over from another
//! language, rather than blaming whatever token happened to come next.
//! See docs/tasks/testbed-takeaways.md (#8).

use petal::lexer::Lexer;
use petal::parse::Parser;

fn parse_err(src: &str) -> String {
    let mut lexer = Lexer::new(src);
    if let Err(e) = lexer.tokenize() {
        return e;
    }
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    match parser.parse_program() {
        Ok(_) => panic!("expected a parse error for {src:?}, but it parsed"),
        Err(e) => e,
    }
}

fn parses(src: &str) {
    let mut lexer = Lexer::new(src);
    lexer
        .tokenize()
        .unwrap_or_else(|e| panic!("lex failed for {src:?}: {e}"));
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    parser
        .parse_program()
        .unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"));
}

#[test]
fn json_in_a_string_names_the_brace_escape() {
    let err = parse_err(r#"print("{\"a\": 1}")"#);
    assert!(
        err.contains("Expected string part in interpolation"),
        "{err}"
    );
    assert!(err.contains("write `\\{` for a literal brace"), "{err}");
}

#[test]
fn an_end_after_an_arrow_lambda_says_arrow_lambdas_take_none() {
    // In argument position the error is the missing comma...
    let err = parse_err("print(map([1], fn(b) -> b * 2 end))");
    assert!(err.contains("Expected ','"), "{err}");
    assert!(
        err.contains("an arrow lambda (`fn(x) -> expr`) takes no `end`"),
        "{err}"
    );
    // ...and at statement level it is the stray `end`.
    let err = parse_err("let f = fn(b) -> b * 2 end");
    assert!(err.contains("takes no `end`"), "{err}");
    // Across a newline too.
    let err = parse_err("let f = fn(b) -> b * 2\nend");
    assert!(err.contains("takes no `end`"), "{err}");
}

#[test]
fn a_stray_end_elsewhere_gets_no_lambda_hint() {
    let err = parse_err("let f = fn(b) -> b * 2\nprint(1)\nend");
    assert!(!err.contains("arrow lambda"), "{err}");
    parses("let f = fn(b) b * 2 end");
}

#[test]
fn an_empty_interpolation_hole_is_reported() {
    // Used to parse the *next* literal part as the hole's expression, so
    // `"{} items"` silently printed " items".
    let err = parse_err(r#"print("{} items")"#);
    assert!(err.contains("Empty interpolation hole"), "{err}");
    assert!(err.contains("[line 1, column 8]"), "{err}");
    let err = parse_err(r#"print("a { } b")"#);
    assert!(err.contains("Empty interpolation hole"), "{err}");
    // The escaped spelling is still a literal.
    parses(r#"print("\{} items {1}")"#);
}

#[test]
fn word_operators_name_the_symbol() {
    let err = parse_err("print(a and b)");
    assert!(err.contains("Petal spells logical and as `&&`"), "{err}");
    let err = parse_err("print(a or b)");
    assert!(err.contains("Petal spells logical or as `||`"), "{err}");
    let err = parse_err("if a and b then print(1) end");
    assert!(err.contains("`&&`"), "{err}");
    let err = parse_err("if not a then print(1) end");
    assert!(err.contains("Petal spells logical not as `!`"), "{err}");
}
