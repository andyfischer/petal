//! Indentation delimits blocks, and `end` is an optional closer
//! (docs/tasks/pythonic-syntax.md §3, §4, §5.1).
//!
//! One group of fixtures per row of the plan's ambiguity table (B1 … B15),
//! valid and error; every construct of §4 with its `end`, without it, and
//! mixed; and the misindented legacy shapes of §5.1, which must fail rather
//! than parse to a different program.
//!
//! The comparison is on the AST with source spans dropped
//! ([`petal::rewrite::ast_shape`]): source written with `end`s and the same
//! source with them removed are one program. Every parse goes through
//! [`petal::cst::parse_source`], whose debug build also checks that the AST
//! projected from the syntax tree is the parser's own, and the tree must
//! reproduce the source byte for byte.

use petal::env::Env;
use petal::source_map::ENTRY_FILE;

fn try_shape(src: &str) -> Result<String, String> {
    let (tree, stmts) = petal::cst::parse_source(src, ENTRY_FILE)?;
    assert_eq!(tree.text(), src, "the syntax tree lost text");
    Ok(petal::rewrite::ast_shape(&stmts)?.to_string())
}

fn shape(src: &str) -> String {
    try_shape(src).unwrap_or_else(|e| panic!("parse failed: {e}\n--- source ---\n{src}"))
}

/// Every source parses, and to the same program as the first.
#[track_caller]
fn same(sources: &[&str]) {
    let first = shape(sources[0]);
    for src in &sources[1..] {
        assert_eq!(shape(src), first, "\n--- differs from the first ---\n{src}");
    }
}

/// The two parse, to different programs.
#[track_caller]
fn differ(a: &str, b: &str) {
    assert_ne!(shape(a), shape(b), "\n--- a ---\n{a}\n--- b ---\n{b}");
}

#[track_caller]
fn err(src: &str) -> String {
    match try_shape(src) {
        Ok(_) => panic!("expected a parse error, but it parsed:\n{src}"),
        Err(e) => e,
    }
}

/// `src` fails with a message that holds every one of `needles`.
#[track_caller]
fn err_has(src: &str, needles: &[&str]) {
    let e = err(src);
    for needle in needles {
        assert!(e.contains(needle), "expected {needle:?} in the error: {e}\n--- source ---\n{src}");
    }
}

fn run(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_or_else(|e| panic!("{e}\n{src}"));
    env.take_output()
}

const MISSING_BLOCK: &str = "expected an indented block after";
const UNEXPECTED_INDENT: &str = "unexpected indent";
const PARTIAL_DEDENT: &str = "this line is indented less than its block, but more than the enclosing one";
const NOT_LINED_UP: &str = "does not line up with";

// ── The summary's example ────────────────────────────────────────────────

#[test]
fn the_plans_example_is_the_program_it_was_with_its_ends() {
    let with_ends = "\
fn clamp_all(xs, lo, hi)
  for x in xs do
    if x < lo then lo
    else if x > hi then hi
    else x
    end
  end
end
print(clamp_all([1, 5, 9], 2, 8))
";
    let without = "\
fn clamp_all(xs, lo, hi)
  for x in xs do
    if x < lo then lo
    else if x > hi then hi
    else x
print(clamp_all([1, 5, 9], 2, 8))
";
    same(&[with_ends, without]);
    assert_eq!(run(without), run(with_ends));
    assert_eq!(run(without), ["[2, 5, 8]"]);
}

// ── B1: how far a one-line body extends ──────────────────────────────────

#[test]
fn b1_a_one_line_body_extends_as_far_as_the_expression() {
    same(&[
        "let x = if c then 1 else 2 + 3 end\n",
        "let x = if c then 1 else 2 + 3\n",
        "let x = if c then 1 else (2 + 3)\n",
    ]);
    // With its `end`, the `if` is an operand, as it always was.
    differ("let x = if c then 1 else 2 end + 3\n", "let x = if c then 1 else 2 + 3\n");
    assert_eq!(run("let c = true\nprint(if c then 1 else 2 + 3)\nprint(if c then 1 else 2 end + 3)\n"), ["1", "4"]);
    // The body ends with its line.
    same(&[
        "let x = if c then 1 else 2\nprint(x)\n",
        "let x = if c then 1 else 2 end\nprint(x)\n",
    ]);
    same(&["for x in xs do print(x)\nprint(0)\n", "for x in xs do print(x) end\nprint(0)\n"]);
    same(&["while n < 3 do n += 1\nprint(n)\n", "while n < 3 do n += 1 end\nprint(n)\n"]);
    // A closing bracket or a separator of an enclosing construct ends it.
    same(&["f(if c then 1 else 2, 3)\n", "f(if c then 1 else 2 end, 3)\n"]);
    same(&["let xs = [for i in r do i * i]\n", "let xs = [for i in r do i * i end]\n"]);
    same(&["let r = {a: if c then 1 else 2, b: 3}\n", "let r = {a: if c then 1 else 2 end, b: 3}\n"]);
    // …and so does the end of a string's hole.
    same(&["let s = \"{if c then 1 else 2}\"\n", "let s = \"{if c then 1 else 2 end}\"\n"]);
    same(&[
        "let s = \"a {if c then \"yes\" else \"no\"} b {for x in xs do x} c\"\n",
        "let s = \"a {if c then \"yes\" else \"no\" end} b {for x in xs do x end} c\"\n",
    ]);
    assert_eq!(run("let c = false\nprint(\"a {if c then \"yes\" else \"no\"} b\")\n"), ["a no b"]);
}

// ── B2: dangling else ────────────────────────────────────────────────────

#[test]
fn b2_an_else_binds_to_the_nearest_if() {
    same(&[
        "let x = if a then if b then 1 else 2\n",
        "let x = if a then if b then 1 else 2 end end\n",
    ]);
    differ(
        "let x = if a then if b then 1 else 2\n",
        "let x = if a then if b then 1 end else 2 end\n",
    );
    // With an `end`, it closes the innermost open block.
    same(&[
        "let x = if a then if b then 1 end else 2\n",
        "let x = if a then if b then 1 end else 2 end\n",
    ]);
    same(&[
        "let v2 = if v > A then A else if v < B then B else v\n",
        "let v2 = if v > A then A else if v < B then B else v end\n",
    ]);
}

// ── B3: `;` after a one-line body with no `end` ──────────────────────────

