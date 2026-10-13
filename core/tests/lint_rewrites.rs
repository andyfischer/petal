//! The lint rules that came out of the Moses debrief
//! (docs/tasks/moses-debrief.md, 2.1), end to end through `lint_source`:
//! what each rewrites, what it only reports, and that a rewritten program
//! prints what it printed before.

use petal::env::Env;
use petal::lint::{
    DEAD_ALIAS, Fixes, INT_DIVISION, LintOptions, LintOutcome, NEG_ZERO_SUB, NO_REDUNDANT_CAST,
    PREFER_ANY, PREFER_COMPOUND_ASSIGN, PREFER_FIELD_SET, REIMPLEMENTS_BUILTIN, RULES, RuleFilter,
    SHADOWS_BUILTIN, lint_source,
};

fn lint_outcome(src: &str) -> LintOutcome {
    lint_source(src, &LintOptions::default()).expect("lint_source should succeed")
}

fn run(src: &str) -> String {
    let mut env = Env::new();
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("load failed for {src}: {e}"));
    let sid = env.create_stack(pid).expect("create_stack");
    env.run(sid)
        .unwrap_or_else(|e| panic!("run failed for {src}: {e}"));
    env.take_output().join("\n").trim().to_string()
}

/// `src` lints to `expected`, which lints to itself, and the two print the
/// same thing.
#[track_caller]
fn assert_lints_to(src: &str, expected: &str) {
    let out = lint_outcome(src).output;
    assert_eq!(out, expected, "lint output mismatch");
    let again = lint_outcome(&out);
    assert_eq!(again.output, out, "lint is not a fixed point on its own output");
    assert_eq!(again.fixable(), 0, "a fixable finding is left: {:?}", again.findings);
    assert_eq!(run(src), run(&out), "the rewrite changed what the program prints");
}

/// `(rule, fixable)` of every finding.
fn findings(src: &str) -> Vec<(&'static str, bool)> {
    lint_outcome(src)
        .findings
        .iter()
        .map(|f| (f.rule, f.fixable))
        .collect()
}

// ── no-redundant-cast: float() beside a float ──────────────────────────────

#[test]
fn float_cast_beside_a_float_operand_is_removed() {
    assert_lints_to(
        "\
fn f(i: int, n: num, w: int)
  let a = float(i) * 0.5
  let b = 2.0 / float(n)
  let c = (float(i) - float(w) * 0.5) * 3.0
  let d = float(i + 1) * 0.5
  let e = 1.5 % float(w)
  [a, b, c, d, e]
end
print(f(3, 2, 4))
for k in range(0, 3) do
  print(float(k) * 0.25 + 1.0)
end
",
        "\
fn f(i: int, n: num, w: int)
  let a = i * 0.5
  let b = 2.0 / n
  let c = (i - w * 0.5) * 3.0
  let d = (i + 1) * 0.5
  let e = 1.5 % w
  [a, b, c, d, e]
end
print(f(3, 2, 4))
for k in range(0, 3) do
  print(k * 0.25 + 1.0)
end
",
    );
}

#[test]
fn a_cast_that_decides_the_division_stays() {
    // int / int truncates: neither cast may go when the other operand is an
    // int, and of two casts exactly one goes.
    assert_lints_to(
        "\
fn f(i: int, n: int)
  let a = float(i) / n
  let b = i / float(n)
  let c = float(i) / float(n)
  let d = float(i) / 2
  let e = float(i) + n
  [a, b, c, d, e]
end
print(f(7, 2))
",
        "\
fn f(i: int, n: int)
  let a = float(i) / n
  let b = i / float(n)
  let c = i / float(n)
  let d = float(i) / 2
  let e = float(i) + n
  [a, b, c, d, e]
end
print(f(7, 2))
",
    );
}

