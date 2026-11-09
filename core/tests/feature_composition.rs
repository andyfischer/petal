//! The 2026-11 language features working together: nullable types
//! (docs/proposals/nullable-types.md), qualified type names and
//! module-scoped type identity (docs/proposals/module-name-collisions.md,
//! parts A, B1 and C), and methods on enums (docs/proposals/enum-methods.md).
//! Each feature has its own test file; this one checks they compose.

use petal::env::Env;
use petal::policy::RunPolicy;

const LEVEL: &str = "\
pub enum Phase
  idle, run(speed),
end

fn Phase.label(p: Phase) -> string
  match p
    when Phase.run(s) -> \"level run {s}\"
    when _ -> \"level \" ++ p.name
  end
end

pub class Box
  w: int,
end
";

fn env() -> Env {
    let mut env = Env::new();
    env.set_echo(false);
    env.register_module("level", LEVEL);
    env
}

/// Run `src` with the optimizer and without; the two must agree.
fn run(src: &str) -> Vec<String> {
    let outs = [RunPolicy::FAST, RunPolicy::BASELINE].map(|policy| {
        let mut env = env();
        env.set_policy(policy);
        let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed: {e}"));
        let sid = env.create_stack(pid).expect("create_stack");
        env.run(sid).unwrap_or_else(|e| panic!("{src}\n-- run failed: {e}"));
        env.take_output()
    });
    let [fast, baseline] = outs;
    assert_eq!(fast, baseline, "FAST and BASELINE differ for\n{src}");
    fast
}

fn warnings(src: &str) -> Vec<String> {
    let mut env = env();
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed: {e}"));
    env.get_program(pid).unwrap().warnings.iter().map(|d| d.message.clone()).collect()
}

#[track_caller]
fn assert_clean(src: &str) {
    let w = warnings(src);
    assert!(w.is_empty(), "{src:?} warned: {w:?}");
}

#[track_caller]
fn assert_warns(src: &str, needle: &str) {
    let w = warnings(src);
    assert!(w.iter().any(|m| m.contains(needle)), "{src:?}: expected {needle:?}, got {w:?}");
}

/// A qualified, nullable annotation of a module-scoped enum that has methods:
/// the method answers through the `T?` slot (dispatched on the tag), and
/// through a pinned `lv.Phase` slot.
#[test]
fn a_qualified_nullable_module_enum_calls_its_methods() {
    let src = "\
import level as lv
fn describe(p: lv.Phase?) -> string
  if p == nil then \"none\" else p.label() end
end
fn pinned(p: lv.Phase) -> string
  p.label()
end
print(describe(nil))
print(describe(lv.Phase.run(2)))
print(pinned(lv.Phase.idle))
";
    assert_clean(src);
    assert_eq!(run(src), ["none", "level run 2", "level idle"]);
}

/// The entry file declares its own `Phase` and `Box`; `lv.Phase` and
/// `lv.Box` in an annotation still name the module's types (part C), and the
/// warning spells out the identity when the bare names collide.
#[test]
fn a_qualified_annotation_names_the_modules_type_under_a_local_twin() {
    let src = "\
import level as lv
enum Phase
  idle,
end
fn Phase.label(p: Phase) -> string
  \"main \" ++ p.name
end
class Box
  h: int,
end
fn w(b: lv.Box?) -> int
  b?.w ?? 0
end
fn both(a: Phase, b: lv.Phase) -> string
  a.label() ++ \" / \" ++ b.label()
end
print(w(lv.Box(3)), w(nil))
print(both(Phase.idle, lv.Phase.idle))
";
    assert_clean(src);
    assert_eq!(run(src), ["3 0", "main idle / level idle"]);
    assert_warns(
        "import level as lv\nclass Box\n  h: int,\nend\nfn w(b: lv.Box) -> int\n  b.w\nend\nw(Box(1))\n",
        "found `Box`",
    );
    assert_warns(
        "import level as lv\nenum Phase\n  idle,\nend\nlet p: lv.Phase? = Phase.idle\n",
        "declared `level::Phase?` but assigned `Phase`",
    );
}

/// B1 under C: a module's `class Rect` is that module's own type, so the
/// warning says it hides the built-in there; the entry file still gets the
/// program-wide one.
#[test]
fn a_modules_rect_hides_the_builtin_only_there() {
    let mut env = Env::new();
    env.register_module("geo", "class Rect\n  q: int,\nend\npub fn make()\n  Rect(1)\nend\n");
    let pid = env
        .load_program("import geo: make\nprint(Rect(0, 0, 4, 2).center_x(), make())\n")
        .expect("compiles");
    let w: Vec<String> =
        env.get_program(pid).unwrap().warnings.iter().map(|d| d.message.clone()).collect();
    assert!(
        w.iter().any(|m| m.contains("class `Rect` hides the built-in `Rect` in this module")),
        "{w:?}"
    );
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["2 { q: 1 }"]);
}

/// `const(…)` (docs/tasks/lifecycle-annotation-functions-refactor.md) of an
/// enum variant: a module enum's through a qualified, nullable annotation,
/// and the entry file's own enum with a method. The variant is a static
/// value, so neither is a `const` error, and its methods answer.
#[test]
fn const_of_an_enum_variant_keeps_its_type_and_methods() {
    let src = "\
import level as lv
enum Mode
  idle, busy,
end
fn Mode.label(m: Mode) -> string
  \"mode \" ++ m.name
end
let P: lv.Phase = const(lv.Phase.run(2))
let Q: lv.Phase? = const(lv.Phase.idle)
let M = const(Mode.busy)
print(P.label(), Q.label(), M.label())
";
    assert_clean(src);
    assert_eq!(run(src), ["level run 2 level idle mode busy"]);
    assert_warns(
        "import level as lv\nlet P: lv.Box? = const(lv.Phase.idle)\n",
        "declared `Box?` but assigned `Phase`",
    );
}
