//! CLI-level tests for `petal bench` — per-call cost of named functions.
//!
//! These shell out to the built `petal` binary (via `CARGO_BIN_EXE_petal`), so
//! they cover argument parsing, name resolution, the repeated-run driver and
//! both report shapes end to end. The measuring itself (`CallBench`: self vs
//! inclusive, recursion, unwinding) has unit tests in `src/profile.rs`; what
//! is under test here is the *command*. Every run pins `--iters`, so nothing
//! waits out the one-second budget.

use std::process::Command;

use serde_json::Value;

/// Path to the freshly built `petal` binary for this test run.
const PETAL: &str = env!("CARGO_BIN_EXE_petal");

fn run(args: &[&str]) -> (String, String, bool) {
    run_env(args, &[])
}

/// [`run`] with the ambient run-policy variables replaced by `env`.
fn run_env(args: &[&str], env: &[(&str, &str)]) -> (String, String, bool) {
    let out = Command::new(PETAL)
        .args(args)
        .env_remove("PETAL_OPT")
        .env_remove("PETAL_POLICY")
        .envs(env.iter().copied())
        .output()
        .expect("failed to run petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

/// `petal bench --json --iters 2 -e <code> --fn <f>...`, parsed.
fn bench_json(code: &str, fns: &[&str]) -> Value {
    let mut args = vec!["bench", "--json", "--iters", "2", "-e", code];
    for f in fns {
        args.extend(["--fn", f]);
    }
    let (stdout, stderr, ok) = run(&args);
    assert!(ok, "bench exited non-zero; stderr:\n{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}):\n{stdout}"))
}

/// The report entry for the function named `name`.
fn function<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .find(|f| f["name"] == name)
        .unwrap_or_else(|| panic!("no entry for {name} in {report:#}"))
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("expected a number, got {v}"))
}

/// `outer` calls `inner` three times and nothing else; ten calls a run.
const NESTED: &str = "\
fn inner(k)
  k * 2
end

fn outer(n)
  inner(n) + inner(n + 1) + inner(n + 2)
end

fn idle()
  0
end

let total = 0
for i in range(0, 10) do
  total = total + outer(i)
