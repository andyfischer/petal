//! Differential tests: every snippet is run under the bytecode VM twice — once
//! with optimizations off (`OptFlags::none`, the clone-and-alloc baseline) and
//! once with everything on (`OptFlags::all`) — and their results must agree.
//! BC-noopt is the correctness oracle; the in-place optimizations must match it
//! exactly (see docs/dev/bytecode-future-ideas.md for the parity invariants).
//! Absolute correctness for fixed programs is anchored by the golden corpus and
//! the `test/<case>/expects` harness.

use crate::backend::OptFlags;
use crate::env::Env;
use crate::value;

/// Run `code` on the bytecode VM with the given `opts`, returning the rendered
/// result value plus the print output buffer. Values are compared by display
/// string because heap ids are not comparable across two independent runs.
fn run(code: &str, opts: OptFlags) -> Result<(String, Vec<String>), String> {
    let mut env = Env::new();
    env.set_opt_flags(opts);
    let v = env.run_source(code)?;
    let rendered = value::value_to_display_string(&v, env.heap());
    Ok((rendered, env.take_output()))
}

/// Run `code` on the bytecode VM with the given `opts` for `runs` successive
/// runs of one persistent stack (reset between runs, keeping state), returning
/// the last rendered value, the concatenated output, and the final state map
/// rendered to a sorted JSON string. Exercises state persistence and the
/// untouched-key sweep.
fn run_stateful(
    code: &str,
    opts: OptFlags,
    runs: usize,
) -> Result<(String, Vec<String>, String), String> {
    let mut env = Env::new();
    env.set_opt_flags(opts);
    let pid = env.load_program(code)?;
    let sid = env.create_stack(pid)?;
    let mut last = value::Value::Nil;
    let mut output = Vec::new();
    for i in 0..runs {
        if i > 0 {
            env.reset_stack(sid)?;
        }
        last = env.run(sid)?;
        output.extend(env.take_output());
    }
    let rendered = value::value_to_display_string(&last, env.heap());
    // get_state_json is a serde Map (unordered); serialize deterministically.
    let state = env.get_state_json(pid, sid);
    let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    Ok((rendered, output, pairs.join(",")))
}

/// Assert the two optimization levels agree across `runs` successive runs,
/// including the final persistent state map.
#[track_caller]
fn assert_stateful_parity(code: &str, runs: usize) {
    let noopt = run_stateful(code, OptFlags::none(), runs);
    let allopt = run_stateful(code, OptFlags::all(), runs);
    match (noopt, allopt) {
        (Ok((gv, go, gs)), Ok((bv, bo, bs))) => {
            assert_eq!(gv, bv, "value mismatch for:\n{code}");
            assert_eq!(go, bo, "output mismatch for:\n{code}");
            assert_eq!(gs, bs, "state mismatch for:\n{code}");
        }
        (Err(_), Err(_)) => {}
        (g, b) => panic!("ok/err mismatch for:\n{code}\n  noopt={g:?}\n  allopt={b:?}"),
    }
}

