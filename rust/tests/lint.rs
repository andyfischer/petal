// Source→source regression tests for `petal lint` (rust/src/lint.rs).
//
// Every case asserts the exact linted output for a given input source, so any
// behavior change in the re-indenter or the cast rewrite shows up as a
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
Red,
Yellow,
Green(brightness),
end
",
        "\
enum Light
  Red,
  Yellow,
  Green(brightness),
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
x: 1,
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
  x: 1,
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
  Circle(r),
  Square(s),
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
Inc,
Dec,
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
  Inc,
  Dec,
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
// Pass 2 — identity casts: `int(n)` where `n` is already an `int`
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn identity_cast_on_a_typed_local_is_dropped() {
    assert_lints_to(
        "\
let n = 5
let m = int(n)
print(m)
",
        "\
let n = 5
let m = n
print(m)
",
    );
}

#[test]
fn each_cast_builtin_drops_on_its_own_type() {
    assert_lints_to(
        "let a = int(1)\nlet b = float(1.5)\nlet c = str(\"hi\")\nprint([a, b, c])\n",
        "let a = 1\nlet b = 1.5\nlet c = \"hi\"\nprint([a, b, c])\n",
    );
}

/// The whole point of the rule: int arithmetic stays int (`7 / 2` is `3`), so
/// wrapping it is a no-op — while a float operand anywhere makes the cast real.
#[test]
fn int_arithmetic_needs_no_int_cast_but_float_arithmetic_does() {
    assert_lints_to(
        "let w = 100\nprint(int(w / 2))\n",
        "let w = 100\nprint(w / 2)\n",
    );
    assert_fixed_point("let w = 100\nprint(int(w * 0.6))\n");
}

/// `clamp` is int-preserving (405562a), so an all-int clamp is an `int`: the
/// redundant cast is the `int` and the `float` is a real conversion. The pair
/// is kept both ways round because it is the result *type* that decides, not
/// the spelling of the call.
#[test]
fn builtin_result_types_drive_the_rule() {
    assert_lints_to(
        "let v = clamp(3, 0, 10)\nprint(int(v))\n",
        "let v = clamp(3, 0, 10)\nprint(v)\n",
    );
    assert_fixed_point("let v = clamp(3, 0, 10)\nprint(float(v))\n");
    // One float argument makes the whole call a float, flipping which cast is
    // the identity.
    assert_lints_to(
        "let v = clamp(3.0, 0, 10)\nprint(float(v))\n",
        "let v = clamp(3.0, 0, 10)\nprint(v)\n",
    );
    // `len` is an int; `sqrt` is a float.
    assert_lints_to(
        "let xs = [1, 2]\nprint(int(len(xs)))\n",
        "let xs = [1, 2]\nprint(len(xs))\n",
    );
    assert_fixed_point("print(int(sqrt(2.0)))\n");
}

/// `round`/`floor`/`ceil`/`abs` preserve int-ness rather than producing one, so
/// the cast is redundant on an int and load-bearing on a float.
#[test]
fn type_preserving_math_builtins_follow_their_argument() {
    assert_lints_to(
        "let n = 5\nprint(int(round(n)))\n",
        "let n = 5\nprint(round(n))\n",
    );
    assert_fixed_point("print(int(round(2.5)))\n");
}

#[test]
fn nested_identity_casts_all_collapse() {
    assert_lints_to(
        "let n = 5\nprint(int(int(int(n))))\n",
        "let n = 5\nprint(n)\n",
    );
}

#[test]
fn parens_are_kept_where_precedence_needs_them() {
    // Operand of a binary op: the argument must stay grouped.
    assert_lints_to(
        "let n = 5\nprint(2 * int(n + 1))\n",
        "let n = 5\nprint(2 * (n + 1))\n",
    );
    // Already-delimited slots drop the parens entirely.
    assert_lints_to(
        "let n = 5\nlet m = int(n + 1)\nprint(m)\n",
        "let n = 5\nlet m = n + 1\nprint(m)\n",
    );
    assert_lints_to(
        "let n = 5\nprint(int(n + 1))\n",
        "let n = 5\nprint(n + 1)\n",
    );
}

/// An argument with an explicit comma on each side is unambiguous, so the
/// grouping goes away with the cast.
#[test]
fn comma_delimited_slots_drop_their_parens() {
    assert_lints_to(
        "let n = 5\nprint(int(n + 1), int(n + 2))\n",
        "let n = 5\nprint(n + 1, n + 2)\n",
    );
    assert_lints_to(
        "let n = 5\nprint([1, int(n + 1)])\n",
        "let n = 5\nprint([1, n + 1])\n",
    );
}

