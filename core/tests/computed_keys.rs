//! Computed record keys — `{[key]: value}`. The frontend parses the field as
//! a spread of the one-field record `__record_of(key, value)`
//! (`ast::computed_record_field`), so every later layer sees a form it
//! already has. What the form evaluates to is tested in Petal:
//! test/ptl/record_keys.ptl.

use petal::ast::{Expr, ExprKind, RECORD_OF_BUILTIN, RecordField, StmtKind};
use petal::env::Env;
use petal::lexer::Lexer;
use petal::parse::Parser;

fn parse_expr(src: &str) -> Result<Expr, String> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize()?;
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    let mut stmts = parser.parse_program()?;
    match stmts.remove(0).kind {
        StmtKind::Expr(e) => Ok(e),
        other => panic!("expected an expression statement, got {other:?}"),
    }
}

fn run(src: &str) -> Result<String, String> {
    let mut env = Env::new();
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

#[test]
fn a_computed_key_is_a_spread_of_the_one_field_record() {
    let e = parse_expr("({a: 1, [k]: v})\n").expect("parse");
    let ExprKind::Record(fields) = e.kind else {
        panic!("expected a record, got {:?}", e.kind);
    };
    assert_eq!(fields.len(), 2);
    assert!(matches!(&fields[0], RecordField::Named(name, _) if name == "a"));
    let RecordField::Spread(call) = &fields[1] else {
        panic!("expected a spread, got {:?}", fields[1]);
    };
    let ExprKind::Call { function, args, arg_names } = &call.kind else {
        panic!("expected a call, got {:?}", call.kind);
    };
    assert!(matches!(&function.kind, ExprKind::Ident(n) if n == RECORD_OF_BUILTIN));
    assert!(arg_names.is_empty());
    assert!(matches!(&args[0].kind, ExprKind::Ident(n) if n == "k"));
    assert!(matches!(&args[1].kind, ExprKind::Ident(n) if n == "v"));
    // The call spans the key through the value.
    assert_eq!(call.span.start.column, args[0].span.start.column);
    assert_eq!(call.span.end.column, args[1].span.end.column);
}

#[test]
fn the_form_needs_its_bracket_its_colon_and_its_value() {
    for (src, want) in [
        ("({[k: 1})\n", "Expected"),
        ("({[k] 1})\n", "Expected"),
        ("({[k]:})\n", ""),
        ("({[]: 1})\n", ""),
    ] {
        let e = parse_expr(src).expect_err(src);
        assert!(e.contains(want), "{src:?}: {e}");
    }
}

#[test]
fn a_key_that_is_not_a_string_is_refused_by_name() {
    for (key, ty) in [("5", "int"), ("nil", "nil"), ("[1]", "list"), ("1.5", "float")] {
        let e = run(&format!("print({{[{key}]: 1}})")).expect_err(key);
        assert!(
            e.contains("A computed record key (`[key]: value`) must be a string")
                && e.contains(&format!("got {ty}"))
                && e.contains("[str(key)]"),
            "{e}"
        );
    }
    assert_eq!(run("print({[str(5)]: 1})").unwrap(), "{ 5: 1 }");
}

/// `petal check` has nothing to say about a record built with one, or about
/// reading it; `petal fmt` keeps the form and tidies its spacing.
#[test]
fn check_and_fmt_take_the_form_as_written() {
    let mut env = Env::new();
    let src = "let k = \"a\"\nlet r = {x: 1, [k]: 2}\nprint(r.a, r.x)\n";
    let pid = env.load_program(src).expect("load");
    assert!(
        env.diagnostics(pid).is_empty(),
        "{}",
        env.render_diagnostics(pid, env.diagnostics(pid))
    );
    let formatted = petal::fmt::format_source("let r = {x: 1,[ k ]:2,  [k ++ \"b\"] : 3}\n")
        .expect("fmt");
    assert!(formatted.contains("[k]: 2"), "{formatted}");
    assert!(formatted.contains("[k ++ \"b\"]: 3"), "{formatted}");
}

/// The internal builtin is not something a program reaches for: it is not
/// offered among the builtins a host or a tool lists as callable by name
/// with parameters, and it refuses a wrong call plainly.
#[test]
fn the_internal_builtin_is_only_the_forms() {
    let e = run("print(__record_of(1))").expect_err("arity");
    assert!(e.contains("__record_of"), "{e}");
    assert_eq!(run("print(__record_of(\"a\", 1))").unwrap(), "{ a: 1 }");
}