#[test]
fn b3_a_semicolon_after_an_endless_one_liner_is_an_error() {
    let ambiguous = "is ambiguous; close it with `end` or use an indented block";
    err_has("if c then a(); b()\n", &["`;` after a one-line `then`", ambiguous, "[line 1, column 14]"]);
    err_has("if c then a() else b(); d()\n", &["`;` after a one-line `else`", ambiguous]);
    err_has("for x in xs do a(x); b(x)\n", &["`;` after a one-line `do`", ambiguous]);
    err_has("while c do a(); b()\n", &["`;` after a one-line `do`", ambiguous]);
    err_has("fn f() a(); b()\n", &["`;` after a one-line `fn` body", ambiguous]);
    err_has("let g = fn() a(); b()\n", &[ambiguous]);
    err_has("match v when 1 -> a(); b()\n", &["`;` after a one-line `->`", ambiguous]);
    // Closed by an `end`, it is today's form.
    same(&["if c then a(); b() end\n", "if c then\n  a()\n  b()\nend\n", "if c then\n  a()\n  b()\n"]);
    same(&["for x in xs do a(x); b(x) end\n", "for x in xs do\n  a(x)\n  b(x)\n"]);
    same(&["fn f() a(); b() end\n", "fn f()\n  a()\n  b()\n"]);
    same(&["match v when 1 -> a(); b() end\n", "match v\n  when 1 ->\n    a()\n    b()\n"]);
    // The `end` may sit on a later line, and an inner block may hold the `;`.
    same(&["if c then a(); b()\nend\n", "if c then a(); b() end\n"]);
    same(&[
        "if c then for x in xs do a(x); b(x) end\n",
        "if c then\n  for x in xs do\n    a(x)\n    b(x)\n",
    ]);
    // Statements of an indented block, and of the top level, may share a line.
    same(&["if c then\n  a(); b()\n", "if c then\n  a()\n  b()\n"]);
    same(&["a(); b()\n", "a()\nb()\n"]);
    // After the `end`, a `;` separates two statements as it always did.
    same(&["if c then a() end; b()\n", "if c then a() end\nb()\n"]);
}

// ── B4: `else` / `else if` on the line after a one-line `if` ─────────────

#[test]
fn b4_a_line_starting_with_else_continues_the_if_at_or_left_of_it() {
    // Hung under the `if` (core-runtime/glsl/src/glsl.ptl).
    let hung = "\
let ty = if t == \"float\" then 1
    else if t == \"vec2\" then 2
    else nil
print(ty)
";
    same(&[
        hung,
        "let ty = if t == \"float\" then 1 else if t == \"vec2\" then 2 else nil end\nprint(ty)\n",
        "let ty = if t == \"float\" then 1\n    else if t == \"vec2\" then 2\n    else nil end\nprint(ty)\n",
        "let ty = if t == \"float\" then 1\n    else if t == \"vec2\" then 2\n    else nil\n    end\nprint(ty)\n",
    ]);
    // Aligned with the statement line (the layout `petal fmt` gives a
    // value-position block).
    let aligned = "\
let bucket = if ci < 0 then
  zeros(1)
else
  zeros(2)
print(bucket)
";
    same(&[aligned, "let bucket = if ci < 0 then\n  zeros(1)\nelse\n  zeros(2)\nend\nprint(bucket)\n"]);
    // The innermost `if` whose statement column is at or left of the `else`.
    same(&[
        "if a then\n  if b then x()\n  else y()\n",
        "if a then\n  if b then x() else y() end\nend\n",
    ]);
    same(&[
        "if a then\n  if b then x()\nelse y()\n",
        "if a then\n  if b then x() end\nelse y() end\n",
    ]);
    // `else` at the end of a line opens a block that holds an `if`: the two
    // are not merged (core-runtime/bloom/src/menu.ptl).
    same(&[
        "let ink = if d then 1 else\n  if g then 2 else 3 end\nend\n",
        "let ink = if d then 1 else\n  if g then 2 else 3\n",
    ]);
    differ(
        "let ink = if d then 1 else\n  if g then 2 else 3\n",
        "let ink = if d then 1 else if g then 2 else 3\n",
    );
    // An `else` left of every open `if` is nobody's.
    err_has("fn f()\n  if a then x()\nelse y()\n", &["Unexpected token: `else`"]);
}

// ── B5: where an `end` may sit ───────────────────────────────────────────

#[test]
fn b5_an_end_on_its_own_line_sits_at_the_column_of_the_line_that_opened_its_block() {
    err_has(
        "fn f(xs)\n  for x in xs do\n    print(x)\n   end\nend\n",
        &["`end` at column 4", NOT_LINED_UP, "the `for` block opened at line 2 (column 3)", "[line 4, column 4]"],
    );
    err_has(
        "fn f(xs)\n  for x in xs do\n    print(x)\n    end\nend\n",
        &["`end` at column 5", "the `for` block opened at line 2 (column 3)"],
    );
    err_has("if c then\n  a()\n  end\nb()\n", &["`end` at column 3", "the `if` block opened at line 1 (column 1)"]);
    err_has("fn f()\n  1\n end\nf()\n", &["the `fn` block opened at line 1 (column 1)"]);
    // (Each of these has a line back at the margin: a source whose every
    // later line is indented reads as a host string, see B12.)
    err_has("while c do\n  a()\n    end\nb()\n", &["the `while` block"]);
    err_has("test \"t\" do\n  a()\n end\nb()\n", &["the `test` block"]);
    err_has("let v = match x\n  when 1 -> 2\n  end\nb()\n", &["the `match` block opened at line 1 (column 1)"]);
    err_has("class P\n  x: int,\n  end\nb()\n", &["the `class` block"]);
    err_has("enum E\n  a,\n b,\nc()\n", &[PARTIAL_DEDENT]);
    err_has("enum E\n  a,\n  end\nb()\n", &["the `enum` block"]);
    // An `end` with no open block.
    err_has("let a = 1\nend\n", &["Unexpected token: `end`"]);
    err_has("if c then a()\nb()\nend\n", &["Unexpected token: `end`"]);
    // An `end` left of its own block is the enclosing one's, when it lines
    // up with that; the inner block was closed by its indentation.
    same(&[
        "fn f()\n  if c then\n    a()\nend\n",
        "fn f()\n  if c then\n    a()\n  end\nend\n",
        "fn f()\n  if c then\n    a()\n",
    ]);
    // …and an error where it lines up with nothing.
    err_has(
        "fn f()\n  for x in xs do\n    if c then\n      a()\n   end\n",
        &["`end` at column 4", "the `if` block opened at line 3 (column 5)"],
    );
    // It cannot close a construct outside the brackets it is written in.
    err_has("fn f()\n  g(fn()\n    a()\nend)\n", &[NOT_LINED_UP, "the `fn` block opened at line 2"]);
    // On the same line, it closes the innermost open block, as today.
    same(&["fn f()\n  a()\n  b() end\n", "fn f()\n  a()\n  b()\nend\n"]);
}