/// Assert the two optimization levels agree: either both error, or both succeed
/// with an equal rendered value and equal print output.
#[track_caller]
fn assert_parity(code: &str) {
    let noopt = run(code, OptFlags::none());
    let allopt = run(code, OptFlags::all());
    match (noopt, allopt) {
        (Ok((gv, go)), Ok((bv, bo))) => {
            assert_eq!(gv, bv, "value mismatch for:\n{code}");
            assert_eq!(go, bo, "output mismatch for:\n{code}");
        }
        (Err(_), Err(_)) => {} // both errored — parity holds (messages may differ)
        (g, b) => panic!("ok/err mismatch for:\n{code}\n  noopt={g:?}\n  allopt={b:?}"),
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
fn list_scalar_broadcast() {
    let all = OptFlags::all();
    // list op scalar, element-wise
    assert_eq!(run("[1, 2, 3] + 10", all).unwrap().0, "[11, 12, 13]");
    assert_eq!(run("[1, 2, 3] - 1", all).unwrap().0, "[0, 1, 2]");
    assert_eq!(run("[1, 2, 3] * 2", all).unwrap().0, "[2, 4, 6]");
    assert_eq!(run("[2, 4, 6] / 2", all).unwrap().0, "[1, 2, 3]");
    // scalar op list (broadcast from the left)
    assert_eq!(run("2 * [1, 2, 3]", all).unwrap().0, "[2, 4, 6]");
    assert_eq!(run("10 + [1, 2, 3]", all).unwrap().0, "[11, 12, 13]");
    // non-commutative scalar-on-left: operand order must be preserved
    // (`s - el` / `s / el`, not `el - s`), so pin the values explicitly.
    assert_eq!(run("4 - [1, 2, 3]", all).unwrap().0, "[3, 2, 1]");
    assert_eq!(run("12 / [2, 3, 4]", all).unwrap().0, "[6, 4, 3]");
    // float scalar promotes int elements
    assert_eq!(run("[1, 3] * 0.5", all).unwrap().0, "[0.5, 1.5]");
    // both engines agree
    assert_parity("let x = [1, 2, 3] * 3");
    assert_parity("let x = 4 - [1, 2, 3]");
    // list op list is NOT supported yet
    assert!(run("[1, 2] + [3, 4]", all).is_err());
    // a non-numeric element is an error
    assert!(run(r#"["a", "b"] + 1"#, all).is_err());
    // division by zero still guarded per element
    assert!(run("[1, 2] / 0", all).is_err());
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
fn nested_closure_captures_top_level_fn() {
    // A lambda nested inside a fn body references a top-level fn. The value
    // must thread transitively through the intermediate function's frame.
    assert_parity(
        "fn dist(p)\n  p + 1\nend\n\
         fn make()\n  let g = fn(q) -> dist(q)\n  g(41)\nend\n\
         print(make())",
    );
    // Two intermediate boundaries between the reference and the top-level fn.
    assert_parity(
        "fn base(p)\n  p * 10\nend\n\
         fn outer()\n  fn middle()\n    let h = fn(q) -> base(q)\n    h(4)\n  end\n  middle()\nend\n\
         print(outer())",
    );
    // Capturing a top-level fn alongside an enclosing param (mixed sources).
    assert_parity(
        "fn twice(p)\n  p + p\nend\n\
         fn make2(n)\n  let g = fn(q) -> twice(q) + n\n  g(5)\nend\n\
         print(make2(100))",
    );
}

#[test]
fn slice_string_snaps_to_char_boundaries() {
    // '─' (U+2500) is 3 bytes. slice()/len() are byte-indexed; a byte index
    // that lands mid-char must snap to a char boundary rather than panic.
    // Snap the start up and the end down so only whole chars are returned.
    let (_v, out) = run(r#"print(slice("a─b", 0, 2))"#, OptFlags::none()).unwrap();
    assert_eq!(out, vec!["a"], "end mid-char snaps down to a boundary");
    let (_v, out) = run(r#"print(slice("a─b", 2, 5))"#, OptFlags::none()).unwrap();
    assert_eq!(out, vec!["b"], "start mid-char snaps up to a boundary");
    let (_v, out) = run(r#"print(slice("a─b", 0, 4))"#, OptFlags::none()).unwrap();
    assert_eq!(out, vec!["a─"], "index on a boundary is unchanged");
    // Parity + no-panic across both backends, including out-of-range indices.
    assert_parity(r#"print(slice("a─b", 0, 2))"#);
    assert_parity(r#"print(slice("a─b", 2, 5))"#);
    assert_parity(r#"print(slice("a─b", 0, 99))"#);
    assert_parity(r#"print(slice("héllo wörld", 1, 6))"#);
}

#[test]
fn overloaded_functions() {
    // Same name, different arities — resolved by argument count.
    assert_parity("fn f(a)\n  a\nend\nfn f(a, b)\n  a + b\nend\nlet y = f(10)\nlet z = f(3, 4)");
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

/// Chunk E: the `??` coalescing operator must lower to identical results under
/// both optimization levels — both the fallback path and the short-circuit (the
/// RHS effect runs only when the LHS is absent, so the print output must match).
///
/// The `??` coalescing operator lowers to `Move` + the `JumpIfPresent` opcode
/// around the RHS block; this pins that the optimized and unoptimized lowerings
/// agree (value + output), including the short-circuit output cases. The
/// value-level semantics (nil/pending fallback, present-but-falsy, precedence)
/// are asserted in `env::tests::pending_coalesce_operator_chunk_e_tests`.
#[test]
fn coalesce() {
    assert_parity("let y = nil ?? 5");
    assert_parity("let y = 3 ?? 5");
    assert_parity("let y = __pending(\"k\") ?? 7");
    assert_parity("let y = 1 ?? print(\"boom\")"); // present LHS: no output either way
    assert_parity("nil ?? print(\"run\")"); // absent LHS: RHS effect runs, output must match
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
    assert_parity(
        "let s = 0\nfor i in range(10) do\n  if i == 5 then break end\n  s = s + i\nend\nlet y = s",
    );
    // break after the update — the phi carry-out still propagates.
    assert_parity(
        "let s = 0\nfor i in range(10) do\n  s = s + i\n  if i == 2 then break end\nend\nlet y = s",
    );
    // continue skips the rest of the body.
    assert_parity(
        "let s = 0\nfor i in range(6) do\n  if i % 2 == 0 then continue end\n  s = s + i\nend\nlet y = s",
    );
    // continue in a while re-evaluates the condition.
    assert_parity(
        "let c = 0\nlet i = 0\nwhile i < 10 do\n  i = i + 1\n  if i % 2 == 0 then continue end\n  c = c + 1\nend\nlet y = c",
    );
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
fn break_continue_transfer_control_immediately() {
    // `break`/`continue` transfer control at once — statements after them
    // (in the same block or an enclosing arm, up to the loop body) must not
    // execute. The graph engine originally ran that trailing dead code
    // (flag-based exit at frame pop); found by the differential fuzzer
    // (seed 431) and fixed for both engines. The dead rebind/division here
    // would change the value / raise an error if trailing code ran.
    let (_, out) = run(
        "let m = 1\nfor i in range(0, 2) do\n  continue\n  m = 10\nend\nprint(m)",
        OptFlags::none(),
    )
    .unwrap();
    assert_eq!(out, vec!["1"], "dead rebind after continue must not run");
    assert_parity("let m = 1\nfor i in range(0, 2) do\n  continue\n  m = 10\nend\nprint(m)");
    assert_parity("let m = 1\nfor i in range(0, 2) do\n  break\n  let x = 1 / 0\nend\nprint(m)");
    // A not-yet-entered loop after a continue must be skipped, not treated
    // as the continue's target.
    assert_parity(
        "let n = 0\nfor i in range(0, 3) do\n  continue\n  for j in range(0, 5) do\n    n = n + 1\n  end\nend\nprint(n)",
    );
}

#[test]
fn arm_carry_slots_survive_mid_block_exits() {
    // Fuzzer seed 431 (minimized): a sibling loop reads `m` after a nested
    // loop rebinds it; the reading loop contains a statically-present but
    // never-executed rebind behind an always-taken continue. The carry-out
    // for that dead rebind must deliver `m`'s live value (via the arm's
    // seeded carry slot), not the dead rebind's uninitialized register.
    let code = "let m = 1\nfor a in range(0, 3) do\n  for b in range(0, 2) do\n    m = 7\n  end\n  for c in range(0, 4) do\n    print(\"read:\", m)\n    if 1 == 1 then\n      if 1 == 1 then\n        continue\n      end\n      m = 10\n      break\n    end\n  end\nend\nprint(\"end\", m)";
    let (_, out) = run(code, OptFlags::none()).unwrap();
    assert_eq!(out.last().map(String::as_str), Some("end 7"));
    assert!(out.iter().all(|l| l != "read: nil"), "nil leak: {out:?}");
    assert_parity(code);
    // Partial execution: the first of two rebinds in an arm runs, then
    // break — the carry must hold the executed rebind's value (103), not
    // the dead second rebind's register or the pre-iteration value.
    let (_, out) = run(
        "let total = 0\nfor x in range(1, 4) do\n  total = total + x\n  if x == 2 then break end\n  total = total + 100\nend\nprint(total)",
        OptFlags::none(),
    )
    .unwrap();
    assert_eq!(out, vec!["103"]);
}

#[test]
fn iterative_algorithms() {
    assert_parity(
        "fn fib(n)\n  let a = 0\n  let b = 1\n  for i in range(n) do\n    let t = a + b\n    a = b\n    b = t\n  end\n  a\nend\nlet y = fib(10)",
    );
}

// -- M2c: match -------------------------------------------------------------

#[test]
fn match_literals_and_wildcard() {
    assert_parity(
        "fn classify(n)\n  match n\n    when 0 -> \"zero\"\n    when 1 -> \"one\"\n    when _ -> \"other\"\n  end\nend\nlet y = classify(1)",
    );
    // No arm matches -> both engines error.
    assert_parity("let r = match 7\n  when 1 -> \"a\"\n  when 2 -> \"b\"\nend");
}

#[test]
fn match_guards_and_bindings() {
    assert_parity(
        "fn label(t)\n  match t\n    when x if x < 0 -> \"neg\"\n    when x if x < 15 -> \"cold\"\n    when _ -> \"warm\"\n  end\nend\nlet y = label(10)",
    );
    // Guard and body both reference the captured binding.
    assert_parity(
        "fn f(n)\n  match n\n    when x if x > 100 -> \"big: \" ++ str(x)\n    when x -> \"small: \" ++ str(x)\n  end\nend\nlet y = f(5)",
    );
}

#[test]
fn match_variants() {
    assert_parity(
        "enum Shape\n  Circle(r),\n  Rect(w, h),\nend\n\
         fn area(s)\n  match s\n    when Circle(r) -> r * r * 3\n    when Rect(w, h) -> w * h\n  end\nend\n\
         let y = area(Rect(3, 4))",
    );
}

#[test]
fn match_list_patterns() {
    assert_parity(
        "fn describe(xs)\n  match xs\n    when [] -> \"empty\"\n    when [x] -> \"one\"\n    when [a, b] -> \"two\"\n    when [head, ...rest] -> \"many: \" ++ str(head)\n  end\nend\n\
         let y = describe([1, 2, 3, 4])",
    );
}

#[test]
fn match_record_patterns() {
    assert_parity(
        "fn f(p)\n  match p\n    when { x: 0, y: b } -> \"y \" ++ str(b)\n    when { x: a, y: 0 } -> \"x \" ++ str(a)\n    when _ -> \"other\"\n  end\nend\n\
         let y = f({ x: 0, y: 5 })",
    );
}

// -- M3: resumability -------------------------------------------------------

#[test]
fn run_bounded_resumes_identically() {
    use crate::env::RunOutcome;
    // Driving a program one instruction at a time through run_bounded must
    // produce the same result as a single run(), since all resumption state
    // lives on the VM frame stack.
    let code = "let s = 0\nfor i in range(20) do s = s + i end\nlet y = s";
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut steps = 0;
    let value = loop {
        match env.run_bounded(sid, 1).unwrap() {
            RunOutcome::Yielded { .. } => {
                steps += 1;
                assert!(steps < 100_000, "run_bounded did not converge");
            }
            RunOutcome::Done(v) => break v,
        }
    };
    let rendered = value::value_to_display_string(&value, env.heap());

    let single = run(code, OptFlags::all()).unwrap().0;
    assert_eq!(rendered, single, "bounded run diverged from single run");
    assert_eq!(
        rendered,
        run(code, OptFlags::none()).unwrap().0,
        "diverged from clone-and-alloc baseline"
    );
}

// -- M3: state --------------------------------------------------------------

#[test]
fn state_single_run() {
    assert_parity("state n = 0\nn = n + 1\nlet y = n");
    assert_parity("state xs = []\nxs = append(xs, 1)\nlet y = xs");
}

#[test]
fn state_persists_across_runs() {
    // A counter incremented once per run must reach the same value on both.
    assert_stateful_parity("state n = 0\nn = n + 1\nprint(n)", 5);
    // Accumulator list grows each run.
    assert_stateful_parity("state xs = []\nxs = append(xs, 1)\nlet y = xs", 4);
}

#[test]
fn per_iteration_state_persists() {
    // Per-iteration state keyed by the loop index, accumulated across runs.
    assert_stateful_parity(
        "state total = 0\nfor i in range(3) do\n  state seen = 0\n  seen = seen + 1\n  total = total + seen\nend\nprint(total)",
        3,
    );
}

#[test]
fn untouched_state_is_swept() {
    // The number of iterations shrinks each run; per-iteration state for the
    // dropped indices must be swept identically by both backends.
    assert_stateful_parity(
        "state n = 3\nfor i in range(n) do\n  state c = i\n  c = c + 1\nend\nn = n - 1\nprint(n)",
        3,
    );
}

#[test]
fn match_in_loop() {
    // Match inside a loop that rebinds an outer variable — exercises arm phi
    // carry-outs through the loop body.
    assert_parity(
        "let total = 0\nfor i in range(5) do\n  let d = match i % 3\n    when 0 -> 10\n    when 1 -> 1\n    when _ -> 0\n  end\n  total = total + d\nend\nlet y = total",
    );
}

// -- M4: in-place mutation (escape-analysis gated) --------------------------
//
// These assert the differential the plan requires: the bytecode VM with
// in-place mutation off (the clone-and-alloc baseline) and the bytecode VM with
// it on must agree on value + output. They also pin that the optimization
// actually *fires* (via `DupStats` and the analysis directly) so the parity
// checks aren't vacuously green on a disabled optimization.

use crate::backend::bytecode::escape;

/// bytecode(no-opt) == bytecode(in-place): the M4 correctness bar. Identical to
/// [`assert_parity`] (which already diffs the two optimization levels); kept as
/// a named entry point so the M4 tests read intent-first.
#[track_caller]
fn assert_inplace_parity(code: &str) {
    assert_parity(code);
}

/// Compile `code` to a `Program` (the front half of the run pipeline).
fn compile_program(code: &str) -> crate::program::Program {
    use crate::compiler::Compiler;
    use crate::native_fn::NativeFnTable;
    use crate::program::ProgramId;
    use crate::source_map::ENTRY_FILE;
    let (_, stmts) = crate::cst::parse_source(code, ENTRY_FILE).expect("parse");
    let mut natives = NativeFnTable::new();
    crate::builtins::register_builtins(&mut natives);
    Compiler::new().compile(&stmts, code.to_string(), ProgramId(0), &natives)
}

/// Count of in-place-eligible mutation terms the analysis finds in `code`.
fn inplace_count(code: &str) -> usize {
    escape::analyze(&compile_program(code)).len()
}

#[test]
fn inplace_append_accumulator_parity() {
    assert_inplace_parity(
        "let xs = []\nfor i in range(0, 20) do\n  xs = append(xs, i * i)\nend\nprint(len(xs), xs[0], xs[19])",
    );
}

#[test]
fn inplace_branchy_accumulator_parity() {
    // The game_of_life shape: two appends in mutually-exclusive branch arms.
    assert_inplace_parity(
        "let row = []\nfor x in range(0, 12) do\n  if x % 2 == 0 then\n    row = append(row, 1)\n  else\n    row = append(row, 0)\n  end\nend\nprint(len(row), row[0], row[1])",
    );
}

#[test]
fn inplace_nested_accumulator_parity() {
    // Nested accumulators: an inner `row` fed into an outer `grid` per iteration.
    assert_inplace_parity(
        "let grid = []\nfor y in range(0, 6) do\n  let row = []\n  for x in range(0, 6) do\n    row = append(row, x + y)\n  end\n  grid = append(grid, row)\nend\nprint(len(grid), grid[5][5])",
    );
}

#[test]
fn inplace_setindex_accumulator_parity() {
    assert_inplace_parity(
        "let xs = [0, 0, 0]\nfor i in range(0, 3) do\n  xs[i] = i * 10\nend\nprint(xs[0], xs[1], xs[2])",
    );
}

#[test]
fn inplace_setfield_accumulator_parity() {
    assert_inplace_parity(
        "let r = { a: 0, b: 0 }\nfor i in range(0, 5) do\n  r.a = r.a + i\nend\nprint(r.a)",
    );
}

#[test]
fn inplace_does_not_fire_when_container_aliased() {
    // `ys = xs` before the append means a live alias observes the old value —
    // in-place must NOT fire, and the two engines must still agree that the
    // append is value-semantic (ys unchanged).
    let code = "let xs = []\nlet ys = xs\nfor i in range(0, 3) do\n  xs = append(xs, i)\nend\nprint(len(xs), len(ys))";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "aliased accumulator must not be in-place"
    );
}

#[test]
fn inplace_does_not_fire_on_bystander_alias_of_carried_value() {
    // Regression (fuzzer seed 84619): `xs` is an outer-loop accumulator; inside
    // a nested loop `al = xs` aliases the carried value and `al = append(al, …)`
    // mutates the alias, but the append's result is discarded — it is NOT the
    // spine mutation. Marking it in-place corrupted `xs` (its length grew). The
    // spine check rejects it; all three engines must agree `len(xs)` stays 3.
    let code = "let xs = [0, 0, 2]\nfor i in range(0, 2) do\n  xs[1] = 5\n  for j in range(1, 5) do\n    let al = xs\n    al = append(al, 13)\n  end\nend\nprint(len(xs))";
    assert_inplace_parity(code);
}

// -- M4 route B: the container lives in `state` -----------------------------
//
// This block replaces a former blanket exclusion (`state container must not be
// in-place`). That rule was sound but far too coarse: it rejected the entire
// frame-loop simulation shape, which keeps its arrays in `state` precisely so
// they survive between runs — the programs that write the most paid a full
// clone per write. What replaces it is a set of obligations specific to a slot
// that outlives the run (`escape::Analysis::state_web_ok`): sole reader,
// immediate commit, unique writers, and no retention past the region.
//
// Every test here uses `assert_stateful_parity`, which compares several
// successive runs *and* the final state map — a single-run parity check cannot
// see a slot that drifts one run later.

#[test]
fn inplace_fires_on_state_backed_accumulator() {
    // The three shapes a frame-loop simulation is made of. Each must agree with
    // clone-and-alloc across repeated runs, and must actually fire.
    let code = "state xs = []\nfor i in range(0, 5) do\n  xs = append(xs, i)\nend\nprint(len(xs))";
    assert_stateful_parity(code, 3);
    assert!(inplace_count(code) >= 1, "state accumulator should fire");
    let code = "state a = f64_array(5)\nfor i in range(0, 5) do\n  a[i] = a[i] + 1.0\nend\nprint(a[0], a[4])";
    assert_stateful_parity(code, 3);
    assert!(
        inplace_count(code) >= 1,
        "state read-modify-write should fire"
    );
    let code = "state pos = f64_array(4)\nstate vel = f64_array(4)\nfor i in range(0, 4) do\n  pos[i] = pos[i] + vel[i]\nend\nprint(pos[0])";
    assert_stateful_parity(code, 3);
    assert!(inplace_count(code) >= 1, "state integrator should fire");
}

#[test]
fn inplace_does_not_fire_on_two_state_slots_sharing_an_id() {
    // `state b = a` puts one id in two persistent slots, and both outlive the
    // run — the sharpest hazard in the whole feature. `b`'s init block reads
    // `a`'s root, which is a retention of a mid-build value, so the web rejects.
    let code = "state a = f64_array(4)\nstate b = a\nfor i in range(0, 4) do\n  a[i] = a[i] + 1.0\nend\nprint(a[0], b[0])";
    assert_stateful_parity(code, 3);
    assert_eq!(
        inplace_count(code),
        0,
        "two state slots sharing an id must not be in-place"
    );
    // Aliased *after* the loop instead: the second slot then holds the finished
    // id, which the next run would mutate under it.
    let code = "state a = f64_array(4)\nfor i in range(0, 4) do\n  a[i] = a[i] + 1.0\nend\nstate b = a\nprint(a[0], b[0])";
    assert_stateful_parity(code, 3);
    assert_eq!(
        inplace_count(code),
        0,
        "a slot aliased after the loop must not be in-place either"
    );
}

#[test]
fn inplace_does_not_fire_when_the_state_container_is_emitted() {
    // The output buffer is drained by the host *after* the run, so the id
    // outlives the run with a live reader — in the loop or after it.
    let code = "state xs = [0, 0, 0]\nlet s = symbol(\"pts\")\nfor i in range(0, 3) do\n  xs[i] = xs[i] + 1\n  push_output(s, xs)\nend\nprint(xs[0])";
    assert_stateful_parity(code, 3);
    assert_eq!(inplace_count(code), 0, "emitted mid-loop must not fire");
    let code = "state xs = [0, 0, 0]\nlet s = symbol(\"pts\")\nfor i in range(0, 3) do\n  xs[i] = xs[i] + 1\nend\npush_output(s, xs)\nprint(xs[0])";
    assert_stateful_parity(code, 3);
    assert_eq!(
        inplace_count(code),
        0,
        "emitted after the loop must not fire either"
    );
}

#[test]
fn inplace_does_not_fire_when_the_mutation_is_not_committed() {
    // `b` is aliased out of the slot and mutated without ever being written
    // back. In place, the slot's own id would grow; with value semantics `a`
    // keeps its original contents. The immediate-commit rule rejects it.
    let code = "state a = [7]\nlet b = a\nfor i in range(0, 3) do\n  b = append(b, i)\nend\nprint(len(a), len(b))";
    assert_stateful_parity(code, 3);
    assert_eq!(
        inplace_count(code),
        0,
        "an uncommitted mutation of the slot's id must not fire"
    );
    // The same, without ever reading `a` again — nothing in *this* run observes
    // the damage, so only the immediate-commit rule stands between the slot and
    // silent corruption on the next run.
    let code =
        "state a = [7]\nlet b = a\nfor i in range(0, 3) do\n  b = append(b, i)\nend\nprint(len(b))";
    assert_stateful_parity(code, 3);
    assert_eq!(
        inplace_count(code),
        0,
        "an unobserved uncommitted mutation must not fire either"
    );
}

#[test]
fn inplace_does_not_fire_on_a_hoisted_writer_of_per_iteration_state() {
    // One `state` declaration inside a loop becomes one runtime slot per
    // iteration. Seeding them from a container allocated *outside* the loop
    // puts a single id in every slot, so mutating one in place would change all
    // of them (and the hoisted list). The writer must be allocated at the write
    // site; this one is not.
    let code = "let shared = [0, 0]\nfor i in range(0, 2) do\n  state c = shared\n  for j in range(0, 2) do\n    c[j] = i\n  end\nend\nprint(shared[0], shared[1])";
    assert_stateful_parity(code, 3);
    assert_eq!(
        inplace_count(code),
        0,
        "per-iteration slots seeded from a hoisted alloc must not fire"
    );
}

#[test]
fn inplace_does_not_fire_on_per_iteration_state() {
    // `state c` *inside* a loop is one runtime slot per iteration
    // (`RuntimeStateKey` carries the live loop indices), and the write from the
    // inner loop carries a longer index list than the read that created the
    // slot — so it commits to a *different* slot than the one it mutated. Value
    // semantics leaves the read slot untouched across runs; an in-place write
    // would quietly accumulate into it. Found by the multi-run sweep below;
    // a single-run parity check cannot see it.
    let code = "for i in range(0, 2) do\n  state c = [0, 0]\n  for j in range(0, 2) do\n    c[j] = c[j] + i\n  end\n  print(c[0], c[1])\nend";
    assert_stateful_parity(code, 4);
    assert_eq!(
        inplace_count(code),
        0,
        "per-iteration state must not be in-place"
    );
}

#[test]
fn state_shapes_agree_across_repeated_runs() {
    // A broad multi-run differential over `state` shapes: each program runs four
    // times under clone-and-alloc and under all optimizations, and both the
    // output and the final state map must match. This is the sweep that caught
    // the per-iteration slot bug above — the fuzzer runs each program once, so
    // it cannot see a slot that only drifts on the second run.
    let cases = [
        // Accumulators and read-modify-write, the shapes that must fire.
        "state xs = []\nfor i in range(0, 4) do\n  if i % 2 == 0 then xs = append(xs, i) end\nend\nprint(len(xs))",
        "state a = f64_array(4)\nfor i in range(0, 4) do a[i] = a[i] + 1.0 end\nprint(a[0], a[3])",
        "state xs = [0, 0, 0]\nfor i in range(0, 3) do xs[i] = xs[i] + 1 end\nprint(xs[0], xs[2])",
        "state r = { a: 0, b: 0 }\nfor i in range(0, 3) do r.a = r.a + i end\nprint(r.a, r.b)",
        "state pos = f64_array(3)\nstate vel = f64_array(3)\nfor i in range(0, 3) do\n  vel[i] = vel[i] + 1.0\n  pos[i] = pos[i] + vel[i]\nend\nprint(pos[0], vel[0])",
        "state xs = []\nfor i in range(0, 3) do xs = append(xs, len(xs)) end\nprint(len(xs), xs[0])",
        // The slot reassigned wholesale, before and after the loop.
        "state xs = []\nxs = []\nfor i in range(0, 3) do xs = append(xs, i) end\nprint(len(xs))",
        "state a = f64_array(4)\nfor i in range(0, 4) do a[i] = a[i] + 1.0 end\na = f64_array(4)\nprint(a[0])",
        "state xs = []\nif len(xs) > 5 then xs = [] end\nfor i in range(0, 3) do xs = append(xs, i) end\nprint(len(xs))",
        // Aliases of the slot, read before and after the mutations.
        "state xs = [9]\nlet old = xs\nprint(len(old))\nfor i in range(0, 3) do xs = append(xs, i) end\nprint(len(xs))",
        "state xs = [9]\nlet old = xs\nfor i in range(0, 3) do xs = append(xs, i) end\nprint(len(old), len(xs))",
        // Retention forms that must keep copying.
        "state xs = [0, 0, 0]\nlet s = symbol(\"p\")\nfor i in range(0, 3) do\n  xs[i] = xs[i] + 1\n  push_output(s, xs)\nend\nprint(xs[0])",
        "state xs = []\nlet log = []\nfor i in range(0, 3) do\n  xs = append(xs, i)\n  log = append(log, xs)\nend\nprint(len(log[0]), len(xs))",
        "state xs = []\nfor i in range(0, 3) do xs = append(xs, i) end\nlet peek = fn() -> len(xs)\nprint(peek())",
        "fn probe(v)\n  len(v)\nend\nstate xs = []\nfor i in range(0, 3) do\n  xs = append(xs, i)\n  print(probe(xs))\nend",
        "state xs = []\nlet snaps = for i in range(0, 3) do\n  xs = append(xs, i)\n  xs\nend\nprint(len(snaps[0]), len(xs))",
        "state a = f64_array(3)\nstate b = a\nfor i in range(0, 3) do a[i] = a[i] + 1.0 end\nprint(a[0], b[0])",
        "state a = f64_array(3)\nfor i in range(0, 3) do a[i] = a[i] + 1.0 end\nstate b = a\nprint(a[0], b[0])",
        "let shared = [0, 0]\nfor i in range(0, 2) do\n  state c = shared\n  for j in range(0, 2) do c[j] = i end\nend\nprint(shared[0], shared[1])",
        // Control flow and nesting over a persistent slot.
        "state xs = []\nfor i in range(0, 3) do xs = append(xs, i) end\nfor i in range(0, 3) do xs[i] = i * 2 end\nprint(xs[0], xs[2], len(xs))",
        "state xs = []\nfor y in range(0, 2) do\n  for x in range(0, 2) do xs = append(xs, x) end\nend\nprint(len(xs))",
        "state a = [0, 0, 0]\nfor i in range(0, 3) do\n  a[i] = a[i] + 1\n  if i == 1 then break end\nend\nprint(a[0], a[1], a[2])",
        "state xs = [3, 1, 2]\nfor i in range(0, 3) do\n  let s = sort(xs)\n  xs[i] = s[0]\nend\nprint(xs[0], xs[1], xs[2])",
        "state grid = [[0, 0], [0, 0]]\nfor i in range(0, 2) do\n  let row = grid[i]\n  grid[i] = append(row, i)\nend\nprint(len(grid[0]), len(grid[1]))",
        "state total = 0\nstate xs = []\nfor i in range(0, 3) do\n  xs = append(xs, i)\n  total = total + i\nend\nprint(total, len(xs))",
    ];
    for code in cases {
        assert_stateful_parity(code, 4);
    }
}

#[test]
fn forking_a_stack_isolates_its_state_container() {
    // `fork_execution` deep-copies the heap into a fresh context, so a fork that
    // mutates its state slots in place must not touch the source's. Confirmed
    // rather than assumed: both stacks run again after the fork and each must
    // accumulate only its own writes, identically under both engines.
    let code = "state xs = []\nfor i in range(0, 2) do\n  xs = append(xs, i)\nend\nprint(len(xs))";
    let run_forked = |opts: OptFlags| {
        let mut env = Env::new();
        env.set_opt_flags(opts);
        let pid = env.load_program(code).expect("load");
        let src = env.create_stack(pid).expect("stack");
        env.run(src).expect("run");
        let _ = env.take_output();
        env.reset_stack(src).expect("reset");
        let fork = env.fork_execution(src).expect("fork");
        let mut out = Vec::new();
        for _ in 0..2 {
            env.run(fork).expect("run fork");
            out.extend(env.take_output());
            env.reset_stack(fork).expect("reset fork");
        }
        env.run(src).expect("run source");
        out.extend(env.take_output());
        out
    };
    assert_eq!(
        run_forked(OptFlags::none()),
        run_forked(OptFlags::all()),
        "a fork's in-place writes must not reach the source stack's slots"
    );
}

#[test]
fn state_container_accumulates_the_same_across_a_resumed_run() {
    // A run split across `run_bounded` budgets executes the same instruction
    // stream, but the untouched-state sweep only fires on completion — so pin
    // that a chopped-up run leaves the same slot contents as a whole one.
    let code = "state xs = []\nfor i in range(0, 6) do\n  xs = append(xs, i)\nend\nprint(len(xs))";
    let stepped = |opts: OptFlags| {
        let mut env = Env::new();
        env.set_opt_flags(opts);
        let pid = env.load_program(code).expect("load");
        let sid = env.create_stack(pid).expect("stack");
        let mut out = Vec::new();
        for _ in 0..3 {
            while let crate::env::RunOutcome::Yielded { .. } =
                env.run_bounded(sid, 3).expect("bounded run")
            {}
            out.extend(env.take_output());
            env.reset_stack(sid).expect("reset");
        }
        let state = env.get_state_json(pid, sid);
        let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
        pairs.sort();
        (out, pairs.join(","))
    };
    assert_eq!(
        stepped(OptFlags::none()),
        stepped(OptFlags::all()),
        "a resumed run must leave the same state as an unbroken one"
    );
}

#[test]
fn state_container_survives_a_hot_reload() {
    // `transfer_state` reshapes one stack onto a new program, keeping the state
    // map — so a slot mutated in place by v1 is inherited by v2, whose own
    // analysis decides afresh. The existing transfer tests cover scalar state
    // only; this is the container case the in-place path reaches.
    let v1 = "state xs = []\nfor i in range(0, 3) do\n  xs = append(xs, i)\nend\nprint(len(xs))";
    let v2 =
        "state xs = []\nfor i in range(0, 2) do\n  xs = append(xs, 9)\nend\nprint(len(xs), xs[0])";
    let reload = |opts: OptFlags| {
        let mut env = Env::new();
        env.set_opt_flags(opts);
        let pid = env.load_program(v1).expect("load");
        let sid = env.create_stack(pid).expect("stack");
        let mut out = Vec::new();
        env.run(sid).expect("run v1");
        out.extend(env.take_output());
        env.reset_stack(sid).expect("reset");
        env.run(sid).expect("run v1 again");
        out.extend(env.take_output());
        let next = env.compile_program(pid, v2).expect("compile v2");
        env.transfer_state(sid, next).expect("transfer");
        env.run(sid).expect("run v2");
        out.extend(env.take_output());
        out
    };
    assert_eq!(
        reload(OptFlags::none()),
        reload(OptFlags::all()),
        "a hot-reloaded state container must carry the same values either way"
    );
}

#[test]
fn state_slot_survives_a_run_that_errors_partway() {
    // The immediate-commit rule's payoff: the mutation and its `StateWrite` are
    // adjacent, so a run that dies mid-loop leaves the slot holding exactly the
    // writes that completed — the same slot contents clone-and-alloc leaves.
    // `assert_stateful_parity` cannot check this: it compares nothing when both
    // engines error, so the state map is compared explicitly here.
    let code = "state a = [0, 0, 0]\nfor i in range(0, 3) do\n  a[i] = a[i] + 1\n  if i == 1 then\n    let boom = a[99]\n  end\nend\nprint(a[0])";
    let state_after = |opts: OptFlags| {
        let mut env = Env::new();
        env.set_opt_flags(opts);
        let pid = env.load_program(code).expect("load");
        let sid = env.create_stack(pid).expect("stack");
        for _ in 0..3 {
            let _ = env.run(sid); // errors partway; state keeps what committed
            let _ = env.take_output();
            env.reset_stack(sid).expect("reset");
        }
        let state = env.get_state_json(pid, sid);
        let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
        pairs.sort();
        pairs.join(",")
    };
    assert_eq!(
        state_after(OptFlags::none()),
        state_after(OptFlags::all()),
        "a partially-completed run must leave the same state either way"
    );
}

#[test]
fn inplace_analysis_fires_on_accumulators() {
    // Guards against a refactor silently disabling the optimization: the
    // canonical accumulator shapes must be recognized.
    assert!(
        inplace_count(
            "let xs = []\nfor i in range(0, 5) do\n  xs = append(xs, i)\nend\nlet n = len(xs)"
        ) >= 1,
        "simple accumulator should fire",
    );
    assert!(
        inplace_count(
            "let row = []\nfor x in range(0, 4) do\n  if x % 2 == 0 then\n    row = append(row, 1)\n  else\n    row = append(row, 0)\n  end\nend\nlet n = len(row)"
        ) >= 2,
        "branchy accumulator should fire on both arms",
    );
}

// -- M4 route B relaxations: copy-chain roots, fresh calls, loop chains ------
//
// Each relaxation below widens what route B accepts, so each gets both a parity
// case (the optimized engine must agree with clone-and-alloc) and a negative
// case pinning that a live alias still blocks it. The end-to-end cost side —
// that these shapes actually stop duplicating — lives in
// `rust/tests/copy_elision.rs`.

#[test]
fn inplace_fires_on_bare_bound_accumulator() {
    // `xs = []` and `let xs = []` are semantically identical; the bare form just
    // lowers an extra `Copy` between the alloc and the loop phi's init. The
    // backbone follows that copy chain, so a missing keyword is not a cliff.
    let code = "xs = []\nfor i in range(0, 20) do\n  xs = append(xs, i)\nend\nprint(len(xs), xs[0], xs[19])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 1,
        "bare-bound accumulator should fire"
    );
    let code =
        "xs = [0, 0, 0]\nfor i in range(0, 9) do\n  xs[i % 3] = i\nend\nprint(xs[0], xs[1], xs[2])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 1,
        "bare-bound indexed write should fire"
    );
}

#[test]
fn inplace_does_not_fire_on_aliased_bare_bound_accumulator() {
    // The copy chain is part of the backbone, so it is held to the same
    // "flows only into the accumulator" rule as the root itself: `ys = xs`
    // reads the *same* `Copy` the phi's init reads, and would observe every
    // in-place append.
    let code = "xs = []\nlet ys = xs\nfor i in range(0, 3) do\n  xs = append(xs, i)\nend\nprint(len(xs), len(ys))";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "aliased bare-bound accumulator must not be in-place"
    );
}

#[test]
fn inplace_fires_on_fresh_builtin_root() {
    // `f64_array(n)` allocates a brand-new array and retains no reference to it,
    // so its result roots a unique web exactly like an `AllocList` would. It is
    // the only way to build an f64 array, so without this no f64-array write
    // could ever be in place.
    let code =
        "let a = f64_array(8)\nfor i in range(0, 8) do\n  a[i] = i * 1.0\nend\nprint(a[0], a[7])";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "f64_array root should fire");
    let code = "let a = f64_array(8)\nfor i in range(0, 8) do\n  a = set_at(a, i, i * 1.0)\nend\nprint(a[0], a[7])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 1,
        "set_at() on f64_array should fire"
    );
}

#[test]
fn inplace_does_not_fire_on_aliased_builtin_root() {
    // A fresh call result is only unique until someone copies it: `b = a` is a
    // live observer of the pre-write array, so the writes must clone.
    let code = "let a = f64_array(4)\nlet b = a\nfor i in range(0, 4) do\n  a[i] = i * 1.0\nend\nprint(a[0], b[0])";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "aliased f64_array must not be in-place"
    );
}

#[test]
fn inplace_fires_on_fresh_function_result_root() {
    // `build()` allocates its result and lets nothing else observe it, so the
    // caller holds the only reference — factoring setup into a helper must not
    // cost the optimization.
    let code = "fn build()\n  let a = f64_array(8)\n  a\nend\nlet a = build()\nfor i in range(0, 8) do\n  a[i] = i * 1.0\nend\nprint(a[0], a[7])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 1,
        "fresh function result root should fire"
    );
}

#[test]
fn inplace_does_not_fire_on_non_fresh_function_result() {
    // `fetch()` returns a *captured* container, not one it allocated: the caller
    // shares it with `shared`. Writing in place would corrupt `shared` — the
    // value-semantics guard for the call-result relaxation.
    let code = "let shared = [0, 0, 0]\nfn fetch()\n  shared\nend\nlet a = fetch()\nfor i in range(0, 3) do\n  a[i] = i + 1\nend\nprint(shared[0], shared[2], a[0], a[2])";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "a returned capture is not a fresh root"
    );
    // Same for a returned *parameter*: the argument's owner still holds it.
    let code = "fn pass(v)\n  v\nend\nlet xs = [0, 0, 0]\nlet a = pass(xs)\nfor i in range(0, 3) do\n  a[i] = i + 1\nend\nprint(xs[0], xs[2], a[0])";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "a returned parameter is not a fresh root"
    );
}

#[test]
fn inplace_fires_across_sequential_loop_spines() {
    // Build in one loop, update in the next: two loop-carried phis chained
    // init-to-init off one fresh root. Nothing between or after the loops
    // observes a mid-build value, so both loops' mutations are in place.
    let code = "let xs = []\nfor i in range(0, 10) do\n  xs = append(xs, 0)\nend\nfor i in range(0, 10) do\n  xs[i] = i * 2\nend\nprint(len(xs), xs[0], xs[9])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 2,
        "both loop spines should fire ({} fired)",
        inplace_count(code)
    );
}

#[test]
fn inplace_does_not_fire_on_alias_taken_between_loop_spines() {
    // `ys = xs` between the two loops observes the *mid-build* value that the
    // second loop then overwrites — so the second loop's write must clone. The
    // first loop's append still fires: `ys` is created after it, and reading a
    // finished accumulator is always safe.
    let code = "let xs = []\nfor i in range(0, 3) do\n  xs = append(xs, i)\nend\nlet ys = xs\nfor i in range(0, 3) do\n  xs[i] = i * 10\nend\nprint(ys[0], ys[2], xs[0], xs[2])";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        1,
        "only the build loop may fire; the update loop is observed by `ys`"
    );
}

#[test]
fn inplace_does_not_fire_when_mid_build_value_is_stored_elsewhere() {
    // The build loop stashes the accumulator's *current* id into `keep` on every
    // iteration, so `keep` holds live references to mid-build states. Neither
    // loop may mutate in place — the appends would grow the stashed lists and
    // the second loop's writes would overwrite them.
    let code = "let keep = []\nlet xs = []\nfor i in range(0, 3) do\n  xs = append(xs, i)\n  keep = append(keep, xs)\nend\nfor i in range(0, 3) do\n  xs[i] = 99\nend\nprint(len(keep), len(keep[0]), keep[0][0], xs[0])";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        1,
        "only `keep` (whose own ids never escape) may fire"
    );
}

#[test]
fn inplace_does_not_fire_on_two_mutated_aliases_of_one_root() {
    // Both names hold the *same* id, and both are mutated in a loop. If the
    // chain rule mistook them for one accumulator, each in-place append would
    // grow the store the other one reads. Two phis whose inits resolve to the
    // same root are not a chain, and an alias taken after a loop leaves the
    // first spine's phi with a user outside the web.
    let code = "let xs = []\nlet ys = xs\nfor i in range(0, 3) do\n  ys = append(ys, i)\n  xs = append(xs, i * 10)\nend\nprint(len(xs), len(ys), xs[0], ys[0])";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "two live aliases must not be in-place"
    );
    let code = "let xs = []\nfor i in range(0, 2) do\n  xs = append(xs, i)\nend\nlet ys = xs\nfor i in range(0, 2) do\n  ys = append(ys, i)\n  xs[i] = 9\nend\nprint(len(xs), len(ys), xs[0], ys[0])";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        1,
        "only the first build loop (before the alias exists) may fire"
    );
}