/// Commas are required between elements, so a list or argument slot is always
/// bounded by a real separator — `print(int(n + 1), int(n + 2))` cannot become
/// a single juxtaposed element once the parens go. The fix therefore always
/// applies here; it no longer has to inspect the source for real commas first.
#[test]
fn a_list_slot_drops_its_parens() {
    // Commas are required between elements, so a cast in a list/argument slot
    // is always separated from its neighbours: the parens can always go.
    assert_lints_to(
        "let n = 5\nprint(int(n + 1), int(n + 2))\n",
        "let n = 5\nprint(n + 1, n + 2)\n",
    );
    assert_lints_to(
        "let n = 5\nlet xs = [int(n + 1), 2]\nprint(xs)\n",
        "let n = 5\nlet xs = [n + 1, 2]\nprint(xs)\n",
    );
    assert_lints_to(
        "let n = 5\nlet r = { k: int(n + 1) }\nprint(r)\n",
        "let n = 5\nlet r = { k: n + 1 }\nprint(r)\n",
    );
}

#[test]
fn a_trailing_comment_survives_the_rewrite() {
    assert_lints_to(
        "let n = 5\nlet m = int(n) // keep me\nprint(m)\n",
        "let n = 5\nlet m = n // keep me\nprint(m)\n",
    );
}

/// A comment between the argument and the closing paren would be inside the
/// text the fix deletes, so the span check rejects the edit and the cast stays.
#[test]
fn a_comment_inside_the_call_parens_blocks_the_fix() {
    assert_fixed_point("let n = 5\nlet m = int(n // why\n)\nprint(m)\n");
}

#[test]
fn casts_are_left_alone_when_the_type_is_unknown() {
    // A parameter with no annotation infers `any`: nothing is provable.
    assert_fixed_point("fn f(x)\n  int(x)\nend\nprint(f(1))\n");
    // A `var` cell may be re-`set` from anywhere, so its initializer says
    // nothing about what a read observes.
    assert_fixed_point("var n = 5\nprint(int(n))\n");
    // A user function of the same name shadows the builtin.
    assert_fixed_point("fn int(v)\n  v * 2\nend\nlet n = 5\nprint(int(n))\n");
    // So does a local binding.
    assert_fixed_point("let n = 5\nlet int = fn(v)\n  v\nend\nprint(int(n))\n");
}