#[test]
fn b5_a_construct_that_hangs_off_a_line_takes_its_end_at_its_own_column_too() {
    // Under the chain's last clause (examples/productivity/settings).
    same(&[
        "let t = if e == nil then 1\n    else if e == 2 then 3\n    else 4\n    end\nprint(t)\n",
        "let t = if e == nil then 1 else if e == 2 then 3 else 4 end\nprint(t)\n",
    ]);
    // Under the keyword itself.
    same(&[
        "let t = if e then\n          1\n        else\n          2\n        end\n",
        "let t = if e then 1 else 2 end\n",
    ]);
    same(&["let f = fn(x)\n          x + 1\n        end\n", "let f = fn(x) x + 1 end\n"]);
    err_has(
        "let t = if e then\n          1\n      end\nprint(t)\n",
        &[NOT_LINED_UP, "the `if` block opened at line 1 (column 1)"],
    );
}

// ── B6, §3.2: blocks inside brackets and JSX holes ───────────────────────

#[test]
fn b6_a_block_inside_brackets_ends_at_the_bracket_that_encloses_it() {
    same(&[
        "defer_paint(fn()\n  draw_shadow(r)\n  draw_rect(r)\n)\n",
        "defer_paint(fn()\n  draw_shadow(r)\n  draw_rect(r)\nend)\n",
        "defer_paint(fn() draw_shadow(r); draw_rect(r) end)\n",
    ]);
    // Measured against the line that opens it, wherever that line sits.
    same(&[
        "let x = total(\n  1,\n  map(xs, fn(v)\n    let w = v * 2\n    w + 1\n  ),\n  3,\n)\n",
        "let x = total(\n  1,\n  map(xs, fn(v)\n    let w = v * 2\n    w + 1\n  end),\n  3,\n)\n",
    ]);
    same(&[
        "let xs = [\n  if c then\n    1\n  else\n    2\n  ,\n  3,\n]\n",
        "let xs = [\n  if c then\n    1\n  else\n    2\n  end,\n  3,\n]\n",
    ]);
    same(&[
        "let r = {\n  a: for i in range(0, 3) do\n    i * 2\n  ,\n  b: 2,\n}\n",
        "let r = {\n  a: for i in range(0, 3) do\n    i * 2\n  end,\n  b: 2,\n}\n",
    ]);
    same(&["let y = (if c then\n  1\nelse\n  2\n)\n", "let y = (if c then\n  1\nelse\n  2\nend)\n"]);
    same(&["let z = xs[if c then\n  0\nelse\n  1\n]\n", "let z = xs[if c then 0 else 1 end]\n"]);
    // Two block lambdas in one call: `end`s, or a leading comma.
    same(&[
        "on(fn()\n  a()\nend, fn()\n  b()\nend)\n",
        "on(fn()\n  a()\n, fn()\n  b()\n)\n",
        "on(fn() a() end, fn() b() end)\n",
    ]);
    // A nested block re-enables layout inside the brackets.
    err_has("f(fn()\n  a()\n    b()\n)\n", &[UNEXPECTED_INDENT, "[line 3, column 5]"]);
    err_has("f(fn()\n    a()\n  b()\n)\n", &[PARTIAL_DEDENT]);
    err_has("f(fn()\na()\n)\n", &[MISSING_BLOCK, "the `fn` header"]);
    assert_eq!(
        run("fn each(xs, f)\n  for x in xs do f(x)\neach([1, 2], fn(x)\n  let y = x * 10\n  print(y)\n)\n"),
        ["10", "20"]
    );
}

#[test]
fn b6_an_element_hole_measures_against_the_first_token_of_its_line() {
    let with_end = "\
fn view(outlined, m)
  <g>
    <text>label</text>
    {if !outlined then
      <rect id=\"indicator\" x={0} y={m.height} />
    else nil end}
  </g>
end
";
    let without = "\
fn view(outlined, m)
  <g>
    <text>label</text>
    {if !outlined then
      <rect id=\"indicator\" x={0} y={m.height} />
    else nil}
  </g>
";
    let hole_closes = "\
fn view(outlined, m)
  <g>
    <text>label</text>
    {if !outlined then
      <rect id=\"indicator\" x={0} y={m.height} />
    else
      nil
    }
  </g>
";
    same(&[with_end, without, hole_closes]);
    // Tags and text ignore layout; a hole after text on an earlier line is
    // still the first token of its own line.
    same(&[
        "let v = <p>\n      some text\n  {for x in xs do\n    <b>{x}</b>\n  }\n        </p>\n",
        "let v = <p>\n      some text\n  {for x in xs do\n    <b>{x}</b>\n  end}\n        </p>\n",
    ]);
    // An attribute hole.
    same(&[
        "let v = <rect fill={if on then\n  :f80\nelse\n  :000\n} />\n",
        "let v = <rect fill={if on then :f80 else :000 end} />\n",
    ]);
    err_has("let v = <g>\n  {if c then\n  <a />\n  }\n</g>\n", &[MISSING_BLOCK, "`then`"]);
}

// ── B7: chaining off an `end` ────────────────────────────────────────────

#[test]
fn b7_chaining_off_an_end_keeps_working() {
    same(&[
        "let s = for x in xs do\n  str(x)\nend |> join(\" \")\n",
        "let s = join(for x in xs do\n  str(x)\nend, \" \")\n",
    ]);
    same(&["let n = if c then a else b end.size\n", "let n = (if c then a else b).size\n"]);
    assert_eq!(run("let s = for x in [1, 2] do\n  str(x)\nend |> join(\"-\")\nprint(s)\n"), ["1-2"]);
    // Without the `end`, a one-line body takes the rest of the expression.
    differ("let n = if c then a else b end + 1\n", "let n = if c then a else b + 1\n");
}

// ── B8, B9: one-line `fn` bodies ─────────────────────────────────────────

#[test]
fn b8_a_one_line_named_fn_runs_to_the_end_of_its_line() {
    same(&[
        "fn box(w, h = w) \"{w}x{h}\"\nprint(box(1))\n",
        "fn box(w, h = w) \"{w}x{h}\" end\nprint(box(1))\n",
        "fn box(w, h = w)\n  \"{w}x{h}\"\nend\nprint(box(1))\n",
        "fn box(w, h = w)\n  \"{w}x{h}\"\nprint(box(1))\n",
    ]);
    // A return type is a single bare name, so no token separates it from
    // the body.
    same(&[
        "fn double(x) -> int x * 2\nprint(double(2))\n",
        "fn double(x) -> int x * 2 end\nprint(double(2))\n",
        "fn double(x) -> int\n  x * 2\nprint(double(2))\n",
    ]);
    same(&["pub fn id(x) x\n", "pub fn id(x) x end\n"]);
    same(&["fn P.len(p) p.x + p.y\n", "fn P.len(p)\n  p.x + p.y\nend\n"]);
    assert_eq!(run("fn box(w, h = w) \"{w}x{h}\"\nfn double(x) -> int x * 2\nprint(box(3), double(4))\n"), ["3x3 8"]);
    err_has("fn f()\nprint(1)\n", &[MISSING_BLOCK, "the `fn` header", "[line 2, column 1]"]);
    err_has("fn f()\n", &[MISSING_BLOCK]);
}

