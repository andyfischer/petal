//! The trivial builtins as VM intrinsics: `float`, `int`, `min`, `max`,
//! `clamp`, `abs`, `floor`, `len` and `type` are lowered to
//! `Inst::Intrinsic`, which the straight-line loop computes without a native
//! call (docs/dev/performance.md, "Inline builtins").
//!
//! The rule under test is that nothing but the time changes: for every
//! argument, the result, the error and the profile are the builtin's. The
//! oracle is the same program with the rewrite off (`fast-intrinsics`), and
//! with the whole optimizer off (`baseline`).

use petal::env::Env;
use petal::native_fn::{NativeEffects, PetalCxt};
use petal::policy::RunPolicy;

fn without() -> RunPolicy {
    RunPolicy::parse("fast-intrinsics").unwrap()
}

fn try_run(src: &str, policy: RunPolicy) -> Result<Vec<String>, String> {
    let mut env = Env::new();
    env.set_echo(false);
    env.set_policy(policy);
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid).unwrap();
    env.run(sid)?;
    Ok(env.take_output())
}

/// What `src` does — its output, or its error — which must be the same with
/// the inline builtins, without them, and with no optimizer at all.
#[track_caller]
fn outcome(src: &str) -> Result<Vec<String>, String> {
    let fast = try_run(src, RunPolicy::FAST);
    assert_eq!(fast, try_run(src, without()), "inline and called builtins disagree:\n{src}");
    assert_eq!(fast, try_run(src, RunPolicy::BASELINE), "fast and baseline disagree:\n{src}");
    fast
}

#[track_caller]
fn run(src: &str) -> Vec<String> {
    outcome(src).unwrap_or_else(|e| panic!("{src}\n-- failed: {e}"))
}

#[track_caller]
fn run_err(src: &str) -> String {
    outcome(src).expect_err("expected a runtime error")
}

/// The bytecode a run of `src` executes under `policy`, disassembled.
fn bytecode(src: &str, policy: RunPolicy) -> String {
    let mut env = Env::new();
    env.set_echo(false);
    env.set_policy(policy);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
    env.bytecode_text(pid).unwrap()
}

/// How many instructions of `src`'s bytecode are inline builtins.
fn inline_count(src: &str) -> usize {
    bytecode(src, RunPolicy::FAST).matches("= intrinsic ").count()
}

// ── Selection ────────────────────────────────────────────────────────────

#[test]
fn each_trivial_builtin_is_lowered_inline() {
    for (call, name) in [
        ("float(x)", "float"),
        ("int(x)", "int"),
        ("min(x, 2)", "min"),
        ("max(x, 2)", "max"),
        ("clamp(x, 0, 2)", "clamp"),
        ("abs(x)", "abs"),
        ("floor(x)", "floor"),
        ("len([x])", "len"),
        ("type(x)", "type"),
    ] {
        let src = format!("let x = 1\nprint({call})\n");
        let text = bytecode(&src, RunPolicy::FAST);
        assert!(
            text.contains(&format!("= intrinsic \"{name}\"")),
            "`{call}` was not lowered inline:\n{text}"
        );
        assert!(!text.contains(&format!("= builtin \"{name}\"")), "{text}");
    }
}

#[test]
fn the_flag_and_the_baseline_leave_them_calls() {
    let src = "let x = 1\nprint(float(x), len([x]))\n";
    for policy in [without(), RunPolicy::BASELINE] {
        let text = bytecode(src, policy);
        assert!(!text.contains("intrinsic"), "{text}");
        assert!(text.contains("= builtin \"float\""), "{text}");
    }
    assert!(bytecode(src, RunPolicy::parse("baseline+intrinsics").unwrap()).contains("intrinsic"));
}

#[test]
fn other_builtins_and_other_call_forms_stay_calls() {
    // Not one of the nine.
    let t = bytecode("print(sqrt(4.0), ceil(1.5), round(2.5), str(1))\n", RunPolicy::FAST);
    assert_eq!(t.matches("= intrinsic ").count(), 0, "{t}");
    // Named arguments the compiler could put in order are a positional
    // call by the time it is lowered; a name that does not bind is refused
    // as before.
    assert_eq!(inline_count("print(clamp(5, hi: 3, lo: 0))\n"), 1);
    assert_eq!(run("print(clamp(5, hi: 3, lo: 0))\n"), ["3"]);
    let bad = "print(clamp(5, low: 0, hi: 3))\n";
    assert!(try_run(bad, RunPolicy::FAST).is_err());
    assert_eq!(try_run(bad, RunPolicy::FAST), try_run(bad, without()));
    // The builtin as a value is a native held in a register, not a call site.
    let src = "\
fn apply(f, x) return f(x) end
let f = float
let fs = {size: len, kind: type}
print(f(2), apply(abs, -3), apply(int, 2.5), fs.size([1, 2]), fs.kind(1))
";
    assert_eq!(inline_count(src), 0);
    assert_eq!(run(src), ["2.0 3 2 2 int"]);
}