#[test]
fn inplace_does_not_fire_when_the_id_leaves_via_a_block_result() {
    // The one value flow with no input edge to show for it: a block's *result*
    // register. An `if` arm that yields the accumulator hands the mid-build id
    // to `keep`, and a `collect` loop whose body ends in the accumulator
    // collects a reference per iteration. Both used to slip past the web
    // (nothing "uses" the arm's copy) — `assert_inplace_parity` is the real
    // assertion here; the counts pin which mutation is responsible.
    let code = "let xs = []\nfor i in range(0, 4) do\n  let keep = if i > 0 then xs else [] end\n  xs = append(xs, i)\n  print(len(keep), len(xs))\nend";
    assert_inplace_parity(code);
    assert_eq!(inplace_count(code), 0, "arm-result escape must not fire");
    let code = "let xs = []\nlet snaps = for i in range(0, 4) do\n  xs = append(xs, i)\n  xs\nend\nprint(len(snaps[0]), len(snaps[3]), len(xs))";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "collect-loop result escape must not fire"
    );
    // The same shapes with the result *discarded* stay eligible: a statement
    // `if` inside the loop still ends its arm with the accumulator's carry-copy.
    let code = "let xs = []\nfor i in range(0, 6) do\n  if i % 2 == 0 then\n    xs = append(xs, i)\n  else\n    xs = append(xs, -i)\n  end\nend\nprint(len(xs), xs[0], xs[1])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 2,
        "a discarded arm result must stay eligible"
    );
}

