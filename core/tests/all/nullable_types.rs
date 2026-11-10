//! Nullable types, `T?` — phase 1 of docs/proposals/nullable-types.md.
//!
//! Annotation only: `nil` and `T` fill a `T?` slot, anything that does not
//! fill `T` warns as it would for `T`, and a `T?` value fills a `T` slot
//! silently (nothing tracks nil yet). No runtime, IR or VM change, so most of
//! this file is about what the checker says — plus the one place a type
//! annotation does reach the compiled program, `-> nil`, which `-> T?` and
//! `-> nil?` must not be mistaken for.

use petal::env::Env;
use petal::lexer::Lexer;
use petal::parse::Parser;

fn run(src: &str) -> String {
    let mut env = Env::new();
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("load failed for {src:?}: {e}"));
    let sid = env.create_stack(pid).expect("create_stack");
    env.run(sid)
        .unwrap_or_else(|e| panic!("run failed for {src:?}: {e}"));
    env.take_output().join("\n").trim().to_string()
}

fn load_err(src: &str) -> String {
    match Env::new().load_program(src) {
        Ok(_) => panic!("expected {src:?} to fail to load"),
        Err(e) => e,
    }
}

/// The type-checker warnings `src` compiles with.
fn warnings(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("load failed for {src:?}: {e}"));
    env.get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

#[track_caller]
fn assert_clean(src: &str) {
    let w = warnings(src);
    assert!(w.is_empty(), "{src:?} warned: {w:?}");
}

#[track_caller]
fn assert_warns(src: &str, needle: &str) {
    let w = warnings(src);
    assert!(
        w.iter().any(|m| m.contains(needle)),
        "{src:?}: expected a warning containing {needle:?}, got {w:?}"
    );
}