#[test]
fn b9_a_one_line_lambda_needs_no_end() {
    same(&[
        "let ys = map(xs, fn(v) v * 2)\n",
        "let ys = map(xs, fn(v) v * 2 end)\n",
        "let ys = map(xs, fn(v) -> v * 2)\n",
    ]);
    same(&["let f = fn(a, b) a + b\nprint(f(1, 2))\n", "let f = fn(a, b) a + b end\nprint(f(1, 2))\n"]);
    same(&["let k = fn 42\n", "let k = fn 42 end\n"]);
    same(&[
        "let describe = fn(x)\n  print(x)\n  x * 2\nprint(describe(2))\n",
        "let describe = fn(x)\n  print(x)\n  x * 2\nend\nprint(describe(2))\n",
    ]);
    assert_eq!(run("print(map([1, 2], fn(v) v * 2))\n"), ["[2, 4]"]);
    // An arrow lambda is unchanged, and still takes no `end`.
    err_has("let f = fn(x) -> x\nend\n", &["an arrow lambda (`fn(x) -> expr`) takes no `end`"]);
}

// ── B10: match arms ──────────────────────────────────────────────────────

#[test]
fn b10_a_match_arm_takes_one_line_or_an_indented_block() {
    let with_ends = "\
let r = match dir
  when \"left\" -> slide(g)
  when \"right\" ->
    let res = slide(mirror(g))
    {grid: mirror(res.grid), gained: res.gained}
  when d -> {grid: g, gained: 0, bad: d}
end
";
    let without = "\
let r = match dir
  when \"left\" -> slide(g)
  when \"right\" ->
    let res = slide(mirror(g))
    {grid: mirror(res.grid), gained: res.gained}
  when d -> {grid: g, gained: 0, bad: d}
";
    same(&[with_ends, without]);
    // `when p do … end` keeps working, with its `end` or without.
    same(&[
        "match v\n  when 1 do\n    a()\n    b()\n  end\n  when _ -> c()\nend\n",
        "match v\n  when 1 do\n    a()\n    b()\n  when _ -> c()\n",
        "match v\n  when 1 ->\n    a()\n    b()\n  when _ -> c()\n",
    ]);
    // On one line.
    same(&["let n = match v when 1 -> 2 when _ -> 3 end\n", "let n = match v when 1 -> 2 when _ -> 3\n"]);
    // The arms may sit under the `match` itself: a `when` starts no statement.
    same(&["match v\nwhen 1 -> a()\nwhen _ -> b()\nend\n", "match v\nwhen 1 -> a()\nwhen _ -> b()\n"]);
    // Nested: an arm's `when` belongs to the match whose arms sit at its column.
    let nested = "\
match a
  when 1 ->
    match b
      when 2 -> x()
      when 3 -> y()
  when 4 -> z()
";
    same(&[
        nested,
        "match a\n  when 1 ->\n    match b\n      when 2 -> x()\n      when 3 -> y()\n    end\n  when 4 -> z()\nend\n",
    ]);
    assert_eq!(
        run("fn f(a, b)\n  match a\n    when 1 ->\n      match b\n        when 2 -> \"x\"\n        when _ -> \"y\"\n    when _ -> \"z\"\nprint(f(1, 2), f(1, 9), f(5, 2))\n"),
        ["x y z"]
    );
    err_has("match v\n  when 1 -> a()\n    when 2 -> b()\n", &[UNEXPECTED_INDENT]);
    err_has("fn f(v)\n  match v\n      when 1 -> a()\n    when 2 -> b()\n", &[PARTIAL_DEDENT]);
    err_has("match v\n  when 1 ->\n  a()\n", &["Expected the arm's body after '->'"]);
    err_has("match v\n  when 1 -> a()\n    b()\n", &[UNEXPECTED_INDENT]);
    err_has("match v\nprint(1)\n", &[MISSING_BLOCK, "the `match` header"]);
    err_has("match v\n  when 1 -> a()\n  else b()\n", &["a `match` has no `else` arm"]);
}

// ── B11: tabs ────────────────────────────────────────────────────────────

#[test]
fn b11_a_tab_in_measured_indentation_is_an_error() {
    let tab = "a tab in the indentation of this line";
    err_has("fn f()\n\tprint(1)\n", &[tab, "[line 2, column 2]"]);
    err_has("fn f()\n  print(1)\n\tprint(2)\n", &[tab]);
    err_has("fn f()\n \tprint(1)\nend\n", &[tab]);
    err_has("if c then\n  a()\n\telse\n  b()\n", &[tab]);
    err_has("\tlet a = 1\n\tlet b = 2\n", &[tab]);
    // A line that only continues a bracket or an expression is not measured.
    same(&["let xs = [\n\t1,\n\t2,\n]\n", "let xs = [1, 2]\n"]);
    same(&["let n = 1 +\n\t2\n", "let n = 1 + 2\n"]);
    // Nor is a tab anywhere else.
    same(&["fn f()\n  let s = \"a\tb\"\t# note\n  s\n", "fn f()\n  let s = \"a\tb\"\n  s\n"]);
    // Read by its `end`s alone (as `petal fmt` reads old source), a tab is
    // whitespace like any other.
    assert!(petal::cst::parse_source_by_end("fn f()\n\tprint(1)\nend\n").is_ok());
}

// ── B12, §3.5: a snippet indented as a whole ─────────────────────────────

#[test]
fn b12_the_first_statement_sets_the_left_margin() {
    let flush = "fn step(input)\n  if input > 0 then\n    input - 1\n  else\n    0\nprint(step(3))\n";
    let indented: String = flush.lines().map(|l| format!("        {l}\n")).collect();
    assert_eq!(shape(&indented), shape(flush));
    assert_eq!(run(&indented), ["2"]);
    // As a host string holds it: a line break, then the indented source.
    let hosted = format!("\n{indented}    ");
    assert_eq!(shape(&hosted), shape(flush));
    // With its `end`s, which then sit at the margin too.
    same(&[
        "    fn f()\n      1\n    end\n    print(f())\n",
        "fn f()\n  1\nend\nprint(f())\n",
    ]);
    err_has("    let a = 1\n    let b = 2\n      let c = 3\n", &[UNEXPECTED_INDENT, "[line 3, column 7]"]);
    err_has("    let a = 1\n  let b = 2\n", &["indented less than the first line of the program"]);
    err_has("let a = 1\n  print(a)\nprint(2)\n", &[UNEXPECTED_INDENT]);
}

