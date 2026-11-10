//! Module-scoped class and enum identity
//! (docs/proposals/module-name-collisions.md, part C).
//!
//! A class or enum belongs to the module that declares it. The entry file's
//! keep their bare names as identities (`Phase`, tag `Phase.play`); any other
//! module's carry its canonical name (`lib::Phase`, tag `lib::Phase.play`).
//! Display (`print`, `str`, `type()`, `.name`) leaves the module out; equality,
//! `match` and method dispatch go by the identity.

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::policy::RunPolicy;
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

/// `host_variant(tag, ...payload)`: a variant built by the host from tag text,
/// the way `PetalCxt::emit` and the C bridge's `pb_builder_begin_enum` do.
fn native_host_variant(cx: &mut PetalCxt) -> NativeResult {
    let Value::String(tag) = cx.get_value(1)? else {
        return Err("host_variant: tag".into());
    };
    let tag = cx.heap().get_string(tag).to_string();
    let payload: Vec<Value> = (2..=cx.arg_count()).map(|i| cx.get_value(i)).collect::<Result<_, _>>()?;
    let heap = cx.heap_mut();
    let tag = heap.enum_tag_str(&tag);
    let data = heap.alloc_list(payload);
    cx.push_value(Value::EnumVariant { tag, data });
    Ok(1)
}

/// Run `src` under the optimizer and without it; the two must print alike.
fn run_with(modules: &[(&str, &str)], src: &str) -> Vec<String> {
    let mut outs = [RunPolicy::FAST, RunPolicy::BASELINE].map(|policy| {
        let mut env = env_with(modules);
        env.set_policy(policy);
        let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap_or_else(|e| panic!("{src}\n-- failed to run: {e}"));
        env.take_output()
    });
    assert_eq!(outs[0], outs[1], "FAST and BASELINE differ for\n{src}");
    std::mem::take(&mut outs[0])
}

fn compile_err_with(modules: &[(&str, &str)], src: &str) -> String {
    match env_with(modules).load_program(src) {
        Ok(_) => panic!("expected a compile error, but this compiled:\n{src}"),
        Err(e) => e,
    }
}

fn warnings_with(modules: &[(&str, &str)], src: &str) -> Vec<String> {
    let mut env = env_with(modules);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- failed to compile: {e}"));
    let program = env.get_program(pid).unwrap();
    program.warnings.iter().map(|d| d.message.clone()).collect()
}

const A: &str = "\
enum Phase
  play, over,
end
pub fn a_phase()
  Phase.play
end
pub fn a_name(p)
  match p
    when Phase.play -> \"a.play\"
    when Phase.over -> \"a.over\"
    when _ -> \"not a's\"
  end
end
";

const B: &str = "\
enum Phase
  menu, play,
end
pub fn b_phase()
  Phase.menu
end
pub fn b_play()
  Phase.play
end
pub fn b_name(p)
  match p
    when Phase.play -> \"b.play\"
    when Phase.menu -> \"b.menu\"
    when _ -> \"not b's\"
  end
end
";

#[test]
fn two_modules_may_each_declare_a_private_enum_of_one_name() {
    let src = "import a\nimport b\nprint(a_phase(), b_phase())\n";
    assert_eq!(run_with(&[("a", A), ("b", B)], src), ["Phase.play Phase.menu"]);
}

#[test]
fn same_named_variants_of_two_modules_are_different_values() {
    let src = "\
import a
import b
print(a_phase() == b_play(), a_phase() != b_play(), a_phase() == a_phase(), b_play() == b_play())
print(a_name(a_phase()), a_name(b_play()), b_name(b_play()), b_name(a_phase()))
";
    assert_eq!(
        run_with(&[("a", A), ("b", B)], src),
        ["false true true true", "a.play not a's b.play not b's"]
    );
}

#[test]
fn display_leaves_the_module_out() {
    let src = "\
import a
let p = a_phase()
print(p, str(p), p.name, type(p))
";
    assert_eq!(run_with(&[("a", A)], src), ["Phase.play Phase.play play Phase"]);
}