#[test]
fn declared_types_make_the_rule_fire() {
    assert_lints_to(
        "fn f(x: int) -> int\n  int(x)\nend\nprint(f(1))\n",
        "fn f(x: int) -> int\n  x\nend\nprint(f(1))\n",
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Both passes together, outcome metadata, and failure modes
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn casts_and_reindent_compose_in_one_pass() {
    assert_lints_to(
        "\
fn widths()
let w = 640
let cols = 4
let out = []
for i in range(0, cols) do
out = append(out, int(w / cols))
end
out
end
print(widths())
",
        "\
fn widths()
  let w = 640
  let cols = 4
  let out = []
  for i in range(0, cols) do
    out = append(out, w / cols)
  end
  out
end
print(widths())
",
    );
}

#[test]
fn outcome_reports_no_change_for_clean_source() {
    let src = "let x = 1\nprint(x)\n";
    let outcome = lint_outcome(src);
    assert!(!outcome.changed(src));
    assert_eq!(outcome.casts_removed, 0);
    assert_eq!(outcome.reindented_lines, 0);
    assert!(outcome.notes.is_empty());
}

#[test]
fn outcome_counts_reindented_lines_and_casts() {
    let src = "fn f()\nlet x = 1\nx\nend\n";
    let outcome = lint_outcome(src);
    assert!(outcome.changed(src));
    assert_eq!(outcome.reindented_lines, 2); // the two body lines
    assert_eq!(outcome.casts_removed, 0);

    let outcome = lint_outcome("let n = 5\nprint(int(n) + int(n))\n");
    assert_eq!(outcome.casts_removed, 2);
    assert_eq!(outcome.notes.len(), 1);
    assert!(
        outcome.notes[0].contains("2 redundant cast"),
        "{:?}",
        outcome.notes
    );
}

/// A file whose imports can't resolve here still gets both passes: the cast
/// rule is a source-level analysis and needs no compile step. Names that come
/// from the unresolved import simply infer `any`, so nothing is proposed for
/// them.
#[test]
fn unresolvable_import_does_not_block_the_rules() {
    let outcome = lint_outcome(
        "\
import nosuchmodule
fn f()
let n = 5
int(n)
end
",
    );
    assert_eq!(outcome.casts_removed, 1);
    assert_eq!(
        outcome.output,
        "\
import nosuchmodule
fn f()
  let n = 5
  n
end
",
    );
}

#[test]
fn unparseable_source_is_an_error() {
    assert!(lint_source("fn (", &LintOptions::default()).is_err());
    assert!(lint_source("let = 3\n", &LintOptions::default()).is_err());
}

// ═══════════════════════════════════════════════════════════════════════════
// Pass 3 — `if`/`elsif` chain to `match`
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn string_chain_becomes_a_match() {
    assert_lints_to(
        "\
fn object_kind(ch)
  if ch == \"@\" then \"spawn\"
  elsif ch == \"o\" then \"coin\"
  elsif ch == \"w\" then \"walker\"
  else nil
  end
end
",
        "\
fn object_kind(ch)
  match ch
    when \"@\" -> \"spawn\"
    when \"o\" -> \"coin\"
    when \"w\" -> \"walker\"
    when _ -> nil
  end
end
",
    );
}

/// An `if` that falls off the end yields nil, but a `match` with no arm left
/// is a runtime error — so the wildcard has to be written out.
#[test]
fn a_chain_with_no_else_gains_an_explicit_nil_arm() {
    assert_lints_to(
        "\
fn f(c)
  if c == \"a\" then 1
  elsif c == \"b\" then 2
  elsif c == \"c\" then 3
  end
end
",
        "\
fn f(c)
  match c
    when \"a\" -> 1
    when \"b\" -> 2
    when \"c\" -> 3
    when _ -> nil
  end
end
",
    );
}

#[test]
fn bool_and_nil_literals_convert() {
    assert_lints_to(
        "\
fn f(c)
  if c == true then \"yes\"
  elsif c == false then \"no\"
  elsif c == nil then \"unset\"
  else \"other\"
  end
end
",
        "\
fn f(c)
  match c
    when true -> \"yes\"
    when false -> \"no\"
    when nil -> \"unset\"
    when _ -> \"other\"
  end
end
",
    );
}

#[test]
fn a_field_path_is_a_valid_subject() {
    assert_lints_to(
        "\
fn f(ev)
  if ev.kind == \"down\" then 1
  elsif ev.kind == \"up\" then 2
  elsif ev.kind == \"move\" then 3
  else 0
  end
end
",
        "\
fn f(ev)
  match ev.kind
    when \"down\" -> 1
    when \"up\" -> 2
    when \"move\" -> 3
    when _ -> 0
  end
end
",
    );
}

/// `==` compares an int against a float numerically (`1 == 1.0` is true) while
/// `when 1` does not match `1.0`, so a numeric chain is left alone: rewriting
/// it would change which arm a float subject takes.
#[test]
fn numeric_literals_are_never_converted() {
    assert_fixed_point(
        "\
fn f(n)
  if n == 1 then \"a\"
  elsif n == 2 then \"b\"
  elsif n == 3 then \"c\"
  else \"z\"
  end
end
",
    );
}

/// Two arms is `if … else … end`, which is the plainer spelling.
#[test]
fn a_two_arm_chain_stays_an_if() {
    assert_fixed_point(
        "\
fn f(c)
  if c == \"a\" then 1
  elsif c == \"b\" then 2
  else 3
  end
end
",
    );
}

#[test]
fn a_chain_over_two_different_subjects_stays_an_if() {
    assert_fixed_point(
        "\
fn f(c, d)
  if c == \"a\" then 1
  elsif d == \"b\" then 2
  elsif c == \"c\" then 3
  else 4
  end
end
",
    );
}

/// `match` reads the subject once where the chain read it per arm, so nothing
/// that could compute is moved into the head.
#[test]
fn a_call_subject_stays_an_if() {
    assert_fixed_point(
        "\
fn f()
  if key() == \"a\" then 1
  elsif key() == \"b\" then 2
  elsif key() == \"c\" then 3
  else 4
  end
end
",
    );
}

/// A `->` arm takes an expression; a multi-statement arm would need the
/// `do … end` form, which this rule does not emit.
#[test]
fn a_multi_statement_arm_stays_an_if() {
    assert_fixed_point(
        "\
fn f(c)
  if c == \"a\" then
    print(1)
    print(2)
  elsif c == \"b\" then 2
  elsif c == \"c\" then 3
  else 4
  end
end
",
    );
}

/// A comment in the glue between arms has no home in the `match`, so the whole
/// chain is skipped rather than dropping it. (The re-indenter still moves the
/// comment line, so this is not a fixed point — what matters is that the
/// `elsif`s survive and the comment is still there.)
#[test]
fn a_comment_between_arms_blocks_the_fix() {
    let outcome = lint_outcome(
        "\
fn f(c)
  if c == \"a\" then 1
  elsif c == \"b\" then 2
  // fall through to c
  elsif c == \"c\" then 3
  else 4
  end
end
",
    );
    assert_eq!(outcome.chains_to_match, 0);
    assert!(outcome.output.contains("elsif"), "{}", outcome.output);
    assert!(
        outcome.output.contains("// fall through to c"),
        "{}",
        outcome.output
    );
}

#[test]
fn a_trailing_comment_on_an_arm_blocks_the_fix() {
    assert_fixed_point(
        "\
fn f(c)
  if c == \"a\" then 1 // first
  elsif c == \"b\" then 2
  elsif c == \"c\" then 3
  else 4
  end
end
",
    );
}

/// The rule rewrites the glue only, so a comment inside an arm's *body* is
/// carried across untouched.
#[test]
fn a_comment_inside_an_arm_body_survives() {
    assert_lints_to(
        "\
fn f(c)
  if c == \"a\" then g(1, // why
    2)
  elsif c == \"b\" then 2
  elsif c == \"c\" then 3
  else 4
  end
end
",
        "\
fn f(c)
  match c
    when \"a\" -> g(1, // why
      2)
    when \"b\" -> 2
    when \"c\" -> 3
    when _ -> 4
  end
end
",
    );
}