#[test]
fn b12_a_host_string_that_starts_at_its_quote_reads_at_the_margin_of_its_other_lines() {
    // `run("import vec2` / `     let v = …")`: the first line lost its
    // indentation to the host language's quote.
    let flush = "let a = 1\nfn f(x)\n  if x then\n    a\n  else\n    0\nprint(f(true))\n";
    let mut hosted = String::new();
    for (i, line) in flush.lines().enumerate() {
        hosted.push_str(if i == 0 { "" } else { "      " });
        hosted.push_str(line);
        hosted.push('\n');
    }
    assert_eq!(shape(&hosted), shape(flush));
    assert_eq!(run(&hosted), ["1"]);
    // The first line may open a block of its own…
    same(&[
        "fn f(x)\n        x + 1\n      print(f(1))\n",
        "fn f(x)\n  x + 1\nprint(f(1))\n",
    ]);
    same(&[
        "if c then\n          a()\n        else\n          b()\n        end\n        d()\n",
        "if c then\n  a()\nelse\n  b()\nend\nd()\n",
    ]);
    // …and when every later line is inside it, the source reads as written.
    same(&["fn f(x)\n    x + 1\n", "fn f(x) x + 1 end\n"]);
    same(&["for x in xs do\n      a(x)\n      b(x)\n", "for x in xs do a(x); b(x) end\n"]);
    // Only the first line: a later line still has to agree with the others.
    err_has("let a = 1\n      let b = 2\n        let c = 3\n", &[UNEXPECTED_INDENT, "[line 3"]);
    err_has("let a = 1\n      let b = 2\n    let c = 3\n", &[UNEXPECTED_INDENT, "[line 2"]);
    // When neither reading parses, the error is from the one that got further.
    err_has("let a = 1\n      let b = 2\n      let c = )\n", &["Unexpected token: ')'", "[line 3"]);
}

// ── B13: multi-line headers ──────────────────────────────────────────────

#[test]
fn b13_a_header_that_spans_lines_is_measured_from_its_first() {
    same(&[
        "fn f(a, b)\n  if (a &&\n      b) then\n    print(1)\n  print(2)\n",
        "fn f(a, b)\n  if (a && b) then\n    print(1)\n  end\n  print(2)\nend\n",
    ]);
    same(&[
        "if a ||\n   b then\n  print(1)\nelse\n  print(2)\n",
        "if a || b then\n  print(1)\nelse\n  print(2)\nend\n",
    ]);
    same(&[
        "for a in xs,\n    b in ys do\n  print(a, b)\n",
        "for a in xs, b in ys do\n  print(a, b)\nend\n",
    ]);
    same(&[
        "fn long(\n  a,\n  b,\n)\n  a + b\nprint(long(1, 2))\n",
        "fn long(a, b)\n  a + b\nend\nprint(long(1, 2))\n",
    ]);
    same(&[
        "while n <\n    10 do\n  n += 1\n",
        "while n < 10 do\n  n += 1\nend\n",
    ]);
    // The body is indented past the first line, not the last.
    err_has("  if (a &&\n      b) then\n  print(1)\n", &[MISSING_BLOCK, "`then`"]);
}

// ── B15: comments and blank lines ────────────────────────────────────────

#[test]
fn b15_comments_and_blank_lines_are_ignored_for_layout() {
    let commented = "\
fn f(c)   # header
# a comment at the margin

      # and one further in
  if c then  # opens the block

    a()
# flush left again
    b()
  else # the other branch
    /* a block
comment */
    d()

  e()
";
    same(&[commented, "fn f(c)\n  if c then\n    a()\n    b()\n  else\n    d()\n  end\n  e()\nend\n"]);
    // Only code decides: a comment where the block would be is not one.
    err_has("fn f()\n  # nothing here\nprint(1)\n", &[MISSING_BLOCK]);
}

// ── Strings are opaque to layout ─────────────────────────────────────────

#[test]
fn a_line_that_begins_inside_a_string_is_not_a_line_start() {
    same(&[
        "fn f()\n  let s = \"one\ntwo\"\n  let r = \"\"\"\nraw\n      more\n\"\"\"\n  s ++ r\n",
        "fn f()\n  let s = \"one\ntwo\"\n  let r = \"\"\"\nraw\n      more\n\"\"\"\n  s ++ r\nend\n",
    ]);
    // What follows a string on its last line belongs to the statement the
    // string is in.
    same(&[
        "fn f(x)\n  let s = \"a\nb\" ++ x\n  s\n",
        "fn f(x)\n  let s = \"a\nb\" ++ x\n  s\nend\n",
    ]);
    // A block may be opened on such a line: it is measured against the line
    // the string began on.
    same(&[
        "fn f(x)\n  if x == \"a\nb\" then\n    print(1)\n  print(2)\n",
        "fn f(x)\n  if x == \"a\nb\" then\n    print(1)\n  end\n  print(2)\nend\n",
    ]);
    let raw = "fn f()\n  let s = \"\"\"\nif x then\n      y\n\"\"\"\n  len(s)\n";
    assert_eq!(run(&format!("{raw}print(f())\n")), run(&format!("{raw}end\nprint(f())\n")));
}

// ── §4: every construct, with `end`, without, and mixed ──────────────────

#[test]
fn s4_if_chains() {
    let with_ends = "\
fn sign(x)
  if x < 0 then
    return -1
  else if x == 0 then
    return 0
  else
    1
  end
end
";
    let without = "\
fn sign(x)
  if x < 0 then
    return -1
  else if x == 0 then
    return 0
  else
    1
";
    let mixed_inner = "\
fn sign(x)
  if x < 0 then
    return -1
  else if x == 0 then
    return 0
  else
    1
  end
";
    let mixed_outer = "\
fn sign(x)
  if x < 0 then
    return -1
  else if x == 0 then
    return 0
  else
    1
end
";
    same(&[with_ends, without, mixed_inner, mixed_outer]);
    assert_eq!(run(&format!("{without}print(sign(-5), sign(0), sign(5))\n")), ["-1 0 1"]);
    same(&["if x < 0 then return -x\nprint(x)\n", "if x < 0 then return -x end\nprint(x)\n"]);
    same(&["if x < 0 then return\nprint(x)\n", "if x < 0 then return end\nprint(x)\n"]);
    same(&[
        "let label = if x > 5 then \"big\" else \"small\"\n",
        "let label = if x > 5 then \"big\" else \"small\" end\n",
    ]);
    // `elsif`, the deprecated spelling, continues a chain the same way.
    same(&[
        "if a then\n  1\nelsif b then\n  2\nelse\n  3\n",
        "if a then\n  1\nelse if b then\n  2\nelse\n  3\nend\n",
    ]);
    // An empty branch is closed where it stands.
    same(&["if c then\nelse\n  a()\n", "if c then else a() end\n"]);
    same(&["if c then\nend\n", "if c then end\n"]);
    err_has("if c then\nprint(1)\n", &[MISSING_BLOCK, "`then`", "column 1"]);
    err_has("if c then\n  a()\nelse\nb()\n", &[MISSING_BLOCK, "`else`"]);
    err_has("if c then\n  a()\nelse if d then\nb()\n", &[MISSING_BLOCK, "`then`"]);
}