fn parse_err(src: &str) -> String {
    let mut lexer = Lexer::new(src);
    lexer.tokenize().expect("lex");
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    match parser.parse_program() {
        Ok(_) => panic!("expected a parse error for {src:?}"),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// What a `T?` slot accepts
// ---------------------------------------------------------------------------

#[test]
fn a_nullable_parameter_takes_nil_and_its_base_type() {
    assert_clean("fn f(x: int?) x end\nprint(f(nil))\nprint(f(3))\n");
    assert_warns(
        "fn f(x: int?) x end\nprint(f(\"s\"))\n",
        "expected `int?`, found `string`",
    );
}

/// The motivating case: a `= nil` default on an annotated parameter warned,
/// so no such parameter in the corpus carried an annotation.
#[test]
fn a_nil_default_fits_a_nullable_parameter() {
    assert_clean("fn g(x: string? = nil) x ?? \"none\" end\nprint(g())\n");
    assert_warns(
        "fn g(x: string = nil) x end\nprint(g())\n",
        "default value for parameter `x`: expected `string`, found `nil`",
    );
    assert_warns(
        "fn g(x: string? = 3) x end\nprint(g())\n",
        "default value for parameter `x`: expected `string?`, found `int`",
    );
}

#[test]
fn let_var_and_state_annotations_accept_nil() {
    assert_clean("let a: int? = nil\nprint(a)\n");
    assert_clean("var b: record? = nil\nset b = {x: 1}\nprint(b.x)\n");
    assert_clean("state var hud: record? = nil\nprint(hud)\n");
    assert_warns(
        "let a: int? = \"s\"\nprint(a)\n",
        "declared `int?` but assigned `string`",
    );
}

#[test]
fn a_class_field_may_be_nullable() {
    assert_clean("class V\n  lit: int?,\nend\nprint(V(nil).lit)\nprint(V(3).lit)\n");
    assert_warns(
        "class V\n  lit: int,\nend\nprint(V(nil))\n",
        "found `nil`",
    );
    assert_warns(
        "class V\n  lit: int?,\nend\nprint(V(\"3\"))\n",
        "found `string`",
    );
}

/// A nullable class reads its fields as the class does, and spells its own
/// name in a diagnostic.
#[test]
fn a_nullable_class_reads_and_spells_like_its_class() {
    let cls = "class P\n  x: int,\nend\n";
    assert_clean(&format!(
        "{cls}fn f(p: P?) -> int\n  if p == nil then return 0 end\n  p.x\nend\nprint(f(P(1)))\n"
    ));
    assert_warns(
        &format!("{cls}fn f(p: P?) p.nosuch end\nprint(f(nil))\n"),
        "class `P` has no field `nosuch`",
    );
    assert_warns(
        &format!("{cls}fn f(p: P?) p end\nprint(f(3))\n"),
        "expected `P?`, found `int`",
    );
}

#[test]
fn the_existing_widenings_carry_through_a_nullable_slot() {
    assert_clean("fn f(x: float?) x end\nprint(f(1))\n");
    assert_clean("fn f(x: num?) x end\nprint(f(1))\nprint(f(2.5))\n");
    assert_clean("class P\n  x: int,\nend\nfn f(r: record?) r end\nprint(f(P(1)))\n");
}

/// Phase 1 tracks no nil, so a `T?` value fills a `T` slot without a word —
/// including a builtin's.
#[test]
fn a_nullable_value_fills_its_base_slot_silently() {
    assert_clean("fn f(x: int?) -> int x end\nprint(f(3))\n");
    assert_clean("fn take(n: int) n end\nfn f(x: int?) take(x) end\nprint(f(3))\n");
    assert_clean("fn f(s: string?) len(s) end\nprint(f(\"ab\"))\n");
}

// ---------------------------------------------------------------------------
// Return types, and `-> nil`
// ---------------------------------------------------------------------------

#[test]
fn a_nullable_return_type_holds_returns_to_t_or_nil() {
    assert_clean(
        "fn find(xs: list, v) -> int?\n  for x in xs do\n    if x == v then return x end\n  end\n  nil\nend\nprint(find([1, 2], 2))\n",
    );
    assert_warns(
        "fn f(c) -> int\n  if c then return nil end\n  1\nend\nprint(f(true))\n",
        "declares `int` but returns `nil`",
    );
    assert_clean("fn f(c) -> int?\n  if c then return nil end\n  1\nend\nprint(f(true))\n");
    assert_warns(
        "fn f(c) -> int?\n  if c then return \"x\" end\n  1\nend\nprint(f(true))\n",
        "declares `int?` but returns `string`",
    );
}

/// `-> nil` is the only return type that turns the implicit return off.
/// `-> int?` and `-> nil?` both keep the tail as the value.
#[test]
fn a_nullable_return_type_keeps_the_implicit_return() {
    assert_eq!(run("fn f() -> int?\n  5\nend\nprint(f())\n"), "5");
    assert_eq!(run("fn f() -> nil?\n  5\nend\nprint(f())\n"), "5");
    assert_eq!(run("fn f() -> nil\n  5\nend\nprint(f())\n"), "nil");

    let ann = |name: &str| petal::ast::TypeAnn::new(name.to_string());
    assert!(petal::ast::declares_nil(Some(&ann("nil"))));
    assert!(!petal::ast::declares_nil(Some(&ann("nil?"))));
    assert!(!petal::ast::declares_nil(Some(&ann("int?"))));
}

// ---------------------------------------------------------------------------
// Spellings
// ---------------------------------------------------------------------------

#[test]
fn nil_question_and_any_question_say_less_than_they_seem_to() {
    assert_warns("fn f(x: nil?) x end\nprint(f(nil))\n", "`nil?` is just `nil`");
    assert_warns("fn f(x: any?) x end\nprint(f(nil))\n", "`any` already includes `nil`");
    // Neither is an *unknown* name.
    let w = warnings("fn f(x: any?) x end\nprint(f(nil))\n");
    assert!(!w.iter().any(|m| m.contains("unknown type name")), "{w:?}");
}

#[test]
fn an_unknown_nullable_name_is_still_unknown() {
    assert_warns("fn f(x: banana?) x end\nprint(f(1))\n", "unknown type name `banana?`");
}

#[test]
fn a_doubled_question_mark_is_a_targeted_parse_error() {
    for src in [
        "fn f(x: int??) x end\n",
        "let a: int?? = nil\n",
        "fn f() -> int??\n  1\nend\n",
    ] {
        let e = parse_err(src);
        assert!(e.contains("at most one `?`"), "{src:?} → {e}");
        assert!(e.contains("`int?`"), "{src:?} → {e}");
    }
}

/// `Foo?` in type position means a nullable `Foo`, so no class or enum may be
/// named with a trailing `?`.
#[test]
fn a_class_or_enum_name_may_not_end_in_a_question_mark() {
    let e = load_err("class Foo?\n  a: int\nend\nprint(Foo?(1))\n");
    assert!(e.contains("may not end in `?`"), "{e}");
    let e = load_err("enum Bar?\n  a,\nend\nprint(1)\n");
    assert!(e.contains("may not end in `?`"), "{e}");
}

#[test]
fn a_nullable_enum_takes_its_variants_and_nil() {
    let decl = "enum Kind\n  a,\n  b,\nend\n";
    assert_clean(&format!("{decl}fn f(k: Kind?) k end\nprint(f(nil))\nprint(f(Kind.a))\n"));
    assert_warns(
        &format!("{decl}fn f(k: Kind?) k end\nprint(f(1))\n"),
        "expected `Kind?`, found `int`",
    );
}

/// Annotations never reach the runtime: a nullable annotation runs exactly as
/// the un-annotated program does, nil or not.
#[test]
fn a_nullable_annotation_does_not_change_what_runs() {
    let typed = "fn plural(n: int, one: string, many: string? = nil) -> string\n  if n == 1 then \"1 {one}\" else \"{n} {many ?? one ++ \"s\"}\" end\nend\nprint(plural(1, \"cat\"))\nprint(plural(3, \"mouse\", \"mice\"))\nprint(plural(2, \"dog\"))\n";
    assert_eq!(run(typed), "1 cat\n3 mice\n2 dogs");
    assert_clean(typed);
}
