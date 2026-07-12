// Source→source regression tests for `petal lint` (rust/src/lint.rs).
//
// Every case asserts the exact linted output for a given input source, so any
// behavior change in the re-indenter or the rebind rewrite shows up as a
// readable string diff. `assert_lints_to` also re-lints its own output and
// asserts a fixed point, so idempotence is pinned by every positive case.
//
// Note on test sources: multi-line Petal programs are written as Rust string
// literals with real newlines starting at column 0, so what you see is
// byte-for-byte what the linter sees.

use petal::lint::{LintOptions, LintOutcome, lint_source};

fn lint_outcome(src: &str) -> LintOutcome {
    lint_source(src, &LintOptions::default()).expect("lint_source should succeed")
}

fn lint(src: &str) -> String {
    lint_outcome(src).output
}

#[track_caller]
fn assert_lints_to(src: &str, expected: &str) {
    let out = lint(src);
    assert_eq!(out, expected, "lint output mismatch");
    let again = lint(&out);
    assert_eq!(again, out, "lint is not idempotent on its own output");
}

/// The source is already in house style: lint must not touch it.
#[track_caller]
fn assert_fixed_point(src: &str) {
    assert_lints_to(src, src);
}

// ═══════════════════════════════════════════════════════════════════════════
// Pass 1 — re-indentation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn flat_fn_body_gets_indented() {
    assert_lints_to(
        "\
fn greet(name)
print(\"hi {name}\")
end
greet(\"petal\")
",
        "\
fn greet(name)
  print(\"hi {name}\")
end
greet(\"petal\")
",
    );
}

#[test]
fn over_indentation_is_flattened() {
    assert_lints_to(
        "\
fn f(a)
        if a then
                return 1
        end
        return 2
end
",
        "\
fn f(a)
  if a then
    return 1
  end
  return 2
end
",
    );
}

#[test]
fn tab_indentation_becomes_two_spaces() {
    assert_lints_to(
        "\
fn f()
\tlet x = 1
\t\tx
end
",
        "\
fn f()
  let x = 1
  x
end
",
    );
}

#[test]
fn nesting_adds_two_spaces_per_level() {
    assert_lints_to(
        "\
fn f(items)
for item in items do
if item > 0 then
while item > 0 do
print(item)
end
end
end
end
",
        "\
fn f(items)
  for item in items do
    if item > 0 then
      while item > 0 do
        print(item)
      end
    end
  end
end
",
    );
}

#[test]
fn elsif_and_else_align_with_their_if() {
    assert_lints_to(
        "\
fn describe(n)
if n < 0 then
\"negative\"
   elsif n == 0 then
\"zero\"
      else
\"positive\"
end
end
",
        "\
fn describe(n)
  if n < 0 then
    \"negative\"
  elsif n == 0 then
    \"zero\"
  else
    \"positive\"
  end
end
",
    );
}

#[test]
fn match_arrow_arms_indent_one_level() {
    assert_lints_to(
        "\
fn label(n)
match n
when 0 -> \"zero\"
when 1 -> \"one\"
when _ -> \"many\"
end
end
",
        "\
fn label(n)
  match n
    when 0 -> \"zero\"
    when 1 -> \"one\"
    when _ -> \"many\"
  end
end
",
    );
}

#[test]
fn match_do_arm_bodies_indent_one_deeper() {
    assert_lints_to(
        "\
let r = match 2
when 1 do
print(\"one\")
true
end
when _ -> false
end
",
        "\
let r = match 2
  when 1 do
    print(\"one\")
    true
  end
  when _ -> false
end
",
    );
}

#[test]
fn when_guard_if_does_not_open_a_block() {
    assert_lints_to(
        "\
fn f(t)
match t
when x if x < 0 -> \"neg\"
when x if x > 0 do
print(x)
\"pos\"
end
when _ -> \"zero\"
end
end
",
        "\
fn f(t)
  match t
    when x if x < 0 -> \"neg\"
    when x if x > 0 do
      print(x)
      \"pos\"
    end
    when _ -> \"zero\"
  end
end
",
    );
}

#[test]
fn enum_variants_indent_one_level() {
    assert_lints_to(
        "\
enum Light
Red
Yellow
Green(brightness)
end
",
        "\
enum Light
  Red
  Yellow
  Green(brightness)
end
",
    );
}

#[test]
fn multiline_list_and_record_indent_one_level() {
    assert_lints_to(
        "\
let xs = [
1,
2,
3
]
let point = {
x: 1
y: 2
}
",
        "\
let xs = [
  1,
  2,
  3
]
let point = {
  x: 1
  y: 2
}
",
    );
}

#[test]
fn multiline_call_args_indent_and_closer_realigns() {
    assert_lints_to(
        "\
print(
1,
2
)
",
        "\
print(
  1,
  2
)
",
    );
}

#[test]
fn closer_run_realigns_with_the_opening_line() {
    // `column([` opens two constructs on one line; its contents indent once
    // and the `])` run realigns with the opening line.
    assert_lints_to(
        "\
layout(
column([
editor(),
toolbar()
])
)
",
        "\
layout(
  column([
    editor(),
    toolbar()
  ])
)
",
    );
}