#[test]
fn s4_loops() {
    same(&[
        "for item in items do\n  print(item)\nend\nprint(0)\n",
        "for item in items do\n  print(item)\nprint(0)\n",
    ]);
    same(&[
        "let squares = for i in range(1, 6) do i * i end\n",
        "let squares = for i in range(1, 6) do i * i\n",
        "let squares = for i in range(1, 6) do\n  i * i\n",
        "let squares = for i in range(1, 6) do\n  i * i\nend\n",
    ]);
    same(&["while x < 10 do x += 1 end\n", "while x < 10 do x += 1\n", "while x < 10 do\n  x += 1\n"]);
    same(&[
        "for a in xs do\n  for b in ys do\n    if a == b then\n      print(a)\n    end\n  end\nend\n",
        "for a in xs do\n  for b in ys do\n    if a == b then\n      print(a)\n",
        "for a in xs do\n  for b in ys do\n    if a == b then\n      print(a)\n  end\n",
        "for a in xs do\n  for b in ys do\n    if a == b then print(a)\nend\n",
    ]);
    same(&[
        "for t in tasks key t.id do\n  draw(t)\n",
        "for t in tasks key t.id do\n  draw(t)\nend\n",
    ]);
    assert_eq!(run("var n = 0\nfor i in range(0, 4) do\n  if i % 2 == 0 then continue\n  set n += i\nprint(n)\n"), ["4"]);
    assert_eq!(run("var n = 0\nwhile true do\n  set n += 1\n  if get n > 2 then break\nprint(n)\n"), ["3"]);
    err_has("for x in xs do\nprint(x)\n", &[MISSING_BLOCK, "`do`"]);
    err_has("while c do\nc = false\nend\n", &[MISSING_BLOCK, "`do`"]);
}

#[test]
fn s4_fn_class_enum_test() {
    same(&[
        "pub fn abs(x: int) -> int\n  if x < 0 then return -x\n  x\nprint(abs(-2))\n",
        "pub fn abs(x: int) -> int\n  if x < 0 then return -x end\n  x\nend\nprint(abs(-2))\n",
    ]);
    same(&[
        "class Point\n  x: int,\n  y: int,\nlet p = Point(1, 2)\n",
        "class Point\n  x: int,\n  y: int,\nend\nlet p = Point(1, 2)\n",
        "class Point\n  x: int,\n  y: int\nlet p = Point(1, 2)\n",
        "class Point x: int, y: int end\nlet p = Point(1, 2)\n",
        "class Point x: int, y: int\nlet p = Point(1, 2)\n",
    ]);
    same(&[
        "enum Shape\n  circle(radius, sides: 0),\n  rect(w, h, sides: 4),\nprint(Shape.rect(1, 2).sides)\n",
        "enum Shape\n  circle(radius, sides: 0),\n  rect(w, h, sides: 4),\nend\nprint(Shape.rect(1, 2).sides)\n",
        "enum Shape\n  circle(radius, sides: 0)\n  , rect(\n    w,\n    h,\n    sides: 4,\n  )\nprint(Shape.rect(1, 2).sides)\n",
        "enum Shape circle(radius, sides: 0), rect(w, h, sides: 4) end\nprint(Shape.rect(1, 2).sides)\n",
    ]);
    same(&["pub enum Dir\n  left,\n  right,\n", "pub enum Dir left, right end\n"]);
    same(&["class Unit\nend\n", "class Unit end\n"]);
    same(&[
        "test \"addition\" do\n  expect_eq(1 + 1, 2)\n",
        "test \"addition\" do\n  expect_eq(1 + 1, 2)\nend\n",
        "test \"addition\" do expect_eq(1 + 1, 2) end\n",
        "test \"addition\" do expect_eq(1 + 1, 2)\n",
    ]);
    assert_eq!(
        run("class Point\n  x: int,\n  y: int,\nenum Shape\n  circle(radius, sides: 0),\n  rect(w, h, sides: 4),\nfn Point.sum(p) p.x + p.y\nprint(Point(1, 2).sum(), Shape.rect(1, 2).sides)\n"),
        ["3 4"]
    );
    err_has("class P\nx: int,\nend\n", &[MISSING_BLOCK, "the `class` header"]);
    err_has("class P\nprint(1)\n", &[MISSING_BLOCK, "the `class` header"]);
    err_has("enum E\na,\nb,\nend\n", &[MISSING_BLOCK, "the `enum` header"]);
    err_has("class P\n  x: int,\n    y: int,\n", &[UNEXPECTED_INDENT]);
    err_has("class P\n  x: int\n  y: int\n", &["Expected ',' between class fields"]);
    // A nested function declaration.
    same(&[
        "fn outer(a)\n  fn inner(b)\n    b * 2\n  inner(a) + 1\nprint(outer(1))\n",
        "fn outer(a)\n  fn inner(b)\n    b * 2\n  end\n  inner(a) + 1\nend\nprint(outer(1))\n",
    ]);
}

