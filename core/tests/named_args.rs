//! Named call arguments — `f(x, limit: 10)`. The frontend records the written
//! names in a vector parallel to `args`, left empty when every argument is
//! positional so nothing about an ordinary call changes.

use petal::ast::{Expr, ExprKind, Stmt, StmtKind};
use petal::ast_display::display_stmts;
use petal::lexer::Lexer;
use petal::parse::Parser;

fn try_parse(src: &str) -> Result<Vec<Stmt>, String> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize()?;
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    parser.parse_program()
}

fn parse(src: &str) -> Vec<Stmt> {
    try_parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"))
}

/// The error message produced by parsing `src`, which must fail.
fn parse_err(src: &str) -> String {
    match try_parse(src) {
        Ok(_) => panic!("expected a parse error for {src:?}, but it parsed"),
        Err(e) => e,
    }
}

/// Pull the single call out of a one-statement program.
fn sole_call(src: &str) -> (Vec<Expr>, Vec<Option<String>>) {
    let mut stmts = parse(src);
    assert_eq!(stmts.len(), 1, "expected one statement in {src:?}");
    match stmts.remove(0).kind {
        StmtKind::Expr(Expr {
            kind: ExprKind::Call {
                args, arg_names, ..
            },
            ..
        }) => (args, arg_names),
        other => panic!("expected a call expression, got {other:?}"),
    }
}

fn names(src: &str) -> Vec<Option<String>> {
    sole_call(src).1
}

#[test]
fn parses_named_arguments() {
    assert_eq!(
        names("f(1, b: 2)\n"),
        vec![None, Some("b".to_string())],
        "a positional argument then a named one"
    );
    assert_eq!(
        names("f(a: 1, b: 2)\n"),
        vec![Some("a".to_string()), Some("b".to_string())]
    );
    // A keyword is a legal argument name, exactly as it is a record key.
    assert_eq!(names("f(end: 1)\n"), vec![Some("end".to_string())]);
    assert_eq!(
        names("f(if: 1, then: 2)\n"),
        vec![Some("if".to_string()), Some("then".to_string())]
    );
    // Values are ordinary expressions, and each name pairs with its own value.
    let (args, arg_names) = sole_call("f(a: 1 + 2, b: {c: 3})\n");
    assert_eq!(args.len(), 2);
    assert_eq!(arg_names.len(), args.len());
}

/// The empty name vector is the fast path every later layer keys off, so a
/// fully positional call must not grow one.
#[test]
fn positional_calls_carry_no_names() {
    assert!(names("f(1, 2)\n").is_empty());
    assert!(names("f()\n").is_empty());
    assert!(names("f(g(x), h.i)\n").is_empty());
    // A `:` that is not an argument label (a type annotation, a record key,
    // an import list) must not be mistaken for one.
    assert!(names("f({a: 1}, [2])\n").is_empty());
}

#[test]
fn rejects_a_positional_argument_after_a_named_one() {
    let err = parse_err("f(a: 1, 2)\n");
    assert!(
        err.contains("positional argument after a named argument"),
        "unexpected message: {err}"
    );
    assert!(err.contains("line 1, column 9"), "no position in: {err}");
}

/// `a |> f(b: 2)` puts the piped value in the first slot *positionally*, so the
/// names stay aligned with the arguments they were written against.
#[test]
fn piping_into_a_named_call_shifts_the_names() {
    assert_eq!(names("x |> f(b: 2)\n"), vec![None, Some("b".to_string())]);
    assert!(names("x |> f(1)\n").is_empty());
}

#[test]
fn display_renders_named_arguments() {
    let out = display_stmts(&parse("f(1, b: 2)\n"));
    assert!(out.contains("Arg b:"), "no argument name in:\n{out}");
    // A positional call renders exactly as it always has: the argument
    // expression directly under the Call, with no label line.
    let positional = display_stmts(&parse("f(1, 2)\n"));
    assert!(
        !positional.contains("Arg "),
        "unexpected label:\n{positional}"
    );
}