end
print(\"total\", total)
";

#[test]
fn json_report_has_the_documented_shape() {
    let report = bench_json(NESTED, &["outer"]);

    assert_eq!(report["file"], "-e");
    assert_eq!(report["host"], "core");
    assert!(num(&report["timer_overhead_ns"]) > 0.0);
    for variant in ["opt", "no_opt"] {
        let runs = &report["runs"][variant];
        assert_eq!(runs["runs"], 2, "--iters pins the run count");
        assert!(runs["policy"].is_string(), "{runs}");
        assert!(num(&runs["ms_per_run"]["median"]) > 0.0);
        assert!(num(&runs["ms_per_run"]["min"]) > 0.0);
        assert!(num(&runs["lower_ms"]) >= 0.0);
        assert!(num(&runs["instructions_per_run"]) > 0.0);
    }
    assert_eq!(report["runs"]["opt"]["policy"], "fast");

    let f = function(&report, "outer");
    assert_eq!(f["query"], "outer");
    // The line of its `fn`, not of the first expression in its body.
    assert_eq!(f["line"], 5);
    for variant in ["opt", "no_opt"] {
        let v = &f[variant];
        assert_eq!(v["calls"], 20, "10 calls a run, 2 runs");
        assert_eq!(v["calls_per_run"], 10.0);
        assert_eq!(v["outermost_calls"], 20);
        assert_eq!(v["replayed_calls"], 0);
        for key in ["inclusive", "self", "min", "max"] {
            assert!(num(&v["instructions_per_call"][key]) > 0.0, "{key}: {v}");
        }
        for key in ["inclusive", "self", "min", "median", "p95", "max"] {
            assert!(num(&v["ms_per_call"][key]) >= 0.0, "{key}: {v}");
        }
        let ms = &v["ms_per_call"];
        assert!(num(&ms["min"]) <= num(&ms["median"]));
        assert!(num(&ms["median"]) <= num(&ms["p95"]));
        assert!(num(&ms["p95"]) <= num(&ms["max"]));
        assert_eq!(ms["percentiles_sampled"], false);
        for key in [
            "allocations_per_call",
            "copies_per_call",
            "copied_bytes_per_call",
            "gc_collections_per_call",
            "gc_ms_per_call",
        ] {
            assert!(num(&v[key]) >= 0.0, "{key}: {v}");
        }
    }
    for key in ["instructions_per_call", "ms_per_call"] {
        assert!(f["delta_pct"][key].is_number(), "{key}: {f}");
    }
    // The optimizer never adds instructions to this function.
    assert!(num(&f["delta_pct"]["instructions_per_call"]) <= 0.0);
}

#[test]
fn inclusive_counts_callees_and_self_does_not() {
    let report = bench_json(NESTED, &["outer", "inner"]);
    for variant in ["opt", "no_opt"] {
        let outer = &function(&report, "outer")[variant]["instructions_per_call"];
        let inner = &function(&report, "inner")[variant];
        assert_eq!(inner["calls_per_run"], 30.0);
        let per_inner = num(&inner["instructions_per_call"]["inclusive"]);
        // `inner` calls nothing, so all of it is self.
        assert_eq!(per_inner, num(&inner["instructions_per_call"]["self"]));
        // `outer`'s inclusive count is its own instructions plus three inners.
        assert_eq!(
            num(&outer["inclusive"]),
            num(&outer["self"]) + 3.0 * per_inner,
            "{variant}"
        );
    }
}

#[test]
fn recursion_is_not_double_counted() {
    // fact(5) is 6 activations (5, 4, 3, 2, 1, 0) under one outermost call.
    let code = "\
fn fact(n)
  if n <= 0 then 1 else n * fact(n - 1) end
end
print(fact(5) + fact(5))
";
    let report = bench_json(code, &["fact"]);
    for variant in ["opt", "no_opt"] {
        let v = &function(&report, "fact")[variant];
        assert_eq!(v["calls_per_run"], 12.0);
        assert_eq!(v["calls"], 24);
        assert_eq!(v["outermost_calls"], 4);
        // Inclusive is per outermost call and self per call; over the whole
        // run they must describe the same instructions, counted once.
        let inclusive_total = num(&v["instructions_per_call"]["inclusive"]) * 4.0;
        let self_total = num(&v["instructions_per_call"]["self"]) * 24.0;
        assert!(
            (inclusive_total - self_total).abs() < 1e-6,
            "{variant}: inclusive {inclusive_total} vs self {self_total}"
        );
    }
}

#[test]
fn allocations_and_copies_are_counted_per_call() {
    // Appending to a list the function owns copies it every time unoptimized,
    // and is done in place once the optimizer has shown nothing else sees it.
    let code = "\
fn build(n)
  let xs = []
  for i in range(0, n) do
    xs = append(xs, i)
  end
  xs
end
let total = 0
for i in range(0, 5) do
  total = total + len(build(8))
end
print(total)
";
    let report = bench_json(code, &["build"]);
    let f = function(&report, "build");
    assert_eq!(f["no_opt"]["copies_per_call"], 8.0);
    assert!(num(&f["no_opt"]["copied_bytes_per_call"]) > 0.0);
    assert!(num(&f["no_opt"]["allocations_per_call"]) >= 8.0);
    assert_eq!(f["opt"]["copies_per_call"], 0.0);
    assert!(num(&f["opt"]["allocations_per_call"]) >= 1.0);
    assert_eq!(f["delta_pct"]["copies_per_call"], -100.0);
}

#[test]
fn collections_inside_a_call_are_attributed_to_it() {
    // Enough garbage in one call to make the collector run during it.
    let code = "\
fn churn(n)
  let last = []
  for i in range(0, n) do
    last = [i, i + 1, i + 2, i + 3]
  end
  len(last)
end
print(churn(200000))
";
    let report = bench_json(code, &["churn"]);
    let v = &function(&report, "churn")["opt"];
    assert!(num(&v["gc_collections_per_call"]) >= 1.0, "{v}");
    assert!(num(&v["gc_ms_per_call"]) > 0.0, "{v}");
    assert!(num(&v["gc_ms_per_call"]) <= num(&v["ms_per_call"]["inclusive"]));
}

#[test]
fn a_function_never_called_gets_a_zero_row() {
    let report = bench_json(NESTED, &["idle"]);
    let f = function(&report, "idle");
    for variant in ["opt", "no_opt"] {
        let v = &f[variant];
        assert_eq!(v["calls"], 0);
        assert_eq!(v["calls_per_run"], 0.0);
        assert!(v["instructions_per_call"]["inclusive"].is_null(), "{v}");
        assert!(v["ms_per_call"]["median"].is_null(), "{v}");
        assert!(v["allocations_per_call"].is_null(), "{v}");
    }
    assert!(f["delta_pct"]["ms_per_call"].is_null());

    let (stdout, _, ok) = run(&["bench", "--iters", "1", "-e", NESTED, "--fn", "idle"]);
    assert!(ok);
    assert!(stdout.contains("fn idle"), "{stdout}");
    assert!(stdout.contains("never called"), "{stdout}");
}

#[test]
fn an_unknown_function_is_an_error_naming_the_ones_there_are() {
    let (stdout, stderr, ok) = run(&["bench", "--iters", "1", "-e", NESTED, "--fn", "nope"]);
    assert!(!ok);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("no function named 'nope'"), "{stderr}");
    assert!(stderr.contains("inner, outer"), "{stderr}");

    let (stdout, _, ok) = run(&[
        "bench", "--json", "--iters", "1", "-e", NESTED, "--fn", "nope",
    ]);
    assert!(!ok);
    let err: Value = serde_json::from_str(&stdout).expect("JSON error");
    assert_eq!(err["error"], true);
    assert_eq!(err["phase"], "bench");
}