#[test]
fn a_call_with_the_wrong_number_of_arguments_is_the_builtins_error() {
    for call in [
        "float()", "float(1, 2)", "int()", "int(1, 2)", "min(1)", "min(1, 2, 3)", "max(1)",
        "max(1, 2, 3)", "clamp(1, 2)", "clamp(1, 2, 3, 4)", "abs()", "abs(1, 2)", "floor()",
        "floor(1, 2)", "len()", "len([], [])", "type()", "type(1, 2)",
    ] {
        let src = format!("print({call})\n");
        // Refused by the compiler or by the native: either way, as before.
        let fast = try_run(&src, RunPolicy::FAST);
        assert!(fast.is_err(), "`{call}` ran: {fast:?}");
        assert_eq!(fast, try_run(&src, without()), "{call}");
    }
}

// ── Shadowing ────────────────────────────────────────────────────────────

#[test]
fn a_user_function_of_the_same_name_is_what_gets_called() {
    let src = "\
fn float(x) return \"mine\" end
fn len(x) return -1 end
fn type(x) return \"thing\" end
fn min(a, b) return a + b end
fn clamp(v, lo, hi) return [v, lo, hi] end
print(float(1), len([1, 2]), type(1), min(1, 2), clamp(1, 2, 3))
print(int(2.5), max(1, 2), abs(-1), floor(1.5))
";
    assert_eq!(run(src), ["mine -1 thing 3 [1, 2, 3]", "2 2 1 1.0"]);
    // The four it does not define are still inline; the five it does are not.
    let text = bytecode(src, RunPolicy::FAST);
    assert_eq!(text.matches("= intrinsic ").count(), 4, "{text}");
    for name in ["float", "len", "type", "min", "clamp"] {
        assert!(!text.contains(&format!("intrinsic \"{name}\"")), "{text}");
    }
}

#[test]
fn a_local_a_parameter_or_a_closure_of_the_same_name_shadows_it() {
    let src = "\
fn apply(float, x) return float(x) end
fn twice(x) return x * 2 end
print(apply(twice, 4))
let abs = fn(x) -> x + 100
print(abs(-1))
fn outer()
  fn max(a, b) return \"inner\" end
  return max(1, 2)
end
print(outer(), max(1, 2))
";
    assert_eq!(run(src), ["8", "99", "inner 2"]);
}

#[test]
fn a_variable_named_like_a_builtin_is_just_a_variable() {
    let src = "\
let len = 3
let type = \"t\"
let min = 1
print(len + min, type)
";
    assert_eq!(run(src), ["4 t"]);
    assert_eq!(inline_count(src), 0);
}

fn native_host_float(cx: &mut PetalCxt) -> Result<u32, String> {
    cx.get_value(1)?;
    cx.push_str("host");
    Ok(1)
}

/// A host that registers its own native under one of the names gets its
/// native: the name no longer resolves to the core builtin's id.
#[test]
fn a_host_native_registered_over_the_name_is_what_gets_called() {
    let src = "print(float(3), int(2.5))\n";
    for policy in [RunPolicy::FAST, without()] {
        let mut env = Env::new();
        env.set_echo(false);
        env.set_policy(policy);
        env.register_native("float", native_host_float, NativeEffects::PURE);
        let pid = env.load_program(src).unwrap();
        let text = env.bytecode_text(pid).unwrap();
        assert!(!text.contains("intrinsic \"float\""), "{text}");
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), ["host 2"]);
    }
}

// ── Results, value by value ──────────────────────────────────────────────

/// The values every builtin is tried on: the ints and floats the inline arms
/// answer for (with the edges of each), and one of everything they do not.
const PRELUDE: &str = "\
enum Color
  red,
  rgb(r, g, b),
end
class Rect
  w,
  h,
