//! Spread call arguments — `f(...args)`. A list supplies arguments by
//! position, a record by parameter name; the frontend records the spread as
//! the call's last argument under the name `ast::SPREAD_ARG`, and the VM
//! decides what it binds when the value is in hand.

use petal::ast::{Expr, ExprKind, SPREAD_ARG, Stmt, StmtKind, is_spread_call};
use petal::env::Env;
use petal::lexer::Lexer;
use petal::parse::Parser;

fn try_parse(src: &str) -> Result<Vec<Stmt>, String> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize()?;
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    parser.parse_program()
}

fn names(src: &str) -> Vec<Option<String>> {
    let mut stmts = try_parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"));
    match stmts.remove(0).kind {
        StmtKind::Expr(Expr {
            kind: ExprKind::Call { arg_names, .. },
            ..
        }) => arg_names,
        other => panic!("expected a call expression, got {other:?}"),
    }
}

fn parse_err(src: &str) -> String {
    match try_parse(src) {
        Ok(_) => panic!("expected a parse error for {src:?}"),
        Err(e) => e,
    }
}

fn run_modules(modules: &[(&str, &str)], src: &str) -> Result<String, String> {
    let mut env = Env::new();
    for (name, source) in modules {
        env.register_module(name, source);
    }
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

fn out(src: &str) -> String {
    run_modules(&[], src).unwrap_or_else(|e| panic!("run failed for {src:?}: {e}"))
}

fn err(src: &str) -> String {
    match run_modules(&[], src) {
        Ok(o) => panic!("expected an error for {src:?}, got output {o:?}"),
        Err(e) => e,
    }
}

const TOLL: &str = "fn toll(id: string, cents: int = 5, note = \"none\")
  \"{id} {cents} {note}\"
end
";

fn toll(call: &str) -> String {
    out(&format!("{TOLL}print({call})"))
}

// ── Syntax ─────────────────────────────────────────────────────────────────

#[test]
fn a_spread_is_the_last_argument_under_the_marker_name() {
    let spread = Some(SPREAD_ARG.to_string());
    assert_eq!(names("f(...args)\n"), vec![spread.clone()]);
    assert_eq!(names("f(1, 2, ...rest)\n"), vec![None, None, spread.clone()]);
    assert_eq!(names("x |> f(...rest)\n"), vec![None, spread.clone()]);
    assert_eq!(names("f(...{a: 1})\n"), vec![spread]);
    assert!(is_spread_call(&names("f(...args)\n")));
    assert!(!is_spread_call(&names("f(a: 1)\n")));
    assert!(!is_spread_call(&names("f(1)\n")));
}

#[test]
fn nothing_follows_a_spread_and_a_spread_follows_no_name() {
    let e = parse_err("f(...a, 1)\n");
    assert!(e.contains("must be the last argument"), "{e}");
    let e = parse_err("f(...a, ...b)\n");
    assert!(e.contains("must be the last argument"), "{e}");
    let e = parse_err("f(...a, b: 1)\n");
    assert!(e.contains("must be the last argument"), "{e}");
    let e = parse_err("f(a: 1, ...b)\n");
    assert!(e.contains("A spread argument after a named argument"), "{e}");
}

// ── A list: by position ────────────────────────────────────────────────────

#[test]
fn a_list_supplies_arguments_by_position() {
    assert_eq!(toll("toll(...[\"a\", 9, \"n\"])"), "a 9 n");
    assert_eq!(toll("toll(...[\"a\", 9])"), "a 9 none");
    assert_eq!(toll("toll(...[\"a\"])"), "a 5 none");
    assert_eq!(toll("toll(\"a\", ...[9])"), "a 9 none");
    assert_eq!(toll("toll(\"a\", 9, ...[])"), "a 9 none");
    assert_eq!(out("let xs = [3, 4]\nlet add = fn(a, b) -> a + b\nprint(add(...xs))"), "7");
}

#[test]
fn a_list_of_the_wrong_length_is_the_arity_error_of_the_written_call() {
    assert_eq!(
        err("fn f(a) a end\nf(...[1, 2])").lines().next().unwrap(),
        err("fn f(a) a end\nf(1, 2)").lines().next().unwrap().replace("f(1, 2)", "f(...[1, 2])")
    );
    let e = err(&format!("{TOLL}toll(...[])"));
    assert!(e.contains("toll() is missing a value for parameter 'id'"), "{e}");
    let e = err(&format!("{TOLL}toll(...[1, 2, 3, 4])"));
    assert!(e.contains("toll() expects 1-3 arguments, got 4"), "{e}");
}

// ── A record: by parameter name ────────────────────────────────────────────

#[test]
fn a_record_supplies_arguments_by_parameter_name() {
    assert_eq!(toll("toll(...{id: \"a\", cents: 7})"), "a 7 none");
    assert_eq!(toll("toll(...{note: \"x\", id: \"a\"})"), "a 5 x");
    assert_eq!(toll("toll(\"a\", ...{note: \"x\"})"), "a 5 x");
    assert_eq!(toll("toll(\"a\", 1, \"n\", ...{})"), "a 1 n");
    // The record is any expression.
    let src = format!(
        "{TOLL}let base = {{id: \"b\"}}\nprint(toll(...{{...base, cents: 2}}))"
    );
    assert_eq!(out(&src), "b 2 none");
}

#[test]
fn defaults_are_the_callees_and_see_the_spread_values() {
    let src = "fn rect(w, h = w * 2)
  [w, h]
end
print(rect(...{w: 3}))
print(rect(...{h: 1, w: 3}))
print(rect(...[4]))";
    assert_eq!(out(src), "[3, 6]\n[3, 1]\n[4, 8]");
}

#[test]
fn an_unknown_name_is_named() {
    let e = err(&format!("{TOLL}toll(...{{id: \"a\", sents: 7}})"));
    assert!(e.contains("toll() has no parameter named 'sents'"), "{e}");
}

#[test]
fn a_missing_required_parameter_is_named() {
    let e = err(&format!("{TOLL}toll(...{{cents: 7}})"));
    assert!(e.contains("toll() is missing a value for parameter 'id'"), "{e}");
    // Also where the function has no defaults, and the count alone is wrong.
    let e = err("fn f(a, b) a end\nf(...{a: 1})");
    assert!(e.contains("f() is missing a value for parameter 'b'"), "{e}");
}

#[test]
fn a_parameter_filled_twice_is_named() {
    let e = err(&format!("{TOLL}toll(\"a\", ...{{id: \"b\"}})"));
    assert!(e.contains("toll() got multiple values for parameter 'id'"), "{e}");
}

#[test]
fn too_many_values_is_the_arity_error() {
    let e = err("fn f(a) a end\nf(1, ...{a: 2})");
    assert!(e.contains("f() expects 1 argument, got 2"), "{e}");
}

// ── What is spread, and what is called ─────────────────────────────────────

#[test]
fn anything_else_is_refused() {
    for (value, ty) in [("5", "int"), ("\"ab\"", "string"), ("nil", "nil"), ("true", "bool")] {
        let e = err(&format!("fn f(a) a end\nf(...{value})"));
        assert!(
            e.contains("must be a list (arguments by position) or a record")
                && e.contains(&format!("got {ty}")),
            "{e}"
        );
    }
    let e = err("let x = 5\nx(...[1])");
    assert!(e.contains("Cannot call int"), "{e}");
}

#[test]
fn closures_and_function_values_take_a_spread() {
    let src = "fn make(k)
  fn(a, b = 10) -> a * k + b
end
let f = make(2)
print(f(...[1, 2]))
print(f(...{a: 4}))
let table = {double: fn(x) -> x * 2}
print(table.double(...[21]))
print(table.double(...{x: 4}))
print([[1, 2], [3, 4]].map(fn(pair) -> f(...pair)))";
    assert_eq!(out(src), "4\n18\n42\n8\n[4, 10]");
}

#[test]
fn an_overload_set_picks_its_variant_from_the_spread() {
    let src = "fn area(side) side * side end
fn area(w, h) w * h end
print(area(...[3]))
print(area(...[3, 4]))
print(area(...{w: 2, h: 5}))
print(area(...{side: 6}))";
    assert_eq!(out(src), "9\n12\n10\n36");
    let e = err("fn area(side) side * side end
fn area(w, h) w * h end
area(...{w: 2, depth: 5})");
    assert!(e.contains("area()"), "{e}");
}

#[test]
fn builtins_bind_by_their_declared_names() {
    assert_eq!(out("print(max(...[3, 9]))"), "9");
    assert_eq!(out("print(clamp(...{value: 15, lo: 0, hi: 10}))"), "10");
    assert_eq!(out("print(clamp(15, ...{hi: 10, lo: 0}))"), "10");
    assert_eq!(out("print(slice([1, 2, 3, 4], ...{start: 1}))"), "[2, 3, 4]");
    assert_eq!(out("print([1, 2, 3, 4] |> slice(...{start: 1, end: 3}))"), "[2, 3]");
    assert_eq!(out("print(...[\"a\", \"b\"])"), "a b");
    // The builtin held in a value, and the higher-order ones.
    assert_eq!(out("let m = min\nprint(m(...[4, 2]))"), "2");
    assert_eq!(out("print(map(...[[1, 2], fn(x) -> x + 1]))"), "[2, 3]");
    assert_eq!(out("print(map(...{f: fn(x) -> x + 1, list: [1, 2]}))"), "[2, 3]");
    assert_eq!(out("import creative\nprint(lerp(...{a: 0, b: 10, t: 0.5}))"), "5.0");
}

#[test]
fn a_builtins_refusals_are_its_own() {
    let e = err("clamp(...{value: 1, low: 0, hi: 2})");
    assert!(e.contains("clamp() has no parameter named 'low'"), "{e}");
    let e = err("print(...{a: 1})");
    assert!(e.contains("builtin 'print' does not accept named arguments"), "{e}");
    let e = err("max(...[1, 2, 3])");
    assert!(e.contains("max() expects 2 arguments, got 3"), "{e}");
}

#[test]
fn a_modules_function_is_called_through_its_qualifier() {
    let lib = "pub fn scale(v, by = 2)\n  v * by\nend\n";
    let src = "import lib\nprint(lib.scale(...[4]))\nprint(lib.scale(...{by: 3, v: 4}))";
    assert_eq!(run_modules(&[("lib", lib)], src).unwrap(), "8\n12");
    let src = "import lib: scale\nprint(scale(...{v: 5}))";
    assert_eq!(run_modules(&[("lib", lib)], src).unwrap(), "10");
}

#[test]
fn methods_take_the_receiver_first() {
    let src = "class Rect
  w: num,
  h: num
end
fn Rect.grow(r: Rect, dw, dh = 0)
  Rect(r.w + dw, r.h + dh)
end
let r = Rect(1, 2)
let g = r.grow(...{dh: 5, dw: 1})
print(g.w, g.h)
let g2 = r.grow(...[1])
print(g2.w, g2.h)
let made = Rect(...{h: 4, w: 3})
print(made.w, made.h)
print([1, 2, 3, 4].slice(...{start: 1, end: 3}))
print([1, 2, 3, 4].slice(...[2]))
print(\"a,b\".split(...[\",\"]))";
    assert_eq!(out(src), "2 7\n2 2\n3 4\n[2, 3]\n[3, 4]\n[\"a\", \"b\"]");
    let e = err("[1, 2].slice(...{from: 1})");
    assert!(e.contains("slice() has no parameter named 'from'"), "{e}");
    let e = err("[1, 2].slice(...3)");
    assert!(e.contains("must be a list"), "{e}");
}

#[test]
fn arguments_are_evaluated_once_in_written_order() {
    let src = "fn say(x)
  print(x)
  x
end
fn f(a, b, c) [a, b, c] end
print(f(say(1), ...say([2, 3])))
print(say(f)(...say({c: 3, b: 2, a: 1})))";
    assert_eq!(out(src).lines().count(), 6);
    assert!(out(src).starts_with("1\n[2, 3]\n[1, 2, 3]\n"));
    assert!(out(src).ends_with("[1, 2, 3]"));
}

/// A spread call is a call: its callee's `state` has one slot per call site,
/// recursion through it is bounded like any other, and it runs in the frame
/// loop rather than nested on the native stack.
#[test]
fn a_spread_call_is_an_ordinary_call() {
    let src = "fn counter(step)
  state var n = 0
  set n = n + step
  n
end
print(counter(...[1]))
print(counter(...{step: 10}))";
    assert_eq!(out(src), "1\n10");
    let src = "fn down(n, acc = 0)
  if n == 0 then acc else down(...{n: n - 1, acc: acc + n}) end
end
print(down(...[2000]))";
    assert_eq!(out(src), "2001000");
    let e = err("fn forever(n) forever(...[n + 1]) end\nforever(0)");
    assert!(e.to_lowercase().contains("stack") || e.contains("depth"), "{e}");
}

/// The debrief's case: a tool is one declaration, and a request (a record of
/// arguments, as a JSON tool call carries them) is checked against
/// `fn_info` and dispatched with a spread.
#[test]
fn a_tool_table_dispatches_requests_with_fn_info_and_a_spread() {
    let src = "fn toll(id: string, cents: int = 5)
  \"toll {id} = {cents}\"
end
fn close(id: string)
  \"closed {id}\"
end
let tools = {toll: toll, close: close}
fn call_tool(name, args)
  let f = tools[name] ?? nil
  if f == nil then return \"no tool {name}\" end
  let known = fn_info(f).params.map(fn(p) -> p.name)
  for k in keys(args) do
    if !contains(known, k) then return \"{name}: no parameter {k}\" end
  end
  f(...args)
end
print(call_tool(\"toll\", {id: \"bridge\", cents: 25}))
print(call_tool(\"toll\", {id: \"bridge\"}))
print(call_tool(\"close\", {id: \"tunnel\"}))
print(call_tool(\"close\", {idd: \"tunnel\"}))
print(call_tool(\"open\", {}))";
    assert_eq!(
        out(src),
        "toll bridge = 25\ntoll bridge = 5\nclosed tunnel\nclose: no parameter idd\nno tool open"
    );
}

/// `petal check` has nothing to say about a spread call that runs, and the
/// call survives a hot reload.
#[test]
fn a_spread_call_checks_clean_and_reloads() {
    let mut env = Env::new();
    let src = format!("{TOLL}let args = {{id: \"a\"}}\nprint(toll(...args))\nprint(max(...[1, 2]))\n");
    let pid = env.load_program(&src).expect("load");
    assert!(
        env.diagnostics(pid).is_empty(),
        "{}",
        env.render_diagnostics(pid, env.diagnostics(pid))
    );
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).expect("run");
    assert_eq!(env.take_output(), vec!["a 5 none", "2"]);
}