#[test]
fn a_cast_of_an_unknown_value_stays() {
    // `x` may be a string, which `float` parses and `*` does not.
    let src = "\
fn f(x, i: int)
  let a = float(x) * 0.5
  let b = float(i) < 0.5
  let c = sqrt(float(i))
  [a, b, c]
end
print(f(\"3\", 2))
var k = 1
for j in range(0.0, 2.0) do
  print(float(j) * 0.5)
end
for j in range(0, 2) do
  j = j + 0.5
  print(float(j) * 0.5)
end
";
    assert!(
        !findings(src).iter().any(|(r, _)| *r == NO_REDUNDANT_CAST),
        "{:?}",
        lint_outcome(src).findings
    );
}

#[test]
fn a_shadowed_float_or_range_is_not_the_builtin() {
    let src = "\
fn range(a, b) [0.5, 1.5] end
fn f()
  for k in range(0, 2) do
    print(float(k) * 2.0)
  end
end
f()
";
    assert!(!findings(src).iter().any(|(r, _)| *r == NO_REDUNDANT_CAST));
}

// ── neg-zero-sub ───────────────────────────────────────────────────────────

#[test]
fn zero_minus_an_int_is_a_negation() {
    assert_lints_to(
        "\
fn f(step: int, n: int)
  let back = 0 - step
  let both = 3 + (0 - step * n)
  let par = 0 - (step + n)
  [back, both, par]
end
print(f(2, 5))
",
        "\
fn f(step: int, n: int)
  let back = -step
  let both = 3 + (-(step * n))
  let par = -(step + n)
  [back, both, par]
end
print(f(2, 5))
",
    );
}

#[test]
fn zero_minus_a_float_is_reported_and_left() {
    // `0.0 - 0.0` is `0.0`; `-0.0` is not, and `atan2` tells them apart.
    let src = "\
fn f(dx: float, i: int, q)
  let a = atan2(0.0 - dx, -1.0)
  let b = 0.0 - i
  let c = 0 - q
  let d = 0 - dx
  [a, b, c, d]
end
print(f(0.0, 2, 1.5))
";
    let out = lint_outcome(src);
    assert_eq!(out.output, src);
    assert_eq!(
        findings(src),
        vec![
            (NEG_ZERO_SUB, false),
            (NEG_ZERO_SUB, false),
            (NEG_ZERO_SUB, false),
            (NEG_ZERO_SUB, false)
        ]
    );
    assert!(out.findings[0].message.contains("sign of a zero"), "{:?}", out.findings[0]);
    // The reason the first is not rewritten.
    assert_eq!(run("print(atan2(0.0 - 0.0, -1.0) == atan2(-0.0, -1.0))"), "false");
}

#[test]
fn a_negation_that_would_start_a_line_is_left() {
    // `-step` on its own line continues the line above.
    let src = "\
fn f(step: int)
  let a = step
  0 - step
end
print(f(2))
";
    let out = lint_outcome(src);
    assert_eq!(out.output, src);
    assert_eq!(findings(src), vec![(NEG_ZERO_SUB, false)]);
}

// ── prefer-any ─────────────────────────────────────────────────────────────

#[test]
fn counting_matches_to_ask_whether_there_is_one_becomes_any() {
    assert_lints_to(
        "\
let fights = [{id: 1, on: true}, {id: 2, on: false}]
let a = len(filter(fights, fn(f) -> f.id == 2 && f.on)) > 0
let b = len(filter(fights, fn(f) -> f.id == 1)) == 0
if len(filter(fights, fn(f) -> len(keys(f)) > 5)) != 0 then print(\"big\") end
print(a, b)
",
        "\
let fights = [{id: 1, on: true}, {id: 2, on: false}]
let a = any(fights, fn(f) -> f.id == 2 && f.on)
let b = !any(fights, fn(f) -> f.id == 1)
if any(fights, fn(f) -> len(keys(f)) > 5) then print(\"big\") end
print(a, b)
",
    );
}

#[test]
fn a_predicate_that_may_have_effects_is_reported_and_left() {
    // `any` would stop calling `seen` at the first match.
    let src = "\
var calls = 0
fn seen(x)
  set calls += 1
  x > 0
end
let hit = len(filter([1, 2, 3], fn(x) -> seen(x))) > 0
print(hit, calls)
";
    let out = lint_outcome(src);
    assert_eq!(out.output, src);
    assert_eq!(findings(src), vec![(PREFER_ANY, false)]);
    assert_eq!(run(src), "true 3");
}

// ── prefer-field-set ───────────────────────────────────────────────────────

#[test]
fn a_spread_that_changes_fields_becomes_field_writes() {
    assert_lints_to(
        "\
var C = {yaw: 0.0, rev: 0, open: true}
fn turn(a)
  set C = {...get C, yaw: a}
  set C = {...get C, rev: get C.rev + 1}
  set C = {...get C, open: false, yaw: a * 2.0}
end
turn(1.5)
print(get C)
let cam = {x: 0, y: 0}
cam = {...cam, x: cam.x + 3}
print(cam)
",
        "\
var C = {yaw: 0.0, rev: 0, open: true}
fn turn(a)
  set C.yaw = a
  set C.rev += 1
  set C.open = false
  set C.yaw = a * 2.0
end
turn(1.5)
print(get C)
let cam = {x: 0, y: 0}
cam.x += 3
print(cam)
",
    );
}

#[test]
fn a_spread_of_what_may_not_be_a_record_is_reported_and_left() {
    // Spreading nil gives a record; a field write to nil fails.
    let src = "\
var T = nil
fn bump()
  set T = {...get T, rev: 1}
end
bump()
print(get T)
";
    let out = lint_outcome(src);
    assert_eq!(out.output, src);
    assert_eq!(findings(src), vec![(PREFER_FIELD_SET, false)]);
    assert_eq!(run(src), "{ rev: 1 }");
}

// ── prefer-compound-assign on places ───────────────────────────────────────

#[test]
fn an_element_or_field_updated_in_place_takes_the_compound_form() {
    assert_lints_to(
        "\
var pop = [10, 20, 30]
var r = {n: {m: 1}}
fn settle(i, d)
  set pop[i] = get pop[i] - d
  set pop[i + 1] = get pop[i + 1] + d
  set r.n.m = get r.n.m * 4
end
settle(0, 3)
print(get pop, get r)
",
        "\
var pop = [10, 20, 30]
var r = {n: {m: 1}}
fn settle(i, d)
  set pop[i] -= d
  set pop[i + 1] += d
  set r.n.m *= 4
end
settle(0, 3)
print(get pop, get r)
",
    );
    assert!(
        lint_outcome("var xs = [1]\nfn f(i) set xs[i] = get xs[i] + 1 end\n")
            .findings
            .iter()
            .all(|f| f.rule == PREFER_COMPOUND_ASSIGN && f.fixable)
    );
}

// ── dead-alias ─────────────────────────────────────────────────────────────

#[test]
fn a_second_name_for_an_unchanging_local_is_removed() {
    assert_lints_to(
        "\
fn route(dist, u)
  let dd = dist
  let first = dd[0]
  let f = first
  let far = fn(i) -> dd[i] > f
  \"{dd[u]} {far(u)} {len(dd)}\"
end
print(route([1, 5, 9], 2))
fn total(xs)
  var acc = [0]
  let peek = fn() -> get acc[0]
  for x in xs do set acc[0] += x end
  let done = acc
  let read = fn() -> done[0]
  read() + done[0] + peek()
end
print(total([1, 2, 3]))
",
        "\
fn route(dist, u)
  let first = dist[0]
  let far = fn(i) -> dist[i] > first
  \"{dist[u]} {far(u)} {len(dist)}\"
end
print(route([1, 5, 9], 2))
fn total(xs)
  var acc = [0]
  let peek = fn() -> get acc[0]
  for x in xs do set acc[0] += x end
  let read = fn() -> get acc[0]
  read() + acc[0] + peek()
end
print(total([1, 2, 3]))
",
    );
}

#[test]
fn a_snapshot_is_not_an_alias() {
    for src in [
        // Written after the alias was taken.
        "fn f()\n  var n = 1\n  let was = n\n  set n = 5\n  was + n\nend\nprint(f())\n",
        // …or on the next pass of the loop.
        "fn f(xs)\n  var n = 0\n  for x in xs do\n    let was = n\n    set n += x\n    print(was)\n  end\n  n\nend\nprint(f([1, 2]))\n",
        // A module-level cell, read into a local.
        "var N = 3\nfn f()\n  let n = N\n  n + 1\nend\nprint(f())\n",
    ] {
        let out = lint_outcome(src);
        assert!(
            !out.findings.iter().any(|f| f.rule == DEAD_ALIAS),
            "{src}: {:?}",
            out.findings
        );
    }
}

// ── the rules that only report ─────────────────────────────────────────────

#[test]
fn a_fn_that_shadows_or_wraps_a_builtin_is_reported() {
    let src = "\
fn sum(xs)
  let t = 0
  for x in xs do t += x end
  t
end
fn gf(rec, key, dflt)
  if rec == nil then dflt else field(rec, key, dflt) end
end
print(sum([1, 2]), gf(nil, \"a\", 3))
";
    let out = lint_outcome(src);
    assert_eq!(out.output, src);
    assert_eq!(
        findings(src),
        vec![(SHADOWS_BUILTIN, false), (REIMPLEMENTS_BUILTIN, false)]
    );
    // What the second message claims.
    assert_eq!(run("print(field(nil, \"a\", 3))"), "3");
}

#[test]
fn int_division_is_opt_in_and_reports_a_truncated_quotient_used_as_a_float() {
    let src = "\
fn f(w: int, i: int)
  let half = float(w / 2)
  let row = (i / w) * 1.5
  let whole = w / 2
  [half, row, whole]
end
print(f(5, 7))
";
    assert_eq!(findings(src), Vec::<(&str, bool)>::new());
    let opts = LintOptions {
        rules: RuleFilter {
            include: Some(vec![INT_DIVISION.to_string()]),
            exclude: Vec::new(),
        },
        ..LintOptions::default()
    };
    let out = lint_source(src, &opts).expect("lint");
    assert_eq!(out.output, src);
    let lines: Vec<usize> = out.findings.iter().map(|f| f.line).collect();
    assert_eq!(lines, vec![2, 3]);
    assert!(out.findings.iter().all(|f| f.rule == INT_DIVISION && !f.fixable));
}

#[test]
fn check_warns_about_a_literal_division_that_drops_a_remainder() {
    let mut env = Env::new();
    let pid = env
        .load_program("let half = 1 / 2\nlet two = 4 / 2\nlet f = 1.0 / 2\nprint(half, two, f)\n")
        .expect("load");
    let warnings: Vec<String> = env
        .get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].starts_with("`1 / 2` is 0"), "{warnings:?}");
}

// ── the rule table ─────────────────────────────────────────────────────────

#[test]
fn the_rule_table_says_which_rules_fix_and_which_are_opt_in() {
    let by_name = |n: &str| RULES.iter().find(|r| r.name == n).expect(n);
    for name in [PREFER_FIELD_SET, NEG_ZERO_SUB, PREFER_ANY, DEAD_ALIAS] {
        assert_eq!(by_name(name).fixes, Fixes::Provable, "{name}");
        assert!(!by_name(name).opt_in, "{name}");
    }
    for name in [SHADOWS_BUILTIN, REIMPLEMENTS_BUILTIN] {
        assert_eq!(by_name(name).fixes, Fixes::None, "{name}");
        assert!(!by_name(name).opt_in, "{name}");
    }
    assert!(by_name(INT_DIVISION).opt_in);
    assert_eq!(by_name(INT_DIVISION).fixes, Fixes::None);
}

#[test]
fn an_ignore_comment_silences_a_report_only_finding() {
    let src = "\
// petal-lint-ignore shadows-builtin -- this module's own sum, by design
fn sum(xs) 0 end
print(sum([1]))
";
    assert_eq!(findings(src), Vec::<(&str, bool)>::new());
}
