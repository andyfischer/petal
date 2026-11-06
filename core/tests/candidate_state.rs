//! Candidate-state ergonomics (docs/proposals/declarative-dragging.md, P3):
//! `get_in` / `set_in`, the path-as-data forms of `x.a[i].b` and
//! `x.a[i].b = v`, and the multi-generator `for` (`for a in xs, b in ys do …
//! end`), which the parser desugars into nested loops.
//!
//! What they compute is also covered in Petal (test/ptl/set_in.ptl) and as a
//! script case (test/multi-generator-for/). This file pins the refusals, the
//! optimizer on/off oracle, and that a multi-generator loop's `state` lands on
//! exactly the slots the hand-written nested loops use.

use petal::env::Env;
use petal::policy::RunPolicy;
use petal::value;

fn run_with(src: &str, policy: RunPolicy, runs: usize) -> Result<(Vec<String>, String), String> {
    let mut env = Env::new();
    env.set_policy(policy);
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    let mut out = Vec::new();
    for i in 0..runs {
        if i > 0 {
            env.reset_stack(sid)?;
        }
        let v = env.run(sid)?;
        out.extend(env.take_output());
        out.push(format!("=> {}", value::value_to_display_string(&v, env.heap())));
    }
    let state = env.get_state_json(pid, sid);
    let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    Ok((out, pairs.join(",")))
}

fn out(src: &str) -> String {
    match run_with(src, RunPolicy::FAST, 1) {
        Ok((lines, _)) => lines[..lines.len() - 1].join("\n"),
        Err(e) => panic!("run failed for {src:?}: {e}"),
    }
}

#[track_caller]
fn refuses(src: &str, what: &str) {
    match run_with(src, RunPolicy::FAST, 1) {
        Ok((o, _)) => panic!("expected an error for {src:?}, got output {o:?}"),
        Err(e) => assert!(e.contains(what), "{src:?} should fail with {what:?}, got: {e}"),
    }
}

// --- get_in / set_in ---

#[test]
fn set_in_is_the_expression_form_of_a_path_assignment() {
    let src = r#"
let st = {meetings: [{room: 1}, {room: 2}, {room: 5}]}
var by_hand = st
set by_hand.meetings[2].room = 3
let by_path = set_in(st, ["meetings", 2, "room"], 3)
print(by_path == by_hand)
print(st.meetings[2].room)
print(get_in(by_path, ["meetings", -1, "room"]))
print(set_in(st, [], 7))
print(get_in(st, []) == st)
print(set_in({a: 1}, ["b"], 2))
print(set_in(Map(), [4], "four"))
print(set_in([[1, 2], [3]], [0, -1], 9))
"#;
    assert_eq!(
        out(src),
        "true\n5\n3\n7\ntrue\n{ a: 1, b: 2 }\nMap([[4, \"four\"]])\n[[1, 9], [3]]"
    );
}