end
fn ident(x) return x end
let big = 9223372036854775807
let small = 0 - big - 1
let inf = 1e300 * 1e300
let nan = inf - inf
let vs = [
  0, 1, -1, 7, -7, big, small, 9007199254740993,
  0.0, 0.0 * -1.0, 0.5, -0.5, 2.5, -2.5, 1e19, -1e19, 9007199254740992.0, inf, 0.0 - inf, nan,
  \"12\", \" 3.5 \", \"-4\", \"abc\", \"\", \"h\u{e9}llo\",
  nil, true, false, [1, 2, 3], [], {a: 1}, f64_array(3), ident,
  Color.red, Color.rgb(1, 2, 3), Rect(1, 2), __pending(\"k\"),
]
";

/// How many values `PRELUDE` puts in `vs`, and how many of them (the first)
/// are numbers.
const VALUES: usize = 38;
const NUMBERS: usize = 20;

/// One call per program, since a failing call ends its program. The result
/// is printed with its type, so an int is told from a float.
fn call_program(call: &str) -> String {
    format!("{PRELUDE}let r = {call}\nprint(type(r), r, is_pending(r))\n")
}

#[test]
fn the_value_list_is_what_the_tests_take_it_for() {
    let last = NUMBERS - 1;
    let out = run(&format!(
        "{PRELUDE}print(len(vs), type(vs[{last}]), type(vs[{NUMBERS}]))\n\
         for i in range(0, {NUMBERS}) do assert(type(vs[i]) == \"int\" || type(vs[i]) == \"float\") end\n"
    ));
    assert_eq!(out, [format!("{VALUES} float string")]);
}

#[test]
fn unary_builtins_agree_on_every_value() {
    let mut answered = 0;
    for op in ["float", "int", "abs", "floor", "len", "type"] {
        for i in 0..VALUES {
            let src = call_program(&format!("{op}(vs[{i}])"));
            answered += outcome(&src).is_ok() as usize;
        }
    }
    // Not a tautology: most calls succeed, and a good many fail.
    assert!(answered > 100 && answered < 6 * VALUES, "{answered}");
}

#[test]
fn min_and_max_agree_on_every_pair() {
    // Every pair of numbers, and every value against a number and a string
    // on both sides.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for i in 0..NUMBERS {
        for j in 0..NUMBERS {
            pairs.push((i, j));
        }
    }
    for i in NUMBERS..VALUES {
        for j in [1, 12, 20, 23, i] {
            pairs.push((i, j));
            pairs.push((j, i));
        }
    }
    for op in ["min", "max"] {
        for &(i, j) in &pairs {
            let _ = outcome(&call_program(&format!("{op}(vs[{i}], vs[{j}])")));
        }
    }
}

#[test]
fn clamp_agrees_on_every_triple() {
    // Ints and floats mixed in every position, the edges of each, a NaN in
    // every position, and bounds the wrong way round.
    let nums = [1, 4, 5, 6, 10, 12, 13, 14, 17, 19];
    for a in nums {
        for b in nums {
            for c in nums {
                let _ = outcome(&call_program(&format!("clamp(vs[{a}], vs[{b}], vs[{c}])")));
            }
        }
    }
    // One of everything else, in each position.
    for i in NUMBERS..VALUES {
        for call in [
            format!("clamp(vs[{i}], vs[1], vs[3])"),
            format!("clamp(vs[1], vs[{i}], vs[3])"),
            format!("clamp(vs[1], vs[3], vs[{i}])"),
            format!("clamp(vs[12], vs[{i}], vs[12])"),
        ] {
            let _ = outcome(&call_program(&call));
        }
    }
}

// ── Results, spelled out ─────────────────────────────────────────────────

#[test]
fn conversions() {
    let src = "\
let xs = [3, -3, 2.9, -2.9, 0.0 * -1.0, 1e19, -1e19, (1e300 * 1e300) - (1e300 * 1e300)]
for x in xs do
  print(float(x), int(x), type(float(x)), type(int(x)))
end
";
    assert_eq!(
        run(src),
        [
            "3.0 3 float int",
            "-3.0 -3 float int",
            "2.9 2 float int",
            "-2.9 -2 float int",
            "-0.0 0 float int",
            // Out of range saturates; NaN is 0.
            "10000000000000000000 9223372036854775807 float int",
            "-10000000000000000000 -9223372036854775808 float int",
            "NaN 0 float int",
        ]
    );
    // A string is the native's to convert, at the same call site.
    let src = "for x in [1, \"2\", 2.5, \" 3.5 \"] do print(float(x)) end\n\
               for x in [1, \"2\", 2.5] do print(int(x)) end\n";
    assert_eq!(run(src), ["1.0", "2.0", "2.5", "3.5", "1", "2", "2"]);
}