#[test]
fn a_nested_chain_converts_too() {
    let outcome = lint_outcome(
        "\
fn f(a, b)
  if a == \"x\" then
    if b == \"p\" then 1
    elsif b == \"q\" then 2
    elsif b == \"r\" then 3
    else 0
    end
  elsif a == \"y\" then 9
  elsif a == \"z\" then 8
  else 7
  end
end
",
    );
    // The outer arm holds a single statement that is itself an `if`
    // expression, so it satisfies the one-expression rule and both chains
    // convert. Their splices are disjoint — the inner ones fall entirely
    // inside a body span the outer rewrite preserves.
    assert_eq!(outcome.chains_to_match, 2);
    assert!(outcome.output.contains("match a"), "{}", outcome.output);
    assert!(outcome.output.contains("match b"), "{}", outcome.output);
    assert!(!outcome.output.contains("elsif"), "{}", outcome.output);
}

#[test]
fn a_chain_in_statement_position_converts() {
    assert_lints_to(
        "\
fn f(c)
  if c == \"a\" then print(1)
  elsif c == \"b\" then print(2)
  elsif c == \"c\" then print(3)
  end
  print(\"done\")
end
",
        "\
fn f(c)
  match c
    when \"a\" -> print(1)
    when \"b\" -> print(2)
    when \"c\" -> print(3)
    when _ -> nil
  end
  print(\"done\")
end
",
    );
}

#[test]
fn outcome_counts_converted_chains() {
    let outcome = lint_outcome(
        "\
fn f(c)
  if c == \"a\" then 1
  elsif c == \"b\" then 2
  elsif c == \"c\" then 3
  else 4
  end
end
",
    );
    assert_eq!(outcome.chains_to_match, 1);
    assert_eq!(outcome.casts_removed, 0);
    assert!(
        outcome.notes.iter().any(|n| n.contains("1 if/elsif chain")),
        "{:?}",
        outcome.notes
    );
}

/// The two semantic rules compose: the cast inside an arm body goes, and the
/// chain around it becomes a `match`.
#[test]
fn casts_and_the_match_rule_compose_in_one_pass() {
    let outcome = lint_outcome(
        "\
fn f(c, n: int)
  if c == \"a\" then int(n)
  elsif c == \"b\" then int(n) + 1
  elsif c == \"c\" then 3
  else 0
  end
end
",
    );
    assert_eq!(outcome.casts_removed, 2);
    assert_eq!(outcome.chains_to_match, 1);
    assert_eq!(
        outcome.output,
        "\
fn f(c, n: int)
  match c
    when \"a\" -> n
    when \"b\" -> n + 1
    when \"c\" -> 3
    when _ -> 0
  end
end
",
    );
}
