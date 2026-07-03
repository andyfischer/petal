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

/// Run `code` on `backend` for `runs` successive runs of one persistent stack
/// (reset between runs, keeping state), returning the last rendered value, the
/// concatenated output, and the final state map rendered to a sorted JSON
/// string. Exercises state persistence and the untouched-key sweep.
fn run_stateful(
    code: &str,
    backend: Backend,
    runs: usize,
) -> Result<(String, Vec<String>, String), String> {
    let mut env = Env::new();
    env.set_backend(backend);
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
    let mut pairs: Vec<String> = state
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    pairs.sort();
    Ok((rendered, output, pairs.join(",")))
}

/// Assert the two backends agree across `runs` successive runs, including the
/// final persistent state map.
#[track_caller]
fn assert_stateful_parity(code: &str, runs: usize) {
    let graph = run_stateful(code, Backend::Graph, runs);
    let bytecode = run_stateful(code, Backend::Bytecode, runs);
    match (graph, bytecode) {
        (Ok((gv, go, gs)), Ok((bv, bo, bs))) => {
            assert_eq!(gv, bv, "value mismatch for:\n{code}");
            assert_eq!(go, bo, "output mismatch for:\n{code}");
            assert_eq!(gs, bs, "state mismatch for:\n{code}");
        }
        (Err(_), Err(_)) => {}
        (g, b) => panic!("ok/err mismatch for:\n{code}\n  graph={g:?}\n  bytecode={b:?}"),
    }
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
    let (_v, out) = run(r#"print(slice("a─b", 0, 2))"#, Backend::Graph).unwrap();
    assert_eq!(out, vec!["a"], "end mid-char snaps down to a boundary");
    let (_v, out) = run(r#"print(slice("a─b", 2, 5))"#, Backend::Graph).unwrap();
    assert_eq!(out, vec!["b"], "start mid-char snaps up to a boundary");
    let (_v, out) = run(r#"print(slice("a─b", 0, 4))"#, Backend::Graph).unwrap();
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
fn break_continue_transfer_control_immediately() {
    // `break`/`continue` transfer control at once — statements after them
    // (in the same block or an enclosing arm, up to the loop body) must not
    // execute. The graph engine originally ran that trailing dead code
    // (flag-based exit at frame pop); found by the differential fuzzer
    // (seed 431) and fixed for both engines. The dead rebind/division here
    // would change the value / raise an error if trailing code ran.
    let (_, out) = run(
        "let m = 1\nfor i in range(0, 2) do\n  continue\n  m = 10\nend\nprint(m)",
        Backend::Bytecode,
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
    let (_, out) = run(code, Backend::Bytecode).unwrap();
    assert_eq!(out.last().map(String::as_str), Some("end 7"));
    assert!(out.iter().all(|l| l != "read: nil"), "nil leak: {out:?}");
    assert_parity(code);
    // Partial execution: the first of two rebinds in an arm runs, then
    // break — the carry must hold the executed rebind's value (103), not
    // the dead second rebind's register or the pre-iteration value.
    let (_, out) = run(
        "let total = 0\nfor x in range(1, 4) do\n  total = total + x\n  if x == 2 then break end\n  total = total + 100\nend\nprint(total)",
        Backend::Bytecode,
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
        "enum Shape\n  Circle(r)\n  Rect(w, h)\nend\n\
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
    env.set_backend(Backend::Bytecode);
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

    let single = run(code, Backend::Bytecode).unwrap().0;
    assert_eq!(rendered, single, "bounded run diverged from single run");
    assert_eq!(rendered, run(code, Backend::Graph).unwrap().0, "diverged from graph");
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
// These assert the triple differential the plan requires: the graph engine,
// the bytecode VM with in-place mutation off, and the bytecode VM with it on
// must all agree on value + output. They also pin that the optimization
// actually *fires* (via `DupStats` and the analysis directly) so the parity
// checks aren't vacuously green on a disabled optimization.

use crate::backend::bytecode::escape;
use crate::backend::OptFlags;

/// Run `code` on the bytecode VM with `opts`, returning value + output.
fn run_opt(code: &str, opts: OptFlags) -> Result<(String, Vec<String>), String> {
    let mut env = Env::new();
    env.set_backend(Backend::Bytecode);
    env.set_opt_flags(opts);
    let v = env.run_source(code)?;
    Ok((value::value_to_display_string(&v, env.heap()), env.take_output()))
}

/// Graph == bytecode(no-opt) == bytecode(in-place): the M4 correctness bar.
#[track_caller]
fn assert_inplace_parity(code: &str) {
    assert_parity(code); // graph vs bytecode(no-opt)
    let noopt = run_opt(code, OptFlags::none());
    let opt = run_opt(code, OptFlags::all());
    match (noopt, opt) {
        (Ok((nv, no)), Ok((ov, oo))) => {
            assert_eq!(nv, ov, "in-place value mismatch for:\n{code}");
            assert_eq!(no, oo, "in-place output mismatch for:\n{code}");
        }
        (Err(_), Err(_)) => {}
        (n, o) => panic!("in-place ok/err mismatch for:\n{code}\n  noopt={n:?}\n  opt={o:?}"),
    }
}

/// Compile `code` to a `Program` (the front half of the run pipeline).
fn compile_program(code: &str) -> crate::program::Program {
    use crate::compiler::Compiler;
    use crate::lexer::Lexer;
    use crate::native_fn::NativeFnTable;
    use crate::parse::Parser;
    use crate::program::ProgramId;
    let mut lexer = Lexer::new(code);
    lexer.tokenize().expect("tokenize");
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    let stmts = parser.parse_program().expect("parse");
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
    let code =
        "let xs = []\nlet ys = xs\nfor i in range(0, 3) do\n  xs = append(xs, i)\nend\nprint(len(xs), len(ys))";
    assert_inplace_parity(code);
    assert_eq!(inplace_count(code), 0, "aliased accumulator must not be in-place");
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

#[test]
fn inplace_does_not_fire_on_state_container() {
    // A state-backed list is not a fresh unique alloc — never in-place.
    let code =
        "state xs = []\nfor i in range(0, 3) do\n  xs = append(xs, i)\nend\nprint(len(xs))";
    assert_eq!(inplace_count(code), 0, "state container must not be in-place");
}

#[test]
fn inplace_analysis_fires_on_accumulators() {
    // Guards against a refactor silently disabling the optimization: the
    // canonical accumulator shapes must be recognized.
    assert!(
        inplace_count("let xs = []\nfor i in range(0, 5) do\n  xs = append(xs, i)\nend\nlet n = len(xs)") >= 1,
        "simple accumulator should fire",
    );
    assert!(
        inplace_count(
            "let row = []\nfor x in range(0, 4) do\n  if x % 2 == 0 then\n    row = append(row, 1)\n  else\n    row = append(row, 0)\n  end\nend\nlet n = len(row)"
        ) >= 2,
        "branchy accumulator should fire on both arms",
    );
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
};

/// Graph == bytecode(no-opt) == bytecode(route A only) == bytecode(all).
#[track_caller]
fn assert_route_a_parity(code: &str) {
    assert_inplace_parity(code); // graph vs no-opt vs all
    let noopt = run_opt(code, OptFlags::none());
    let ra = run_opt(code, ROUTE_A_ONLY);
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
    let mut bc = crate::backend::bytecode::lower_program_opt(&program, &in_place)
        .expect("lower");
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
    assert!(route_a_count(code) >= 1, "per-iteration builder should fire");
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
    assert_eq!(route_a_count(code), 0, "stored-into-container must not fire");
    assert_route_a_parity(code);
    // Captured by a closure before the mutation.
    let code = "let xs = [1, 2]\nlet get = fn() -> xs[0]\nxs[0] = 9\nprint(get(), xs[0])";
    assert_eq!(route_a_count(code), 0, "closure-captured must not fire");
    assert_route_a_parity(code);
    // Passed to a user function before the mutation (arbitrary code could
    // retain it).
    let code = "fn probe(v)\n  v[0]\nend\nlet xs = [1, 2]\nlet a = probe(xs)\nxs[0] = 9\nprint(a, xs[0])";
    assert_eq!(route_a_count(code), 0, "call argument must not fire");
    assert_route_a_parity(code);
    // Returning a pre-mutation alias out of a function.
    let code = "fn f()\n  let xs = [1, 2]\n  let ys = xs\n  xs[0] = 5\n  ys\nend\nprint(f()[0])";
    assert_eq!(route_a_count(code), 0, "escaping pre-mutation alias must not fire");
    assert_route_a_parity(code);
}

#[test]
fn route_a_fires_on_returned_mutation_result() {
    // Returning the *rebound* (post-mutation) value is safe: only the final
    // container escapes, which is value-identical to the clone. The
    // pre-mutation id has no other observer.
    let code = "fn f(v)\n  let xs = [1, 2]\n  xs[0] = v\n  xs\nend\nprint(f(5)[0], f(7)[0])";
    assert!(route_a_count(code) >= 1, "returned mutation result should fire");
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
        env.set_backend(Backend::Bytecode);
        env.set_opt_flags(opts);
        env.run_source(code).expect("run");
        env.heap().dup_stats().total_bytes()
    };
    let off = bytes(OptFlags::none());
    let on = bytes(ROUTE_A_ONLY);
    assert!(off > 0, "baseline should copy something");
    assert!(on < off, "route A should strictly reduce copied bytes ({on} !< {off})");
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
        env.set_backend(Backend::Bytecode);
        env.set_opt_flags(opts);
        env.run_source(code).expect("run");
        env.heap().dup_stats().total_bytes()
    };
    let off = bytes(OptFlags::none());
    let on = bytes(OptFlags::all());
    assert!(off > 0, "baseline should copy something");
    assert!(on < off, "in-place should strictly reduce copied bytes ({on} !< {off})");
}
