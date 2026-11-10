//! Methods on enums (docs/proposals/enum-methods.md).
//!
//! `fn Kind.describe(k: Kind, …)` declares a method on an enum, called as
//! `k.describe(…)` on any variant. It is a class method in every respect:
//! the declaration rules, the receiver-first call, pinning when the checker
//! knows the receiver's enum, and runtime dispatch on the receiver's label
//! otherwise — for a variant, the enum half of its tag, module and all
//! (`./level::Phase.run` → `./level::Phase`).

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::policy::RunPolicy;
use petal::program::TermOp;
use petal::value::Value;

fn env_with(modules: &[(&str, &str)]) -> Env {
    let mut env = Env::new();
    env.set_echo(false);
    for (name, source) in modules {
        env.register_module(name, source);
    }
    env.register_native("host_variant", native_host_variant, NativeEffects::PURE);
    env
}

/// `host_variant(tag)`: a payload-free variant built by the host from tag
/// text, the way `PetalCxt::emit` and the C bridge do.
fn native_host_variant(cx: &mut PetalCxt) -> NativeResult {
    let Value::String(tag) = cx.get_value(1)? else {
        return Err("host_variant: tag".into());
    };
    let tag = cx.heap().get_string(tag).to_string();
    let heap = cx.heap_mut();
    let tag = heap.enum_tag_str(&tag);
    let data = heap.alloc_list(Vec::new());
    cx.push_value(Value::EnumVariant { tag, data });
    Ok(1)
}

/// Run `src` under the optimizer and without it; the two must agree.
fn outcome_with(modules: &[(&str, &str)], src: &str) -> Result<Vec<String>, String> {
    let outs = [RunPolicy::FAST, RunPolicy::BASELINE].map(|policy| {
        let mut env = env_with(modules);
        env.set_policy(policy);
        let pid = env.load_program(src)?;
        let sid = env.create_stack(pid)?;
        env.run(sid)?;
        Ok::<_, String>(env.take_output())
    });
    let [fast, baseline] = outs;
    assert_eq!(fast, baseline, "FAST and BASELINE differ for\n{src}");
    fast
}

fn run_with(modules: &[(&str, &str)], src: &str) -> Vec<String> {
    outcome_with(modules, src).unwrap_or_else(|e| panic!("{src}\n-- failed: {e}"))
}

fn run(src: &str) -> Vec<String> {
    run_with(&[], src)
}

fn run_err(src: &str) -> String {
    match outcome_with(&[], src) {
        Ok(out) => panic!("expected an error, got {out:?} for\n{src}"),
        Err(e) => e,
    }
}

fn compile_err_with(modules: &[(&str, &str)], src: &str) -> String {
    match env_with(modules).load_program(src) {
        Ok(_) => panic!("expected a compile error, but this compiled:\n{src}"),
        Err(e) => e,
    }
}

fn warnings(src: &str) -> Vec<String> {
    let mut env = env_with(&[]);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
    let program = env.get_program(pid).unwrap();
    program.warnings.iter().map(|d| d.message.clone()).collect()
}

/// Whether `src` still contains a method call dispatched at runtime.
fn dispatches(src: &str) -> bool {
    let mut env = env_with(&[]);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("compiles: {e}\n{src}"));
    env.get_program(pid)
        .unwrap()
        .terms
        .iter()
        .any(|t| matches!(t.op, TermOp::MethodCall { .. }))
}

const TY: &str = "\
enum Ty
  float(dim: 1), vec2(dim: 2), vec3(dim: 3),
  named(id, dim: 0),
  unknown(dim: 0),
end

fn Ty.text(t: Ty) -> string
  match t
    when Ty.named(id) -> id
    when Ty.unknown -> \"?\"
    when _ -> t.name
  end
end

fn Ty.is_vec(t: Ty) -> bool
  t.dim > 1
end
";

// ── Calls ──────────────────────────────────────────────────────────────────

/// The proposal's headline example, with payload and data reads inside.
#[test]
fn a_method_answers_on_every_variant() {
    let src = format!("{TY}print(Ty.vec3.text(), Ty.named(\"Fx\").text(), Ty.vec2.is_vec())\n");
    assert_eq!(run(&src), ["vec3 Fx true"]);
}

