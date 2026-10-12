//! One test per rule of the classifier, and the negatives that are easy to
//! get wrong. Every program is compiled from source: the shapes that matter
//! (a hoisted function's cell, a capture phantom, a phi, an overload set's
//! patched self-capture) are the compiler's, and a hand-built approximation
//! of them would pass without proving anything.

use super::{DeclClass, DeclKind, ReaderKind, StaticDecl, StaticReport, TermClass};
use crate::env::Env;
use crate::native_fn::{NativeEffects, PetalCxt};
use crate::program::{Program, ProgramId, TermOp};

fn compile(env: &Env, src: &str) -> Program {
    let program = env
        .compile_program(ProgramId(0), src)
        .unwrap_or_else(|e| panic!("does not compile: {e}\n{src}"));
    // A check-only compile turns some mistakes into error terms rather than
    // failing; a test program with one is a broken test.
    assert!(
        !program.terms.iter().any(|t| matches!(t.op, TermOp::Error(_))),
        "the test program has a compile error:\n{src}"
    );
    program
}

/// A check-only `Env`, as `petal show-static` and `petal check` build one: a
/// `native fn` nothing registers still compiles.
fn check_env() -> Env {
    let mut env = Env::new();
    env.set_check_only(true);
    env
}

fn report(src: &str) -> StaticReport {
    let env = check_env();
    env.static_report(&compile(&env, src))
}

fn decl<'a>(r: &'a StaticReport, name: &str) -> &'a StaticDecl {
    r.get(name).unwrap_or_else(|| {
        let names: Vec<&str> = r.entry().map(|d| d.qualified.as_str()).collect();
        panic!("no declaration {name}; the entry file declares {names:?}")
    })
}

fn class(r: &StaticReport, name: &str) -> DeclClass {
    decl(r, name).class
}

/// The reason chain of a declaration, one link per line.
fn why(r: &StaticReport, name: &str) -> String {
    decl(r, name)
        .reason
        .iter()
        .map(|s| s.render())
        .collect::<Vec<_>>()
        .join("\n")
}

#[track_caller]
fn assert_static(r: &StaticReport, names: &[&str]) {
    for n in names {
        assert_eq!(class(r, n), DeclClass::Static, "{n}: {}", why(r, n));
    }
}

#[track_caller]
fn assert_dynamic(r: &StaticReport, name: &str, because: &str) {
    assert_eq!(class(r, name), DeclClass::Dynamic, "{name} should be dynamic");
    let chain = why(r, name);
    assert!(chain.contains(because), "{name}: expected {because:?} in\n{chain}");
}

#[track_caller]
fn assert_pure(r: &StaticReport, name: &str, pure: bool) {
    let d = decl(r, name);
    assert_eq!(d.class, DeclClass::Function, "{name}");
    assert_eq!(d.static_pure, Some(pure), "{name}: {}", why(r, name));
}

fn deps(r: &StaticReport, name: &str) -> Vec<String> {
    decl(r, name).depends_on.iter().cloned().collect()
}

fn add_ints(cxt: &mut PetalCxt) -> crate::native_fn::NativeResult {
    let a = cxt.get_int(0)?;
    let b = cxt.get_int(1)?;
    cxt.push_int(a + b);
    Ok(1)
}

// ── What is static ─────────────────────────────────────────────────────────

