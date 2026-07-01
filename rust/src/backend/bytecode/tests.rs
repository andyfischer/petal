//! Differential tests: every snippet is run under both `Backend::Graph` and
//! `Backend::Bytecode` (optimizations off) and their results must agree. The
//! graph engine is the correctness oracle; "bytecode with opts off" must match
//! it exactly (see docs/bytecode-status.md, M1 step 6).

use crate::backend::Backend;
use crate::env::Env;
use crate::value;

/// Run `code` on `backend`, returning the rendered result value plus the print
/// output buffer. Values are compared by display string because heap ids are
/// not comparable across two independent runs.
fn run(code: &str, backend: Backend) -> Result<(String, Vec<String>), String> {
    let mut env = Env::new();
    env.set_backend(backend);
    let v = env.run_source(code)?;
    let rendered = value::value_to_display_string(&v, env.heap());
    Ok((rendered, env.take_output()))
}

/// Assert the two backends agree: either both error, or both succeed with an
/// equal rendered value and equal print output.
#[track_caller]
fn assert_parity(code: &str) {
    let graph = run(code, Backend::Graph);
    let bytecode = run(code, Backend::Bytecode);
    match (graph, bytecode) {
        (Ok((gv, go)), Ok((bv, bo))) => {
            assert_eq!(gv, bv, "value mismatch for:\n{code}");
            assert_eq!(go, bo, "output mismatch for:\n{code}");
        }
        (Err(_), Err(_)) => {} // both errored — parity holds (messages may differ)
        (g, b) => panic!("ok/err mismatch for:\n{code}\n  graph={g:?}\n  bytecode={b:?}"),
    }
}

#[test]
fn arithmetic() {
    assert_parity("let x = 1 + 2 * 3");
    assert_parity("let x = (10 - 4) / 2");
    assert_parity("let x = 17 % 5");
    assert_parity("let x = -42");
    assert_parity("let x = 3.5 * 2.0 + 1.0");
    assert_parity("let x = 2 + 3.5");
}

#[test]
fn comparisons_and_logic() {
    assert_parity("let x = 3 < 5");
    assert_parity("let x = 5 <= 5");
    assert_parity("let x = 9 > 2");
    assert_parity("let x = 4 >= 7");
    assert_parity("let x = 1 == 1");
    assert_parity("let x = 1 != 2");
    assert_parity("let x = !true");
}