#[test]
fn inplace_fires_on_nested_loop_carried_accumulator() {
    // The other chain shape: an inner loop carrying the outer loop's
    // accumulator. The inner phi's init is the outer phi, so they chain, and
    // every mutation is inside the union of the two bodies.
    let code = "let xs = []\nfor y in range(0, 4) do\n  for x in range(0, 3) do\n    xs = append(xs, x + y)\n  end\nend\nprint(len(xs), xs[0], xs[11])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 1,
        "nested loop-carried accumulator should fire"
    );
}

#[test]
fn inplace_does_not_fire_when_mid_build_value_escapes_a_nested_spine() {
    // Same nested shape, but the outer body *retains* the partially-built list
    // in another container between inner-loop passes, so `snapshots` holds live
    // references to states the loop later moves past.
    let code = "let snapshots = []\nlet xs = []\nfor y in range(0, 3) do\n  for x in range(0, 2) do\n    xs = append(xs, x)\n  end\n  snapshots = append(snapshots, xs)\nend\nprint(len(snapshots), len(snapshots[0]), len(xs))";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        1,
        "only `snapshots` (whose own ids never escape) may fire"
    );
}

// -- M4 route B: observing the container being mutated ----------------------
//
// Condition 4 admits a non-web in-region user when it *observes* the container
// (reads content that shares no backing store with it) and finishes before the
// id is rewritten. Read-modify-write is the dominant simulation shape, so these
// are the highest-value firings; the negative cases below enumerate the
// retention forms that must still reject, plus the ordering hazard.

