//! Implicit return values, and the one switch that turns them off: `-> nil`.
//!
//! A function with no `return` yields its body's last statement, and that
//! statement is compiled in *value position* — which is what makes a trailing
//! `for` collect a list. A function declared `-> nil` has no implicit return:
//! its tail is an ordinary statement, so a tail expression is dropped and a
//! trailing loop is a side-effect loop. Every other declaration, and none at
//! all, leaves the tail as the value.
//!
//! The claim that matters most is that the loop builds *no list* — not that
//! the list is built and then thrown away — so it is pinned at three levels:
//! the `collect` flag on the IR's loop term, the `LoopCollect` instructions in
//! the bytecode, and the heap's own allocation counter at run time.
//! See docs/implicit-return-values.md.

use petal::backend::bytecode::isa::Inst;
use petal::backend::bytecode::lower::lower_program;
use petal::env::Env;
use petal::program::TermOp;
use petal::stats::{AllocKind, DUP_STATS_ENABLED};

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

/// The type-checker warnings `src` compiles with.
fn warnings(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("load");
    env.get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// The `collect` flag of every `for` loop term `src` compiles to, in term
/// order: true for a mapping loop, false for a side-effect one.
fn loop_collects(src: &str) -> Vec<bool> {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("load");
    env.get_program(pid)
        .expect("program")
        .terms
        .iter()
        .filter(|t| matches!(t.op, TermOp::ForLoop | TermOp::NumericForLoop))
        .map(|t| t.collect)
        .collect()
}

/// How many list-collecting instructions the function named `name` lowers to.
fn collect_insts(src: &str, name: &str) -> usize {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("load");
    let bc = lower_program(env.get_program(pid).expect("program")).expect("lower");
    let f = bc
        .fns
        .iter()
        .find(|f| f.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no function `{name}` in the bytecode"));
    f.code
        .iter()
        .filter(|i| matches!(i, Inst::LoopCollect { .. } | Inst::LoopCollectEnd { .. }))
        .count()
}

/// How many lists running `src` allocates.
fn lists_allocated(src: &str) -> u64 {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("load");
    let sid = env.create_stack(pid).expect("create_stack");
    env.run(sid).expect("run");
    env.alloc_stats().get(AllocKind::List)
}

// ---------------------------------------------------------------- `-> nil`

#[test]
fn a_nil_function_ending_in_a_loop_returns_nil() {
    let src = "fn f(n: num) -> nil\n  for i in range(0, n) do i * 2 end\nend\nprint(f(3))\n";
    assert_eq!(run(src), "nil");
}

#[test]
fn the_same_loop_under_any_other_declaration_is_the_return_value() {
    for ret in ["", " -> list", " -> any"] {
        let src =
            format!("fn f(n: num){ret}\n  for i in range(0, n) do i * 2 end\nend\nprint(f(3))\n");
        assert_eq!(run(&src), "[0, 2, 4]", "declared `{ret}`");
        assert_eq!(loop_collects(&src), [true], "declared `{ret}`");
    }
}

#[test]
fn a_nil_function_builds_no_list_at_any_level() {
    let nil = "fn f(n: num) -> nil\n  for i in range(0, n) do i * 2 end\nend\nf(50)\nf(50)\n";
    let plain = nil.replace(" -> nil", "");

    // IR: the loop term is not a collecting one.
    assert_eq!(loop_collects(nil), [false]);
    assert_eq!(loop_collects(&plain), [true]);

    // Bytecode: nothing gathers elements, nothing finishes a list.
    assert_eq!(collect_insts(nil, "f"), 0);
    assert_eq!(collect_insts(&plain, "f"), 2);

    // Run time: the heap is never asked for a list.
    if DUP_STATS_ENABLED {
        assert_eq!(lists_allocated(nil), 0);
        assert_eq!(lists_allocated(&plain), 2);
    }
}

#[test]
fn any_tail_expression_of_a_nil_function_yields_nil() {
    // A literal, a call, a binding read: none of them is the result.
    assert_eq!(run("fn f(n) -> nil\n  n + 1\nend\nprint(f(1))\n"), "nil");
    assert_eq!(
        run("fn g()\n  7\nend\nfn f() -> nil\n  g()\nend\nprint(f())\n"),
        "nil"
    );
    assert_eq!(run("fn f() -> nil\nend\nprint(f())\n"), "nil");
    // The tail still runs — it is a statement, not dead code.
    assert_eq!(
        run("fn f() -> nil\n  print(\"ran\")\nend\nprint(f())\n"),
        "ran\nnil"
    );
}

#[test]
fn the_tail_mismatch_warning_does_not_fire_for_a_nil_function() {
    assert_eq!(
        warnings("fn f(n: int) -> nil\n  n + 1\nend\nf(1)\n"),
        Vec::<String>::new()
    );
    // The same tail under a type it does not satisfy still warns.
    let w = warnings("fn f(n: int) -> string\n  n + 1\nend\nf(1)\n");
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("declares `string` but returns `int`"), "{w:?}");
}

#[test]
fn an_explicit_return_in_a_nil_function_still_returns_its_value() {
    let src = "fn f(n: int) -> nil\n  if n > 1 then return 5 end\n  n\nend\nprint(f(2), f(1))\n";
    assert_eq!(run(src), "5 nil");
    // …and that is what the checker complains about, not the tail.
    let w = warnings(src);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("`f` declares `nil` but returns `int`"), "{w:?}");

    // A body that *ends* in `return` is the same thing.
    assert_eq!(run("fn f() -> nil\n  return 9\nend\nprint(f())\n"), "9");
}