#[test]
fn strings() {
    assert_parity(r#"let x = "foo" ++ "bar""#);
    assert_parity(r#"let x = "n=" ++ 42"#);
    assert_parity(r#"let x = "abc".length"#);
}

#[test]
fn containers_and_access() {
    assert_parity("let x = [1, 2, 3]");
    assert_parity("let p = { a: 1, b: 2 }\nlet y = p.a");
    assert_parity("let p = { a: 1, b: [2, 3] }\nlet y = p.b[1]");
    assert_parity("let xs = [10, 20, 30]\nlet y = xs[0] + xs[-1]");
    assert_parity("let xs = [10, 20, 30]\nlet y = xs.length");
    assert_parity("let p = { a: 1 }\nlet q = { ...p, b: 2 }\nlet y = q.b");
}

#[test]
fn value_semantics_setindex_setfield() {
    assert_parity("let xs = [1, 2, 3]\nlet ys = xs[1] = 99\nlet y = xs[1]");
    assert_parity("let p = { a: 1 }\nlet q = p.a = 5\nlet y = p.a");
}

#[test]
fn error_parity() {
    assert_parity("let x = 1 / 0");
    assert_parity("let xs = [1, 2]\nlet y = xs[5]");
    assert_parity(r#"let x = 1 + "a""#);
}

#[test]
fn print_output() {
    assert_parity(r#"print("hello")"#);
    assert_parity("print(1 + 2)");
    assert_parity(r#"print("sum =", 3 + 4)"#);
}

#[test]
fn function_calls() {
    assert_parity("fn add(a, b)\n  a + b\nend\nlet y = add(3, 4)");
    assert_parity("fn square(n)\n  n * n\nend\nprint(square(5))");
    // Lambda bound to a name, then called.
    assert_parity("let double = fn(x) -> x * 2\nlet y = double(7)");
}

#[test]
fn closures_capture() {
    assert_parity(
        "fn make_adder(n)\n  fn adder(x)\n    x + n\n  end\n  adder\nend\n\
         let add5 = make_adder(5)\nlet y = add5(3)",
    );
}

#[test]
fn overloaded_functions() {
    // Same name, different arities — resolved by argument count.
    assert_parity(
        "fn f(a)\n  a\nend\nfn f(a, b)\n  a + b\nend\nlet y = f(10)\nlet z = f(3, 4)",
    );
}

#[test]
fn higher_order_intrinsics() {
    assert_parity("let ys = map([1, 2, 3], fn(x) -> x * 2)");
    assert_parity("let ys = filter([1, 2, 3, 4], fn(x) -> x > 2)");
    assert_parity("let s = reduce([1, 2, 3, 4], 0, fn(a, b) -> a + b)");
    assert_parity("forEach([1, 2, 3], fn(x) -> print(x))");
}

#[test]
fn method_call_syntax() {
    assert_parity("let ys = [1, 2, 3].map(fn(x) -> x + 1)");
    assert_parity("let s = [1, 2, 3, 4].filter(fn(x) -> x > 2)");
}

#[test]
fn builtin_calls() {
    assert_parity("let n = len([1, 2, 3])");
    assert_parity(r#"let s = str(42)"#);
    assert_parity("let xs = append([1, 2], 3)");
}

#[test]
fn call_arity_error_parity() {
    assert_parity("fn add(a, b)\n  a + b\nend\nlet y = add(1)");
}

// -- M2a: conditionals ------------------------------------------------------

#[test]
fn if_else() {
    assert_parity("let x = 5\nlet y = if x > 0 then 1 else -1 end");
    assert_parity("let x = -5\nlet y = if x > 0 then 1 else -1 end");
    // `if` with no else, untaken → nil result.
    assert_parity("let y = if false then 10 end");
    assert_parity("let y = if true then 10 end");
}

#[test]
fn phi_joins() {
    // Rebind inside the taken branch carries out.
    assert_parity("let x = 1\nif x > 0 then x = 99 end\nlet y = x");
    // Untaken branch leaves the pre-branch value in place.
    assert_parity("let x = 5\nif x > 100 then x = 99 end\nlet y = x");
    // Multiple rebinds in one branch.
    assert_parity("let a = 1\nlet b = 2\nif a < b then\n  a = 10\n  b = 20\nend\nlet y = a + b");
}

#[test]
fn nested_conditionals() {
    assert_parity(
        "fn sign(n)\n  if n > 0 then \"pos\" else if n < 0 then \"neg\" else \"zero\" end end\nend\n\
         let y = sign(-3)",
    );
    assert_parity(
        "fn absval(n)\n  let r = n\n  if n < 0 then r = -n end\n  r\nend\n\
         let y = absval(-7)",
    );
}

#[test]
fn short_circuit() {
    assert_parity("let y = true && false");
    assert_parity("let y = true && 7");
    assert_parity("let y = false && 7");
    assert_parity("let y = false || 42");
    assert_parity("let y = true || 42");
    assert_parity("let a = 3\nlet y = a > 0 && a < 10");
    assert_parity("let a = 3\nlet y = a < 0 || a > 100");
}

// -- M2b: loops -------------------------------------------------------------

#[test]
fn for_loops() {
    assert_parity("let s = 0\nfor i in range(5) do s = s + i end\nlet y = s");
    assert_parity("let s = 0\nfor x in [10, 20, 30] do s = s + x end\nlet y = s");
    assert_parity("let out = []\nfor i in range(4) do out = append(out, i * i) end\nlet y = out");
    assert_parity("let s = 0\nfor i in range(0) do s = s + 1 end\nlet y = s"); // empty range
}

#[test]
fn while_loops() {
    assert_parity("let i = 0\nlet s = 0\nwhile i < 5 do\n  s = s + i\n  i = i + 1\nend\nlet y = s");
    assert_parity(
        "fn count_down(n)\n  let out = []\n  while n > 0 do\n    out = append(out, n)\n    n = n - 1\n  end\n  out\nend\nlet y = count_down(4)",
    );
}

#[test]
fn break_and_continue() {
    // break before the loop-carried update.
    assert_parity("let s = 0\nfor i in range(10) do\n  if i == 5 then break end\n  s = s + i\nend\nlet y = s");
    // break after the update — the phi carry-out still propagates.
    assert_parity("let s = 0\nfor i in range(10) do\n  s = s + i\n  if i == 2 then break end\nend\nlet y = s");
    // continue skips the rest of the body.
    assert_parity("let s = 0\nfor i in range(6) do\n  if i % 2 == 0 then continue end\n  s = s + i\nend\nlet y = s");
    // continue in a while re-evaluates the condition.
    assert_parity("let c = 0\nlet i = 0\nwhile i < 10 do\n  i = i + 1\n  if i % 2 == 0 then continue end\n  c = c + 1\nend\nlet y = c");
}

#[test]
fn nested_loops_and_break() {
    assert_parity(
        "let total = 0\nfor i in range(3) do\n  for j in range(3) do\n    total = total + (i * 3 + j)\n  end\nend\nlet y = total",
    );
    // break exits only the inner loop.
    assert_parity(
        "let hits = []\nfor i in range(3) do\n  for j in range(3) do\n    if j == 1 then break end\n    hits = append(hits, i * 10 + j)\n  end\nend\nlet y = hits",
    );
}

#[test]
fn break_carries_rebind_through_nested_if() {
    // A rebinding inside an `if` that then `break`s must carry out — exercises
    // the enclosing-phi-chain emission on the break path.
    assert_parity(
        "let x = 0\nfor i in range(10) do\n  if i == 3 then\n    x = 99\n    break\n  end\nend\nlet y = x",
    );
}

#[test]
fn iterative_algorithms() {
    assert_parity(
        "fn fib(n)\n  let a = 0\n  let b = 1\n  for i in range(n) do\n    let t = a + b\n    a = b\n    b = t\n  end\n  a\nend\nlet y = fib(10)",
    );
}