#[test]
fn inplace_fires_on_read_modify_write() {
    // `a[i] = a[i] + 1.0`: the read yields a float, and it happens before the
    // write. The same for a list element and a record field.
    let code = "let a = f64_array(8)\nfor i in range(0, 8) do\n  a[i] = a[i] + 1.0\nend\nprint(a[0], a[7])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 1,
        "f64 read-modify-write should fire"
    );
    let code = "let xs = [1, 2, 3]\nfor i in range(0, 9) do\n  xs[i % 3] = xs[i % 3] * 2\nend\nprint(xs[0], xs[1], xs[2])";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 1,
        "list read-modify-write should fire"
    );
    let code =
        "let r = { a: 0, b: 1 }\nfor i in range(0, 5) do\n  r.a = r.a + i\nend\nprint(r.a, r.b)";
    assert_inplace_parity(code);
    assert!(
        inplace_count(code) >= 1,
        "record read-modify-write should fire"
    );
}

#[test]
fn inplace_fires_on_integrator_over_two_arrays() {
    // `pos[i] = pos[i] + vel[i]`: `vel` is read-only, so only `pos` is a web at
    // all — and reading a *second* container does not disturb the first.
    let code = "let pos = f64_array(8)\nlet vel = f64_array(8)\nfor i in range(0, 8) do\n  pos[i] = pos[i] + vel[i]\nend\nprint(pos[0], vel[0])";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "integrator step should fire");
    // Reading a *different* index than the one written is the same argument.
    let code =
        "let a = f64_array(8)\nfor i in range(0, 7) do\n  a[i] = a[i + 1]\nend\nprint(a[0], a[7])";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "cross-index read should fire");
}

#[test]
fn inplace_fires_on_in_loop_observations() {
    // `len(xs)` returns an int and retains nothing; here it also gates the
    // mutation, so the observation sits in the parent block and the append in a
    // branch arm below it.
    let code = "let xs = []\nfor i in range(0, 20) do\n  if len(xs) < 5 then\n    xs = append(xs, i)\n  end\nend\nprint(len(xs))";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "gated append should fire");
    // A named observation-only binding, read before the mutation.
    let code = "let xs = []\nfor i in range(0, 6) do\n  let n = len(xs)\n  xs = append(xs, n)\nend\nprint(len(xs), xs[5])";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "read-then-append should fire");
}

#[test]
fn inplace_does_not_fire_when_an_alias_is_observed_after_the_mutation() {
    // The ordering hazard, and the reason observations carry an execution
    // index: `snap` aliases the *pre*-append id, and `len(snap)` runs after the
    // append. Value semantics says the old length; an in-place append would
    // report the new one.
    let code = "let xs = []\nfor i in range(0, 4) do\n  let snap = xs\n  xs = append(xs, i)\n  print(len(snap), len(xs))\nend";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "an alias observed after the mutation must not be in-place"
    );
    // Same hazard across the loop boundary: the alias is taken before the loop
    // and read after it.
    let code = "let xs = []\nlet ys = xs\nfor i in range(0, 4) do\n  xs = append(xs, i)\nend\nprint(len(xs), len(ys))";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "an alias observed after the loop must not be in-place"
    );
}

#[test]
fn inplace_does_not_fire_on_retention_forms() {
    // Every way of *keeping* the id, as opposed to observing it, still rejects.
    // Stored into another container:
    let code = "let xs = []\nlet log = []\nfor i in range(0, 4) do\n  xs = append(xs, i)\n  log = append(log, xs)\nend\nprint(len(log[0]), len(log[3]))";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        1,
        "stored-into-container must not fire"
    );
    // Wrapped in a fresh list (an `AllocList` input is a store too):
    let code = "let xs = []\nlet log = []\nfor i in range(0, 4) do\n  xs = append(xs, i)\n  log = append(log, [xs])\nend\nprint(len(log[0][0]), len(xs))";
    assert_inplace_parity(code);
    assert_eq!(inplace_count(code), 1, "wrapped-in-a-list must not fire");
    // Captured by a closure:
    let code = "let xs = []\nfor i in range(0, 4) do\n  xs = append(xs, i)\n  let peek = fn() -> len(xs)\n  print(peek())\nend";
    assert_inplace_parity(code);
    assert_eq!(inplace_count(code), 0, "closure-captured must not fire");
    // Passed to a user function, which could stash it anywhere:
    let code = "fn probe(v)\n  len(v)\nend\nlet xs = []\nfor i in range(0, 4) do\n  xs = append(xs, i)\n  print(probe(xs))\nend";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "user-function argument must not fire"
    );
    // Written to state:
    let code = "state saved = []\nlet xs = []\nfor i in range(0, 4) do\n  xs = append(xs, i)\n  saved = xs\nend\nprint(len(xs), len(saved))";
    assert_eq!(inplace_count(code), 0, "state write must not fire");
    assert_stateful_parity(code, 3);
    // Returned out of the region mid-build:
    let code = "fn f()\n  let xs = []\n  for i in range(0, 4) do\n    xs = append(xs, i)\n    if i == 1 then\n      return xs\n    end\n  end\n  xs\nend\nprint(len(f()))";
    assert_inplace_parity(code);
    assert_eq!(inplace_count(code), 0, "mid-build return must not fire");
}

// -- M4 route B: the mutation loop sits inside a guard ----------------------
//
// A loop inside an `if`/`match` puts the arm's *merge* phi between the root and
// the loop phi. Resolving the spine through merge phis (and treating the spine
// as a tree, so nested and sequential loops over one container both hang off
// it) is what makes a real frame loop — which checks an epoch or a cooling
// schedule before doing any work — eligible at all.
//
// A branch is also a new way for a live alias to reach past the analysis, so
// each negative below pins one of them: the sibling arm, the alias taken before
// the guard, and the carry-out into another variable's phi.