#[test]
fn min_max_and_clamp_keep_ints_and_the_argument_they_chose() {
    let src = "\
print(min(1, 2), max(1, 2), min(2.0, 1.0), max(2.0, 1.0))
print(min(1, 2.0), min(2.0, 1), max(1, 2.0), max(2.0, 1))
print(min(1, 1.0), min(1.0, 1), max(1, 1.0), max(1.0, 1))
print(clamp(5, 0, 3), clamp(-5, 0, 3), clamp(2, 0, 3), clamp(5, 0.0, 3), clamp(0.5, 0, 1))
print(clamp(5, 3, 0), clamp(5.0, 3.0, 0.0))
let nan = (1e300 * 1e300) - (1e300 * 1e300)
print(min(nan, 1.0), min(1.0, nan), max(nan, 1.0), max(1.0, nan))
print(clamp(nan, 0.0, 1.0), clamp(0.5, nan, 1.0), clamp(0.5, 0.0, nan))
let xs = [10, 20, 30]
print(xs[clamp(7, 0, len(xs) - 1)], xs[min(1, 2)], xs[max(0, -1)])
";
    assert_eq!(
        run(src),
        [
            "1 2 1.0 2.0",
            "1 1 2.0 2.0",
            // A tie keeps the first argument.
            "1 1.0 1 1.0",
            "3 0 2 3.0 0.5",
            "0 0.0",
            // An unordered pair keeps the first argument too.
            "NaN 1.0 NaN 1.0",
            "0.0 0.5 0.5",
            "30 20 10",
        ]
    );
}

#[test]
fn abs_and_floor_keep_ints() {
    let src = "\
print(abs(-3), abs(3), abs(-2.5), abs(0.0 * -1.0), type(abs(-3)), type(abs(-2.5)))
print(floor(3), floor(2.5), floor(-2.5), type(floor(3)), type(floor(2.5)))
";
    assert_eq!(run(src), ["3 3 2.5 0.0 int float", "3 2.0 -3.0 int float"]);
}

#[test]
fn len_counts_elements_and_code_points() {
    let src = "\
print(len([]), len([1, 2, 3]), len(\"\"), len(\"h\u{e9}llo\"), len(f64_array(4)), len(range(0, 10)))
var xs = []
for i in range(0, 5) do
  set xs = append(xs, len(xs))
end
print(xs)
";
    assert_eq!(run(src), ["0 3 0 5 4 10", "[0, 1, 2, 3, 4]"]);
}

#[test]
fn type_names_every_kind_of_value() {
    let src = format!("{PRELUDE}for v in vs do print(type(v)) end\n");
    let out = run(&src);
    // The `Pending` is absorbed by `type` and then by `print`, which emits
    // nothing: one line short.
    assert_eq!(out.len(), VALUES - 1);
    assert_eq!(&out[..2], ["int", "int"]);
    assert_eq!(
        &out[NUMBERS..],
        [
            "string", "string", "string", "string", "string", "string", "nil", "bool", "bool",
            "list", "list", "record", "f64_array", "function", "Color", "Color", "Rect",
        ]
    );
}

// ── Errors ───────────────────────────────────────────────────────────────

#[test]
fn a_wrong_type_is_the_builtins_error_at_the_call() {
    for (call, message) in [
        ("float(nil)", "Expected float at arg 1, got nil"),
        ("float(\"abc\")", "Cannot convert 'abc' to float"),
        ("int([1])", "Cannot convert list to int"),
        ("int(\"x\")", "Cannot convert 'x' to int"),
        ("abs(\"x\")", "abs() expects a number"),
        ("floor(nil)", "floor() expects a number"),
        ("len(3)", "Cannot get length of int"),
        ("len(nil)", "Cannot get length of nil"),
        ("min(1, \"a\")", "Cannot compare int and string"),
        ("max(nil, 1)", "Cannot compare nil and int"),
        ("clamp(\"a\", 0, 1)", "Expected float at arg 1, got string"),
        ("clamp(1, 0, nil)", "Expected float at arg 3, got nil"),
        ("abs(0 - 9223372036854775807 - 1)", "Integer overflow in abs()"),
    ] {
        // Through a function, so no check can refuse the call beforehand.
        let src = format!("fn go()\n  return {call}\nend\nprint(1)\nprint(go())\n");
        let err = run_err(&src);
        assert!(err.contains(message), "`{call}`: {err}");
        // Annotated where the call is, like any failing builtin call.
        assert!(err.contains("line 2"), "`{call}`: {err}");
        assert!(err.contains(call), "`{call}`: {err}");
    }
}