/// A receiver the checker cannot type dispatches on the variant's tag.
#[test]
fn an_untyped_receiver_dispatches_on_the_tag() {
    let src = format!(
        "{TY}fn show(x)\n  x.text()\nend\nfor t in [Ty.float, Ty.unknown, Ty.named(\"S\")] do\n  print(show(t))\nend\n"
    );
    assert_eq!(run(&src), ["float", "?", "S"]);
}

/// One untyped call site, two enums: each variant reaches its own enum's
/// method, as a class instance reaches its own class's.
#[test]
fn dispatch_is_per_enum() {
    let src = "\
enum A
  x,
end
enum B
  x,
end
fn A.who(a)
  \"A\"
end
fn B.who(b)
  \"B\"
end
fn who(v)
  v.who()
end
print(who(A.x), who(B.x))
";
    assert_eq!(run(src), ["A B"]);
}

/// Overloads by arity, and arguments after the receiver.
#[test]
fn methods_overload_by_arity_and_take_arguments() {
    let src = "\
enum Piece
  I(box: 4), O(box: 2),
end
fn Piece.size(p: Piece) -> int
  p.box
end
fn Piece.size(p: Piece, scale: int) -> int
  p.box * scale
end
print(Piece.I.size(), Piece.O.size(3))
";
    assert_eq!(run(src), ["4 6"]);
}

/// A method may call another method of its enum, and may be declared below
/// its use (methods are hoisted).
#[test]
fn methods_are_hoisted_and_call_each_other() {
    let src = "\
enum Mode
  idle, run(speed),
end
print(Mode.run(4).loud())
fn Mode.label(m: Mode) -> string
  match m
    when Mode.run(s) -> \"run {s}\"
    when _ -> m.name
  end
end
fn Mode.loud(m: Mode) -> string
  m.label() ++ \"!\"
end
";
    assert_eq!(run(src), ["run 4!"]);
}

/// A variant built by the constructor is pinned: the call compiles to a
/// direct `Call` of the declaration, not a dispatched `MethodCall`.
#[test]
fn a_typed_receiver_is_pinned() {
    for form in ["let t: Ty = Ty.vec2", "let t = Ty.vec2", "state t: Ty = Ty.vec2", "let t = Ty.named(\"x\")"] {
        assert!(!dispatches(&format!("{TY}{form}\nprint(t.text())\n")), "{form}");
    }
    assert!(!dispatches(&format!("{TY}fn show(t: Ty)\n  t.text()\nend\n")));
    assert!(dispatches(&format!("{TY}fn show(x)\n  x.text()\nend\n")), "an untyped receiver dispatches");
}

/// What no method answers is reported as before.
#[test]
fn a_missing_method_is_reported_against_the_enum() {
    let e = run_err("enum Kind\n  park,\nend\nfn f(k)\n  k.describe()\nend\nf(Kind.park)\n");
    assert!(e.contains("No method 'describe' on enum Kind"), "{e}");
}

/// The global-native fallback still answers on a variant.
#[test]
fn a_builtin_still_answers_through_method_syntax() {
    let src = format!("{TY}print(Ty.vec3.str())\n");
    assert_eq!(run(&src), ["Ty.vec3"]);
}

// ── The checker ────────────────────────────────────────────────────────────

/// A pinned call is checked against the declaration: its arity, and its
/// declared return type flows into the caller.
#[test]
fn a_typed_call_is_checked_against_the_declaration() {
    let w = warnings(&format!("{TY}let t: Ty = Ty.vec2\nprint(t.text(1, 2))\n"));
    assert!(w.iter().any(|w| w.contains("method `Ty.text`")), "{w:?}");
    let w = warnings(&format!("{TY}let n: int = Ty.vec2.text()\nprint(n)\n"));
    assert!(w.iter().any(|w| w.contains("`string`")), "{w:?}");
    assert!(warnings(&format!("{TY}let s: string = Ty.vec2.text()\nprint(s)\n")).is_empty());
}

// ── Modules: the enum comes from the tag's identity ────────────────────────

const LEVEL: &str = "\
pub enum Phase
  idle, run(speed),
end
pub fn Phase.label(p: Phase) -> string
  match p
    when Phase.run(s) -> \"level run {s}\"
    when _ -> \"level \" ++ p.name
  end
end
pub fn make(n)
  Phase.run(n)
end
";