#[test]
fn inplace_fires_on_a_guarded_mutation_loop() {
    // `let`-bound and `state`-backed, `if`, `else`, `match`, and nested guards.
    let code = "let go = 1\nlet a = f64_array(8)\nif go == 1 then\n  for i in range(0, 8) do a[i] = a[i] + 1.0 end\nend\nprint(a[0])";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "guarded loop should fire");
    let code = "let go = 0\nlet a = f64_array(8)\nif go == 1 then\n  print(0)\nelse\n  for i in range(0, 8) do a[i] = a[i] + 1.0 end\nend\nprint(a[0])";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "else-arm loop should fire");
    let code = "let go = 1\nlet a = f64_array(8)\nmatch go\n  when 1 -> for i in range(0, 8) do a[i] = a[i] + 1.0 end\n  when _ -> print(0)\nend\nprint(a[0])";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "match-arm loop should fire");
    let code = "let go = 1\nlet a = f64_array(8)\nif go == 1 then\n  if go > 0 then\n    for i in range(0, 8) do a[i] = a[i] + 1.0 end\n  end\nend\nprint(a[0])";
    assert_inplace_parity(code);
    assert!(inplace_count(code) >= 1, "nested guards should fire");
    let code = "state a = f64_array(8)\nstate alpha = 1.0\nif alpha > 0.001 then\n  for i in range(0, 8) do a[i] = a[i] + 1.0 end\n  alpha = alpha * 0.98\nend\nprint(a[0])";
    assert_stateful_parity(code, 3);
    assert!(inplace_count(code) >= 1, "guarded state loop should fire");
}

#[test]
fn inplace_fires_on_a_container_allocated_inside_the_guard() {
    // Reallocating the buffer when the input changes, then filling it: the
    // fresh array is committed to the slot and the loop's result leaves the
    // guard in that variable's own merge phi.
    let code = "state a = f64_array(1)\nstate epoch = 0\nif epoch == 0 then\n  a = f64_array(8)\n  for i in range(0, 8) do a[i] = a[i] + 1.0 end\n  epoch = 1\nend\nprint(a[0], len(a))";
    assert_stateful_parity(code, 3);
    assert!(inplace_count(code) >= 1, "realloc-then-fill should fire");
}

#[test]
fn inplace_fires_on_a_spine_tree_of_loops() {
    // One container, several loops: nested inside each other and sequential
    // after each other, all hanging off one spine. Only the last loop fired
    // before, because the spine was built backward from the seed alone.
    let code = "state v = f64_array(4)\nstate go = 1\nif go == 1 then\n  for i in range(0, 4) do\n    for j in range(0, 4) do v[j] = v[j] - 1.0 end\n  end\n  for i in range(0, 4) do v[i] = v[i] * 2.0 end\nend\nprint(v[0])";
    assert_stateful_parity(code, 3);
    assert_eq!(inplace_count(code), 2, "both loops of the tree should fire");
    let code = "state v = f64_array(4)\nstate go = 1\nif go == 1 then\n  for i in range(0, 4) do\n    if i > 0 then v[i] = v[i] + 1.0 end\n  end\n  for i in range(0, 4) do v[i] = v[i] * 2.0 end\nend\nprint(v[0])";
    assert_stateful_parity(code, 3);
    assert_eq!(inplace_count(code), 2, "guarded write plus a second loop");
}

#[test]
fn inplace_does_not_fire_on_an_alias_taken_through_a_branch() {
    // The sibling arm keeps a reference to the pre-mutation container. Only one
    // arm runs, but the analysis cannot know which — and the same shape inside a
    // loop genuinely alternates.
    let code = "let go = 1\nlet a = f64_array(4)\nlet keep = []\nif go == 1 then\n  for i in range(0, 4) do a[i] = a[i] + 1.0 end\nelse\n  keep = a\nend\nprint(a[0], len(keep))";
    assert_inplace_parity(code);
    assert_eq!(inplace_count(code), 0, "sibling-arm alias must not fire");
    // Taken before the guard and read after it.
    let code = "let go = 1\nlet a = f64_array(4)\nlet snap = a\nif go == 1 then\n  for i in range(0, 4) do a[i] = a[i] + 1.0 end\nend\nprint(a[0], snap[0])";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "alias before the guard must not fire"
    );
    // A match arm that *yields* the container binds it to the match's result.
    let code = "let go = 2\nlet a = f64_array(4)\nlet r = match go\n  when 1 -> for i in range(0, 4) do a[i] = a[i] + 1.0 end\n  when _ -> a\nend\nprint(a[0], len(r))";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "match sibling-arm alias must not fire"
    );
}

#[test]
fn inplace_does_not_fire_when_an_arm_carries_the_id_into_another_variable() {
    // Regression, and a bug this chunk found on the committed analysis: a web
    // term's *phi carry-out* into another variable's phi is a value flow with no
    // input edge, so nothing checked it — and the web BFS then absorbed that
    // phi, hiding the escape completely. `keep` watches later iterations mutate
    // the container it snapshotted; the two engines disagreed (`3 3` vs `3 1`).
    let code = "let xs = []\nlet keep = []\nfor i in range(0, 4) do\n  if i == 1 then\n    keep = xs\n  else\n    xs = append(xs, i)\n  end\nend\nprint(len(xs), len(keep))";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "carry-out into `keep` must not fire"
    );
}

#[test]
fn inplace_does_not_fire_on_a_stale_spine_link() {
    // `a` snapshots the container before the first loop and then drives a second
    // one. Both loops would mutate the same id, so this is two spines over one
    // container, not one spine with two stages — the copy on the second link is
    // stale by the time it is read.
    let code = "let xs = []\nlet a = xs\nfor i in range(0, 3) do\n  xs = append(xs, i)\nend\nfor i in range(0, 3) do\n  a = append(a, i)\nend\nprint(len(xs), len(a))";
    assert_inplace_parity(code);
    assert_eq!(inplace_count(code), 0, "a stale spine link must not fire");
}

#[test]
fn inplace_does_not_fire_when_a_guarded_state_container_escapes() {
    // The guard's continuation phi is where the container leaves the construct;
    // anything retaining it from there outlives the run just as it would from
    // inside the region.
    let code = "state a = f64_array(4)\nstate go = 1\nlet s = symbol(\"p\")\nif go == 1 then\n  for i in range(0, 4) do a[i] = a[i] + 1.0 end\nend\npush_output(s, a)\nprint(a[0])";
    assert_stateful_parity(code, 3);
    assert_eq!(
        inplace_count(code),
        0,
        "emitted after the guard must not fire"
    );
    // A second slot fed from inside the guard.
    let code = "state a = f64_array(1)\nstate n = 0\nstate other = []\nif n == 0 then\n  a = f64_array(4)\n  other = a\n  for i in range(0, 4) do a[i] = a[i] + 1.0 end\n  n = 1\nend\nprint(a[0], len(other))";
    assert_stateful_parity(code, 3);
    assert_eq!(inplace_count(code), 0, "a second slot must not fire");
}

#[test]
fn inplace_does_not_fire_when_a_guard_arm_mutates_an_alias() {
    // Fuzzer seed 113278, found while resolving spines through merge phis. The
    // arm mutates `al`, an *alias*, and carries the untouched `xs` back into the
    // merge — so the merge stays a live pre-mutation holder that the code after
    // the `if` reads. The two engines disagreed on `len(xs)` (6 vs 3).
    let code = "let xs = [7, 11, 1]\nif 1 == 1 then\n  let al = xs\n  let w = 3\n  while w > 0 do\n    w = w - 1\n    al = append(al, 5)\n  end\nelse\n  xs[2] = 3\nend\nprint(len(xs))";
    assert_inplace_parity(code);
    assert_eq!(inplace_count(code), 0, "a mutated alias must not fire");
}

#[test]
fn inplace_does_not_fire_on_two_spines_live_at_once() {
    // Fuzzer seed 132768, found while relaxing the spine from a chain to a tree.
    // `al` aliases `xs` at the top of each outer iteration and both are appended
    // in the same inner loop: two accumulators over one id, whose regions
    // overlap. In place, the list grew twice as fast (23 vs 13).
    let code = "let xs = [1, 6, 0]\nlet last = 0\nfor i in range(1, 6) do\n  let al = xs\n  let w = 2\n  while w > 0 do\n    w = w - 1\n    xs = append(xs, w)\n    al = append(al, 1)\n  end\n  last = len(al)\nend\nprint(len(xs), last)";
    assert_inplace_parity(code);
    assert_eq!(
        inplace_count(code),
        0,
        "two spines live at once must not fire"
    );
}