#[test]
fn the_tag_carries_the_module_in_the_ir() {
    let mut env = env_with(&[("a", A)]);
    let pid = env.load_program("import a\nprint(a_phase())").unwrap();
    let program = env.get_program(pid).unwrap();
    assert!(program.enums.contains_key("a::Phase"), "{:?}", program.enums.keys());
    let ir = serde_json::to_string(program).unwrap();
    assert!(ir.contains("a::Phase.play"), "the IR spells the full tag");
}

#[test]
fn an_entry_file_enum_keeps_its_bare_tag() {
    let mut env = env_with(&[("a", A)]);
    let pid = env
        .load_program("import a\nenum Phase\n  idle,\nend\nprint(Phase.idle, a_phase())")
        .unwrap();
    let program = env.get_program(pid).unwrap();
    let mut keys: Vec<&String> = program.enums.keys().collect();
    keys.sort();
    assert_eq!(keys, ["Phase", "a::Phase"]);
}

const SHAPES: &str = "\
pub class Circle
  radius: int,
end
fn Circle.area(c: Circle) -> int
  c.radius * c.radius * 3
end
pub enum Kind
  round, square,
end
";

#[test]
fn a_file_may_declare_a_class_an_import_exports() {
    // The file's own `Circle` wins the bare name (tier 1); `shapes.Circle`
    // still names the module's, and each dispatches its own methods.
    let src = "\
import shapes
class Circle
  r: int,
end
fn Circle.area(c: Circle) -> int
  c.r
end
let mine = Circle(4)
let theirs = shapes.Circle(2)
print(mine.area(), theirs.area(), type(mine), type(theirs), mine == theirs)
";
    assert_eq!(run_with(&[("shapes", SHAPES)], src), ["4 12 Circle Circle false"]);
}

#[test]
fn a_file_may_declare_an_enum_an_import_exports() {
    let src = "\
import shapes
enum Kind
  round, oval,
end
print(Kind.round, shapes.Kind.round, Kind.round == shapes.Kind.round, Kind.all, shapes.Kind.all)
print(match shapes.Kind.round
  when Kind.round -> \"mine\"
  when shapes.Kind.round -> \"theirs\"
end)
";
    assert_eq!(
        run_with(&[("shapes", SHAPES)], src),
        [
            "Kind.round Kind.round false [Kind.round, Kind.oval] [Kind.round, Kind.square]",
            "theirs"
        ]
    );
}

#[test]
fn two_modules_may_each_declare_a_class_of_one_name_with_methods() {
    let box_mod = |factor: i32| {
        format!(
            "class Box\n  w: int,\nend\nfn Box.area(b: Box) -> int\n  b.w * {factor}\nend\n\
             pub fn make(w)\n  Box(w)\nend\n"
        )
    };
    let (a, b) = (box_mod(2), box_mod(10));
    let src = "import a as one\nimport b as two\nprint(one.make(3).area(), two.make(3).area(), type(one.make(1)))";
    assert_eq!(run_with(&[("a", &a), ("b", &b)], src), ["6 30 Box"]);
}

#[test]
fn a_method_attaches_to_the_class_visible_where_it_is_declared() {
    let src = "\
import shapes: Circle
fn Circle.diameter(c: Circle) -> int
  c.radius * 2
end
print(Circle(5).diameter(), Circle(5).area())
";
    assert_eq!(run_with(&[("shapes", SHAPES)], src), ["10 75"]);
}

#[test]
fn one_module_declaring_a_name_twice_is_still_an_error() {
    let e = compile_err_with(
        &[("lib", "pub enum E\n  a,\nend\nenum E\n  b,\nend")],
        "import lib\nprint(1)",
    );
    assert!(e.contains("enum `E` is already declared"), "{e}");
    let e = compile_err_with(
        &[("lib", "class E\n  x,\nend\nenum E\n  a,\nend")],
        "import lib\nprint(1)",
    );
    assert!(e.contains("enum `E` has the same name as class `E`"), "{e}");
}

#[test]
fn two_plain_imports_of_one_type_name_are_ambiguous_only_bare() {
    let other = "pub enum Kind\n  round, pointy,\nend\n";
    let e = compile_err_with(
        &[("shapes", SHAPES), ("other", other)],
        "import shapes\nimport other\nprint(Kind.round)",
    );
    assert!(e.contains("`Kind` is ambiguous"), "{e}");
    let src = "import shapes\nimport other\nprint(shapes.Kind.round, other.Kind.pointy)";
    assert_eq!(
        run_with(&[("shapes", SHAPES), ("other", other)], src),
        ["Kind.round Kind.pointy"]
    );
    // A selective import picks one.
    let src = "import shapes\nimport other: Kind\nprint(Kind.pointy)";
    assert_eq!(run_with(&[("shapes", SHAPES), ("other", other)], src), ["Kind.pointy"]);
}