#[test]
fn an_error_inside_a_loop_follows_the_good_iterations() {
    // The same call site answers inline, then hands a string to the native,
    // then fails in the native.
    let src = "\
var seen = []
for v in [1, 2.5, \"3\", nil, 4] do
  set seen = append(seen, float(v))
  print(seen)
end
";
    let fast = try_run(src, RunPolicy::FAST).unwrap_err();
    assert_eq!(fast, try_run(src, without()).unwrap_err());
    assert!(fast.contains("Expected float at arg 1, got nil"), "{fast}");
}

// ── Pending and strictness ───────────────────────────────────────────────

#[test]
fn a_pending_argument_is_absorbed_as_by_the_builtin() {
    let src = "\
let p = __pending(\"k\")
print(is_pending(float(p)), is_pending(int(p)), is_pending(abs(p)), is_pending(floor(p)))
print(is_pending(len(p)), is_pending(type(p)))
print(is_pending(min(p, 1)), is_pending(min(1, p)), is_pending(max(p, 1)), is_pending(max(1, p)))
print(is_pending(clamp(p, 0, 1)), is_pending(clamp(0, p, 1)), is_pending(clamp(0, 0, p)))
";
    assert_eq!(
        run(src),
        ["true true true true", "true true", "true true true true", "true true true"]
    );
}

// ── Who is watching ──────────────────────────────────────────────────────

/// A profiled run counts every one of these calls against its builtin, as it
/// did when they were calls: the profiler takes the general path, where an
/// intrinsic is the builtin call.
#[test]
fn a_profiled_run_attributes_them_to_the_builtin() {
    let src = "\
var s = 0.0
for i in range(0, 50) do
  set s = s + float(i) + clamp(i, 0, 10) + min(i, 3) + len([i]) + abs(0 - i)
  if type(i) == \"int\" then set s = s + int(floor(1.5)) + max(i, 2) end
end
print(s)
";
    let profile = |policy: RunPolicy| {
        let mut env = Env::new();
        env.set_echo(false);
        env.set_policy(policy);
        env.profile_mut().set_enabled(true);
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let mut natives: Vec<(String, u64)> = env
            .profile()
            .natives_by_count()
            .into_iter()
            .map(|(id, n)| (env.native_fn_name(id), n))
            .collect();
        natives.sort();
        let timed: u64 = env.profile().natives_by_time().iter().map(|(_, n, _)| n).sum();
        (natives, timed, env.profile().total_insts(), env.take_output())
    };
    let inline = profile(RunPolicy::FAST);
    let called = profile(without());
    assert_eq!(inline, called);
    for name in ["float", "int", "min", "max", "clamp", "abs", "floor", "len", "type"] {
        let n = inline.0.iter().find(|(n, _)| n == name).map(|(_, n)| *n);
        assert_eq!(n, Some(50), "{name}: {:?}", inline.0);
    }
}

/// The effect audit brackets every native call; with it on the inline arms
/// stand down, so it sees each of these as it always did.
#[test]
fn the_effect_audit_still_sees_every_call() {
    let src = "\
var s = 0
for i in range(0, 20) do set s = s + int(float(i)) + len([i]) end
print(s)
";
    let audited = |policy: RunPolicy| {
        let mut env = Env::new();
        env.set_echo(false);
        env.set_policy(policy);
        env.set_effect_audit(true);
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let report = env.effect_audit_report();
        (format!("{report:?}"), env.take_output())
    };
    let inline = audited(RunPolicy::FAST);
    assert_eq!(inline, audited(without()));
    assert_eq!(inline.1, ["210"]);
}

/// Memoized scopes and the frame gate see nothing new: the builtins are
/// pure, inline or called. Run several frames on one stack, as a host does.
#[test]
fn frames_on_a_kept_stack_agree() {
    let src = "\
fn score(xs)
  var t = 0.0
  for x in xs do
    set t = t + float(clamp(x, 0, 5)) + abs(min(x, 2) - max(x, 3))
  end
  return t
end
state var n = 0
set n = get n + 1
print(get n, score([1, 2, 3, get n]), len(str(get n)), type(get n))
";
    let frames = |policy: RunPolicy| {
        let mut env = Env::new();
        env.set_echo(false);
        env.set_policy(policy);
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        let mut out = Vec::new();
        for _ in 0..12 {
            env.reset_stack(sid).unwrap();
            env.run(sid).unwrap();
            out.extend(env.take_output());
        }
        out
    };
    let fast = frames(RunPolicy::FAST);
    assert_eq!(fast.len(), 12);
    assert_eq!(fast, frames(without()));
    assert_eq!(fast, frames(RunPolicy::BASELINE));
    assert_eq!(fast, frames(RunPolicy::REPLAY));
}
