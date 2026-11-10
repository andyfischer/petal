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

// ── Cross-feature scenarios (the post-merge test plan) ─────────────────────

/// A second module, `hud`, imports `level` and declares its own private
/// `Phase` with a `label` method of its own. The two `Phase`s are different
/// types (module-scoped identity), each answers its own method, and a
/// `lv.Phase?` slot in `hud` takes the module's variant and `nil`.
const HUD: &str = "\
import level as lv
enum Phase
  idle, run(speed),
end
fn Phase.label(p: Phase) -> string
  \"hud \" ++ p.name
end
pub fn both(p: lv.Phase?) -> string
  let mine = Phase.run(1)
  let theirs = if p == nil then \"none\" else p.label() end
  mine.label() ++ \" / \" ++ theirs
end
pub fn same(p: lv.Phase) -> bool
  p == lv.Phase.idle
end
pub fn twin_equal() -> bool
  Phase.idle == lv.Phase.idle
end
";

fn env_with_hud() -> Env {
    let mut env = env();
    env.register_module("hud", HUD);
    env
}

#[test]
fn a_modules_enum_with_methods_beside_another_modules_private_twin() {
    let src = "\
import level as lv
import hud
print(hud.both(lv.Phase.run(4)))
print(hud.both(nil))
print(hud.same(lv.Phase.idle), hud.twin_equal())
print(lv.Phase.idle.label())
";
    let outs = [RunPolicy::FAST, RunPolicy::BASELINE].map(|policy| {
        let mut env = env_with_hud();
        env.set_policy(policy);
        let pid = env.load_program(src).unwrap_or_else(|e| panic!("failed: {e}"));
        let w: Vec<String> =
            env.get_program(pid).unwrap().warnings.iter().map(|d| d.message.clone()).collect();
        // The one warning is the deliberate cross-twin comparison in `hud`,
        // which names both identities.
        assert_eq!(
            w,
            ["comparing variants of two different enums (`hud::Phase` and `level::Phase`) is always false"]
        );
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap_or_else(|e| panic!("run failed: {e}"));
        env.take_output()
    });
    assert_eq!(outs[0], outs[1], "FAST and BASELINE differ");
    assert_eq!(
        outs[0],
        ["hud run / level run 4", "hud run / none", "true false", "level idle"]
    );

    // `hud`'s `Phase` is private: the entry file cannot reach it. The check
    // reports it (what `petal run` refuses on), and the run fails the same way.
    let mut env = env_with_hud();
    env.set_check_only(true);
    let pid = env.load_program("import hud\nprint(hud.Phase.idle)\n").expect("loads for the check");
    let report = env.check_program(pid);
    assert!(!report.passed());
    let err = report.render(env.get_program(pid).unwrap());
    assert!(err.contains("module 'hud' has no export 'Phase'"), "{err:?}");
}

/// `state` holding an imported module's variant (with a payload) across hot
/// reloads: an edit to the entry file, then an edit to the module that
/// changes its method's text. The kept value stays the module's type — not
/// the entry file's same-named `Phase` — and answers the new method.
#[test]
fn state_holding_a_module_scoped_variant_survives_hot_reload() {
    let v1 = "\
import level as lv
enum Phase
  idle, run(speed),
end
state n = 0
n += 1
state p: lv.Phase? = nil
if n == 1 then p = lv.Phase.run(5) end
print(\"v1\", p.label(), p == lv.Phase.run(5), p == Phase.run(5))
";
    let v2 = v1.replace("\"v1\"", "\"v2\"");
    let level2 = LEVEL.replace("\"level run {s}\"", "\"level2 run {s}\"");
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE, RunPolicy::REPLAY] {
        let mut env = env();
        env.set_policy(policy);
        let pid = env.load_program(v1).expect("v1 compiles");
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), ["v1 level run 5 true false"], "{policy:?}");

        env.reload_program(sid, &v2, None).unwrap_or_else(|e| panic!("reload v2: {e}"));
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), ["v2 level run 5 true false"], "{policy:?} after entry edit");

        env.register_module("level", &level2);
        env.reload_program(sid, &v2, None).unwrap_or_else(|e| panic!("reload level2: {e}"));
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), ["v2 level2 run 5 true false"], "{policy:?} after module edit");
        let state = serde_json::Value::Object(env.get_state_json(pid, sid)).to_string();
        assert!(state.contains("run"), "{policy:?}: {state}");
    }
}
