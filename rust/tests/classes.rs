//! The *grammar and lowering* of `class` declarations and method declarations.
//!
//! Runtime dispatch and the checker's behaviour are covered end-to-end by
//! `ts/test/classes.test.ts`; this pins what the parser produces, what the
//! prescan makes of it, and the diagnostics that reject a malformed
//! declaration. See docs/language-guide.md#classes--methods.

use petal::ast::{Stmt, StmtKind};
use petal::classes::ClassTable;
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

/// The class table a real compilation would build for `src`, plus the fatal
/// diagnostics the prescan found. Mirrors `Compiler::prescan_classes`.
fn classes_of(src: &str) -> (ClassTable, Vec<String>) {
    let stmts = parse(src);
    let mut table = ClassTable::new();
    let diags = petal::compiler::collect_classes(&mut table, &stmts);
    (table, diags.into_iter().map(|d| d.message).collect())
}

fn errors_of(src: &str) -> Vec<String> {
    classes_of(src).1
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_a_class_with_typed_fields() {
    let stmts = parse("class Point\n  x: int\n  y: float\nend\n");
    let StmtKind::ClassDecl { name, fields } = &stmts[0].kind else {
        panic!("expected a ClassDecl, got {:?}", stmts[0].kind);
    };
    assert_eq!(name, "Point");
    assert_eq!(
        fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["x", "y"]
    );
    assert_eq!(fields[0].ty.as_ref().unwrap().resolved, Some(Type::Int));
    assert_eq!(fields[1].ty.as_ref().unwrap().resolved, Some(Type::Float));
}

#[test]
fn field_annotations_are_optional() {
    let stmts = parse("class Bag\n  a\n  b: int\nend\n");
    let StmtKind::ClassDecl { fields, .. } = &stmts[0].kind else {
        panic!("expected a ClassDecl");
    };
    assert!(fields[0].ty.is_none(), "an un-annotated field is `any`");
    assert!(fields[1].ty.is_some());
}

/// A class body is a block of declarations, not a delimited list, so it does
/// not follow the comma rule — but a comma is accepted for a one-liner.
#[test]
fn fields_separate_on_newlines_or_commas() {
    for src in [
        "class P\n  x: int\n  y: int\nend\n",
        "class P\n  x: int, y: int\nend\n",
        "class P\n  x: int,\n  y: int,\nend\n",
    ] {
        let StmtKind::ClassDecl { fields, .. } = &parse(src)[0].kind else {
            panic!("expected a ClassDecl for {src:?}");
        };
        assert_eq!(fields.len(), 2, "{src:?}");
    }
}

#[test]
fn an_empty_class_is_allowed() {
    let StmtKind::ClassDecl { fields, .. } = &parse("class Unit\nend\n")[0].kind else {
        panic!("expected a ClassDecl");
    };
    assert!(fields.is_empty());
}

#[test]
fn a_junk_field_separator_is_named() {
    let err = parse_err("class P\n  x: int y: int\nend\n");
    assert!(err.contains("between class fields"), "{err}");
}

/// `class` is contextual: it stays an ordinary identifier, which is what keeps
/// the JSX attribute `class="…"` (and any variable called `class`) working.
#[test]
fn class_is_not_a_reserved_word() {
    let stmts = parse("let class = 5\nprint(class)\n");
    assert!(matches!(stmts[0].kind, StmtKind::Let { .. }));
    let stmts = parse("let e = <div class=\"card\">hi</div>\n");
    assert!(matches!(stmts[0].kind, StmtKind::Let { .. }));
}

#[test]
fn parses_a_method_declaration() {
    let stmts = parse("fn Rect.center_x(r: Rect) -> int\n  r.x\nend\n");
    let StmtKind::FnDecl {
        name,
        class,
        params,
        ..
    } = &stmts[0].kind
    else {
        panic!("expected an FnDecl");
    };
    // The binding is the qualified name; the receiver is an ordinary first param.
    assert_eq!(name, "Rect.center_x");
    assert_eq!(class.as_deref(), Some("Rect"));
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "r");
}

#[test]
fn a_plain_function_has_no_receiver_class() {
    let StmtKind::FnDecl { name, class, .. } = &parse("fn f(x)\n  x\nend\n")[0].kind else {
        panic!("expected an FnDecl");
    };
    assert_eq!(name, "f");
    assert_eq!(*class, None);
}

// ---------------------------------------------------------------------------
// Prescan: the class table and its diagnostics
// ---------------------------------------------------------------------------

#[test]
fn rect_is_available_with_no_declaration() {
    let (table, errs) = classes_of("let r = Rect(0, 0, 1, 1)\n");
    assert!(errs.is_empty());
    let def = table.get(table.lookup("Rect").expect("Rect is built in"));
    assert_eq!(
        def.fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        ["x", "y", "w", "h"]
    );
    assert!(def.method("center_x").is_some());
}

#[test]
fn a_declared_class_becomes_a_type_name() {
    let (table, errs) = classes_of("class Point\n  x: int\nend\n");
    assert!(errs.is_empty());
    let id = table.lookup("Point").expect("Point");
    assert_eq!(Type::resolve("Point", &table), Some(Type::Class(id)));
    assert_eq!(table.get(id).field("x").unwrap().ty, Some(Type::Int));
}

/// Order-independent: a method may be declared above its class, and a class
/// name may be written in type position above the `class` statement.
#[test]
fn methods_resolve_against_classes_declared_later() {
    let (table, errs) =
        classes_of("fn Point.norm(p: Point)\n  p.x\nend\nclass Point\n  x: int\nend\n");
    assert!(errs.is_empty(), "{errs:?}");
    let def = table.get(table.lookup("Point").unwrap());
    assert_eq!(def.method("norm").map(|m| m.arity), Some(1));
}

#[test]
fn duplicate_fields_are_rejected() {
    let errs = errors_of("class P\n  x: int\n  x: int\nend\n");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("duplicate field `x`"), "{errs:?}");
}

#[test]
fn a_duplicate_class_is_rejected() {
    let errs = errors_of("class P\n  x: int\nend\nclass P\n  y: int\nend\n");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("already declared"), "{errs:?}");
}

#[test]
fn a_method_on_an_unknown_type_is_rejected() {
    let errs = errors_of("fn Nope.thing(n)\n  n\nend\n");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("no class of that name"), "{errs:?}");
}

#[test]
fn a_duplicate_method_is_rejected_but_an_arity_overload_is_not() {
    let errs = errors_of(
        "class P\n  x: int\nend\nfn P.f(p)\n  1\nend\nfn P.f(p, n)\n  2\nend\nfn P.f(p)\n  3\nend\n",
    );
    assert_eq!(errs.len(), 1, "only the same-arity redeclaration: {errs:?}");
    assert!(errs[0].contains("`P.f` is already declared"), "{errs:?}");
}

/// A user `class Rect` shadows the built-in rather than colliding with it —
/// the same rule that lets a user binding shadow a builtin function.
#[test]
fn a_user_class_may_shadow_a_builtin_class() {
    let (table, errs) = classes_of("class Rect\n  left: int\nend\n");
    assert!(errs.is_empty(), "{errs:?}");
    let def = table.get(table.lookup("Rect").unwrap());
    assert!(def.field("left").is_some());
    assert!(def.field("w").is_none());
}

/// Likewise for a single method: `fn Rect.center_x(...)` replaces the built-in
/// one, which is also the order runtime dispatch consults them in.
#[test]
fn a_user_method_may_override_a_builtin_method() {
    let errs = errors_of("fn Rect.center_x(r: Rect)\n  0\nend\n");
    assert!(errs.is_empty(), "{errs:?}");
}