#[test]
fn arrow_lambda_stays_inline() {
    assert_fixed_point(
        "\
let doubled = map([1, 2, 3], fn(x) -> x * 2)
let evens = filter([1, 2, 3, 4], fn(x) -> x % 2 == 0)
",
    );
}

#[test]
fn block_lambda_body_indents_and_end_closes() {
    assert_lints_to(
        "\
let f = fn(x)
let y = x * 2
y + 1
end
",
        "\
let f = fn(x)
  let y = x * 2
  y + 1
end
",
    );
}

#[test]
fn block_lambda_as_call_argument() {
    assert_lints_to(
        "\
let ys = map([1, 2], fn(x)
x * 10
end)
",
        "\
let ys = map([1, 2], fn(x)
  x * 10
end)
",
    );
}

#[test]
fn jsx_children_indent_and_close_tag_dedents() {
    assert_lints_to(
        "\
let e = <div class=\"box\">
<p>hi</p>
<br/>
</div>
",
        "\
let e = <div class=\"box\">
  <p>hi</p>
  <br/>
</div>
",
    );
}

#[test]
fn gt_inside_jsx_attribute_brace_is_not_a_tag_end() {
    // The full parser currently rejects `>` inside a JSX attribute brace, so
    // this case is pinned at the re-indenter layer (which is token-driven and
    // must not mistake that `>` for the end of the opening tag).
    let src = "\
let e = <div wide={x > 1}>
<p>y</p>
</div>
";
    let out = petal::lint::reindent(src).expect("reindent");
    assert_eq!(
        out,
        "\
let e = <div wide={x > 1}>
  <p>y</p>
</div>
",
    );
}

#[test]
fn raw_string_interior_lines_are_untouched() {
    assert_lints_to(
        "\
fn banner()
let s = \"\"\"
   spaced   art
      more art
\"\"\"
s
end
",
        "\
fn banner()
  let s = \"\"\"
   spaced   art
      more art
\"\"\"
  s
end
",
    );
}

#[test]
fn comments_indent_with_their_block() {
    assert_lints_to(
        "\
fn f()
// leading comment
let x = 1 // trailing comment
    // over-indented comment
x
end
",
        "\
fn f()
  // leading comment
  let x = 1 // trailing comment
  // over-indented comment
  x
end
",
    );
}

#[test]
fn comment_only_file_is_a_fixed_point() {
    assert_fixed_point("// just a comment\n");
}

#[test]
fn trailing_whitespace_is_trimmed() {
    assert_lints_to(
        "let x = 1   \nlet y = 2\t\nprint(x + y)  \n",
        "let x = 1\nlet y = 2\nprint(x + y)\n",
    );
}