#[test]
fn branch_and_arm_tails_of_a_nil_function_are_statements_too() {
    let src = "\
fn f(n: num) -> nil
  if n > 0 then
    for i in range(0, n) do i end
  else
    7
  end
end
fn g(n: num) -> nil
  match n
    when 0 -> 7
    when m -> for i in range(0, m) do i end
  end
end
print(f(2), f(0), g(2), g(0))
";
    assert_eq!(run(src), "nil nil nil nil");
    assert_eq!(loop_collects(src), [false, false]);
    assert_eq!(collect_insts(src, "f"), 0);
    assert_eq!(collect_insts(src, "g"), 0);
}

#[test]
fn a_loop_whose_value_is_named_inside_a_nil_function_still_collects() {
    // `-> nil` moves the *tail* out of value position. A loop that is bound,
    // passed or returned is used, wherever it is written.
    let src = "\
fn f(n: num) -> nil
  let xs = for i in range(0, n) do i end
  print(len(xs))
  for i in range(0, n) do i end
end
f(3)
";
    assert_eq!(run(src), "3");
    assert_eq!(loop_collects(src), [true, false]);
}

#[test]
fn a_method_declared_nil_follows_the_same_rule() {
    let src = "\
class Bag
  items: list,
end
fn Bag.each(self) -> nil
  for i in self.items do i * 2 end
end
fn Bag.doubled(self)
  for i in self.items do i * 2 end
end
let b = Bag([1, 2])
print(b.each(), b.doubled())
";
    assert_eq!(run(src), "nil [2, 4]");
    assert_eq!(loop_collects(src), [false, true]);
}

#[test]
fn a_nil_overload_variant_does_not_affect_its_siblings() {
    let src = "\
fn f(a) -> nil
  for i in range(0, a) do i end
end
fn f(a, b)
  for i in range(a, b) do i end
end
print(f(2), f(1, 3))
";
    assert_eq!(run(src), "nil [1, 2]");
}

#[test]
fn a_nested_function_is_governed_by_its_own_declaration() {
    // The outer `-> nil` says nothing about the inner function's tail, and a
    // lambda — which has nowhere to write a return type — always returns its.
    let src = "\
fn outer() -> nil
  fn inner(n)
    for i in range(0, n) do i end
  end
  let lam = fn(n) for i in range(0, n) do i + 10 end end
  print(inner(2), lam(2))
end
print(outer())
";
    assert_eq!(run(src), "[0, 1] [10, 11]\nnil");
    assert_eq!(loop_collects(src), [true, true]);
}

// ------------------------------------------------- the rules `-> nil` is not

#[test]
fn a_bare_statement_loop_never_collects() {
    let src = "for i in range(0, 3) do i end\nfn f(n)\n  for i in range(0, n) do i end\n  n\nend\nprint(f(2))\n";
    assert_eq!(run(src), "2");
    assert_eq!(loop_collects(src), [false, false]);
}

#[test]
fn a_discarded_if_or_match_does_not_put_its_tails_in_value_position() {
    // Mid-body, the `if` and the `match` are statements; so are their tails.
    let src = "\
fn f(n)
  if n > 0 then
    for i in range(0, n) do i end
  end
  match n
    when 0 -> 0
    when m -> for i in range(0, m) do i end
  end
  n
end
print(f(2))
";
    assert_eq!(run(src), "2");
    assert_eq!(loop_collects(src), [false, false]);
}

#[test]
fn a_used_if_or_match_does() {
    let src = "\
fn f(n)
  let a = if n > 0 then
    for i in range(0, n) do i end
  else
    []
  end
  let b = match n
    when 0 -> []
    when m -> for i in range(0, m) do i + 100 end
  end
  [a, b]
end
print(f(2))
";
    assert_eq!(run(src), "[[0, 1], [100, 101]]");
    assert_eq!(loop_collects(src), [true, true]);
}

#[test]
fn a_while_loop_is_never_a_value() {
    let src = "fn f()\n  var i = 0\n  while get i < 3 do\n    set i = get i + 1\n  end\nend\nprint(f())\n";
    assert_eq!(run(src), "nil");
}
