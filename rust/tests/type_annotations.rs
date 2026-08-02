//! The *grammar* of optional type declarations: which constructs carry a `:`
//! annotation, which carries `->`, and what a type name may be.
//!
//! The checker's behaviour lives in `src/typecheck` unit tests and the AST/CST
//! differential in `src/cst_project.rs`; this pins the surface syntax, which is
//! what users actually type. See docs/dev/type-declarations-plan.md and
//! docs/language-guide.md#type-annotations.

use petal::ast::{Stmt, StmtKind, TypeAnn};
use petal::lexer::Lexer;
use petal::parse::Parser;
use petal::types::Type;

fn try_parse(src: &str) -> Result<Vec<Stmt>, String> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize()?;
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    parser.parse_program()
}

fn parse(src: &str) -> Vec<Stmt> {
    try_parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"))
}

fn parse_err(src: &str) -> String {
    match try_parse(src) {
        Ok(_) => panic!("expected a parse error for {src:?}, but it parsed"),
        Err(e) => e,
    }
}

/// The declared type of the first `let`/`var` in a program.
fn let_ty(src: &str) -> Option<TypeAnn> {
    for stmt in parse(src) {
        if let StmtKind::Let { ty, .. } = stmt.kind {
            return ty;
        }
    }
    panic!("no let statement in {src:?}");
}

/// The declared type of the first `state` in a program.
fn state_ty(src: &str) -> Option<TypeAnn> {
    for stmt in parse(src) {
        if let StmtKind::State { ty, .. } = stmt.kind {
            return ty;
        }
    }
    panic!("no state statement in {src:?}");
}

fn resolved(ann: Option<TypeAnn>) -> (String, Option<Type>) {
    let a = ann.expect("expected an annotation");
    (a.name, a.resolved)
}

// ── Every binding form takes the same `: type` slot ─────────────────────────

#[test]
fn let_and_var_take_annotations() {
    assert_eq!(
        resolved(let_ty("let x: int = 1")),
        ("int".into(), Some(Type::Int))
    );
    assert_eq!(
        resolved(let_ty("var x: float = 1.0")),
        ("float".into(), Some(Type::Float))
    );
    assert_eq!(let_ty("let x = 1"), None);
}

#[test]
fn state_takes_an_annotation_in_all_three_spellings() {
    assert_eq!(
        resolved(state_ty("state n: int = 0")),
        ("int".into(), Some(Type::Int))
    );
    assert_eq!(
        resolved(state_ty("state var n: float = 0.0")),
        ("float".into(), Some(Type::Float))
    );
    // The annotation follows the *name*, after the optional key group.
    assert_eq!(
        resolved(state_ty("state(1) n: string = \"a\"")),
        ("string".into(), Some(Type::String))
    );
    assert_eq!(
        resolved(state_ty("export state var n: bool = true")),
        ("bool".into(), Some(Type::Bool))
    );
    assert_eq!(state_ty("state n = 0"), None);
}

#[test]
fn params_and_return_types_parse() {
    let stmts = parse("fn f(a: int, b, c: str) -> float\n  1.0\nend");
    let StmtKind::FnDecl { params, ret, .. } = &stmts[0].kind else {
        panic!("expected a fn decl");
    };
    let names: Vec<Option<&str>> = params
        .iter()
        .map(|p| p.ty.as_ref().map(|t| t.name.as_str()))
        .collect();
    assert_eq!(names, vec![Some("int"), None, Some("str")]);
    assert_eq!(ret.as_ref().unwrap().resolved, Some(Type::Float));
}

#[test]
fn lambdas_take_param_annotations_but_no_return_type() {
    parse("let d = fn(n: int) -> n * 2");
    // A lambda's `->` introduces its body, so a return annotation would need two
    // arrows. Deliberately unsupported (type-declarations-plan.md §2).
    parse_err("let f = fn(x: int) -> int -> x + 1");
}

// ── Type names ──────────────────────────────────────────────────────────────

#[test]
fn nil_and_enum_are_usable_as_type_names_despite_lexing_as_keywords() {
    assert_eq!(
        resolved(let_ty("let x: nil = nil")),
        ("nil".into(), Some(Type::Nil))
    );
    assert_eq!(
        resolved(let_ty("let x: enum = 1")),
        ("enum".into(), Some(Type::Enum))
    );
}

#[test]
fn an_unknown_type_name_is_preserved_not_rejected() {
    // Kept verbatim with `resolved: None`; the *checker* warns about it, so a
    // typo never blocks compilation.
    assert_eq!(
        resolved(let_ty("let x: banana = 5")),
        ("banana".into(), None)
    );
    assert_eq!(
        resolved(state_ty("state n: banana = 5")),
        ("banana".into(), None)
    );
}

#[test]
fn parameterized_types_get_a_targeted_error_in_every_position() {
    // `list<int>` is not a type. Without a dedicated error the mistake surfaces
    // as whatever the *next* construct complains about, which differs by
    // position — including "Unclosed JSX element", since `<int>` lexes as a tag.
    for src in [
        "let xs: list<int> = [1]",
        "var xs: list<int> = [1]",
        "state xs: list<int> = []",
        "fn f(a: list<int>)\n  a\nend",
        "fn f() -> list<int>\n  []\nend",
        "let f = fn(a: list<int>) -> a",
    ] {
        let e = parse_err(src);
        assert!(
            e.contains("parameterized types are not supported") && e.contains("`list`"),
            "{src:?} produced {e:?}"
        );
    }
}

#[test]
fn a_comparison_after_a_binding_is_still_a_comparison() {
    // The `<` guard fires only in type position, immediately after a type name.
    parse("let a = 1\nlet b = a < 2");
    parse("let a: int = 1\nlet b: bool = a < 2");
}