#[test]
fn whitespace_only_lines_become_empty() {
    assert_lints_to("let x = 1\n   \nprint(x)\n", "let x = 1\n\nprint(x)\n");
}

#[test]
fn interior_blank_lines_are_preserved() {
    assert_fixed_point(
        "\
fn f()

  let x = 1

  x
end
",
    );
}

#[test]
fn final_newline_is_added_and_eof_blanks_dropped() {
    assert_lints_to("let x = 1", "let x = 1\n");
    assert_lints_to("let x = 1\n\n\n\n", "let x = 1\n");
}

#[test]
fn empty_source_stays_empty() {
    assert_lints_to("", "");
}

#[test]
fn inline_if_expression_is_neutral() {
    assert_fixed_point(
        "\
let n = 42
let parity = if n % 2 == 0 then \"even\" else \"odd\" end
print(parity)
",
    );
}

#[test]
fn string_interpolation_is_neutral() {
    assert_fixed_point(
        "\
let total = 7
print(\"sum = {total + (3 * 2)} done\")
",
    );
}

#[test]
fn well_formatted_program_is_a_fixed_point() {
    assert_fixed_point(
        "\
// A realistic program already in house style.
enum Shape
  Circle(r)
  Square(s)
end

fn area(shape)
  match shape
    when Circle(r) -> 3 * r * r
    when Square(s) -> s * s
  end
end

fn describe(shapes)
  for s in shapes do
    print(\"area = {area(s)}\")
  end
end

describe([Circle(1), Square(2)])
",
    );
}

#[test]
fn kitchen_sink_reindent() {
    assert_lints_to(
        "\
enum Op
Inc
Dec
end
fn apply(op, n)
    match op
   when Inc do
  n + 1
     end
     when Dec -> n - 1
      end
end
fn run(ops)
let n = 0
for op in ops do
print(apply(op, n))
end
end
run([
Inc,
Dec
])
",
        "\
enum Op
  Inc
  Dec
end
fn apply(op, n)
  match op
    when Inc do
      n + 1
    end
    when Dec -> n - 1
  end
end
fn run(ops)
  let n = 0
  for op in ops do
    print(apply(op, n))
  end
end
run([
  Inc,
  Dec
])
",
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Pass 2 — rebind: `x = f(x)` → `f(@x)`
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn rebind_basic_user_function() {
    assert_lints_to(
        "\
fn double(n)
n * 2
end
let x = 3
x = double(x)
print(x)
",
        "\
fn double(n)
  n * 2
end
let x = 3
double(@x)
print(x)
",
    );
}

#[test]
fn rebind_builtin_append() {
    assert_lints_to(
        "\
let nums = [1, 2, 3]
nums = append(nums, 4)
print(nums)
",
        "\
let nums = [1, 2, 3]
append(@nums, 4)
print(nums)
",
    );
}

#[test]
fn rebind_matches_any_argument_position() {
    assert_lints_to(
        "\
fn first(a, b)
a
end
fn second(a, b)
b
end
let u = 1
let v = 2
u = first(u, 9)
v = second(9, v)
print(u, v)
",
        "\
fn first(a, b)
  a
end
fn second(a, b)
  b
end
let u = 1
let v = 2
first(@u, 9)
second(9, @v)
print(u, v)
",
    );
}

#[test]
fn rebind_inside_fn_body() {
    assert_lints_to(
        "\
fn grow()
let items = [1]
items = append(items, 2)
items
end
print(grow())
",
        "\
fn grow()
  let items = [1]
  append(@items, 2)
  items
end
print(grow())
",
    );
}

#[test]
fn rebind_inside_for_body() {
    assert_lints_to(
        "\
let out = []
for i in range(0, 3) do
out = append(out, i)
end
print(out)
",
        "\
let out = []
for i in range(0, 3) do
  append(@out, i)
end
print(out)
",
    );
}

#[test]
fn rebind_inside_while_body() {
    assert_lints_to(
        "\
let i = 0
let acc = []
while i < 3 do
acc = append(acc, i)
i = i + 1
end
print(acc)
",
        "\
let i = 0
let acc = []
while i < 3 do
  append(@acc, i)
  i = i + 1
end
print(acc)
",
    );
}

#[test]
fn rebind_inside_if_and_else_bodies() {
    assert_lints_to(
        "\
let xs = [1]
if len(xs) > 0 then
xs = append(xs, 2)
else
xs = append(xs, 3)
end
print(xs)
",
        "\
let xs = [1]
if len(xs) > 0 then
  append(@xs, 2)
else
  append(@xs, 3)
end
print(xs)
",
    );
}

#[test]
fn rebind_inside_block_lambda_body() {
    assert_lints_to(
        "\
let g = fn(n)
let acc = [n]
acc = append(acc, n)
acc
end
print(g(1))
",
        "\
let g = fn(n)
  let acc = [n]
  append(@acc, n)
  acc
end
print(g(1))
",
    );
}

#[test]
fn rebind_on_state_variable() {
    assert_lints_to(
        "\
fn tick()
state acc = []
acc = append(acc, 1)
len(acc)
end
print(tick())
",
        "\
fn tick()
  state acc = []
  append(@acc, 1)
  len(acc)
end
print(tick())
",
    );
}

#[test]
fn rebind_keeps_trailing_comment() {
    assert_lints_to(
        "\
let n = [1]
n = append(n, 2) // grow the list
print(n)
",
        "\
let n = [1]
append(@n, 2) // grow the list
print(n)
",
    );
}

#[test]
fn rebind_multiline_call_keeps_interior_comments() {
    assert_lints_to(
        "\
fn push2(xs, a, b)
append(append(xs, a), b)
end
let xs = [1]
xs = push2(
xs, // keep me
2,
3
)
print(xs)
",
        "\
fn push2(xs, a, b)
  append(append(xs, a), b)
end
let xs = [1]
push2(
  @xs, // keep me
  2,
  3
)
print(xs)
",
    );
}

#[test]
fn multiple_rebinds_in_one_program() {
    let outcome = lint_outcome(
        "\
let a = [1]
let b = [2]
a = append(a, 10)
b = append(b, 20)
print(a, b)
",
    );
    assert_eq!(
        outcome.output,
        "\
let a = [1]
let b = [2]
append(@a, 10)
append(@b, 20)
print(a, b)
",
    );
    assert_eq!(outcome.rebinds, 2);
}

// ── Rebind skip cases: each must survive lint byte-for-byte (mod formatting) ──

#[test]
fn no_rebind_when_var_appears_twice_in_args() {
    assert_fixed_point("let x = 1\nx = add(x, x)\n");
}

#[test]
fn no_rebind_when_var_also_used_outside_the_arg() {
    assert_fixed_point("let x = 1\nx = add(x, x + 1)\n");
}

#[test]
fn no_rebind_when_var_does_not_appear_in_call() {
    assert_fixed_point("let x = 1\nlet y = 2\nx = double(y)\nprint(x)\n");
}

#[test]
fn no_rebind_when_callee_is_the_var_itself() {
    assert_fixed_point("let x = double\nx = x(1)\n");
}

#[test]
fn no_rebind_when_rhs_is_not_a_call() {
    assert_fixed_point("let x = 1\nx = x + 1\nprint(x)\n");
}

#[test]
fn no_rebind_on_let_bindings() {
    // `let` introduces a new binding; the RHS var is a different variable.
    assert_fixed_point("let x = 1\nlet y = double(x)\nprint(y)\n");
}

#[test]
fn no_rebind_when_callee_is_a_field_access() {
    assert_fixed_point("let x = 1\nx = obj.push(x)\n");
}

#[test]
fn no_rebind_on_field_or_index_targets() {
    assert_fixed_point(
        "\
let p = { x: 1 }
p.x = double(p.x)
let xs = [1]
xs[0] = double(xs[0])
",
    );
}

#[test]
fn no_rebind_when_value_already_contains_at_marker() {
    // An existing `@` in the value could become a second marker on the same
    // call, which the desugarer refuses to lift.
    assert_fixed_point("let x = 1\nlet y = 2\nx = wrap(@y, x)\n");
}

#[test]
fn no_rebind_when_var_is_captured_by_a_nested_lambda() {
    assert_fixed_point("let x = 1\nx = run(x, fn() -> x)\n");
}

#[test]
fn no_rebind_inside_match_arm_bodies() {
    // The desugarer doesn't lift `@` out of match arms, so the linter must
    // not rewrite assignments there.
    assert_fixed_point(
        "\
let x = [1]
let r = match len(x)
  when 1 do
    x = append(x, 2)
    \"one\"
  end
  when _ -> \"other\"
end
print(r)
",
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Both passes together, outcome metadata, and failure modes
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn rebind_and_reindent_compose_in_one_pass() {
    assert_lints_to(
        "\
fn collect(items)
let out = []
for item in items do
if item > 0 then
out = append(out, item)
end
end
out
end
print(collect([1, -2, 3]))
",
        "\
fn collect(items)
  let out = []
  for item in items do
    if item > 0 then
      append(@out, item)
    end
  end
  out
end
print(collect([1, -2, 3]))
",
    );
}

#[test]
fn outcome_reports_no_change_for_clean_source() {
    let src = "let x = 1\nprint(x)\n";
    let outcome = lint_outcome(src);
    assert!(!outcome.changed(src));
    assert_eq!(outcome.rebinds, 0);
    assert_eq!(outcome.reindented_lines, 0);
    assert!(outcome.notes.is_empty());
}

#[test]
fn outcome_counts_reindented_lines() {
    let src = "fn f()\nlet x = 1\nx\nend\n";
    let outcome = lint_outcome(src);
    assert!(outcome.changed(src));
    assert_eq!(outcome.reindented_lines, 2); // the two body lines
    assert_eq!(outcome.rebinds, 0);
}

#[test]
fn unverifiable_rebind_is_skipped_with_a_note_but_formatting_still_applies() {
    // The import can't resolve here, so the IR gate is unavailable: the
    // rebind must be skipped (never applied unverified) while re-indentation
    // still runs.
    let src = "\
import nosuchmodule
fn f()
let a = [1]
a = append(a, 2)
a
end
";
    let outcome = lint_outcome(src);
    assert_eq!(outcome.rebinds, 0);
    assert_eq!(outcome.notes.len(), 1);
    assert!(
        outcome.notes[0].contains("skipped 1 rebind"),
        "unexpected note: {}",
        outcome.notes[0]
    );
    assert_eq!(
        outcome.output,
        "\
import nosuchmodule
fn f()
  let a = [1]
  a = append(a, 2)
  a
end
",
    );
}

#[test]
fn unparseable_source_is_an_error() {
    assert!(lint_source("fn (", &LintOptions::default()).is_err());
    assert!(lint_source("let = 3\n", &LintOptions::default()).is_err());
}

#[test]
fn no_rebind_on_pipe_operator_rhs() {
    // `x = x |> double` is call-shaped after desugaring, but the rebind
    // rewrite must only fire on a literal call expression in the source.
    assert_fixed_point(
        "\
fn double(n)
  n * 2
end
let x = 1
x = x |> double
print(x)
",
    );
}

#[test]
fn no_rebind_on_method_call_rhs() {
    // `xs.append(4)` has a field-access callee, which the rebind rule skips.
    assert_fixed_point("let xs = [1]\nxs = xs.append(4)\nprint(xs)\n");
}