/// The serde skip is what keeps the `show-ast --json` golden corpus
/// byte-identical: a positional call must serialize without the new field.
#[test]
fn positional_call_json_is_unchanged() {
    let json = serde_json::to_string(&parse("f(1, 2)\n")).expect("serialize");
    assert!(!json.contains("arg_names"), "field leaked into: {json}");
    let named = serde_json::to_string(&parse("f(a: 1)\n")).expect("serialize");
    assert!(named.contains("arg_names"), "field missing from: {named}");
}

// ---------------------------------------------------------------------------
// IR and bytecode
// ---------------------------------------------------------------------------
//
// Below the frontend the names ride on `Term.arg_names` and on the three call
// instructions, always parallel to the op's *argument* slice — never to the
// whole `inputs`, whose first entry is the callee or receiver.

use petal::env::Env;
use petal::program::{Program, TermOp};

/// Compile `src` and hand back its program, keeping the owning `Env` alive.
fn compile(src: &str) -> (Env, petal::program::ProgramId) {
    let mut env = Env::new();
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("compiles: {e}\n{src}"));
    (env, pid)
}

/// The argument names of the sole call term matching `pick`, resolved to
/// strings.
fn term_names(program: &Program, pick: impl Fn(&TermOp) -> bool) -> Vec<Option<String>> {
    let term = program
        .terms
        .iter()
        .find(|t| pick(&t.op))
        .expect("a call term");
    term.arg_names
        .iter()
        .map(|n| n.map(|c| program.get_string_constant(c).expect("string").to_string()))
        .collect()
}

const F: &str = "fn f(a, b)\n  a\nend\n";

#[test]
fn call_term_carries_the_argument_names() {
    let (env, pid) = compile(&format!("{F}f(1, b: 2)\n"));
    let program = env.get_program(pid).expect("program");
    assert_eq!(
        term_names(program, |op| matches!(op, TermOp::Call)),
        vec![None, Some("b".to_string())]
    );
}

/// A positional call carries nothing, and the whole IR document serializes
/// without the field — which is what keeps the golden corpus byte-identical.
#[test]
fn positional_calls_carry_nothing() {
    let (env, pid) = compile(&format!("{F}f(1, 2)\n"));
    let program = env.get_program(pid).expect("program");
    assert!(program.terms.iter().all(|t| t.arg_names.is_empty()));
    let json = serde_json::to_string(program).expect("serialize");
    assert!(!json.contains("arg_names"), "field leaked into the IR JSON");
}

/// A builtin call's names are parallel to *all* of its inputs (there is no
/// callee input to skip).
#[test]
fn builtin_call_names_start_at_input_zero() {
    let (env, pid) = compile("print(x: 1)\n");
    let program = env.get_program(pid).expect("program");
    assert_eq!(
        term_names(program, |op| matches!(op, TermOp::BuiltinCall(_))),
        vec![Some("x".to_string())]
    );
}

/// A method call's receiver is `inputs[0]` and is never named, so the names
/// line up with the arguments written after it.
#[test]
fn method_call_names_skip_the_receiver() {
    let (env, pid) = compile("obj.m(1, k: 2)\n");
    let program = env.get_program(pid).expect("program");
    assert_eq!(
        term_names(program, |op| matches!(op, TermOp::MethodCall { .. })),
        vec![None, Some("k".to_string())]
    );
}

/// `show-ir` prefixes a named input with the parameter it binds; a positional
/// call renders exactly as before.
#[test]
fn ir_display_shows_named_inputs() {
    let (env, pid) = compile(&format!("{F}f(1, b: 2)\n"));
    let out = petal::ir_display::display_program(env.get_program(pid).expect("program"));
    assert!(out.contains("b: t"), "no argument name in:\n{out}");
}