/// Two modules each declare `enum Phase` with a `label` method. A variant
/// reaches the method of the enum its tag names, pinned or not.
#[test]
fn a_module_enums_method_is_found_from_its_qualified_tag() {
    let src = "\
import level as lv
enum Phase
  idle, run(speed),
end
fn Phase.label(p: Phase) -> string
  \"main \" ++ p.name
end
fn any_label(x)
  x.label()
end
let a = lv.make(3)
print(a.label(), Phase.run(1).label(), lv.Phase.idle.label())
print(any_label(a), any_label(Phase.idle))
";
    assert_eq!(
        run_with(&[("level", LEVEL)], src),
        ["level run 3 main run level idle", "level run 3 main idle"]
    );
}

/// A host-built variant with a bare tag reaches the module enum's method
/// through the bare-tag fallback; one with a tag no enum declares reaches
/// nothing.
#[test]
fn host_built_variants() {
    let src = "import level\nnative fn host_variant(tag)\nprint(host_variant(\"Phase.idle\").label())\n";
    assert_eq!(run_with(&[("level", LEVEL)], src), ["level idle"]);
    let e = match outcome_with(&[("level", LEVEL)], "import level\nnative fn host_variant(tag)\nprint(host_variant(\"rect\").label())\n") {
        Ok(out) => panic!("expected an error, got {out:?}"),
        Err(e) => e,
    };
    assert!(e.contains("No method 'label'"), "{e}");
}

/// Only an enum of this file takes methods.
#[test]
fn a_method_on_an_imported_enum_is_refused() {
    let e = compile_err_with(&[("level", LEVEL)], "import level\nfn Phase.flip(p)\n  1\nend\n");
    assert!(e.contains("`Phase` is an enum of another module"), "{e}");
}

// ── Live edits (the proposal's pair 5) ─────────────────────────────────────

/// Run `v1`, transfer onto `v2`, run again, and return the second run's output.
fn after_reload(v1: &str, v2: &str) -> Result<Vec<String>, String> {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(v1).expect("v1 compiles");
    let sid = env.create_stack(pid).expect("v1 stack");
    env.run(sid).expect("v1 runs");
    env.take_output();
    let next = env.compile_program(pid, v2).expect("v2 compiles");
    env.transfer_state(sid, next).expect("transfer");
    env.run(sid).map(|_| env.take_output())
}

const MODE_V1: &str = "\
enum Mode
  idle, run(speed),
end
fn Mode.label(m: Mode) -> string
  match m
    when Mode.run(s) -> \"run {s}\"
    when _ -> m.name
  end
end
state m: Mode = Mode.run(3)
print(m.label())
";

const GAIT: &str = "\
enum Gait
  idle, run(speed),
end
fn Gait.label(m: Gait) -> string
  match m
    when Gait.run(s) -> \"gait {s}\"
    when _ -> \"other \" ++ m.name
  end
end
";

/// The enum is renamed while `state` holds a `Mode.run`. An annotated slot
/// binds the call to `Gait.label`, which runs on the old value; its arms do
/// not match a variant of an enum that no longer exists, and `when _`
/// answers.
#[test]
fn renaming_the_enum_runs_the_new_method_on_a_pinned_receiver() {
    let v2 = format!("{GAIT}state m: Gait = Gait.run(3)\nprint(m.label())\n");
    assert_eq!(after_reload(MODE_V1, &v2).expect("runs"), ["other run"]);
}

/// The same edit with an un-annotated slot: dispatch reads the label `Mode`,
/// finds nothing, and falls back to the enum the declaration names.
#[test]
fn renaming_the_enum_falls_back_to_the_declarations_enum_when_unpinned() {
    let v1 = MODE_V1.replace("state m: Mode", "state m");
    let v2 = format!("{GAIT}state m = Gait.run(3)\nprint(m.label())\n");
    assert_eq!(after_reload(&v1, &v2).expect("runs"), ["other run"]);
}

/// A method edit alone is an ordinary function edit.
#[test]
fn an_edited_method_body_takes_effect() {
    let v2 = MODE_V1.replace("\"run {s}\"", "\"running {s}\"");
    assert_eq!(after_reload(MODE_V1, &v2).expect("runs"), ["running 3"]);
    let (v1, v2) = (MODE_V1.replace("state m: Mode", "state m"), v2.replace("state m: Mode", "state m"));
    assert_eq!(after_reload(&v1, &v2).expect("runs"), ["running 3"]);
}