#[test]
fn a_path_error_names_the_step_that_failed() {
    refuses(
        r#"set_in({a: [1, 2]}, ["a", 5], 0)"#,
        "set_in(): path step 2 of 2 (.a[5]): Index 5 out of bounds (len 2)",
    );
    refuses(
        r#"get_in({a: {b: 1}}, ["a", "c", "d"])"#,
        "get_in(): path step 2 of 3 (.a.c): No key 'c' on record",
    );
    // A missing record field is an error on the way, as `x.a.c.d = v` is.
    refuses(
        r#"set_in({a: {}}, ["a", "c", "d"], 1)"#,
        "set_in(): path step 2 of 3 (.a.c): No key 'c' on record",
    );
    refuses(r#"set_in({a: nil}, ["a", "b"], 1)"#, "path step 2 of 2 (.a.b): .a is nil");
    refuses(r#"get_in(nil, ["a"])"#, "path step 1 of 1 (.a): the value is nil");
    refuses(r#"get_in([1], [true])"#, "path step 1 of 1 ([<bool>]): Cannot index list with bool");
    refuses(
        r#"set_in({a: {}}, ["a", "two words", 0], 1)"#,
        r#"path step 2 of 3 (.a["two words"])"#,
    );
    refuses(r#"set_in({a: 1}, "a", 1)"#, "set_in() expects its path to be a list");
    refuses(r#"get_in({a: 1})"#, "get_in() expects 2 arguments");
}

// --- multi-generator for ---

#[test]
fn a_multi_generator_for_collects_one_flat_list_in_cartesian_order() {
    let src = r#"
print(for a in [1, 2, 3], b in ["x", "y"] do "{a}{b}" end)
print(for a in [1, 2], b in [10, 20] do [a, b] end)
print(for a in range(2), b in range(2), c in range(2) do a * 4 + b * 2 + c end)
print(for a in range(3), b in range(a) do "{a}{b}" end)
print(for a in [1], b in [] do a end)
print(for a in range(3), b in range(3) do
  if a == b then continue end
  a * 10 + b
end)
fn tail()
  for i in range(2), j in range(i + 1) do
    i + j
  end
end
print(tail())
var n = 0
for a in range(3), b in range(4) do
  set n += a * b
end
print(n)
"#;
    assert_eq!(
        out(src),
        "[\"1x\", \"1y\", \"2x\", \"2y\", \"3x\", \"3y\"]\n\
         [[1, 10], [1, 20], [2, 10], [2, 20]]\n\
         [0, 1, 2, 3, 4, 5, 6, 7]\n\
         [\"10\", \"20\", \"21\"]\n\
         []\n\
         [1, 2, 10, 12, 20, 21]\n\
         [0, 1, 2]\n\
         18"
    );
}

/// A program's own `flat` does not take the place of the one the loop uses.
#[test]
fn the_flattening_ignores_a_programs_own_flat() {
    let src = "fn flat(x)\n  \"mine\"\nend\nprint(for a in [1], b in [2, 3] do a + b end)\n";
    assert_eq!(out(src), "[3, 4]");
}

#[test]
fn optimizer_on_and_off_agree() {
    let src = r#"
let grid = for r in range(3), c in range(4) do {r: r, c: c, v: r * c} end
var sum = 0
for cell in grid, k in range(2) do
  set sum += cell.v + k
end
print(len(grid), sum)
let st = {rows: [[0, 0], [0, 0]]}
let cands = for r in range(2), c in range(2) do set_in(st, ["rows", r, c], 1) end
print(cands)
"#;
    let base = run_with(src, RunPolicy::BASELINE, 2).unwrap();
    let fast = run_with(src, RunPolicy::FAST, 2).unwrap();
    assert_eq!(base, fast);
}

/// `state` in the body gets one call-path `Index` part per generator — the
/// very slots the hand-written nested loops use, frame after frame.
#[test]
fn state_in_the_body_keys_like_nested_loops() {
    let multi = r#"
fn cells()
  for a in ["p", "q"] key a, b in range(2) do
    state seen = 0
    seen += 1
    "{a}{b}:{seen}"
  end
end
print(cells())
"#;
    let nested = r#"
fn cells()
  flat(for a in ["p", "q"] key a do
    for b in range(2) do
      state seen = 0
      seen += 1
      "{a}{b}:{seen}"
    end
  end)
end
print(cells())
"#;
    let m = run_with(multi, RunPolicy::FAST, 3).unwrap();
    let n = run_with(nested, RunPolicy::FAST, 3).unwrap();
    assert_eq!(m.0, n.0);
    assert_eq!(m.1, n.1, "state slots differ");
    assert!(m.0[4].contains("p0:3"), "{:?}", m.0);
}

#[test]
fn break_in_a_multi_generator_body_is_refused() {
    refuses(
        "for a in [1], b in [2] do\n  if a > 0 then\n    break\n  end\nend\n",
        "`break` is not allowed in a `for` with several generators",
    );
    // A nested loop's own break, and a lambda's, are not the outer loop's.
    let ok = "for a in [1], b in [2] do\n  for c in [3] do\n    break\n  end\n  print(a + b)\nend\n";
    assert_eq!(out(ok), "3");
}

#[test]
fn a_generator_needs_a_name_and_in() {
    refuses("for a in [1], do\nend\n", "Expected an identifier");
    refuses("for a in [1], b do\nend\n", "Expected `in`");
}