/// The IR is still valid with names attached, and the term-level check rejects
/// a list that does not match the argument count.
#[test]
fn validation_accepts_names_and_rejects_a_bad_length() {
    let (env, pid) = compile(&format!("{F}f(1, b: 2)\n"));
    let json = serde_json::to_string(env.get_program(pid).expect("program")).expect("serialize");
    // Round-tripping through the IR document is also the check that the names
    // survive serialization.
    let mut program = Program::from_json(&json).expect("named call validates");
    let idx = program
        .terms
        .iter()
        .position(|t| !t.arg_names.is_empty())
        .expect("a named call term");
    program.terms[idx].arg_names.pop();
    assert!(program.validate().is_err(), "short arg_names accepted");
}

/// The disassembly prefixes a named argument register.
#[test]
fn bytecode_carries_the_names() {
    let src = format!("{F}f(1, b: 2)\n");
    let text = petal::inspect::render(&src, petal::inspect::Stage::Bytecode).expect("lowers");
    assert!(text.contains("b: r"), "no argument name in:\n{text}");
}

// ---------------------------------------------------------------------------
// Runtime binding
// ---------------------------------------------------------------------------

/// Run a program and return its printed output.
fn run(src: &str) -> Result<String, String> {
    let mut env = Env::new();
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

fn out(src: &str) -> String {
    run(src).unwrap_or_else(|e| panic!("run failed for {src:?}: {e}"))
}

fn err(src: &str) -> String {
    match run(src) {
        Ok(o) => panic!("expected an error for {src:?}, got output {o:?}"),
        Err(e) => e,
    }
}

/// A subtracting `f` so a swapped binding is visible in the answer.
const SUB: &str = "fn sub(a, b)\n  a - b\nend\n";

#[test]
fn named_arguments_bind_by_name() {
    assert_eq!(out(&format!("{SUB}print(sub(b: 2, a: 10))")), "8");
    assert_eq!(out(&format!("{SUB}print(sub(a: 10, b: 2))")), "8");
}

#[test]
fn positional_and_named_can_mix() {
    assert_eq!(out(&format!("{SUB}print(sub(10, b: 2))")), "8");
}

#[test]
fn an_unnamed_call_is_unchanged() {
    assert_eq!(out(&format!("{SUB}print(sub(10, 2))")), "8");
}

#[test]
fn overloads_select_by_total_count_then_bind_by_name() {
    let src = "fn g(a)
  a
end
fn g(a, b)
  a - b
end
";
    assert_eq!(out(&format!("{src}print(g(a: 5))")), "5");
    assert_eq!(out(&format!("{src}print(g(b: 2, a: 5))")), "3");
}

#[test]
fn a_method_binds_named_arguments_after_the_receiver() {
    let src = "class Point
  x,
  y,
end
fn Point.shift(p, dx)
  p.x - dx
end
let p = Point(10, 0)
";
    assert_eq!(out(&format!("{src}print(p.shift(dx: 2))")), "8");
    // A class constructor is a function like any other.
    assert_eq!(out(&format!("{src}print(Point(y: 1, x: 7).x)")), "7");
}

#[test]
fn a_named_argument_cannot_rebind_the_receiver() {
    let src = "class Point
  x,
  y,
end
fn Point.shift(p, dx)
  p.x - dx
end
let p = Point(10, 0)
print(p.shift(p: 1))
";
    assert!(
        err(src).contains("Point.shift() got multiple values for parameter 'p'"),
        "unexpected error: {}",
        err(src)
    );
}

#[test]
fn a_lambda_binds_named_arguments_too() {
    let src = "let k = 3
let f = fn(a, b)
  (a - b) * k
end
print(f(b: 1, a: 5))
";
    assert_eq!(out(src), "12");
}

#[test]
fn recursion_through_named_arguments_still_terminates() {
    let src = "fn fact(n, acc)
  if n <= 1 then acc else fact(acc: acc * n, n: n - 1) end
end
print(fact(n: 5, acc: 1))
";
    assert_eq!(out(src), "120");
}

#[test]
fn an_unknown_parameter_name_is_reported() {
    let e = err(&format!("{SUB}print(sub(c: 1, a: 2))"));
    assert!(
        e.contains("sub() has no parameter named 'c'"),
        "unexpected error: {e}"
    );
}

/// An overload variant is `box#1` internally; the message names it as written.
#[test]
fn an_overloads_error_names_the_source_function() {
    let src = "fn box(w)
  box(w, w)
end
fn box(w, h)
  [w, h]
end
print(box(depth: 1))
";
    let e = err(src);
    assert!(
        e.contains("box() has no parameter named 'depth'") && !e.contains("box#"),
        "unexpected error: {e}"
    );
}

#[test]
fn a_slot_filled_twice_is_reported() {
    let e = err(&format!("{SUB}print(sub(1, a: 2))"));
    assert!(
        e.contains("sub() got multiple values for parameter 'a'"),
        "unexpected error: {e}"
    );
    let e = err(&format!("{SUB}print(sub(b: 1, b: 2))"));
    assert!(
        e.contains("sub() got multiple values for parameter 'b'"),
        "unexpected error: {e}"
    );
}

/// An unfilled slot cannot be reached from source: the arity check runs first,
/// so an over-filled slot always errors before any slot is left empty. The
/// binder still answers for it, since hand-written bytecode skips that check.
#[test]
fn an_unfilled_slot_is_reported() {
    use petal::backend::calls::bind_named_args;
    use petal::value::Value;

    let params = vec!["a".to_string(), "b".to_string()];
    let e =
        bind_named_args("sub", &params, 0, &[Value::Int(1)], &[Some("b")]).expect_err("a is unfilled");
    assert_eq!(e, "sub() is missing a value for parameter 'a'");
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------
//
// A native reads its arguments by index. It takes names once it declares its
// parameters (`NativeFnTable::declare_params`; the core builtins do, in
// `builtins/params.rs`): the names are permuted into positional order and the
// native runs exactly as the positional call would have run it. There are two
// places that happens — the compiler, for a direct call to an unshadowed
// builtin, and the VM, for every path where the callee is only known at run
// time — so each dispatch path gets its own test.

/// A builtin that declares no parameters (the variadic ones) keeps refusing.
#[test]
fn an_undeclared_builtin_refuses_named_arguments() {
    let e = err("print(1, sep: 2)");
    assert!(
        e.contains("builtin 'print' does not accept named arguments"),
        "unexpected error: {e}"
    );
}

#[test]
fn a_builtin_binds_named_arguments() {
    assert_eq!(out("print(clamp(value: 15, lo: 0, hi: 10))"), "10");
    assert_eq!(out("print(clamp(hi: 10, value: 15, lo: 0))"), "10");
    assert_eq!(out("print(clamp(15, hi: 10, lo: 0))"), "10");
    assert_eq!(out("print(append([1], value: 2))"), "[1, 2]");
    assert_eq!(out("print(pow(exp: 3, base: 2))"), "8.0");
    assert_eq!(out("print(join(separator: \"-\", list: [1, 2]))"), "1-2");
}

/// The direct call is put in positional order by the compiler, so the term —
/// and everything downstream of it — is the positional call's.
#[test]
fn a_direct_builtin_call_is_normalized_at_compile_time() {
    let (env, pid) = compile("print(pow(exp: 3, base: 2))\n");
    let program = env.get_program(pid).expect("program");
    let named = program
        .terms
        .iter()
        .filter(|t| !t.arg_names.is_empty())
        .count();
    assert_eq!(named, 0, "names survived on a call that binds");
    // A call whose names do not bind keeps them, for the VM (and `check`) to
    // report against.
    let (env, pid) = compile("pow(exp: 3, bass: 2)\n");
    let program = env.get_program(pid).expect("program");
    assert_eq!(
        term_names(program, |op| matches!(op, TermOp::BuiltinCall(_))),
        vec![Some("exp".to_string()), Some("bass".to_string())]
    );
}

/// Arguments are evaluated in the order they are written, whatever order the
/// names put them in.
#[test]
fn named_builtin_arguments_evaluate_in_written_order() {
    let src = "fn say(x)
  print(x)
  x
end
print(pow(exp: say(3), base: say(2)))";
    assert_eq!(out(src), "3\n2\n8.0");
}

/// An in-place-eligible builtin still sees its container in slot 0.
#[test]
fn a_named_mutating_builtin_keeps_value_semantics() {
    let src = "let xs = [1]
let ys = append(value: 2, list: xs)
let zs = append(value: 3, list: xs)
print(xs, ys, zs)";
    assert_eq!(out(src), "[1] [1, 2] [1, 3]");
    let src = "var xs = [1]
for i in range(3) do
  set xs = append(value: i, list: xs)
end
print(xs)";
    assert_eq!(out(src), "[1, 0, 1, 2]");
}

#[test]
fn trailing_optional_parameters_may_be_left_off() {
    assert_eq!(out("print(slice([1, 2, 3, 4], start: 1))"), "[2, 3, 4]");
    assert_eq!(out("print(slice([1, 2, 3, 4], start: 1, end: 3))"), "[2, 3]");
    assert_eq!(out("print(slice(end: 3, collection: [1, 2, 3, 4], start: 1))"), "[2, 3]");
    assert_eq!(out("print(round(x: 3.14159, places: 2))"), "3.14");
    assert_eq!(out("print(round(x: 2.6))"), "3.0");
}

/// `noise(x, y?, z?)`: `z` cannot be supplied past an unfilled `y`, since the
/// native has no value to read in between.
#[test]
fn a_hole_before_a_supplied_optional_is_reported() {
    let e = err("print(noise(x: 1, z: 2))");
    assert!(
        e.contains("noise() is missing a value for parameter 'y'"),
        "unexpected error: {e}"
    );
    let e = err("print(clamp(5, hi: 3))");
    assert!(
        e.contains("clamp() is missing a value for parameter 'lo'"),
        "unexpected error: {e}"
    );
}

#[test]
fn a_builtin_reports_an_unknown_or_repeated_name() {
    let e = err("print(clamp(5, low: 0, hi: 3))");
    assert!(
        e.contains("clamp() has no parameter named 'low'"),
        "unexpected error: {e}"
    );
    let e = err("print(clamp(5, value: 0, hi: 3))");
    assert!(
        e.contains("clamp() got multiple values for parameter 'value'"),
        "unexpected error: {e}"
    );
    let e = err("print(clamp(5, lo: 0, lo: 3))");
    assert!(
        e.contains("clamp() got multiple values for parameter 'lo'"),
        "unexpected error: {e}"
    );
}

/// A builtin that reads its arguments differently by count declares each form;
/// the names pick the form.
#[test]
fn a_builtin_with_several_forms_binds_the_one_the_names_fit() {
    assert_eq!(out("print(distance(x1: 0, y1: 0, x2: 3, y2: 4))"), "5.0");
    assert_eq!(out("print(distance(v2: vec2(3, 4), v1: vec2(0, 0)))"), "5.0");
    assert_eq!(out("print(mag(y: 4, x: 3))"), "5.0");
    assert_eq!(out("print(mag(v: vec2(3, 4)))"), "5.0");
    assert_eq!(out("print(range(end: 3))"), "[0, 1, 2]");
    assert_eq!(out("print(range(start: 1, end: 3))"), "[1, 2]");
    assert_eq!(out("print(range(step: 2, start: 0, end: 5))"), "[0, 2, 4]");
    assert_eq!(out("print(random(max: 1) < 1, random(min: 5, max: 6) >= 5)"), "true true");
    let e = err("print(random(lo: 1, hi: 2))");
    assert!(
        e.contains("random() has no parameter named 'lo'"),
        "unexpected error: {e}"
    );
}

/// The higher-order builtins are VM intrinsics; names reach them the same way.
#[test]
fn an_intrinsic_binds_named_arguments() {
    assert_eq!(out("print(map(f: fn(x) x * 2 end, list: [1, 2]))"), "[2, 4]");
    assert_eq!(
        out("print(reduce([1, 2, 3], f: fn(a, b) a + b end, initial: 10))"),
        "16"
    );
    assert_eq!(
        out("print(sort_by([3, 1, 2], key: fn(x) x end, descending: true))"),
        "[3, 2, 1]"
    );
    assert_eq!(out("print(sort(compare: fn(a, b) b - a end, list: [1, 3, 2]))"), "[3, 2, 1]");
}

/// A native held in a value is only known at run time, so the VM binds it.
#[test]
fn a_native_value_binds_named_arguments() {
    assert_eq!(out("let c = clamp\nprint(c(hi: 10, value: 15, lo: 0))"), "10");
    assert_eq!(
        out("fn apply(f)\n  f(exp: 3, base: 2)\nend\nprint(apply(pow))"),
        "8.0"
    );
    let e = err("let c = clamp\nprint(c(5, low: 0, hi: 3))");
    assert!(
        e.contains("clamp() has no parameter named 'low'"),
        "unexpected error: {e}"
    );
    let e = err("let p = print\np(1, sep: 2)");
    assert!(
        e.contains("builtin 'print' does not accept named arguments"),
        "unexpected error: {e}"
    );
    // …and one held in a record field, called with method syntax.
    assert_eq!(
        out("let m = {c: clamp}\nprint(m.c(hi: 10, value: 15, lo: 0))"),
        "10"
    );
}

/// `xs.slice(start: 1, end: 3)`: the receiver is the builtin's first
/// parameter, and the names bind against the rest.
#[test]
fn method_syntax_on_a_builtin_binds_named_arguments() {
    assert_eq!(out("print([1, 2, 3, 4].slice(start: 1, end: 3))"), "[2, 3]");
    assert_eq!(out("print([1, 2, 3, 4].slice(end: 3, start: 1))"), "[2, 3]");
    assert_eq!(out("print([1, 2, 3, 4].slice(start: 2))"), "[3, 4]");
    assert_eq!(out("print(\"a,b\".split(separator: \",\"))"), "[\"a\", \"b\"]");
    assert_eq!(out("print([1, 2].map(f: fn(x) x + 1 end))"), "[2, 3]");
    // Naming the receiver's own parameter is the double-bind error, as it is
    // for a Petal method.
    let e = err("print([1, 2, 3].slice(collection: [9], start: 1))");
    assert!(
        e.contains("slice() got multiple values for parameter 'collection'"),
        "unexpected error: {e}"
    );
    let e = err("print([1, 2, 3].slice(from: 1))");
    assert!(
        e.contains("slice() has no parameter named 'from'"),
        "unexpected error: {e}"
    );
    let e = err("[1].print(sep: 2)");
    assert!(
        e.contains("builtin 'print' does not accept named arguments"),
        "unexpected error: {e}"
    );
}

/// A built-in class: the constructor, and its methods through both dispatch
/// routes — pinned statically when the receiver's class is known, by the
/// receiver's tag when it is not.
#[test]
fn a_builtin_class_binds_named_arguments() {
    assert_eq!(out("let r = Rect(w: 4, h: 2, x: 1, y: 1)\nprint(r.w, r.x)"), "4 1");
    assert_eq!(
        out("let r = Rect(0, 0, 10, 10)\nlet m = r.offset(dy: 2, dx: 1)\nprint(m.x, m.y)"),
        "1 2"
    );
    assert_eq!(out("print(Rect(0, 0, 10, 10).inset(n: 2).w)"), "6");
    // Through a parameter the checker cannot type: dispatched on the tag.
    let src = "fn shift(r)
  r.offset(dy: 2, dx: 1)
end
let m = shift(Rect(0, 0, 10, 10))
print(m.x, m.y)";
    assert_eq!(out(src), "1 2");
    let e = err("fn shift(r)\n  r.offset(dz: 2, dx: 1)\nend\nshift(Rect(0, 0, 1, 1))");
    assert!(
        e.contains("Rect.offset() has no parameter named 'dz'"),
        "unexpected error: {e}"
    );
    let e = err("fn shift(r)\n  r.offset(r: 2, dx: 1)\nend\nshift(Rect(0, 0, 1, 1))");
    assert!(
        e.contains("Rect.offset() got multiple values for parameter 'r'"),
        "unexpected error: {e}"
    );
}

/// `x |> f(b: 2)`: the piped value is the first positional argument.
#[test]
fn a_pipe_into_a_builtin_binds_named_arguments() {
    assert_eq!(out("print(15 |> clamp(hi: 10, lo: 0))"), "10");
    assert_eq!(out("print([1, 2, 3, 4] |> slice(end: 3, start: 1))"), "[2, 3]");
    assert_eq!(out("let c = clamp\nprint(15 |> c(hi: 10, lo: 0))"), "10");
    let e = err("print(15 |> clamp(value: 1, lo: 0))");
    assert!(
        e.contains("clamp() got multiple values for parameter 'value'"),
        "unexpected error: {e}"
    );
}

/// A user function shadowing a builtin is a Petal `fn`: its own parameter
/// names apply, not the builtin's.
#[test]
fn a_shadowing_fn_binds_its_own_names() {
    let src = "fn clamp(n, floor, top)
  n - floor - top
end
print(clamp(top: 1, floor: 2, n: 10))";
    assert_eq!(out(src), "7");
}

/// The embedder's side: a host native takes names once it declares them, and
/// is handed the positional list — it never sees a name.
#[test]
fn a_host_native_binds_named_arguments_once_declared() {
    use petal::native_fn::{NativeEffects, PetalCxt};

    fn native_sub(cxt: &mut PetalCxt) -> Result<u32, String> {
        let mut v = cxt.get_int(1)? - cxt.get_int(2)?;
        if cxt.arg_count() == 3 {
            v += cxt.get_int(3)?;
        }
        cxt.push_int(v);
        Ok(1)
    }
    let run_with = |declare: bool, src: &str| -> Result<String, String> {
        let mut env = Env::new();
        let id = env.register_native("hsub", native_sub, NativeEffects::PURE);
        if declare {
            env.declare_native_params(id, "a, b, extra?")?;
        }
        let pid = env.load_program(src)?;
        let sid = env.create_stack(pid)?;
        env.run(sid)?;
        Ok(env.take_output().join("\n").trim().to_string())
    };
    assert_eq!(run_with(true, "print(hsub(b: 1, a: 10))").unwrap(), "9");
    assert_eq!(run_with(true, "print(hsub(10, extra: 5, b: 1))").unwrap(), "14");
    assert_eq!(run_with(true, "let h = hsub\nprint(h(b: 1, a: 10))").unwrap(), "9");
    assert_eq!(run_with(true, "print(10.hsub(b: 1))").unwrap(), "9");
    let e = run_with(true, "print(hsub(a: 1, extra: 2))").unwrap_err();
    assert!(e.contains("hsub() is missing a value for parameter 'b'"), "{e}");
    let e = run_with(false, "print(hsub(b: 1, a: 10))").unwrap_err();
    assert!(
        e.contains("builtin 'hsub' does not accept named arguments"),
        "{e}"
    );
    // The by-name form, and what a bad declaration says.
    let mut env = Env::new();
    env.register_native("hsub", native_sub, NativeEffects::PURE);
    assert!(env.declare_native_params_by_name("hsub", "a, b").is_ok());
    assert!(env.declare_native_params_by_name("nope", "a").is_err());
    assert!(env.declare_native_params_by_name("hsub", "a?, b").is_err());
    assert!(env.declare_native_params_by_name("hsub", "a, a").is_err());
    assert!(env.declare_native_params_by_name("hsub", "a, 1b").is_err());
}

/// A native that dispatches on argument count declares one form per shape; a
/// named call takes the first form it fits.
#[test]
fn a_host_native_may_declare_several_forms() {
    use petal::native_fn::{NativeEffects, PetalCxt};

    fn native_area(cxt: &mut PetalCxt) -> Result<u32, String> {
        let v = match cxt.arg_count() {
            1 => cxt.get_int(1)? * cxt.get_int(1)?,
            _ => cxt.get_int(1)? * cxt.get_int(2)?,
        };
        cxt.push_int(v);
        Ok(1)
    }
    let mut env = Env::new();
    let id = env.register_native("area", native_area, NativeEffects::PURE);
    env.declare_native_params(id, "side").unwrap();
    env.declare_native_params(id, "w, h").unwrap();
    let pid = env
        .load_program("print(area(side: 3), area(h: 2, w: 5))")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output().join("\n").trim(), "9 10");
}

/// Default values and native parameter names meet: a default may be a named
/// builtin call, a builtin's optional trailing parameter is left off the way
/// a defaulted one is, and a fn that shadows a builtin brings its own
/// defaults with it.
#[test]
fn defaults_and_builtin_names_work_together() {
    let src = "fn unit(v, lo = 0, hi = clamp(value: lo + 1, lo: 1, hi: 10))
  clamp(v, hi: hi, lo: lo)
end
print(unit(5), unit(5, hi: 3), unit(-2, lo: -1))";
    assert_eq!(out(src), "1 3 -1");
    let src = "fn head(xs, n = len(xs))
  slice(xs, start: 0, end: n)
end
print(head([1, 2, 3]), head([1, 2, 3], n: 1), slice([1, 2, 3], start: 1))";
    assert_eq!(out(src), "[1, 2, 3] [1] [2, 3]");
    let src = "fn round(x, places = 1)
  [x, places]
end
print(round(x: 2), round(2, places: 3))";
    assert_eq!(out(src), "[2, 1] [2, 3]");
}

/// A builtin with several call forms and an overloaded `fn` report a call
/// that fits none of them in the same words.
#[test]
fn builtin_forms_and_fn_overloads_report_alike() {
    // A form (variant) of exactly the call's length: its own complaint.
    let e = err("print(random(min: 1))");
    assert!(e.contains("random() has no parameter named 'min'"), "{e}");
    let e = err("fn pick(max) max end\nfn pick(min, max) min end\nprint(pick(min: 1))");
    assert!(e.contains("pick() has no parameter named 'min'"), "{e}");
    // None of that length, but the count fits one: the forms are listed.
    let e = err("print(range(start: 1, stop: 5))");
    assert!(
        e.contains(
            "range() has no variant that accepts 2 arguments with some named 'start', 'stop' \
             (variants: range(end), range(start, end, step?))"
        ),
        "{e}"
    );
    let e = err("fn span(hi) hi end
fn span(lo, hi, step = 1) hi - lo end
print(span(lo: 1, top: 5))");
    assert!(
        e.contains(
            "span() has no variant that accepts 2 arguments with some named 'lo', 'top' \
             (variants: span(hi), span(lo, hi, step = …))"
        ),
        "{e}"
    );
    // Too many for an optional trailing parameter: a range, spelt one way.
    let e = err("print(round(1.5, 2, places: 3))");
    assert!(e.contains("round() expects 1-2 arguments, got 3"), "{e}");
    let e = err("fn near(x, places = 0) x end\nprint(near(1.5, 2, places: 3))");
    assert!(e.contains("near() expects 1-2 arguments, got 3"), "{e}");
}