#[test]
fn literals_tables_and_operators_are_static() {
    let r = report(
        "let N = 3
let TABLE = [{id: 1, tags: [\"a\", \"b\"]}, {id: 2, tags: []}]
let FIRST = TABLE[0].id + N * 2
let LABEL = \"n={N} first={FIRST}\"
let NEG = -N
let BOTH = N > 1 && FIRST < 10
let ROW = {...TABLE[0], extra: true}
",
    );
    assert_static(&r, &["N", "TABLE", "FIRST", "LABEL", "NEG", "BOTH", "ROW"]);
    assert_eq!(decl(&r, "TABLE").kind, DeclKind::Let);
    assert!(decl(&r, "TABLE").reason.is_empty());
}

#[test]
fn a_config_let_is_static() {
    let r = report("config let SCALE = 2\nlet W = SCALE * 3\n");
    assert_static(&r, &["SCALE", "W"]);
    assert_eq!(decl(&r, "SCALE").kind, DeclKind::ConfigLet);
    assert_eq!(deps(&r, "W"), ["SCALE"]);
}

#[test]
fn state_is_dynamic_and_so_is_what_reads_it() {
    let r = report("state counter = 0\nlet next = counter + 1\nlet far = [next]\n");
    assert_dynamic(&r, "counter", "counter is state");
    assert_eq!(decl(&r, "counter").kind, DeclKind::State);
    assert_dynamic(&r, "next", "next reads counter");
    // The chain goes all the way down, one declaration per link.
    assert_dynamic(&r, "far", "far reads next");
    assert_dynamic(&r, "far", "next reads counter");
    assert_dynamic(&r, "far", "counter is state");
}

#[test]
fn a_top_level_var_is_dynamic_and_so_is_what_reads_it() {
    let r = report("var total = 0\nlet doubled = total * 2\n");
    assert_dynamic(&r, "total", "total is a var");
    assert_eq!(decl(&r, "total").kind, DeclKind::Var);
    assert_dynamic(&r, "doubled", "reads the var total");
}

#[test]
fn a_state_var_is_dynamic() {
    let r = report("state var hits = 0\nlet h = hits\n");
    assert_dynamic(&r, "hits", "is state");
    assert_eq!(class(&r, "h"), DeclClass::Dynamic);
    // It keeps a cell, so it could never have been a constant.
    assert_eq!(decl(&r, "hits").initializer_static, Some(false));
}

// ── Natives ────────────────────────────────────────────────────────────────

#[test]
fn a_const_builtin_is_static_and_another_builtin_is_not() {
    let r = report(
        "let N = len([1, 2, 3])
let ROOT = sqrt(16.0)
let BIG = max(N, 10)
let SHOWN = print(N)
",
    );
    assert_static(&r, &["N", "ROOT", "BIG"]);
    assert_dynamic(&r, "SHOWN", "calls print, which is not declared constant");
}

#[test]
fn a_host_native_is_static_only_when_registered_const() {
    let src = "native fn h_const(a, b)
native fn h_pure(a, b)
let A = h_const(1, 2)
let B = h_pure(1, 2)
";
    let mut env = check_env();
    env.register_native("h_const", add_ints, NativeEffects::CONST);
    env.register_native("h_pure", add_ints, NativeEffects::PURE);
    let r = env.static_report(&compile(&env, src));
    assert_static(&r, &["A"]);
    assert_dynamic(&r, "B", "calls h_pure, which is not declared constant");
}

/// `petal check` and the language server compile without a host. They may
/// report less as static than the host does, never more.
#[test]
fn without_the_host_a_native_is_dynamic() {
    let src = "native fn h_const(a, b)
let A = h_const(1, 2)
let B = [A, 3]
let C = [4, 5]
";
    let without = report(src);
    assert_dynamic(&without, "A", "calls h_const, a native that is not registered here");
    assert_dynamic(&without, "B", "B reads A");
    assert_static(&without, &["C"]);

    let mut env = check_env();
    env.register_native("h_const", add_ints, NativeEffects::CONST);
    let with = env.static_report(&compile(&env, src));
    assert_static(&with, &["A", "B", "C"]);

    // Never more: whatever is static without the host is static with it.
    for d in without.entry() {
        if d.class == DeclClass::Static {
            assert_eq!(class(&with, &d.qualified), DeclClass::Static, "{}", d.qualified);
        }
    }
}

#[test]
fn a_builtin_held_in_a_name_is_a_function_and_calls_through_it_resolve() {
    let r = report("let f = sqrt\nlet R = f(4.0)\nlet p = print\nlet Q = p(1)\n");
    assert_pure(&r, "f", true);
    assert_static(&r, &["R"]);
    assert_pure(&r, "p", false);
    assert_dynamic(&r, "Q", "calls print");
}

// ── User functions ─────────────────────────────────────────────────────────

#[test]
fn a_table_built_by_a_constructor_function_is_static() {
    let r = report(
        "config let SCALE = 2
let W = SCALE * 3
fn hd(a, b)
  {a: a, b: b, w: W}
end
let ROWS = [hd(1, 2), hd(3, 4)]
",
    );
    assert_pure(&r, "hd", true);
    assert_static(&r, &["ROWS"]);
    assert_eq!(decl(&r, "hd").kind, DeclKind::Function);
}

/// `codes` is declared below its use, so the call goes through the
/// write-once cell a hoisted forward reference uses. Without resolving that
/// cell the table would be dynamic (`moses/config.ptl:129`).
#[test]
fn a_forward_referenced_function_resolves_through_its_cell() {
    let src = "let TILES = [{code: 1}, {code: 2}]
let CODES = codes(TILES)
fn codes(ts)
  var out = []
  for t in ts do
    set out = append(out, t.code)
  end
  out
end
";
    let env = check_env();
    let program = compile(&env, src);
    // The test is about the cell: make sure the compiler still uses one.
    assert!(
        program.terms.iter().any(|t| matches!(t.op, TermOp::CellRead)
            && t.block_id == program.root_block),
        "the forward reference no longer goes through a cell"
    );
    let r = env.static_report(&program);
    assert_pure(&r, "codes", true);
    assert_static(&r, &["CODES"]);
    assert_eq!(deps(&r, "CODES"), ["TILES", "codes"]);
}

/// A function that is not hoisted is bound where it is written. A top-level
/// read above that point finds the cell still empty.
#[test]
fn a_function_used_before_its_declaration_has_run_is_dynamic() {
    let r = report(
        "let early = late(1)
let n = len([1, 2])
fn late(x) x + n end
let after = late(2)
",
    );
    assert_dynamic(&r, "early", "uses a function before its declaration has run");
    assert_pure(&r, "late", true);
    assert_static(&r, &["after"]);
}

#[test]
fn a_function_that_reads_state_is_not_static_pure() {
    let r = report(
        "state counter = 0
fn bump(x) x + counter end
let V = bump(1)
",
    );
    assert_pure(&r, "bump", false);
    assert_dynamic(&r, "V", "V calls bump");
    assert_dynamic(&r, "V", "bump reads counter");
    assert_dynamic(&r, "V", "counter is state");
}

/// A debugging `print` in a constructor makes every table built with it
/// dynamic, and the chain says which line.
#[test]
fn a_function_that_prints_is_not_static_pure() {
    let r = report(
        "fn row(a)
  print(\"building\", a)
  {a: a}
end
let ROWS = [row(1), row(2)]
",
    );
    assert_pure(&r, "row", false);
    assert_dynamic(&r, "ROWS", "ROWS calls row");
    assert_dynamic(&r, "ROWS", "row calls print, which is not declared constant at line 2");
}

#[test]
fn impurity_is_transitive_and_the_chain_names_every_call() {
    let r = report(
        "native fn text_width(s)
fn measure(s) text_width(s) + 4 end
fn label_w(items) map(items, fn(s) measure(s) end) end
fn build(items) {widths: label_w(items)} end
let LAB_W = build([\"a\", \"bb\"])
",
    );
    assert_pure(&r, "measure", false);
    assert_pure(&r, "label_w", false);
    assert_pure(&r, "build", false);
    let chain = why(&r, "LAB_W");
    let links: Vec<&str> = chain.lines().collect();
    assert_eq!(links.len(), 5, "{chain}");
    assert!(links[0].starts_with("LAB_W calls build at"), "{chain}");
    assert!(links[1].starts_with("build calls label_w at"), "{chain}");
    assert!(links[2].contains("to a higher-order builtin"), "{chain}");
    assert!(links[3].contains("calls measure at"), "{chain}");
    assert!(
        links[4].starts_with("measure calls text_width, a native that is not registered here"),
        "{chain}"
    );
}

#[test]
fn recursion_is_solved_optimistically() {
    let r = report(
        "fn fact(n)
  if n <= 1 then return 1 end
  n * fact(n - 1)
end
fn is_even(n) if n == 0 then true else is_odd(n - 1) end end
fn is_odd(n) if n == 0 then false else is_even(n - 1) end end
let F = fact(5)
let E = is_even(10)
",
    );
    assert_pure(&r, "fact", true);
    assert_pure(&r, "is_even", true);
    assert_pure(&r, "is_odd", true);
    assert_static(&r, &["F", "E"]);
}

/// Optimism must not survive a real dynamic read: one impure member makes
/// the whole cycle impure.
#[test]
fn a_recursive_cycle_with_a_dynamic_read_is_not_static_pure() {
    let r = report(
        "state bias = 0
fn ping(n) if n == 0 then bias else pong(n - 1) end end
fn pong(n) if n == 0 then 0 else ping(n - 1) end end
let P = pong(4)
",
    );
    assert_pure(&r, "ping", false);
    assert_pure(&r, "pong", false);
    assert_dynamic(&r, "P", "ping reads bias");
}

#[test]
fn a_function_may_use_its_own_vars_and_loops() {
    let r = report(
        "fn build(n)
  var out = []
  var i = 0
  while i < n do
    set out = append(out, i * i)
    set i = i + 1
  end
  out
end
fn build2(n)
  let out = []
  for i in range(0, n) do
    out = append(out, {i: i})
  end
  out
end
let A = build(4)
let B = build2(4)
",
    );
    assert_pure(&r, "build", true);
    assert_pure(&r, "build2", true);
    assert_static(&r, &["A", "B"]);
}

#[test]
fn a_function_that_touches_a_top_level_var_is_not_static_pure() {
    let r = report(
        "var cache = 0
fn reads() get cache + 1 end
fn writes(v)
  set cache = v
  v
end
let R = reads()
let W = writes(3)
",
    );
    assert_pure(&r, "reads", false);
    assert_pure(&r, "writes", false);
    assert_dynamic(&r, "R", "reads the var cache");
    assert_dynamic(&r, "W", "writes the var cache");
}

/// The closure writes a `var` that is not its own, so it is not static-pure,
/// and calling it makes the function that owns the `var` dynamic too.
#[test]
fn a_closure_that_writes_its_makers_var_is_not_static_pure() {
    let r = report(
        "fn counter()
  var c = 0
  let bump = fn() set c = get c + 1 end
  bump()
  bump()
  c
end
let N = counter()
",
    );
    assert_pure(&r, "counter", false);
    assert_eq!(class(&r, "N"), DeclClass::Dynamic);
}

#[test]
fn a_function_is_as_static_as_what_it_captures() {
    let r = report(
        "state theme = 0
let BASE = 10
let live = theme + 1
fn from_const(x) x + BASE end
fn from_live(x) x + live end
let A = from_const(1)
let B = from_live(1)
",
    );
    assert_pure(&r, "from_const", true);
    assert_pure(&r, "from_live", false);
    assert_static(&r, &["A"]);
    assert_dynamic(&r, "B", "from_live reads live");
}

#[test]
fn default_parameters_and_named_arguments_are_static() {
    let r = report(
        "fn pad(s, n = 2) [s, n] end
let A = pad(\"x\")
let B = pad(\"x\", n: 5)
",
    );
    assert_pure(&r, "pad", true);
    assert_static(&r, &["A", "B"]);
}

/// One arity of an overloaded function calling another captures its own
/// name before the overload set exists; the VM patches the capture when the
/// set is made.
#[test]
fn an_overload_may_call_another_arity_of_itself() {
    let r = report(
        "fn scaled(r)
  scaled(r, 2)
end
fn scaled(r, k)
  r * k
end
let X = scaled(3)
",
    );
    assert_pure(&r, "scaled", true);
    assert_static(&r, &["X"]);
    // One declaration, not three.
    assert_eq!(r.entry().filter(|d| d.name == "scaled").count(), 1);
}

/// Dispatch among arities is by argument count at run time; the classifier
/// does not model it, so one impure arity makes every call dynamic.
#[test]
fn an_overload_set_is_static_pure_only_if_every_arity_is() {
    let r = report(
        "fn shown(a) a end
fn shown(a, b)
  print(a, b)
  a
end
let X = shown(1)
",
    );
    assert_pure(&r, "shown", false);
    assert_dynamic(&r, "X", "calls print");
}

// ── Function values ────────────────────────────────────────────────────────

#[test]
fn a_function_returned_by_a_static_call_is_still_a_known_function() {
    let r = report(
        "state live = 0
fn adder(n) fn(x) x + n end end
let add2 = adder(2)
let FIVE = add2(3)
let add_live = adder(live)
let later = add_live(3)
",
    );
    assert_pure(&r, "adder", true);
    assert_eq!(class(&r, "add2"), DeclClass::Function);
    assert_static(&r, &["FIVE"]);
    // The same closure made from a dynamic argument is dynamic: the capture
    // is only data when the call that made it was static.
    assert_dynamic(&r, "add_live", "add_live reads live");
    assert_eq!(class(&r, "later"), DeclClass::Dynamic);
}

/// A kept value may not contain a closure, so a table of functions is not
/// static data even though every function in it is known.
#[test]
fn a_value_that_holds_a_function_is_dynamic() {
    let r = report(
        "fn double(x) x * 2 end
let HANDLERS = {double: double}
let LIST = [double]
let V = HANDLERS.double(2)
",
    );
    assert_dynamic(&r, "HANDLERS", "holds a function");
    assert_dynamic(&r, "LIST", "holds a function");
    assert_eq!(class(&r, "V"), DeclClass::Dynamic);
}

#[test]
fn a_function_picked_by_a_static_branch_is_a_function() {
    let r = report(
        "let WIDE = true
fn a(x) x + 1 end
fn b(x) x + 2 end
let pick = if WIDE then a else b end
let V = pick(1)
let mixed = if WIDE then a else 3 end
",
    );
    assert_pure(&r, "pick", true);
    assert_static(&r, &["V"]);
    assert_dynamic(&r, "mixed", "is a function on one path and data on another");
}

/// A parameter is assumed to be data. A function that calls one is not
/// static-pure, and passing a function to a user function is dynamic.
#[test]
fn a_function_argument_is_only_static_for_the_higher_order_builtins() {
    let r = report(
        "fn apply(f, x) f(x) end
fn keep(f, x) x end
fn double(x) x * 2 end
let A = apply(double, 2)
let B = keep(double, 2)
",
    );
    assert_pure(&r, "apply", false);
    assert_dynamic(&r, "A", "passes a function as an argument");
    // `keep` never calls it, but nothing says so to the caller.
    assert_dynamic(&r, "B", "passes a function as an argument");
    let chain = why(&r, "apply");
    assert!(chain.contains("calls f, a function it was passed"), "{chain}");
}

#[test]
fn a_higher_order_builtin_is_as_static_as_its_callback() {
    let r = report(
        "state live = 0
let XS = [3, 1, 2]
fn double(x) x * 2 end
let A = map(XS, fn(x) x * 2 end)
let B = map(XS, double)
let C = filter(XS, fn(x) x > 1 end)
let D = reduce(XS, fn(acc, x) acc + x end, 0)
let E = sort_by(XS, fn(x) -x end)
let F = sort(XS)
let G = map(XS, sqrt)
let H = XS.map(fn(x) x + 1 end)
let I = map(XS, fn(x) x + live end)
let J = map(XS, print)
let K = map(XS, fn(x) fn(y) x + y end end)
let L = forEach(XS, fn(x) x end)
",
    );
    assert_static(&r, &["A", "B", "C", "D", "E", "F", "G", "H"]);
    assert_dynamic(&r, "I", "to a higher-order builtin");
    assert_dynamic(&r, "I", "reads live");
    assert_dynamic(&r, "J", "calls print");
    assert_dynamic(&r, "K", "passes a callback that returns a function");
    assert_dynamic(&r, "L", "calls forEach, which is not declared constant");
}

// ── Methods ────────────────────────────────────────────────────────────────

const SHAPES: &str = "class Circle
  r,
end
class Square
  s,
end
fn Circle.area(c) c.r * c.r * 3 end
";

#[test]
fn a_method_call_is_static_when_every_method_of_that_name_is() {
    let src = format!(
        "{SHAPES}fn Square.area(q) q.s * q.s end
let SHAPE_LIST = [Circle(1), Square(2)]
let AREAS = map(SHAPE_LIST, fn(s) s.area() end)
let FIRST = SHAPE_LIST[0].area()
"
    );
    let r = report(&src);
    assert_eq!(decl(&r, "Circle").kind, DeclKind::Class);
    assert_pure(&r, "Circle", true);
    assert_pure(&r, "Circle.area", true);
    assert_static(&r, &["SHAPE_LIST", "AREAS", "FIRST"]);
    // Every method of the name is a dependency, not only the one the
    // receiver's class would pick: an added method must dirty the value.
    let d = deps(&r, "FIRST");
    assert!(d.contains(&"Circle.area".to_string()), "{d:?}");
    assert!(d.contains(&"Square.area".to_string()), "{d:?}");
}

/// Dispatch is by name at run time. The receiver here is plainly a Circle,
/// and it does not matter: one dynamic method of the name is enough.
#[test]
fn one_dynamic_method_of_a_name_makes_every_call_of_it_dynamic() {
    let src = format!(
        "{SHAPES}state zoom = 1
fn Square.area(q) q.s * q.s * zoom end
let SHAPE_LIST = [Circle(1)]
let FIRST = SHAPE_LIST[0].area()
"
    );
    let r = report(&src);
    assert_pure(&r, "Square.area", false);
    assert_dynamic(&r, "FIRST", "calls method .area, which may be Square.area");
    assert_dynamic(&r, "FIRST", "reads zoom");
}

#[test]
fn a_method_call_may_fall_back_to_a_constant_builtin() {
    let r = report(
        "let WORD = \"petal\"
let N = WORD.len()
let UP = [1, 2].append(3)
let BOX = Rect(0, 0, 10, 20)
let CX = BOX.center_x()
let SHOWN = WORD.print()
let LOST = WORD.no_such_method()
",
    );
    assert_static(&r, &["N", "UP", "BOX", "CX"]);
    assert_dynamic(&r, "SHOWN", "calls print");
    assert_dynamic(&r, "LOST", "calls method .no_such_method, which no method");
}

/// A builtin of the method's name is one of the things the call can reach,
/// so it counts even when a user method of that name exists and is pure.
#[test]
fn a_user_method_does_not_hide_a_dynamic_builtin_of_the_same_name() {
    let src = format!(
        "{SHAPES}fn Circle.print(c) c.r end
let ONE = [Circle(1)]
let V = ONE[0].print()
"
    );
    let r = report(&src);
    assert_pure(&r, "Circle.print", true);
    assert_dynamic(&r, "V", "calls print, which is not declared constant");
}

// ── Regions ────────────────────────────────────────────────────────────────

#[test]
fn a_branch_or_match_is_static_only_as_a_whole() {
    let r = report(
        "state live = 0
let N = 3
let A = if N > 2 then \"big\" else \"small\" end
let B = match N
  when 1 -> \"one\"
  when n -> \"other {n}\"
end
let C = if live > 2 then \"big\" else \"small\" end
let D = if N > 2 then live else 0 end
let E = match N
  when 1 -> 1
  when n if n > live -> 2
  when n -> 3
end
let F = N ?? 4
let G = N > 1 || live > 1
let H = N > 1 && N < 5
",
    );
    assert_static(&r, &["A", "B", "F", "H"]);
    // A dynamic condition…
    assert_dynamic(&r, "C", "C reads live");
    // …a dynamic term in one arm, which the condition may never pick…
    assert_dynamic(&r, "D", "D reads live");
    // …a dynamic guard…
    assert_dynamic(&r, "E", "E reads live");
    // …and the right side of a short-circuit operator.
    assert_dynamic(&r, "G", "G reads live");
}

/// The defect the design review found: a value computed inside a loop from
/// the loop variable must never be treated as one constant.
#[test]
fn a_loop_is_static_only_as_a_whole() {
    let r = report(
        "state live = 0
let SQUARES = for i in range(0, 4) do [i, i * i] end
let SHIFTED = for i in range(0, 4) do [i, i + live] end
let EACH = for x in [1, 2, 3] do x * 2 end
for i in range(0, 3) do
  let sq = [i, i * i]
  print(sq)
end
",
    );
    assert_static(&r, &["SQUARES", "EACH"]);
    assert_dynamic(&r, "SHIFTED", "SHIFTED reads live");
    // A binding inside a loop body is not a top-level declaration at all:
    // nothing offers to keep one value of it.
    assert!(r.get("sq").is_none());
    assert!(r.get("i").is_none());
}

#[test]
fn a_top_level_loop_over_a_var_is_dynamic() {
    let r = report(
        "var n = 0
while n < 3 do
  set n = n + 1
end
let N = n
",
    );
    assert_dynamic(&r, "N", "reads the var n");
}

// ── Mutation ───────────────────────────────────────────────────────────────

/// `xs[1] = 5` is lowered to an in-place write on the literal's own storage.
/// Keeping the literal would let every run write into the kept value.
#[test]
fn a_name_that_is_assigned_again_is_dynamic() {
    let r = report(
        "let by_sec = [0, 0, 0]
by_sec[1] = 5
let xs = []
xs = append(xs, 1)
let rec = {a: 1}
rec.a = 2
let total = 0
if len(xs) > 0 then
  total = 5
end
let acc = []
for i in range(0, 3) do
  acc = append(acc, i)
end
let reader = [by_sec, xs, rec, total, acc]
",
    );
    for name in ["by_sec", "xs", "rec", "total", "acc"] {
        assert_dynamic(&r, name, "is assigned again");
    }
    assert_eq!(decl(&r, "by_sec").reason[0].line, Some(2));
    assert_dynamic(&r, "reader", "reader reads");
}

/// The value is still static, and so is what is built from it — but the
/// update may land on the declaration's own storage, so it is not kept.
#[test]
fn a_value_that_reaches_an_in_place_update_is_static_but_marked() {
    let r = report(
        "let BASE = [1, 2, 3]
let MORE = append(BASE, 4)
let SRC = [1, 2, 3]
let alias = SRC
alias[0] = 9
let CLEAN = [1, 2, 3]
let N = len(CLEAN)
let FIRST = CLEAN[0]
",
    );
    assert_static(&r, &["BASE", "MORE", "SRC", "CLEAN", "N", "FIRST"]);
    let base = decl(&r, "BASE").mutated_by.as_ref().expect("BASE reaches append");
    assert!(base.text.contains("append"), "{base:?}");
    assert_eq!(base.line, Some(2));
    assert!(decl(&r, "MORE").mutated_by.is_none());
    // Through another name.
    let src = decl(&r, "SRC").mutated_by.as_ref().expect("SRC is written through alias");
    assert!(src.text.contains("an indexed write"), "{src:?}");
    assert_dynamic(&r, "alias", "is assigned again");
    // Reading is not mutating.
    assert!(decl(&r, "CLEAN").mutated_by.is_none());
    assert_eq!(r.summary(false).static_mutated, 2);
}

// ── Enums ──────────────────────────────────────────────────────────────────

#[test]
fn enum_variants_and_constant_data_are_static() {
    let r = report(
        "enum Tile
  grass(code: 1, label: \"g\"),
  water(code: 2, label: \"w\"),
end
enum Op
  move(dx, dy),
  rest,
end
let ALL = Tile.all
let ONE = Tile.from(\"grass\")
let CODE = Tile.water.code
let CODES = map(Tile.all, fn(t) t.code end)
let MOVE = Op.move(1, 2)
let DX = MOVE.dx
let NAME = Tile.grass.name
",
    );
    assert_eq!(decl(&r, "Tile").kind, DeclKind::Enum);
    assert_static(&r, &["Tile", "Op", "ALL", "ONE", "CODE", "CODES", "MOVE", "DX", "NAME"]);
    assert_eq!(deps(&r, "CODE"), ["Tile"]);
    assert_eq!(deps(&r, "MOVE"), ["Op"]);
    assert_eq!(deps(&r, "ALL"), ["Tile"]);
    // A payload variant, uncalled, is its constructor.
    let r = report("enum Op\n  move(dx, dy),\nend\nlet make = Op.move\nlet M = make(1, 2)\n");
    assert_eq!(class(&r, "make"), DeclClass::Function);
    assert_static(&r, &["M"]);
}

/// A field read cannot tell a record from a variant, so a read of `.w` is
/// static only if every enum's `w` data is — the same rule as methods.
#[test]
fn dynamic_enum_data_makes_every_read_of_that_key_dynamic() {
    let r = report(
        "native fn text_width(s)
enum Col
  name(w: text_width(\"name\"), pad: 2),
  size(w: text_width(\"size\"), pad: 4),
end
let W = Col.name.w
let REC = {w: 1, pad: 3}
let RW = REC.w
let OTHER = {h: 1}.h
",
    );
    assert_dynamic(&r, "Col", "calls text_width");
    assert_dynamic(&r, "W", "reads .w, which may be enum constant data that is not static");
    assert_dynamic(&r, "W", "calls text_width");
    // A plain record's `.w` is caught by the same rule…
    assert_eq!(class(&r, "RW"), DeclClass::Dynamic);
    // …and so is the other key of the same record, which is published with it…
    assert_eq!(class(&r, "Col"), DeclClass::Dynamic);
    // …but a key no enum declares is untouched.
    assert_static(&r, &["OTHER", "REC"]);
}

// ── Classes, elements ──────────────────────────────────────────────────────

#[test]
fn class_instances_and_elements_are_static_data() {
    let r = report(
        "class Point
  x,
  y,
end
fn Point.len2(p) p.x * p.x + p.y * p.y end
let P = Point(1, 2)
let L = P.len2()
let EL = <box w={P.x}>hi</box>
",
    );
    assert_eq!(decl(&r, "Point").kind, DeclKind::Class);
    assert_static(&r, &["P", "L", "EL"]);
    assert_eq!(deps(&r, "L"), ["P", "Point", "Point.len2"]);
}

// ── Dependency sets ────────────────────────────────────────────────────────

#[test]
fn dependency_sets_are_resolved_and_closed_transitively() {
    let r = report(
        "let A = 1
fn f(x) x + A end
fn g(x) f(x) * 2 end
let B = g(2)
let C = B + 1
let ALONE = 5
state seed = C
",
    );
    assert_eq!(deps(&r, "A"), Vec::<String>::new());
    assert_eq!(deps(&r, "f"), ["A"]);
    assert_eq!(deps(&r, "g"), ["A", "f"]);
    assert_eq!(deps(&r, "B"), ["A", "f", "g"]);
    assert_eq!(deps(&r, "C"), ["A", "B", "f", "g"]);
    let direct: Vec<&String> = decl(&r, "C").direct_depends_on.iter().collect();
    assert_eq!(direct, ["B"]);
    assert_eq!(deps(&r, "ALONE"), Vec::<String>::new());
    // A `state` cell's initializer has a dependency set too: that is what a
    // reload reports as a stale seed when something in it changed.
    assert_eq!(deps(&r, "seed"), ["A", "B", "C", "f", "g"]);
    assert_eq!(decl(&r, "seed").initializer_static, Some(true));
}

#[test]
fn a_state_initializer_that_is_dynamic_says_so() {
    let r = report("state a = 0\nstate b = a + 1\nstate c = [1, 2]\n");
    assert_eq!(decl(&r, "a").initializer_static, Some(true));
    assert_eq!(decl(&r, "b").initializer_static, Some(false));
    assert_eq!(decl(&r, "c").initializer_static, Some(true));
    assert_eq!(deps(&r, "b"), ["a"]);
}

#[test]
fn dependency_names_are_module_qualified() {
    let mut env = check_env();
    env.register_module(
        "lib",
        "pub let BASE = [1, 2, 3]
let hidden = 4
pub fn twice(x) x * 2 + hidden end
pub fn later(x) helper(x) end
fn helper(x) x + 1 end
",
    );
    let src = "import lib
let T = lib.twice(lib.BASE[0])
let U = lib.later(1)
";
    let r = env.static_report(&compile(&env, src));
    assert_static(&r, &["T", "U", "lib::BASE", "lib::hidden"]);
    assert_eq!(deps(&r, "T"), ["lib::BASE", "lib::hidden", "lib::twice"]);
    assert_eq!(deps(&r, "U"), ["lib::helper", "lib::later"]);
    let base = decl(&r, "lib::BASE");
    assert_eq!(base.module.as_deref(), Some("lib"));
    assert_eq!(base.name, "BASE");
    // The entry file's view leaves the module's declarations out.
    assert!(r.entry().all(|d| d.module.is_none()));
    assert_eq!(r.summary(false).declarations, 2);
    assert!(r.summary(true).declarations >= 7);
    // A module's static read by the entry file's static code is still only
    // read by static code.
    assert_eq!(base.read_by_dynamic, Some(false));
    assert_eq!(decl(&r, "lib::hidden").read_by_dynamic, Some(true));
}

// ── Who reads a static declaration ─────────────────────────────────────────

#[test]
fn read_by_dynamic_code_is_an_operand_a_capture_or_a_statement() {
    let r = report(
        "state live = 0
let OPERAND = [1, 2]
let CAPTURED = [3, 4]
let ASSERTED = [5, 6]
let FEEDS = [7, 8]
let DERIVED = len(FEEDS)
let UNREAD = [9]
let IN_DYNAMIC = [10]
let IN_REGION = [11]
print(OPERAND)
fn uses() CAPTURED[0] end
assert(len(ASSERTED) == 2)
let mixed = len(IN_DYNAMIC) + live
if live > 0 then
  print(len(IN_REGION))
end
",
    );
    let readers = |name: &str| -> Vec<ReaderKind> {
        decl(&r, name).readers.iter().map(|x| x.kind).collect()
    };
    assert_eq!(decl(&r, "OPERAND").read_by_dynamic, Some(true));
    assert_eq!(readers("OPERAND"), [ReaderKind::Dynamic]);

    assert_eq!(decl(&r, "CAPTURED").read_by_dynamic, Some(true));
    assert_eq!(readers("CAPTURED"), [ReaderKind::Capture]);
    assert_eq!(decl(&r, "CAPTURED").readers[0].by.as_deref(), Some("uses"));

    // `assert` is constant, and it still has to run every frame.
    assert_eq!(decl(&r, "ASSERTED").read_by_dynamic, Some(true));
    assert_eq!(readers("ASSERTED"), [ReaderKind::Statement]);

    // Only another static declaration reads it: computed during a build,
    // then dropped.
    assert_eq!(decl(&r, "FEEDS").read_by_dynamic, Some(false));
    assert_eq!(readers("FEEDS"), [ReaderKind::Static]);
    assert_eq!(decl(&r, "FEEDS").readers[0].by.as_deref(), Some("DERIVED"));

    assert_eq!(decl(&r, "UNREAD").read_by_dynamic, Some(false));
    assert!(decl(&r, "UNREAD").readers.is_empty());

    // A static sub-expression of a dynamic declaration runs as often as the
    // declaration does.
    assert_eq!(decl(&r, "IN_DYNAMIC").read_by_dynamic, Some(true));
    assert_eq!(decl(&r, "IN_DYNAMIC").readers[0].by.as_deref(), Some("mixed"));
    // So does a static term inside a dynamic region.
    assert_eq!(decl(&r, "IN_REGION").read_by_dynamic, Some(true));

    // The question is not asked of a function or a dynamic declaration.
    assert_eq!(decl(&r, "uses").read_by_dynamic, None);
    assert_eq!(decl(&r, "mixed").read_by_dynamic, None);

    let s = r.summary(false);
    assert_eq!(s.static_data, 8);
    assert_eq!(s.static_read_by_dynamic, 5);
}

// ── The classification never reads a literal's value ───────────────────────

/// A literal-only edit is a patched reload that recompiles nothing, so the
/// classification has to be the same on both sides of one. These two
/// programs differ only in literal values — including ones that decide a
/// branch, divide by zero and index out of range.
#[test]
fn a_literal_only_edit_changes_no_answer() {
    let shape = |flag: &str, n: &str, s: &str, idx: &str| {
        format!(
            "state live = 0
let FLAG = {flag}
let N = {n}
let WORD = {s}
let TABLE = [1, 2, 3]
let PICK = if FLAG then TABLE else live end
let DIV = 10 / N
let AT = TABLE[{idx}]
let LOOPED = for i in range(0, N) do i end
let M = match N
  when {n} -> WORD
  when other -> other
end
fn f(x) if x > {n} then live else x end end
let V = f({n})
"
        )
    };
    let a = report(&shape("true", "2", "\"a\"", "0"));
    let b = report(&shape("false", "0", "\"zzz\"", "99"));
    let view = |r: &StaticReport| -> Vec<String> {
        r.entry()
            .map(|d| {
                format!(
                    "{} {:?} {:?} {:?} {:?} {:?}",
                    d.qualified, d.class, d.static_pure, d.read_by_dynamic, d.depends_on,
                    d.reason.iter().map(|s| (&s.subject, &s.text)).collect::<Vec<_>>()
                )
            })
            .collect()
    };
    assert_eq!(view(&a), view(&b));
    // And the answers are the structural ones: the branch that reads `live`
    // makes PICK dynamic whichever way FLAG points.
    assert_dynamic(&a, "PICK", "PICK reads live");
    assert_static(&a, &["DIV", "AT", "LOOPED", "M"]);
    assert_pure(&a, "f", false);
}

// ── The report ─────────────────────────────────────────────────────────────

#[test]
fn a_statement_is_not_a_declaration() {
    let r = report("let A = 1\nprint(A)\nassert(A == 1)\n");
    let names: Vec<&str> = r.entry().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["A"]);
}

#[test]
fn the_text_report_lists_class_reason_and_readers() {
    let text = report(
        "state live = 0
let TABLE = [1, 2]
let SIZE = len(TABLE)
fn row(i) TABLE[i] end
let now = live + SIZE
",
    )
    .render_text(false);
    let line = |needle: &str| -> &str {
        text.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle:?} in\n{text}"))
    };
    assert!(line("state live").starts_with("dynamic"), "{text}");
    // A `state` is dynamic by being one: no reason line of its own, and a
    // note when its initializer is static (it may really be a constant).
    assert!(line("state live").ends_with("its initializer is static"), "{text}");
    assert_eq!(text.matches("live is state").count(), 1, "{text}");
    assert!(line("let TABLE").starts_with("static"), "{text}");
    assert!(line("let TABLE").contains("read by dynamic code (captured by row)"), "{text}");
    assert!(line("let SIZE").contains("read by dynamic code (now)"), "{text}");
    assert!(line("fn row").contains("static-pure"), "{text}");
    assert!(line("let now").starts_with("dynamic"), "{text}");
    assert!(text.contains("= now reads live at line 5"), "{text}");
    assert!(text.contains("= live is state, which changes between runs at line 1"), "{text}");
    assert!(
        text.contains("5 declarations: 2 static (2 read by dynamic code), 1 function (1 static-pure), 2 dynamic"),
        "{text}"
    );
}

#[test]
fn the_json_report_has_the_documented_fields() {
    let json = report(
        "state live = 0
let TABLE = [1, 2]
fn row(i) TABLE[i] end
let now = live + 1
",
    )
    .to_json(false);
    assert_eq!(json["summary"]["declarations"], 4);
    assert_eq!(json["summary"]["static"], 1);
    let decls = json["declarations"].as_array().unwrap();
    let get = |name: &str| decls.iter().find(|d| d["name"] == name).unwrap();
    let table = get("TABLE");
    assert_eq!(table["class"], "static");
    assert_eq!(table["kind"], "let");
    assert_eq!(table["qualified"], "TABLE");
    assert_eq!(table["line"], 2);
    assert_eq!(table["read_by_dynamic"], true);
    assert_eq!(table["readers"][0]["kind"], "capture");
    assert_eq!(table["readers"][0]["by"], "row");
    let row = get("row");
    assert_eq!(row["class"], "function");
    assert_eq!(row["kind"], "fn");
    assert_eq!(row["static_pure"], true);
    assert_eq!(row["depends_on"][0], "TABLE");
    let now = get("now");
    assert_eq!(now["class"], "dynamic");
    assert_eq!(now["reason"][0]["subject"], "now");
    assert_eq!(now["reason"][0]["text"], "reads live");
    assert_eq!(now["reason"][0]["line"], 4);
    assert_eq!(get("live")["kind"], "state");
    assert_eq!(get("live")["initializer_static"], true);
}

#[test]
fn per_term_classes_are_available_to_a_later_stage() {
    let env = check_env();
    let program = compile(&env, "state live = 0\nlet T = [1, 2]\nfn f(x) x end\nlet d = live + 1\n");
    let analysis = env.static_analysis(&program);
    let term = |name: &str| program.find_term(name).unwrap();
    assert_eq!(analysis.term_class(term("T")), TermClass::Data);
    assert_eq!(analysis.term_class(term("f")), TermClass::Function);
    assert_eq!(analysis.term_class(term("d")), TermClass::Dynamic);
    assert_eq!(analysis.term_class(term("live")), TermClass::Dynamic);
    assert_eq!(analysis.report.declarations.len(), env.static_report(&program).declarations.len());
}

/// The whole example corpus classifies without a panic, and every dynamic
/// declaration comes with a reason.
#[test]
fn every_bundled_example_classifies_with_a_reason_for_each_dynamic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/basic");
    let Ok(dir) = std::fs::read_dir(&root) else {
        return;
    };
    let mut seen = 0;
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ptl") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        let env = check_env();
        let Ok(program) = env.compile_program_at(ProgramId(0), &src, &path) else {
            continue;
        };
        let r = env.static_report(&program);
        for d in &r.declarations {
            let needs_reason = d.class == DeclClass::Dynamic || d.static_pure == Some(false);
            assert_eq!(
                !d.reason.is_empty(),
                needs_reason,
                "{}: {} ({:?})",
                path.display(),
                d.qualified,
                d.class
            );
        }
        seen += 1;
    }
    assert!(seen > 0, "no example under {} compiled", root.display());
}
