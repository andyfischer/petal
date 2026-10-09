//! A `fn` declared inside another function is a plain local binding: it
//! shadows whatever the enclosing scopes bound to that name, and it leaves
//! that outer binding alone.
//!
//! Two pieces of compiler state are collected at *module scope* but were
//! consulted for every declaration, so a nested `fn` sharing a name with a
//! top-level one was mistaken for part of the top-level one:
//!
//! * `overloaded_fns` (`compiler/mod.rs`) counts the arities a name is declared
//!   at, scanning a module's top-level statements only. A nested declaration
//!   found the count, compiled itself as an extra variant under an internal
//!   `name#arity`, and never completed the set — so `compile_fn_decl` returned
//!   `None` and *nothing was bound*. The call fell through to the outer
//!   overload set. With the nested declaration first in source order the count
//!   completed early instead, building a `MakeOverloadSet` over a term living
//!   in the enclosing function's block: `bytecode lowering failed: term t126 in
//!   block b1 not in this function`.
//! * `binding_is_fn_cell` walks scopes outward. Only a *hoisted* declaration
//!   writes a cell, and hoisting happens at module scope only, so a nested
//!   declaration would write the enclosing function's cell — capturing a
//!   root-block term — rather than shadowing it.
//!
//! Both are gated on `fn_name_chain.is_empty()`, which is true exactly at
//! module scope: where the prescan looked, and where cells exist.

use petal::env::Env;

/// Run `src` and return everything it printed, one entry per `print()`.
fn output(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("compile failed: {e}\n--- source ---\n{src}"));
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid)
        .unwrap_or_else(|e| panic!("run failed: {e}\n--- source ---\n{src}"));
    env.take_output()
}

/// The nested declaration wins inside its own function, and the top-level
/// overload set is untouched outside it. Before the fix `outer()` returned 9 —
/// the nested `box` was discarded and the call reached the outer `box(w)`.
#[test]
fn a_nested_fn_shadows_a_top_level_overload_set() {
    let out = output(
        "\
fn box(w) return w * w end
fn box(w, h) return w * h end
fn outer()
  fn box(x) return 10 end
  return box(3)
end
print(outer())
print(box(3))
print(box(3, 4))
",
    );
    assert_eq!(out, ["10", "9", "12"]);
}

/// The same program with the nested declaration ahead of the top-level ones.
/// This is the ordering that used to fail to compile outright, because the
/// overload set completed one variant early over a term from `outer()`'s block.
#[test]
fn a_nested_fn_declared_first_still_compiles() {
    let out = output(
        "\
fn outer()
  fn box(x) return 10 end
  return box(3)
end
fn box(w) return w * w end
fn box(w, h) return w * h end
print(outer())
print(box(3))
print(box(3, 4))
",
    );
    assert_eq!(out, ["10", "9", "12"]);
}

/// Shadowing a non-overloaded top-level function: the nested binding is local,
/// and the outer one still answers after `outer()` has run.
#[test]
fn a_nested_fn_shadows_a_plain_top_level_fn() {
    let out = output(
        "\
fn f(x) return x * 2 end
fn outer()
  fn f(x) return x + 100 end
  return f(1)
end
print(outer())
print(f(1))
",
    );
    assert_eq!(out, ["101", "2"]);
}

/// Two levels deep, each shadowing the last, with every level still reachable
/// from where it was written.
#[test]
fn nested_shadowing_nests() {
    let out = output(
        "\
fn f(x) return x end
fn outer()
  fn f(x) return x + 10 end
  fn inner()
    fn f(x) return x + 100 end
    return f(1)
  end
  return [inner(), f(1)]
end
print(outer())
print(f(1))
",
    );
    assert_eq!(out, ["[101, 11]", "1"]);
}

/// A nested declaration must not consume one of the top-level set's arity
/// slots: the outer set still resolves both of its own arities, and reports
/// its own arities on a mismatch.
#[test]
fn a_nested_fn_does_not_join_the_outer_overload_set() {
    let src = "\
fn box(w) return w * w end
fn box(w, h) return w * h end
fn outer()
  fn box(x) return 10 end
  return box(3)
end
print(outer())
print(box(9, 9, 9))
";
    let mut env = Env::new();
    let pid = env.load_program(src).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    let e = env.run(sid).expect_err("no 3-argument variant");
    assert!(
        e.contains("box() expects 1 or 2 arguments, got 3"),
        "the nested arity leaked into the outer set: {e}"
    );
}

/// A nested function may itself recurse under the shadowed name — the
/// self-reference resolves to the nested one, not the enclosing binding.
#[test]
fn a_nested_fn_recurses_under_its_own_name() {
    let out = output(
        "\
fn fact(n) return 0 end
fn outer()
  fn fact(n)
    if n <= 1 then return 1 end
    return n * fact(n - 1)
  end
  return fact(5)
end
print(outer())
print(fact(5))
",
    );
    assert_eq!(out, ["120", "0"]);
}

/// A `fn` nested inside a *lambda* is equally local — `fn_name_chain` tracks
/// lambdas too, so the gate holds there.
#[test]
fn a_fn_nested_in_a_lambda_is_local() {
    let out = output(
        "\
fn g(x) return x * 2 end
let run = fn ()
  fn g(x) return x + 50 end
  return g(1)
end
print(run())
print(g(1))
",
    );
    assert_eq!(out, ["51", "2"]);
}