#[test]
fn a_script_that_errors_reports_the_error_and_no_figures() {
    let code = "\
fn pick(xs, i)
  xs[i]
end
print(pick([1, 2], 0))
print(pick([1, 2], 7))
";
    let (stdout, stderr, ok) = run(&["bench", "--iters", "1", "-e", code, "--fn", "pick"]);
    assert!(!ok);
    assert!(stdout.is_empty(), "no report for a failed run:\n{stdout}");
    assert!(stderr.contains("out of bounds"), "{stderr}");

    let (stdout, _, ok) = run(&["bench", "--json", "-e", code, "--fn", "pick"]);
    assert!(!ok);
    let err: Value = serde_json::from_str(&stdout).expect("JSON error");
    assert_eq!(err["phase"], "runtime");
}

#[test]
fn text_report_shows_both_variants_and_hides_script_output() {
    let (stdout, stderr, ok) = run(&[
        "bench", "--iters", "2", "-e", NESTED, "--fn", "outer", "--fn", "inner",
    ]);
    assert!(ok, "{stderr}");
    assert!(
        !stdout.contains("total "),
        "the script's own prints are suppressed:\n{stdout}"
    );
    for needle in [
        "bench -e",
        "(policy fast)",
        "ms to lower",
        "fn outer  line 5",
        "fn inner  line 1",
        "opt vs no-opt",
        "calls/run",
        "instructions/call",
        "ms/call",
        "  min",
        "  median",
        "  p95",
        "  self",
        "allocations/call",
        "copies/call",
        "collections/call",
        "gc ms/call",
    ] {
        assert!(stdout.contains(needle), "missing {needle:?} in:\n{stdout}");
    }
    assert_eq!(stdout.matches("\nfn ").count(), 2);
}

