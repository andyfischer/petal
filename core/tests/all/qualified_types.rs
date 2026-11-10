//! Qualified type names in annotations (`c: sh.Circle`) and the warning for a
//! user type that takes a built-in class's name — parts A and B1 of
//! docs/proposals/module-name-collisions.md.

use petal::env::Env;

const SHAPES: &str = "\
pub class Circle
  radius: int,
end
class Hidden
  a: int,
end
pub enum Kind
  round, square,
end
pub fn helper()
  1
end
";

fn env() -> Env {
    let mut env = Env::new();
    env.register_module("shapes", SHAPES);
    env
}

fn run(src: &str) -> String {
    let mut env = env();
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("load failed for {src:?}: {e}"));
    let sid = env.create_stack(pid).expect("create_stack");
    env.run(sid)
        .unwrap_or_else(|e| panic!("run failed for {src:?}: {e}"));
    env.take_output().join("\n").trim().to_string()
}

fn warnings(src: &str) -> Vec<String> {
    let mut env = env();
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

// ---- A: qualified type names ----

#[test]
fn an_aliased_class_and_enum_annotate_and_run() {
    let src = "\
import shapes as sh
fn area(c: sh.Circle) -> int
  c.radius * c.radius
end
fn rounded(k: sh.Kind) -> bool
  k == sh.Kind.round
end
print(area(sh.Circle(3)), rounded(sh.Kind.round))
";
    assert_clean(src);
    assert_eq!(run(src), "9 true");
}

#[test]
fn a_plain_import_qualifier_works_too() {
    assert_clean(
        "import shapes\nfn f(c: shapes.Circle) -> shapes.Kind\n  shapes.Kind.round\nend\n\
         f(shapes.Circle(1))\n",
    );
}

#[test]
fn a_qualified_type_is_the_same_type_as_the_bare_one() {
    assert_warns(
        "import shapes as sh\nlet c: sh.Circle = 5\n",
        "`c` declared `Circle` but assigned `int`",
    );
    assert_warns(
        "import shapes as sh\nfn f(c: sh.Circle)\n  c\nend\nf(\"no\")\n",
        "expected `Circle`, found `string`",
    );
    // A qualified slot takes the bare-spelled class's value: one type.
    assert_clean(
        "import shapes\nfn f(c: shapes.Circle)\n  c\nend\nlet c: Circle = Circle(1)\nf(c)\n",
    );
}

#[test]
fn a_qualified_nullable_type_takes_nil() {
    let src = "\
import shapes as sh
fn r(c: sh.Circle?) -> int
  c?.radius ?? 0
end
let none: sh.Circle? = nil
print(r(nil), r(sh.Circle(4)))
";
    assert_clean(src);
    assert_eq!(run(src), "0 4");
    assert_warns(
        "import shapes as sh\nlet k: sh.Kind? = 3\n",
        "declared `Kind?` but assigned `int`",
    );
}

#[test]
fn an_unknown_qualified_name_says_why() {
    assert_warns(
        "import shapes as sh\nfn f(c: sh.Hidden)\n  c\nend\n",
        "unknown type name `sh.Hidden`: module 'shapes' has no export `Hidden`",
    );
    assert_warns(
        "import shapes as sh\nfn f(c: nope.Circle)\n  c\nend\n",
        "unknown type name `nope.Circle`: no import binds the module qualifier `nope`",
    );
    assert_warns(
        "import shapes as sh\nfn f(c: sh.helper)\n  c\nend\n",
        "`helper` exported by module 'shapes' is not a class or enum",
    );
    // A selective import binds no qualifier.
    assert_warns(
        "import shapes: Circle\nfn f(c: shapes.Circle)\n  c\nend\n",
        "no import binds the module qualifier `shapes`",
    );
    assert_warns(
        "fn f(c: sh.Circle?)\n  c\nend\n",
        "unknown type name `sh.Circle?`",
    );
}

#[test]
fn a_qualified_return_type_is_not_the_nil_statement_form() {
    // `-> sh.Kind` keeps the implicit return.
    assert_eq!(
        run("import shapes as sh\nfn k() -> sh.Kind\n  sh.Kind.square\nend\nprint(k())\n"),
        "Kind.square"
    );
}

#[test]
fn a_dotted_annotation_does_not_swallow_what_follows() {
    // `= a.b` after an annotation still parses as the initializer.
    assert_eq!(
        run("import shapes as sh\nlet c = sh.Circle(2)\nlet r: int = c.radius\nprint(r)\n"),
        "2"
    );
}

// ---- B1: a user type named like a built-in class ----

#[test]
fn a_class_named_like_a_builtin_class_warns_and_still_runs() {
    let src = "class Rect\n  q: int,\nend\nprint(Rect(1))\n";
    assert_warns(
        src,
        "class `Rect` replaces the built-in `Rect` in this program",
    );
    assert_eq!(run(src), "{ q: 1 }");
}

#[test]
fn an_enum_named_like_a_builtin_class_warns() {
    assert_warns(
        "enum Rect\n  a, b,\nend\nprint(Rect.a)\n",
        "enum `Rect` replaces the built-in `Rect`",
    );
}

#[test]
fn other_type_names_do_not_warn() {
    assert_clean("class Rectangle\n  q: int,\nend\nprint(Rectangle(1))\n");
}