/// A differential sweep over guard shapes: every mutation loop wrapped in an
/// `if`/`match`, every way a branch can leak the container, and the `state`
/// variants run four times each. The fuzzer generates statement `if`s but never
/// binds a container from value-position control flow, so branch-related shapes
/// are exactly where it is weakest — this table is the net that caught the
/// spine-hand-off and stale-link cases while they were still wrong.
#[test]
fn guarded_shapes_agree_with_clone_and_alloc() {
    // let-bound (single run) shapes
    let plain = [
        "let go = 1\nlet a = f64_array(4)\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nend\nprint(a[0])",
        "let go = 0\nlet a = f64_array(4)\nif go == 1 then\n  print(0)\nelse\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nend\nprint(a[0])",
        "let go = 1\nlet a = f64_array(4)\nlet keep = []\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nelse\n  keep = a\nend\nprint(a[0], len(keep))",
        "let go = 0\nlet a = f64_array(4)\nlet keep = []\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nelse\n  keep = a\nend\nprint(a[0], len(keep))",
        "let go = 1\nlet a = f64_array(4)\nlet snap = a\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nend\nprint(a[0], snap[0])",
        "let xs = []\nlet keep = []\nfor i in range(0,4) do\n  if i == 1 then keep = xs else xs = append(xs, i) end\nend\nprint(len(xs), len(keep))",
        "let xs = []\nlet keep = []\nfor i in range(0,4) do\n  if i % 2 == 0 then xs = append(xs, i) else keep = xs end\nend\nprint(len(xs), len(keep))",
        "let a = f64_array(4)\nlet b = a\nfor i in range(0,4) do a[i] = a[i] + 1.0 end\nfor i in range(0,4) do b[i] = b[i] + 2.0 end\nprint(a[0], b[0])",
        "let go = 1\nlet a = f64_array(4)\nmatch go\n  when 1 -> for i in range(0,4) do a[i] = a[i] + 1.0 end\n  when _ -> print(0)\nend\nprint(a[0])",
        "let go = 2\nlet a = f64_array(4)\nlet r = match go\n  when 1 -> for i in range(0,4) do a[i] = a[i] + 1.0 end\n  when _ -> a\nend\nprint(a[0], len(r))",
        "let go = 1\nlet a = f64_array(4)\nif go == 1 then\n  if go > 0 then\n    for i in range(0,4) do a[i] = a[i] + 1.0 end\n  end\nend\nprint(a[0])",
        "let go = 1\nlet xs = []\nfor k in range(0,3) do\n  if k > 0 then\n    for i in range(0,2) do xs = append(xs, i) end\n  end\nend\nprint(len(xs))",
        "let go = 1\nlet a = f64_array(4)\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nend\nlet s = symbol(\"p\")\npush_output(s, a)\nprint(a[0])",
        "let go = 1\nlet a = f64_array(4)\nlet log = []\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\n  log = append(log, a)\nend\nprint(a[0], len(log))",
        "let go = 1\nlet a = f64_array(4)\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nend\nlet peek = fn() -> a[0]\nprint(peek())",
        "let go = 1\nlet a = f64_array(4)\nlet r = if go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\n  a\nelse\n  a\nend\nprint(r[0], a[0])",
    ];
    for code in plain {
        // A snippet that fails to parse would pass parity vacuously (both
        // engines error), so pin that each one actually runs.
        assert!(
            run(code, OptFlags::none()).is_ok(),
            "sweep case failed:\n{code}"
        );
        assert_inplace_parity(code);
    }
    // state (multi-run) shapes
    let stateful = [
        "state a = f64_array(4)\nstate go = 1\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nend\nprint(a[0])",
        "state a = f64_array(4)\nstate alpha = 1.0\nif alpha > 0.001 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\n  alpha = alpha * 0.5\nend\nprint(a[0], alpha)",
        "state a = f64_array(1)\nstate epoch = 0\nif epoch == 0 then\n  a = f64_array(4)\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\n  epoch = 1\nend\nprint(a[0], len(a))",
        "state a = f64_array(4)\nfor i in range(0,4) do a[i] = a[i] + 1.0 end\nfor i in range(0,4) do a[i] = a[i] * 2.0 end\nprint(a[0])",
        "state v = f64_array(4)\nstate go = 1\nif go == 1 then\n  for i in range(0,4) do\n    for j in range(0,4) do v[j] = v[j] - 1.0 end\n  end\n  for i in range(0,4) do v[i] = v[i] * 2.0 end\nend\nprint(v[0])",
        "state v = f64_array(4)\nstate go = 1\nif go == 1 then\n  for i in range(0,4) do\n    if i > 0 then v[i] = v[i] + 1.0 end\n  end\n  for i in range(0,4) do v[i] = v[i] * 2.0 end\nend\nprint(v[0])",
        "state a = f64_array(4)\nstate keep = []\nstate go = 1\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nelse\n  keep = a\nend\nprint(a[0], len(keep))",
        "state a = f64_array(4)\nstate log = []\nstate go = 1\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nend\nlog = append(log, a[0])\nprint(a[0], len(log))",
        "state a = f64_array(4)\nstate go = 1\nlet s = symbol(\"p\")\nif go == 1 then\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\nend\npush_output(s, a)\nprint(a[0])",
        "state a = f64_array(1)\nstate n = 0\nstate other = []\nif n == 0 then\n  a = f64_array(4)\n  other = a\n  for i in range(0,4) do a[i] = a[i] + 1.0 end\n  n = 1\nend\nprint(a[0], len(other))",
        "state px = f64_array(4)\nstate vx = f64_array(4)\nstate go = 1\nif go == 1 then\n  for i in range(0,4) do vx[i] = vx[i] + 1.0 end\n  for i in range(0,4) do px[i] = px[i] + vx[i] end\nend\nprint(px[0], vx[0])",
        "state xs = []\nstate go = 1\nif go == 1 then\n  for i in range(0,3) do\n    if i % 2 == 0 then xs = append(xs, i) end\n  end\nend\nprint(len(xs))",
    ];
    for code in stateful {
        assert!(
            run(code, OptFlags::none()).is_ok(),
            "sweep case failed:\n{code}"
        );
        assert_stateful_parity(code, 4);
    }
}

// -- M4 route A: straight-line last-use in-place mutation -------------------
//
// Route A rewrites mutations of freshly-allocated, dead-after containers on
// the *lowered bytecode* (see `lastuse.rs`). The same triple-differential bar
// applies, plus a route-A-only oracle so a route-B interaction can't mask a
// route-A bug (and vice versa).

/// Route A alone — isolates the last-use pass from route B.
const ROUTE_A_ONLY: OptFlags = OptFlags {
    in_place_mutation: false,
    in_place_straight_line: true,
    copy_propagation: false,
    preserve_observations: false,
    preserve_trace: false,
};

/// bytecode(no-opt) == bytecode(route A only) == bytecode(all).
#[track_caller]
fn assert_route_a_parity(code: &str) {
    assert_inplace_parity(code); // no-opt vs all
    let noopt = run(code, OptFlags::none());
    let ra = run(code, ROUTE_A_ONLY);
    match (noopt, ra) {
        (Ok((nv, no)), Ok((rv, ro))) => {
            assert_eq!(nv, rv, "route-A value mismatch for:\n{code}");
            assert_eq!(no, ro, "route-A output mismatch for:\n{code}");
        }
        (Err(_), Err(_)) => {}
        (n, r) => panic!("route-A ok/err mismatch for:\n{code}\n  noopt={n:?}\n  routeA={r:?}"),
    }
}

/// Number of instructions the route-A pass rewrites in `code`, measured on
/// the same lowering a default-flags run would execute (route B applied
/// first, so route A only counts what it adds).
fn route_a_count(code: &str) -> usize {
    let program = compile_program(code);
    let in_place = escape::analyze(&program);
    let mut bc = crate::backend::bytecode::lower_program_opt(&program, &in_place).expect("lower");
    crate::backend::bytecode::lastuse::apply(&mut bc, &program)
}

#[test]
fn route_a_fires_on_straight_line_builder() {
    // The plan's canonical case: fresh alloc, then a mutation chain, dead
    // register at each step.
    let code = "let xs = [1, 2, 3]\nxs[0] = 9\nxs[1] = 8\nprint(xs)";
    assert!(route_a_count(code) >= 2, "builder chain should fire");
    assert_route_a_parity(code);
}

#[test]
fn route_a_fires_on_read_then_mutate() {
    // `len(xs)` before `append(xs, …)` — the precision case the plan calls
    // out: a read *before* the mutation is fine; a single-static-consumer
    // test would wrongly forbid it.
    let code = "let xs = [1, 2, 3]\nlet n = len(xs)\nxs = append(xs, n)\nprint(xs)";
    assert!(route_a_count(code) >= 1, "read-then-mutate should fire");
    assert_route_a_parity(code);
}

#[test]
fn route_a_fires_on_per_iteration_builder() {
    // A fresh container allocated *inside* the loop body: the back edge
    // re-executes the alloc (the kill) before any re-read, so each
    // iteration's mutation is in-place even though the mutation sits in a
    // loop. This composes with route B on the outer accumulator.
    let code = "let grid = []\nfor y in range(0, 4) do\n  let t = [0, 0]\n  t[0] = y\n  grid = append(grid, t)\nend\nprint(len(grid), grid[3][0])";
    assert!(
        route_a_count(code) >= 1,
        "per-iteration builder should fire"
    );
    assert_route_a_parity(code);
}

#[test]
fn route_a_fires_on_record_builder() {
    let code = "let p = { a: 0, b: 0 }\np.a = 1\np.b = 2\nprint(p.a, p.b)";
    assert!(route_a_count(code) >= 2, "record field chain should fire");
    assert_route_a_parity(code);
}

#[test]
fn route_a_does_not_fire_on_alias() {
    // `let ys = xs` retains the pre-mutation id — mutating in place would be
    // observable through `ys`.
    let code = "let xs = [1, 2, 3]\nlet ys = xs\nxs[0] = 9\nprint(xs[0], ys[0])";
    assert_eq!(route_a_count(code), 0, "aliased container must not fire");
    assert_route_a_parity(code);
}

#[test]
fn route_a_does_not_fire_on_escapes() {
    // Stored into another container before the mutation.
    let code = "let xs = [1, 2]\nlet outer = [xs]\nxs[0] = 9\nprint(outer[0][0], xs[0])";
    assert_eq!(
        route_a_count(code),
        0,
        "stored-into-container must not fire"
    );
    assert_route_a_parity(code);
    // Captured by a closure before the mutation. Written inside a function
    // because at module scope this is now a compile error: the closure would
    // read the value captured at its definition, not the rebound one (see
    // `compiler::capture_lag`). The escape itself is what route A must notice,
    // and that is unchanged by the wrapper.
    let code = "fn f()\n  let xs = [1, 2]\n  let peek = fn() -> xs[0]\n  xs[0] = 9\n  print(peek(), xs[0])\nend\nf()";
    assert_eq!(route_a_count(code), 0, "closure-captured must not fire");
    assert_route_a_parity(code);
    // Passed to a user function before the mutation (arbitrary code could
    // retain it).
    let code =
        "fn probe(v)\n  v[0]\nend\nlet xs = [1, 2]\nlet a = probe(xs)\nxs[0] = 9\nprint(a, xs[0])";
    assert_eq!(route_a_count(code), 0, "call argument must not fire");
    assert_route_a_parity(code);
    // Returning a pre-mutation alias out of a function.
    let code = "fn f()\n  let xs = [1, 2]\n  let ys = xs\n  xs[0] = 5\n  ys\nend\nprint(f()[0])";
    assert_eq!(
        route_a_count(code),
        0,
        "escaping pre-mutation alias must not fire"
    );
    assert_route_a_parity(code);
}

#[test]
fn route_a_fires_on_returned_mutation_result() {
    // Returning the *rebound* (post-mutation) value is safe: only the final
    // container escapes, which is value-identical to the clone. The
    // pre-mutation id has no other observer.
    let code = "fn f(v)\n  let xs = [1, 2]\n  xs[0] = v\n  xs\nend\nprint(f(5)[0], f(7)[0])";
    assert!(
        route_a_count(code) >= 1,
        "returned mutation result should fire"
    );
    assert_route_a_parity(code);
}

#[test]
fn route_a_does_not_fire_on_state_container() {
    // A state-backed container is not a fresh alloc; and the value written
    // to state escapes. Never in-place.
    let code = "state xs = [0, 0]\nxs[0] = 1\nprint(xs[0])";
    assert_eq!(route_a_count(code), 0, "state container must not fire");
    assert_stateful_parity(code, 3);
}

#[test]
fn route_a_dup_bytes_drop_on_builder() {
    if !crate::stats::DUP_STATS_ENABLED {
        return;
    }
    // Route A alone must strictly drop copied bytes on the per-iteration
    // builder (route B is off, so the drop is attributable to route A).
    let code = "let grid = []\nfor y in range(0, 50) do\n  let t = [0, 0, 0, 0]\n  t[0] = y\n  t[1] = y\n  grid = append(grid, t)\nend\nprint(len(grid))";
    let bytes = |opts| {
        let mut env = Env::new();
        env.set_opt_flags(opts);
        env.run_source(code).expect("run");
        env.heap().dup_stats().total_bytes()
    };
    let off = bytes(OptFlags::none());
    let on = bytes(ROUTE_A_ONLY);
    assert!(off > 0, "baseline should copy something");
    assert!(
        on < off,
        "route A should strictly reduce copied bytes ({on} !< {off})"
    );
}