#[test]
fn methods_nested_functions_and_imports_are_all_nameable() {
    let dir = std::env::temp_dir().join(format!("petal-bench-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shapes.ptl"),
        "pub fn double(x)\n  x * 2\nend\n",
    )
    .unwrap();
    let main = dir.join("main.ptl");
    std::fs::write(
        &main,
        "\
import shapes

class Box
  w: int,
  h: int,
end

fn Box.area(b: Box) -> int
  b.w * b.h
end

fn sum_to(n)
  fn step(acc, k)
    acc + k
  end
  let acc = 0
  for k in range(0, n) do
    acc = step(acc, k)
  end
  acc
end

let b = Box(3, 4)
print(b.area() + shapes.double(sum_to(4)))
",
    )
    .unwrap();
    let (stdout, stderr, ok) = run(&[
        "bench",
        "--json",
        "--iters",
        "1",
        "--host",
        "core",
        main.to_str().unwrap(),
        "--fn",
        "area",
        "--fn",
        "step",
        "--fn",
        "double",
    ]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("JSON report");

    // A method is selected by its bare name and reported by its full one.
    let area = function(&report, "Box.area");
    assert_eq!(area["query"], "area");
    assert_eq!(area["opt"]["calls"], 1);
    // A function declared inside another one.
    assert_eq!(function(&report, "step")["opt"]["calls"], 4);
    // A function from an imported module, located in its own file.
    let double = function(&report, "double");
    assert_eq!(double["opt"]["calls"], 1);
    assert_eq!(double["file"], "shapes.ptl");
    assert_eq!(double["line"], 1);
    // Methods and nested functions are located by their declaration too.
    assert_eq!(area["line"], 8);
    assert_eq!(function(&report, "step")["line"], 13);
}

#[test]
fn only_the_core_host_is_accepted() {
    let (_, stderr, ok) = run(&["bench", "--host", "ui", "-e", NESTED, "--fn", "outer"]);
    assert!(!ok);
    assert!(stderr.contains("core host only"), "{stderr}");

    let (_, stderr, ok) = run(&["bench", "-e", NESTED]);
    assert!(!ok);
    assert!(stderr.contains("--fn"), "{stderr}");
}

#[test]
fn functions_sharing_a_name_are_told_apart_by_their_declaration_line() {
    let code = "\
fn a()
  fn helper(x)
    x + 1
  end
  helper(1)
end

fn b()
  fn helper(x)
    x + 2
  end
  helper(1) + helper(2)
end
print(a() + b())
";
    let report = bench_json(code, &["helper"]);
    let found: Vec<(u64, u64)> = report["functions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["line"].as_u64().unwrap(), f["opt"]["calls_per_run"].as_f64().unwrap() as u64))
        .collect();
    assert_eq!(found, [(2, 1), (9, 2)]);
}

#[test]
fn a_callee_reached_through_a_builtin_is_left_out_of_self() {
    // `total` runs `sq` ten times through `map`; those frames are its direct
    // callees just as if it had called them itself.
    let code = "\
fn sq(x)
  x * x
end

fn total(xs)
  len(map(xs, sq))
end
print(total(range(0, 10)))
";
    let report = bench_json(code, &["total", "sq"]);
    for variant in ["opt", "no_opt"] {
        let total = &function(&report, "total")[variant]["instructions_per_call"];
        let sq = &function(&report, "sq")[variant];
        assert_eq!(sq["calls_per_run"], 10.0);
        let sq_insts = num(&sq["instructions_per_call"]["inclusive"]);
        assert_eq!(
            num(&total["inclusive"]),
            num(&total["self"]) + 10.0 * sq_insts,
            "{variant}: {total}"
        );
    }
}

#[test]
fn the_opt_side_is_optimized_whatever_the_environment_says() {
    // `PETAL_OPT=off` makes every `Env` start on the baseline policy. Bench
    // still has to compare optimized against unoptimized, not the baseline
    // against itself.
    for env in [("PETAL_OPT", "off"), ("PETAL_POLICY", "baseline")] {
        let (stdout, stderr, ok) = run_env(
            &["bench", "--json", "--iters", "1", "-e", NESTED, "--fn", "outer"],
            &[env],
        );
        assert!(ok, "{stderr}");
        let report: Value = serde_json::from_str(&stdout).expect("JSON report");
        let opt = num(&report["runs"]["opt"]["instructions_per_run"]);
        let no_opt = num(&report["runs"]["no_opt"]["instructions_per_run"]);
        assert!(opt < no_opt, "{env:?}: opt {opt} vs no-opt {no_opt}");
        assert_eq!(report["runs"]["no_opt"]["policy"], "baseline");
        let f = function(&report, "outer");
        assert!(num(&f["delta_pct"]["instructions_per_call"]) < 0.0, "{f}");
    }
}

#[test]
fn arity_overloads_are_reported_separately_at_their_own_lines() {
    let code = "\
fn area(w)
  w * w
end

fn area(w, h)
  w * h
end
print(area(2), area(2, 3), area(4, 5))
";
    let report = bench_json(code, &["area"]);
    let found: Vec<(u64, u64)> = report["functions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            assert_eq!(f["name"], "area", "no internal #arity suffix: {f}");
            (f["line"].as_u64().unwrap(), f["opt"]["calls_per_run"].as_f64().unwrap() as u64)
        })
        .collect();
    assert_eq!(found, [(1, 1), (5, 2)]);
}