#[test]
fn s4_mixed_styles_block_by_block() {
    let all_ends = "\
fn tally(rows)
  var total = 0
  for row in rows do
    match row.kind
      when \"add\" ->
        set total += row.n
      when \"sub\" do
        set total -= row.n
      end
      when _ ->
        if row.n > 100 then
          print(\"big\")
        else
          print(\"odd\")
        end
    end
  end
  get total
end
print(tally([{kind: \"add\", n: 5}, {kind: \"sub\", n: 2}, {kind: \"x\", n: 1}]))
";
    let no_ends = "\
fn tally(rows)
  var total = 0
  for row in rows do
    match row.kind
      when \"add\" ->
        set total += row.n
      when \"sub\" do
        set total -= row.n
      when _ ->
        if row.n > 100 then
          print(\"big\")
        else
          print(\"odd\")
  get total
print(tally([{kind: \"add\", n: 5}, {kind: \"sub\", n: 2}, {kind: \"x\", n: 1}]))
";
    // Each `end` of the first, removed alone.
    let lines: Vec<&str> = all_ends.lines().collect();
    let mut variants = vec![no_ends.to_string()];
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "end" {
            let mut kept = lines.clone();
            kept.remove(i);
            variants.push(kept.join("\n") + "\n");
        }
    }
    assert_eq!(variants.len(), 6);
    for variant in &variants {
        assert_eq!(shape(variant), shape(all_ends), "\n{variant}");
    }
    assert_eq!(run(no_ends), ["odd", "3"]);
    assert_eq!(run(all_ends), run(no_ends));
}

// ── §5.1: misindented legacy code fails loudly ───────────────────────────

/// `src` as it was read before indentation delimited blocks.
fn by_end(src: &str) -> Result<String, String> {
    let (_tree, stmts) = petal::cst::parse_source_by_end(src)?;
    Ok(petal::rewrite::ast_shape(&stmts)?.to_string())
}

#[test]
fn s5_1_code_indented_against_its_ends_is_an_error_not_another_program() {
    for (src, needle) in [
        // A block body that is not indented.
        ("fn f()\nif c then\na()\nend\nend\n", MISSING_BLOCK),
        ("if c then\na()\nend\n", MISSING_BLOCK),
        // The rest of the file under an `else` (a guard written flush left).
        ("if m == nil then\n  wait()\nelse\n\nlet s = fetch(m)\nprint(s)\nend\n", MISSING_BLOCK),
        // A dedented line inside a block closes it early; its `end` then
        // lines up with nothing.
        ("fn f()\n  if c then\n    a()\n  b()\n  end\nend\n", NOT_LINED_UP),
        ("fn f()\n  for x in xs do\n    if c then\n      a()\n    b()\n    end\n  end\nend\n", NOT_LINED_UP),
        // …or is one `end` too many at the top level.
        ("if c then\n  a()\nb()\nend\n", "Unexpected token: `end`"),
        ("fn f()\n  if c then a()\n  b()\nend\nend\n", "Unexpected token: `end`"),
        // Over-indentation.
        ("fn f()\n  a()\n    b()\nend\n", UNEXPECTED_INDENT),
        ("a()\n  b()\nc()\n", UNEXPECTED_INDENT),
        // A one-line body carried on over the following lines.
        ("if c then a()\n  b()\nend\n", UNEXPECTED_INDENT),
        ("fn f() let t = 2\n t end\n", UNEXPECTED_INDENT),
        // A line between two levels.
        ("fn f()\n  if c then\n      a()\n    b()\n  end\nend\n", PARTIAL_DEDENT),
    ] {
        // Its author's compiler read it by its `end`s.
        by_end(src).unwrap_or_else(|e| panic!("not valid by its `end`s: {e}\n{src}"));
        err_has(src, &[needle]);
    }
}

#[test]
fn s5_1_code_both_readings_accept_is_one_program() {
    for src in [
        "fn f(c)\n  if c then\n    a()\n  else if d then\n    b()\n  else\n    e()\n  end\n  g()\nend\n",
        "let b = if ci < 0 then\n  zeros(1)\nelse\n  zeros(2)\nend\n",
        "f(fn()\n  a()\nend, [for x in xs do\n  x\nend], if c then 1 else 2 end)\n",
        "match v\n  when 1 -> a()\n  when 2 do\n    b()\n  end\n  when _ ->\n    c()\n    d()\nend\n",
        "class P\n  x: int,\nend\nenum E\n  a,\n  b(n),\nend\ntest \"t\" do\n  expect(true)\nend\n",
        "while c do\n  if d then break end\n  e(); g()\nend\n",
    ] {
        assert_eq!(shape(src), by_end(src).unwrap(), "\n{src}");
    }
}

// ── Spans ────────────────────────────────────────────────────────────────

#[test]
fn a_block_with_no_end_spans_to_its_last_token() {
    let src = "fn f(c)\n  if c then\n    g(1)\n\n# trailing comment\nprint(2)\n";
    let (_tree, stmts) = petal::cst::parse_source(src, ENTRY_FILE).unwrap();
    assert_eq!(stmts.len(), 2);
    let end = stmts[0].span.end;
    // The `)` of `g(1)`: line 3, and not the line breaks or the comment after.
    assert_eq!((end.line, end.column), (3, 9));
    let petal::ast::StmtKind::FnDecl { body, .. } = &stmts[0].kind else {
        panic!("a fn");
    };
    assert_eq!((body[0].span.end.line, body[0].span.end.column), (3, 9));
    // With its `end`s, the spans end at them, as before.
    let src = "fn f(c)\n  if c then\n    g(1)\n  end\nend\nprint(2)\n";
    let (_tree, stmts) = petal::cst::parse_source(src, ENTRY_FILE).unwrap();
    assert_eq!((stmts[0].span.end.line, stmts[0].span.end.column), (5, 4));
}

// ── Generative: a random program, printed with its `end`s and without ────

/// A small deterministic generator (xorshift): the test must fail the same
/// way on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// A statement of a generated program. Every block-bearing node can be
/// printed with an `end` or with none; `id` picks which, per node, so a
/// printing can mix the two.
enum Node {
    /// `f3(x)`
    Call(u32),
    /// `if … then` / `else if … then` / `else`, as a statement or as the
    /// value of a `let`.
    If { id: usize, value: bool, branches: Vec<Vec<Node>>, otherwise: Option<Vec<Node>> },
    /// `if c then f() [else g()]` on one line.
    IfLine { id: usize, otherwise: bool },
    /// `for x in xs do`, as a statement or as the value of a `let`.
    For { id: usize, value: bool, body: Vec<Node> },
    /// `for x in xs do f(x)` on one line.
    ForLine { id: usize },
    While { id: usize, body: Vec<Node> },
    /// `fn name(a)`
    Fn { id: usize, body: Vec<Node> },
    /// `match s` with one arm per body: a one-statement body may be printed
    /// on the arm's line.
    Match { id: usize, value: bool, arms: Vec<(bool, Vec<Node>)> },
    /// `g(1, fn(x)` … `)`: a block inside a call's brackets.
    Lambda { id: usize, body: Vec<Node> },
    /// `let xs = [` … `]` whose elements are blocks: each needs its `end` or
    /// a line that holds the separating comma.
    List { elements: Vec<Node> },
}

struct Gen {
    rng: Rng,
    ids: usize,
}

impl Gen {
    fn id(&mut self) -> usize {
        self.ids += 1;
        self.ids - 1
    }

    fn block(&mut self, depth: u32) -> Vec<Node> {
        let n = 1 + self.rng.below(3);
        (0..n).map(|_| self.node(depth)).collect()
    }

    fn node(&mut self, depth: u32) -> Node {
        if depth == 0 || self.rng.chance(30) {
            return match self.rng.below(4) {
                0 => Node::IfLine { id: self.id(), otherwise: self.rng.chance(50) },
                1 => Node::ForLine { id: self.id() },
                _ => Node::Call(self.rng.below(9) as u32),
            };
        }
        let d = depth - 1;
        match self.rng.below(8) {
            0 | 1 => {
                let n = 1 + self.rng.below(3);
                Node::If {
                    id: self.id(),
                    value: self.rng.chance(30),
                    branches: (0..n).map(|_| self.block(d)).collect(),
                    otherwise: self.rng.chance(60).then(|| self.block(d)),
                }
            }
            2 => Node::For { id: self.id(), value: self.rng.chance(30), body: self.block(d) },
            3 => Node::While { id: self.id(), body: self.block(d) },
            4 => Node::Fn { id: self.id(), body: self.block(d) },
            5 => {
                let n = 1 + self.rng.below(3);
                Node::Match {
                    id: self.id(),
                    value: self.rng.chance(30),
                    arms: (0..n).map(|_| (self.rng.chance(50), self.block(d))).collect(),
                }
            }
            6 => Node::Lambda { id: self.id(), body: self.block(d) },
            _ => {
                let n = 1 + self.rng.below(3);
                Node::List {
                    elements: (0..n)
                        .map(|_| Node::If {
                            id: self.id(),
                            value: false,
                            branches: vec![self.block(d)],
                            otherwise: Some(self.block(d)),
                        })
                        .collect(),
                }
            }
        }
    }
}

/// Print `nodes` at `indent` columns. `ends(id)` says whether the node with
/// that id is closed by an `end`.
fn print_nodes(nodes: &[Node], indent: usize, ends: &dyn Fn(usize) -> bool, out: &mut String) {
    for node in nodes {
        print_node(node, indent, ends, "", out);
    }
}

/// `after` is what follows the node's closer on its last line (the `,` of a
/// list element); without an `end` it goes on a line of its own.
fn print_node(node: &Node, indent: usize, ends: &dyn Fn(usize) -> bool, after: &str, out: &mut String) {
    let pad = " ".repeat(indent);
    let close = |id: usize, out: &mut String| {
        if ends(id) {
            out.push_str(&format!("{pad}end{after}\n"));
        } else if !after.is_empty() {
            out.push_str(&format!("{pad}{after}\n"));
        }
    };
    match node {
        Node::Call(n) => out.push_str(&format!("{pad}f{n}(x)\n")),
        Node::IfLine { id, otherwise } => {
            let tail = if *otherwise { " else g(2)" } else { "" };
            let end = if ends(*id) { " end" } else { "" };
            out.push_str(&format!("{pad}if c then g(1){tail}{end}\n"));
        }
        Node::ForLine { id } => {
            let end = if ends(*id) { " end" } else { "" };
            out.push_str(&format!("{pad}for i in xs do g(i){end}\n"));
        }
        Node::If { id, value, branches, otherwise } => {
            let head = if *value { "let v = " } else { "" };
            for (i, body) in branches.iter().enumerate() {
                let keyword = if i == 0 { format!("{head}if") } else { "else if".to_string() };
                out.push_str(&format!("{pad}{keyword} c{i} then\n"));
                print_nodes(body, indent + 2, ends, out);
            }
            if let Some(body) = otherwise {
                out.push_str(&format!("{pad}else\n"));
                print_nodes(body, indent + 2, ends, out);
            }
            close(*id, out);
        }
        Node::For { id, value, body } => {
            let head = if *value { "let v = " } else { "" };
            out.push_str(&format!("{pad}{head}for i in xs do\n"));
            print_nodes(body, indent + 2, ends, out);
            close(*id, out);
        }
        Node::While { id, body } => {
            out.push_str(&format!("{pad}while c do\n"));
            print_nodes(body, indent + 2, ends, out);
            close(*id, out);
        }
        Node::Fn { id, body } => {
            out.push_str(&format!("{pad}fn h{id}(a)\n"));
            print_nodes(body, indent + 2, ends, out);
            close(*id, out);
        }
        Node::Match { id, value, arms } => {
            let head = if *value { "let v = " } else { "" };
            out.push_str(&format!("{pad}{head}match s\n"));
            for (i, (one_line, body)) in arms.iter().enumerate() {
                match body.as_slice() {
                    [Node::Call(n)] if *one_line => {
                        out.push_str(&format!("{pad}  when {i} -> f{n}(x)\n"));
                    }
                    _ => {
                        out.push_str(&format!("{pad}  when {i} ->\n"));
                        print_nodes(body, indent + 4, ends, out);
                    }
                }
            }
            close(*id, out);
        }
        Node::Lambda { id, body } => {
            out.push_str(&format!("{pad}g(1, fn(a)\n"));
            print_nodes(body, indent + 2, ends, out);
            let end = if ends(*id) { "end" } else { "" };
            out.push_str(&format!("{pad}{end})\n"));
        }
        Node::List { elements } => {
            out.push_str(&format!("{pad}let xs = [\n"));
            for element in elements {
                print_node(element, indent + 2, ends, ",", out);
            }
            out.push_str(&format!("{pad}]\n"));
        }
    }
}

#[test]
fn generated_programs_parse_the_same_with_their_ends_without_and_mixed() {
    let mut blocks = 0;
    for seed in 1..=250u64 {
        let mut generator = Gen { rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)), ids: 0 };
        let program = generator.block(4);
        blocks += generator.ids;
        let print = |ends: &dyn Fn(usize) -> bool| {
            let mut out = String::new();
            print_nodes(&program, 0, ends, &mut out);
            out
        };
        let with_ends = print(&|_| true);
        let expected = shape(&with_ends);
        // The reading from before the layout parser agrees.
        assert_eq!(by_end(&with_ends).unwrap(), expected, "seed {seed}\n{with_ends}");
        let without = print(&|_| false);
        assert_eq!(shape(&without), expected, "seed {seed}, no `end`s\n{without}");
        for mix in 0..4u64 {
            // Each block keeps or drops its `end` on a coin fixed by its id.
            let keep = |id: usize| ((id as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D + seed + mix) >> 17) & 1 == 0;
            let mixed = print(&keep);
            assert_eq!(shape(&mixed), expected, "seed {seed}, mix {mix}\n{mixed}");
        }
        // Indented as a whole, it is the same program (B12).
        let shifted: String = without.lines().map(|l| format!("      {l}\n")).collect();
        assert_eq!(shape(&shifted), expected, "seed {seed}, indented\n{shifted}");
    }
    assert!(blocks > 1000, "the generator made {blocks} blocks");
}