#[test]
fn inplace_dup_bytes_drop_on_accumulator() {
    if !crate::stats::DUP_STATS_ENABLED {
        return;
    }
    // The plan's verification: turning the optimization on must strictly drop
    // the bytes copied for a loop-carried accumulator.
    let code = "let xs = []\nfor i in range(0, 200) do\n  xs = append(xs, i)\nend\nlet n = len(xs)";
    let bytes = |opts| {
        let mut env = Env::new();
        env.set_opt_flags(opts);
        env.run_source(code).expect("run");
        env.heap().dup_stats().total_bytes()
    };
    let off = bytes(OptFlags::none());
    let on = bytes(OptFlags::all());
    assert!(off > 0, "baseline should copy something");
    assert!(
        on < off,
        "in-place should strictly reduce copied bytes ({on} !< {off})"
    );
}

// -- Trace / provenance (best-effort under the VM) --------------------------

/// The bytecode VM must populate the trace buffer keyed by origin TermId so
/// `explain` / `ExplainTerm` keep working once the graph engine is gone. This
/// is best-effort (in-place mutation and register reuse can thin coverage), but
/// straight-line value-producing terms and call results must be recorded.
#[test]
fn bytecode_trace_records_term_values() {
    let mut env = Env::new(); // default backend is bytecode
    env.trace_mut().enable();
    let pid = env
        .load_program("let x = 2 + 3\nlet total = x * 4\nprint(total)")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let program = env.get_program(pid).expect("program");
    let render = |tid| {
        let ev = env
            .trace()
            .last_for_term(tid)
            .unwrap_or_else(|| panic!("term {tid:?} was not traced"));
        value::value_to_display_string(&ev.result, env.heap())
    };

    let x = program.find_term("x").expect("x term");
    assert_eq!(render(x), "5", "x = 2 + 3");
    let total = program.find_term("total").expect("total term");
    assert_eq!(render(total), "20", "total = x * 4");
}

/// A user function's call result must be traced (recorded when the callee frame
/// delivers its value back to the caller), so `explain` can show the value of a
/// term whose value came from a call.
#[test]
fn bytecode_trace_records_call_results() {
    let mut env = Env::new();
    env.trace_mut().enable();
    let pid = env
        .load_program("fn double(n)\n  n * 2\nend\nlet r = double(21)\nprint(r)")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let program = env.get_program(pid).expect("program");
    let r = program.find_term("r").expect("r term");
    let ev = env
        .trace()
        .last_for_term(r)
        .expect("r (call result) was not traced");
    assert_eq!(value::value_to_display_string(&ev.result, env.heap()), "42");
}

/// A local `let` whose name collides with an outer binding must not produce a
/// phi against that outer binding. `petal-ui`'s `_wrap_segment` hit this by
/// naming a local `take`, which collides with `std::take` from the auto-loaded
/// core prelude: the phi was initialized from the prelude's root-block closure,
/// so lowering failed with "not in this function" — and renaming the local was
/// the only fix. See docs/dev/var-next-steps.md (Lexical shadowing).
#[test]
fn a_local_shadowing_an_outer_name_lowers() {
    // `outer` stands in for the prelude binding; the nesting is `_wrap_segment`'s.
    let program = compile_program(
        "let outer = 1\n\
         fn f(words)\n\
         \x20 for w in words do\n\
         \x20   while len(w) > 2 do\n\
         \x20     let outer = 2\n\
         \x20     while outer < 3 do\n\
         \x20       outer = outer + 1\n\
         \x20     end\n\
         \x20     w = slice(w, outer, len(w))\n\
         \x20   end\n\
         \x20 end\n\
         end\n",
    );
    crate::backend::bytecode::lower_program(&program)
        .expect("a shadowing local must not phi against the outer binding");
}

/// A term whose input edge points into another function's block is a malformed
/// IR graph. Lowering must surface that as a `Result::Err` — which reaches the
/// user as a clean "bytecode lowering failed" message — rather than panicking
/// mid-lowering (the pre-existing `flat()` panic this replaced).
#[test]
fn lowering_reports_cross_function_term_reference_as_error() {
    let mut program = compile_program("fn f()\n  1\nend\nlet a = 2\na + 3\n");

    // A term that lives in `f`'s body block — foreign to the root function.
    let f_body = program.functions[0].body_block;
    let foreign = program
        .terms
        .iter()
        .find(|t| t.block_id == f_body)
        .expect("f has at least one body term")
        .id;

    // Corrupt a root-level term so one of its inputs crosses the function
    // boundary, exactly the shape that used to hit the `flat()` panic.
    let root = program.root_block;
    let victim = program
        .terms
        .iter()
        .find(|t| t.block_id == root && !t.inputs.is_empty())
        .expect("root has a term with inputs")
        .id;
    program.terms[victim.0 as usize].inputs[0] = foreign;

    let err = crate::backend::bytecode::lower_program(&program)
        .expect_err("cross-function reference must lower to an error, not a panic");
    assert!(
        err.contains("not in this function"),
        "unexpected lowering error message: {err}"
    );
}

// ---------------------------------------------------------------------------
// Ragged records: `??` tolerates an absent field, a bare read does not
// ---------------------------------------------------------------------------

#[test]
fn coalesce_tolerates_a_missing_field() {
    let all = OptFlags::all();
    // The reported shape: ragged JSON where the key is simply not there.
    assert_eq!(run(r#"{a: 1}.fragment ?? "none""#, all).unwrap().0, "none");
    assert_eq!(
        run(r#"{a: 1}["fragment"] ?? "none""#, all).unwrap().0,
        "none"
    );
    // A key that *is* there still wins, and a present-but-nil key still
    // coalesces (that is what `??` has always meant).
    assert_eq!(run(r#"{a: 1}.a ?? "none""#, all).unwrap().0, "1");
    assert_eq!(run(r#"{a: nil}.a ?? "none""#, all).unwrap().0, "none");
    // The whole access spine is tolerant, so an absent link mid-chain does not
    // abort before the `??` can run.
    assert_eq!(run(r#"{a: {b: 1}}.x.b ?? "d""#, all).unwrap().0, "d");
    assert_eq!(run(r#"{a: {b: 1}}.a.b ?? "d""#, all).unwrap().0, "1");
    assert_eq!(run(r#"{a: {b: 1}}["x"]["b"] ?? "d""#, all).unwrap().0, "d");
    // The plain forms are untouched.
    assert_eq!(run(r#"nil ?? "d""#, all).unwrap().0, "d");
    assert_eq!(run(r#"7 ?? "d""#, all).unwrap().0, "7");

    for code in [
        r#"{a: 1}.fragment ?? "none""#,
        r#"{a: {b: 1}}.x.b ?? "d""#,
        r#"{a: 1}["fragment"] ?? "none""#,
    ] {
        assert_parity(code);
    }
}

#[test]
fn optional_access_reads_a_missing_link_as_nil() {
    let all = OptFlags::all();
    // `?.` is the same tolerance the left of a `??` gets, asked for outright —
    // so it stands on its own, with no fallback to write.
    assert_eq!(run("{a: 1}?.fragment", all).unwrap().0, "nil");
    assert_eq!(run(r#"{a: 1}?.fragment ?? "none""#, all).unwrap().0, "none");
    assert_eq!(run("{a: 1}?.a", all).unwrap().0, "1");
    // One `?.` short-circuits the rest of its chain, like JavaScript's: a
    // missing `x` must not then error on the `.b` written after it.
    assert_eq!(run("{a: {b: 1}}?.x.b", all).unwrap().0, "nil");
    assert_eq!(run("{a: {b: 1}}?.a.b", all).unwrap().0, "1");
    assert_eq!(run("{a: 1}?.x.y.z", all).unwrap().0, "nil");
    // The index spelling, `a?.[i]`.
    assert_eq!(run(r#"{a: 1}?.["z"]"#, all).unwrap().0, "nil");
    assert_eq!(run(r#"{a: 1}?.["a"]"#, all).unwrap().0, "1");
    assert_eq!(run("[10, 20]?.[0]", all).unwrap().0, "10");

    for code in ["{a: 1}?.fragment", "{a: {b: 1}}?.x.b", r#"{a: 1}?.["z"]"#] {
        assert_parity(code);
    }
}

#[test]
fn optional_access_does_not_soften_a_real_bug() {
    let all = OptFlags::all();
    // Absence is a property of ragged data, not of wrong types or bad indices.
    // `?.` says "this link may be missing", never "ignore what goes wrong here".
    assert!(run("3?.field", all).unwrap_err().contains("Cannot access"));
    assert!(
        run("[1, 2]?.[9]", all)
            .unwrap_err()
            .contains("out of bounds")
    );
    // And it is confined to its own chain: a `?.` inside an index expression
    // does not make the enclosing read tolerant.
    assert!(
        run(r#"{a: 1}[{}?.k ?? "zz"]"#, all)
            .unwrap_err()
            .contains("No key")
    );
}

#[test]
fn a_bare_missing_field_is_still_an_error() {
    let all = OptFlags::all();
    // Absence is tolerated only where the program asked for it. A typo in a
    // plain read must still be loud.
    assert!(
        run("{a: 1}.fragment", all)
            .unwrap_err()
            .contains("No field")
    );
    assert!(run(r#"{a: 1}["z"]"#, all).unwrap_err().contains("No key"));
    // A wrong-typed base is a bug, not ragged data — `??` does not soften it.
    assert!(
        run(
            r#"let x = 3
x.field ?? "d""#,
            all
        )
        .unwrap_err()
        .contains("Cannot access field")
    );
    // Nor does it soften an out-of-bounds list index: a list has no ragged keys.
    assert!(
        run(r#"[1, 2][9] ?? "d""#, all)
            .unwrap_err()
            .contains("out of bounds")
    );
}

#[test]
fn prelude_field_helpers_read_ragged_records() {
    let all = OptFlags::all();
    assert_eq!(run(r#"field({a: 1}, "zz", 7)"#, all).unwrap().0, "7");
    assert_eq!(run(r#"field({a: 1}, "a", 7)"#, all).unwrap().0, "1");
    assert_eq!(run(r#"has_field({a: 1}, "a")"#, all).unwrap().0, "true");
    assert_eq!(run(r#"has_field({a: 1}, "zz")"#, all).unwrap().0, "false");
    // `has_field` is the one that can see a key whose value is nil.
    assert_eq!(run(r#"has_field({a: nil}, "a")"#, all).unwrap().0, "true");
}