#[test]
fn comparing_variants_of_two_different_enums_warns() {
    let other = "pub enum Kind\n  round, pointy,\nend\n";
    let w = warnings_with(
        &[("shapes", SHAPES), ("other", other)],
        "import shapes\nimport other\nprint(shapes.Kind.round == other.Kind.round)",
    );
    assert!(
        w.iter().any(|m| m.contains("comparing variants of two different enums (`shapes::Kind` and `other::Kind`) is always false")),
        "{w:?}"
    );
    // In one file too, with the bare names.
    let w = warnings_with(&[], "enum A\n  x,\nend\nenum B\n  x,\nend\nprint(A.x != B.x)");
    assert!(w.iter().any(|m| m.contains("comparing variants of two different enums (`A` and `B`) is always true")), "{w:?}");
    // Not between variants of one enum.
    let w = warnings_with(&[("shapes", SHAPES)], "import shapes\nprint(shapes.Kind.round == shapes.Kind.square)");
    assert!(!w.iter().any(|m| m.contains("comparing")), "{w:?}");
}

#[test]
fn a_type_mismatch_between_same_named_types_spells_the_identity() {
    let src = "\
import shapes
class Circle
  r: int,
end
fn own(c: Circle) -> int
  c.r
end
print(own(shapes.Circle(2)))
";
    let w = warnings_with(&[("shapes", SHAPES)], src);
    assert!(
        w.iter().any(|m| m.contains("expected `Circle`, found `shapes::Circle`")),
        "{w:?}"
    );
}

#[test]
fn a_host_built_bare_tag_means_the_one_enum_of_that_name() {
    // Only `shapes` declares `Kind`: the host's `"Kind.round"` is its variant.
    let src = "\
import shapes
native fn host_variant(...args)
let k = host_variant(\"Kind.round\")
print(k == shapes.Kind.round, k, type(k))
print(match k
  when shapes.Kind.round -> \"round\"
  when _ -> \"other\"
end)
";
    assert_eq!(
        run_with(&[("shapes", SHAPES)], src),
        ["true Kind.round Kind", "round"]
    );
    // The qualified tag works whatever else is declared.
    let src = "import shapes\nnative fn host_variant(...args)\nprint(host_variant(\"shapes::Kind.square\") == shapes.Kind.square)";
    assert_eq!(run_with(&[("shapes", SHAPES)], src), ["true"]);
}

#[test]
fn a_host_built_bare_tag_of_a_shared_name_needs_the_module() {
    // Two modules declare `Kind`: the bare tag means neither, and the host
    // writes the qualified one.
    let other = "pub enum Kind\n  round, pointy,\nend\n";
    let src = "\
import shapes
import other
native fn host_variant(...args)
let k = host_variant(\"Kind.round\")
print(k == shapes.Kind.round, k == other.Kind.round, k)
print(host_variant(\"other::Kind.round\") == other.Kind.round)
";
    assert_eq!(
        run_with(&[("shapes", SHAPES), ("other", other)], src),
        ["false false Kind.round", "true"]
    );
    // An entry-file enum owns its bare name.
    let src = "\
import shapes
native fn host_variant(...args)
enum Kind
  round,
end
print(host_variant(\"Kind.round\") == Kind.round, host_variant(\"Kind.round\") == shapes.Kind.round)
";
    assert_eq!(run_with(&[("shapes", SHAPES)], src), ["true false"]);
}

#[test]
fn a_module_variant_in_state_survives_a_reload() {
    let cfg = "pub enum Kind\n  park, road,\nend\n";
    let mut env = env_with(&[("cfg", cfg)]);
    let src = "import cfg\nstate k = cfg.Kind.park\nk = cfg.Kind.road\nprint(k)\n";
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["Kind.road"]);
    let edited = "import cfg\nstate k = cfg.Kind.park\nprint(k, k == cfg.Kind.road, k.name)\n";
    env.reload_program(sid, edited, None).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["Kind.road true road"]);
}
