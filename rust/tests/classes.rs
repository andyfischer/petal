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
    classes_of_in(src, None)
}

/// The same, for `src` compiled as the named module — the file attribution a
/// cross-module duplicate names.
fn classes_of_in(src: &str, module: Option<&str>) -> (ClassTable, Vec<String>) {
    let stmts = parse(src);
    let mut table = ClassTable::new();
    let diags = petal::compiler::collect_classes(&mut table, &stmts, module);
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

/// The receiver of `fn A.go(...)` is an `A`. Annotating it as some other class
/// describes a call that can never happen — the declaration is malformed, not
/// merely suspect, so it is a hard error rather than a warning.
#[test]
fn a_receiver_annotated_as_another_class_is_rejected() {
    let errs =
        errors_of("class A\n  a: int\nend\nclass B\n  b: int\nend\nfn A.go(x: B)\n  x.a\nend\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("`A.go`"), "{errs:?}");
    assert!(errs[0].contains('B') && errs[0].contains('A'), "{errs:?}");
}

/// Same for a receiver annotated with a non-class type.
#[test]
fn a_receiver_annotated_as_a_builtin_type_is_rejected() {
    let errs = errors_of("class A\n  a: int\nend\nfn A.go(x: int)\n  x\nend\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("`A.go`"), "{errs:?}");
}

/// The receiver annotation is optional, and any slot an instance *fits* is
/// fine: its own class, `any`, `record` (an instance is a record), or nothing.
#[test]
fn a_receiver_slot_that_accepts_an_instance_is_accepted() {
    for src in [
        "class A\n  a: int\nend\nfn A.go(x: A)\n  x.a\nend\n",
        "class A\n  a: int\nend\nfn A.go(x)\n  x.a\nend\n",
        "class A\n  a: int\nend\nfn A.go(x: any)\n  x.a\nend\n",
        "class A\n  a: int\nend\nfn A.go(x: record)\n  x\nend\n",
        // An unrecognized name is already reported as a warning by the checker;
        // it must not also be read as a conflicting receiver.
        "class A\n  a: int\nend\nfn A.go(x: banana)\n  x\nend\n",
    ] {
        assert!(errors_of(src).is_empty(), "{src:?}");
    }
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

// ---------------------------------------------------------------------------
// Scoping: a class is a top-level, file-scoped declaration
// ---------------------------------------------------------------------------

/// A `class` is a top-level declaration. Nesting one inside a function used to
/// parse and compile while being invisible to the class table, so the inner
/// name silently aliased an outer class of the same name (same heap tag, same
/// method dispatch) — declared enough to dispatch, not declared enough to
/// resolve as a type.
#[test]
fn a_nested_class_is_rejected() {
    let errs = errors_of("fn f()\n  class Inner\n    a: int\n  end\n  Inner(1)\nend\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("`Inner`"), "{errs:?}");
    assert!(errs[0].contains("top level"), "{errs:?}");
}

/// Every nesting form, not just a function body.
#[test]
fn a_class_nested_in_control_flow_is_rejected() {
    for src in [
        "if true then\n  class A\n    a: int\n  end\nend\n",
        "while false do\n  class A\n    a: int\n  end\nend\n",
        "for i in [1] do\n  class A\n    a: int\n  end\nend\n",
        "let f = fn()\n  class A\n    a: int\n  end\nend\n",
    ] {
        let errs = errors_of(src);
        assert_eq!(errs.len(), 1, "{src:?} → {errs:?}");
        assert!(errs[0].contains("top level"), "{src:?} → {errs:?}");
    }
}

/// A nested class is *not* registered, so an outer class of the same name is
/// left alone rather than being replaced by the inner shape.
#[test]
fn a_nested_class_does_not_redefine_the_outer_one() {
    let (table, errs) = classes_of(
        "class Inner\n  a: int\nend\nfn f()\n  class Inner\n    b: int\n    c: int\n  end\n  1\nend\n",
    );
    assert_eq!(errs.len(), 1, "{errs:?}");
    let def = table.get(table.lookup("Inner").unwrap());
    assert!(def.field("a").is_some(), "the top-level shape survives");
    assert!(def.field("b").is_none());
}

/// A class name may not collide with a built-in type name. `Type::resolve`
/// puts the built-in vocabulary first, so `class int` could never be reached
/// in type position — the annotation would silently mean the primitive while
/// the constructor produced a record, and a diagnostic would read
/// "expected `int`, found `int`".
#[test]
fn a_class_named_after_a_builtin_type_is_rejected() {
    for name in [
        "int", "float", "string", "str", "list", "record", "any", "bool",
    ] {
        let errs = errors_of(&format!("class {name}\n  a: int\nend\n"));
        assert_eq!(errs.len(), 1, "class {name} → {errs:?}");
        assert!(errs[0].contains("built-in type name"), "{errs:?}");
        assert!(errs[0].contains(name), "{errs:?}");
    }
}

/// A class name that merely collides with a built-in *function* is fine — the
/// class wins in call position, as any user binding does.
#[test]
fn a_class_named_after_a_builtin_function_is_allowed() {
    let (table, errs) = classes_of("class len\n  a: int\nend\n");
    assert!(errs.is_empty(), "{errs:?}");
    assert!(table.lookup("len").is_some());
}

/// The class namespace spans the compilation, so two modules declaring the
/// same class name still collide — but the error has to say *where*.
#[test]
fn a_cross_module_duplicate_names_both_files() {
    let mut table = ClassTable::new();
    let a = parse("class Dup\n  a: int\nend\n");
    let b = parse("class Dup\n  b: int\nend\n");
    assert!(petal::compiler::collect_classes(&mut table, &a, Some("ma.ptl")).is_empty());
    let diags = petal::compiler::collect_classes(&mut table, &b, Some("mb.ptl"));
    assert_eq!(diags.len(), 1);
    let msg = &diags[0].message;
    assert!(msg.contains("ma.ptl"), "{msg}");
    assert!(msg.contains("mb.ptl"), "{msg}");
}

/// Only classes named in the scope are resolvable — that is how a
/// module-private class stops being a type name in an importer.
#[test]
fn a_scope_hides_classes_it_does_not_name() {
    let (mut table, errs) = classes_of("class Shown\n  a: int\nend\nclass Hidden\n  b: int\nend\n");
    assert!(errs.is_empty(), "{errs:?}");
    table.set_scope(["Shown".to_string()].into_iter().collect());
    assert!(table.lookup("Shown").is_some());
    assert!(table.lookup("Hidden").is_none(), "not named by the scope");
    assert!(
        table.lookup("Rect").is_some(),
        "built-ins are always visible"
    );
    table.clear_scope();
    assert!(table.lookup("Hidden").is_some());
}

/// …and a method declaration obeys the same scope, so an importer cannot
/// extend a class another module keeps private.
#[test]
fn a_method_on_an_out_of_scope_class_is_rejected() {
    let mut table = ClassTable::new();
    let owner = parse("class Hidden\n  a: int\nend\n");
    assert!(petal::compiler::collect_classes(&mut table, &owner, Some("m.ptl")).is_empty());
    table.set_scope(std::collections::HashSet::new()); // an importer, importing nothing
    let importer = parse("fn Hidden.twice(h)\n  2 * h.a\nend\n");
    let diags = petal::compiler::collect_classes(&mut table, &importer, Some("app.ptl"));
    assert_eq!(diags.len(), 1);
    assert!(
        diags[0].message.contains("no class of that name"),
        "{diags:?}"
    );
}
