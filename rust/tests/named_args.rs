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
        bind_named_args("sub", &params, &[Value::Int(1)], &[Some("b")]).expect_err("a is unfilled");
    assert_eq!(e, "sub() is missing a value for parameter 'a'");
}

#[test]
fn a_builtin_refuses_named_arguments() {
    let e = err("print(append([1], x: 2))");
    assert!(
        e.contains("builtin 'append' does not accept named arguments"),
        "unexpected error: {e}"
    );
}
