//! CLI golden tests for `petal propose-edit --v2` (direct manipulation v2,
//! docs/dev/direct-manipulation-v2-plan.md §4.2, M4 acceptance item 1).
//!
//! These shell out to the built `petal` binary. They freeze the `--json`
//! shape (the key sets of the report, its goals, alternatives, changes,
//! edits, collateral, candidates and diagnostics), because M5's study
//! harness and the MCP server read it: a new field means adding its key
//! here, a renamed one is a break. They also cover the `--explain` text,
//! `--apply --pick` writing the file, the v1 output under `--v1` (unchanged),
//! the engine selection rules (v2 by default, v1 only with `--v1`), and the `key` / `native_args` that
//! `run --trace-emits` gained.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// `place(i)` computes a position through a helper; v1 cannot see into it.
const HELPER: &str = "\
import io
config let gap = 30.0
fn place(i)
  return 20 + i * gap
end
for i in range(0, 4) do
  push_output(symbol(\"shapes\"), place(i))
end
";

/// The v1 example of docs/features/direct-manipulation.md.
const V1_SKETCH: &str = "\
import io
let offset = 10
let x = 20
push_output(symbol(\"shapes\"), x + offset)
push_output(symbol(\"shapes\"), \"label\")
";

/// A fresh scratch file holding `src`.
fn scratch(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petal-cli-dm-v2-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.ptl"));
    std::fs::write(&path, src).unwrap();
    path
}

fn run(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(PETAL).args(args).env_remove("PETAL_SEED").output().expect("runs petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

fn run_json(args: &[&str]) -> Value {
    let (stdout, stderr, ok) = run(args);
    assert!(ok, "petal {args:?} failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("not JSON ({e}):\n{stdout}"))
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap_or_else(|| panic!("not an object: {v}")).keys().cloned().collect()
}

fn set(ks: &[&str]) -> BTreeSet<String> {
    ks.iter().map(|k| k.to_string()).collect()
}

fn helper_json(extra: &[&str]) -> Value {
    let path = scratch("helper", HELPER);
    let p = path.to_str().unwrap();
    let mut args = vec!["propose-edit", "--v2", "--json", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--by", "25"];
    args.extend_from_slice(extra);
    args.push(p);
    run_json(&args)
}

#[test]
fn json_shape_is_frozen() {
    let r = helper_json(&["--explain"]);
    assert_eq!(
        keys(&r),
        set(&["engine", "seed", "goals", "alternatives", "candidates", "diagnostics", "explain", "applied"])
    );
    assert_eq!(r["engine"], "v2");
    assert_eq!(r["seed"], 0);
    assert!(r["applied"].is_null());

    let g = &r["goals"][0];
    assert_eq!(
        keys(g),
        set(&[
            "channel", "emit", "kind", "index", "path", "key", "callee", "line", "to", "by", "baseline", "target",
            "tolerance",
        ])
    );
    assert_eq!(g["kind"], "native_arg");
    assert_eq!(g["baseline"], 80.0);
    assert_eq!(g["target"], 105.0);
    assert_eq!(keys(&g["key"]), set(&["chain_hash", "path_hash", "occurrence"]));

    let alts = r["alternatives"].as_array().unwrap();
    assert!(!alts.is_empty());
    let a = &alts[0];
    assert_eq!(
        keys(a),
        set(&[
            "rank", "engine", "verified", "description", "kind", "score", "score_parts", "max_error", "misses",
            "achieved", "predicted", "iterations", "config_only", "stateful", "regime_changed",
            "rng_order_changed", "rng_difference", "changes", "edits", "collateral",
        ])
    );
    assert_eq!(
        keys(&a["score_parts"]),
        set(&["edit", "collateral", "count", "respelled", "regime", "rng", "error"])
    );
    assert_eq!(keys(&a["collateral"]), set(&["norm", "items"]));
    let item = &a["collateral"]["items"][0];
    assert_eq!(
        keys(item),
        set(&["channel", "callee", "line", "max_move", "key", "emit", "arg", "path", "moved", "label"])
    );

    // The top alternative: the config literal, through the helper.
    assert_eq!(a["rank"], 1);
    assert_eq!(a["verified"], true);
    assert_eq!(a["config_only"], true);
    let c = &a["changes"][0];
    assert_eq!(
        keys(c),
        set(&[
            "term", "file", "path", "line", "span", "binding", "function", "config", "old", "new", "old_text",
            "new_text", "respelled",
        ])
    );
    assert_eq!(c["binding"], "gap");
    assert_eq!(c["old_text"], "30.0");
    assert_eq!(c["new_text"], "42.5");
    assert_eq!(c["line"], 2);
    assert_eq!(keys(&c["span"]), set(&["file", "start", "end"]));
    let e = &a["edits"][0];
    assert_eq!(keys(e), set(&["file", "path", "span", "new_text"]));
    assert_eq!(e["new_text"], "42.5");
    assert_eq!(a["achieved"][0], 105.0);

    // The helper's own literal is the runner-up.
    assert!(
        alts.iter().any(|a| a["verified"] == true && a["changes"][0]["function"] == "place"),
        "no verified alternative edits the helper: {alts:#?}"
    );

    let cand = &r["candidates"][0];
    assert_eq!(
        keys(cand),
        set(&[
            "term", "file", "line", "span", "value", "is_int", "binding", "function", "config", "hint", "origin",
            "distance", "focus", "in_goal_frame", "flags", "loaded", "eligible", "ineligible_reason", "quality", "column",
            "weight", "clock_coupled",
        ])
    );
    assert_eq!(
        keys(&cand["flags"]),
        set(&["through_var", "through_state", "state_init", "through_rng", "structural", "hint_bound"])
    );
    assert_eq!(
        keys(&r["diagnostics"]),
        set(&[
            "runs", "wall_ms", "suppressed", "budget_exhausted", "stopped_by", "max_runs", "max_wall_ms", "warnings",
            "discovered", "eligible", "screened", "measured", "truncated", "subsets_predicted", "subsets_verified",
        ])
    );
    assert!(r["diagnostics"]["runs"].as_u64().unwrap() > 0);
    assert!(r["explain"].as_array().is_some_and(|l| !l.is_empty()));
}

#[test]
fn explain_text_lists_the_pipeline_and_candidates() {
    let path = scratch("explain", HELPER);
    let (stdout, stderr, ok) = run(&[
        "propose-edit", "--v2", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--to", "105",
        "--explain", path.to_str().unwrap(),
    ]);
    assert!(ok, "{stderr}");
    assert!(stdout.starts_with("engine v2 · seed 0 · "), "{stdout}");
    assert!(stdout.contains("goal: shapes[2] native-arg 1 (push_output, line 7): 80 -> 105"), "{stdout}");
    assert!(stdout.contains("1. [verified, config] set `gap` to 42.5 (line 2)"), "{stdout}");
    assert!(stdout.contains("line 2 `gap`: 30.0 -> 42.5"), "{stdout}");
    assert!(stdout.contains("\nexplain:\n"), "{stdout}");
    assert!(stdout.contains("discovered "), "{stdout}");
    assert!(stdout.contains("\ncandidates ("), "{stdout}");
    // The loop bound is reported, and why it is not offered.
    assert!(stdout.contains("excluded: structural"), "{stdout}");
}

#[test]
fn apply_needs_a_pick_then_writes_the_file() {
    let path = scratch("apply", HELPER);
    let p = path.to_str().unwrap();
    let base = ["propose-edit", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--to", "105", "--apply"];

    let mut args = base.to_vec();
    args.push(p);
    let (_, stderr, ok) = run(&args);
    assert!(!ok, "several alternatives and no --pick must refuse");
    assert!(stderr.contains("--pick"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), HELPER, "a refusal writes nothing");

    let mut args = base.to_vec();
    args.extend_from_slice(&["--pick", "2", "--json", p]);
    let r = run_json(&args);
    assert_eq!(r["applied"]["pick"], 2);
    assert_eq!(r["applied"]["files"][0], p);
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("return 45 + i * gap"), "{written}");
    assert!(written.contains("config let gap = 30.0"), "{written}");

    // The written program meets the goal.
    let (stdout, _, ok) = run(&["run", "--trace-emits", "--json", p]);
    assert!(ok);
    let t: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(t["channels"]["shapes"][2]["value"], 105.0);
}

#[test]
fn apply_with_one_alternative_needs_no_pick() {
    let path = scratch("apply-one", HELPER);
    let p = path.to_str().unwrap();
    let r = run_json(&[
        "propose-edit", "--json", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--to", "105",
        "--allow", "gap", "--apply", p,
    ]);
    assert_eq!(r["alternatives"].as_array().unwrap().len(), 1, "{r:#}");
    assert_eq!(r["applied"]["pick"], 1);
    assert!(std::fs::read_to_string(&path).unwrap().contains("config let gap = 42.5"));
}

#[test]
fn arg_goals_and_selectors() {
    let path = scratch("arg", HELPER);
    let r = run_json(&[
        "propose-edit", "--v2", "--json", "--channel", "shapes", "--emit", "2", "--arg", "1", "--by", "25",
        "--allow", "fn:place", path.to_str().unwrap(),
    ]);
    assert_eq!(r["goals"][0]["kind"], "arg");
    let alts = r["alternatives"].as_array().unwrap();
    assert_eq!(alts.len(), 1, "{r:#}");
    assert_eq!(alts[0]["changes"][0]["function"], "place");
    assert_eq!(alts[0]["changes"][0]["new_text"], "45");

    let r = run_json(&[
        "propose-edit", "--json", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--by", "25",
        "--deny", "config", path.to_str().unwrap(),
    ]);
    assert!(r["alternatives"].as_array().unwrap().iter().all(|a| a["changes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["binding"] != "gap")));

    // `knob` is an alias of `config`: the same knobs, the same answer.
    let by = |sel: &str| {
        run_json(&[
            "propose-edit", "--json", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--by", "25",
            "--allow", sel, path.to_str().unwrap(),
        ])["alternatives"]
            .clone()
    };
    let knob = by("knob");
    assert_eq!(knob, by("config"));
    assert!(knob.as_array().unwrap().iter().all(|a| a["config_only"] == true), "{knob:#}");
}

#[test]
fn goals_on_two_emits_select_v2() {
    let path = scratch("two", HELPER);
    let r = run_json(&[
        "propose-edit", "--json", "--channel", "shapes", "--emit", "0", "--arg", "1", "--to", "3", "--emit", "1",
        "--arg", "1", "--to", "5", path.to_str().unwrap(),
    ]);
    assert_eq!(r["engine"], "v2");
    assert_eq!(r["goals"].as_array().unwrap().len(), 2);
    assert_eq!(r["goals"][1]["emit"], 1);
    let top = &r["alternatives"][0];
    assert_eq!(top["verified"], true, "{r:#}");
    assert_eq!(top["achieved"], serde_json::json!([3.0, 5.0]));
}

#[test]
fn v1_output_is_unchanged_under_v1() {
    let path = scratch("v1", V1_SKETCH);
    let p = path.to_str().unwrap();
    let (stdout, stderr, ok) = run(&["propose-edit", "--v1", "--channel", "shapes", "--emit", "0", "--arg", "1", "--to", "42.5", p]);
    assert!(ok, "{stderr}");
    assert_eq!(
        stdout,
        "2 proposals:\n  1. set `x` to 32.5 (line 3)\n  2. set `offset` to 22.5 (line 2)\n\
         Narrow with --configurable <var> / --static <var>, or apply one by hand.\n"
    );
    // Without an engine flag, v1-shaped requests get v2 (the default since
    // the study, plan §9). v2 keeps int literals ints unless told otherwise,
    // so with --allow-respell it reaches v1's answers, verified.
    let (default, stderr, ok) = run(&[
        "propose-edit", "--channel", "shapes", "--emit", "0", "--arg", "1", "--to", "42.5", "--allow-respell", p,
    ]);
    assert!(ok, "{stderr}");
    assert!(default.starts_with("engine v2 · seed 0 · "), "{default}");
    assert!(default.contains("1. [verified] set `x` to 32.5 (line 3)"), "{default}");
    assert!(default.contains("2. [verified] set `offset` to 22.5 (line 2)"), "{default}");
    // v1's --static reads as --deny under v2.
    let r = run_json(&[
        "propose-edit", "--json", "--channel", "shapes", "--emit", "0", "--arg", "1", "--to", "42", "--static", "x", p,
    ]);
    assert_eq!(r["engine"], "v2");
    let alts = r["alternatives"].as_array().unwrap();
    assert!(!alts.is_empty(), "{r:#}");
    assert!(alts.iter().all(|a| a["changes"].as_array().unwrap().iter().all(|c| c["binding"] != "x")), "{r:#}");
    assert_eq!(alts[0]["changes"][0]["binding"], "offset");

    let r = run_json(&["propose-edit", "--v1", "--json", "--channel", "shapes", "--emit", "0", "--arg", "1", "--to", "42.5", p]);
    assert_eq!(keys(&r), set(&["channel", "emit", "goals", "applied", "arg", "goal", "proposals"]));
    assert_eq!(r["proposals"][0]["variable"], "x");

    let (stdout, _, ok) = run(&[
        "propose-edit", "--v1", "--channel", "shapes", "--emit", "0", "--arg", "1", "--to", "42.5", "--static", "x",
        "--apply", p,
    ]);
    assert!(ok);
    assert!(stdout.ends_with("Applied.\n"), "{stdout}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("let offset = 22.5"));
}

#[test]
fn v2_only_flags_are_refused_under_v1() {
    let path = scratch("refuse", HELPER);
    let (_, stderr, ok) = run(&[
        "propose-edit", "--v1", "--channel", "shapes", "--emit", "0", "--native-arg", "1", "--to", "3",
        path.to_str().unwrap(),
    ]);
    assert!(!ok);
    assert!(stderr.contains("--native-arg needs the v2 engine"), "{stderr}");

    let (_, stderr, ok) = run(&["propose-edit", "--v1", "--v2", "--channel", "shapes", path.to_str().unwrap()]);
    assert!(!ok);
    assert!(stderr.contains("exclusive"), "{stderr}");

    let (_, stderr, ok) = run(&[
        "propose-edit", "--channel", "shapes", "--emit", "0", "--native-arg", "1", "--to", "3", "--allow", "a b",
        path.to_str().unwrap(),
    ]);
    assert!(!ok);
    assert!(stderr.contains("unknown selector"), "{stderr}");
}

#[test]
fn errors_are_reported_with_a_phase() {
    let path = scratch("errors", HELPER);
    let p = path.to_str().unwrap();
    let (stdout, _, ok) = run(&["propose-edit", "--v2", "--json", "--channel", "shapes", "--emit", "9", "--value", "--to", "1", p]);
    assert!(!ok);
    let r: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(r["phase"], "goal");
    assert!(r["message"].as_str().unwrap().contains("has 4 emit(s), no index 9"));

    let (_, stderr, ok) = run(&["propose-edit", "--v2", "--channel", "nope", "--emit", "0", "--value", "--to", "1", p]);
    assert!(!ok);
    assert!(stderr.contains("channels with emits: shapes"), "{stderr}");
}

#[test]
fn non_numeric_goal_falls_back_to_v1() {
    let path = scratch("string", V1_SKETCH);
    let r = run_json(&[
        "propose-edit", "--v2", "--json", "--channel", "shapes", "--emit", "1", "--arg", "1", "--to", "hi",
        path.to_str().unwrap(),
    ]);
    assert_eq!(r["engine"], "v1");
    assert_eq!(r["alternatives"][0]["engine"], "v1");
    assert_eq!(r["alternatives"][0]["verified"], false);
    assert_eq!(r["alternatives"][0]["edits"][0]["new_text"], "\"hi\"");
}

#[test]
fn seed_is_pinned_and_echoed() {
    let src = "import io\nlet r = random() * 100\npush_output(symbol(\"s\"), r + 5)\n";
    let path = scratch("seed", src);
    let p = path.to_str().unwrap();
    let a = run_json(&["propose-edit", "--json", "--seed", "4", "--channel", "s", "--emit", "0", "--value", "--by", "1", p]);
    let b = run_json(&["propose-edit", "--json", "--seed", "4", "--channel", "s", "--emit", "0", "--value", "--by", "1", p]);
    assert_eq!(a["seed"], 4);
    assert_eq!(a["goals"][0]["baseline"], b["goals"][0]["baseline"]);
    // The random draw cannot receive feedback; the offset takes the change.
    let top = &a["alternatives"][0];
    assert_eq!(top["verified"], true, "{a:#}");
    assert_eq!(top["changes"][0]["old_text"], "5");

    // run --trace-emits with the same seed sees the same value.
    let t = run_json(&["run", "--trace-emits", "--json", "--seed", "4", p]);
    assert_eq!(t["channels"]["s"][0]["value"], a["goals"][0]["baseline"]);
}

#[test]
fn trace_emits_reports_keys_and_native_args() {
    let path = scratch("trace", HELPER);
    let t = run_json(&["run", "--trace-emits", "--json", path.to_str().unwrap()]);
    let emits = t["channels"]["shapes"].as_array().unwrap();
    assert_eq!(emits.len(), 4);
    for (i, e) in emits.iter().enumerate() {
        assert_eq!(keys(&e["key"]), set(&["chain_hash", "path_hash", "occurrence"]));
        assert_eq!(e["native_args"][1], 20.0 + 30.0 * i as f64);
    }
    // One call site in a loop: the keys differ by the loop path.
    assert_ne!(emits[0]["key"], emits[1]["key"]);
    let (text, _, _) = run(&["run", "--trace-emits", path.to_str().unwrap()]);
    assert!(text.contains("      native args: [0, 50.0]"), "{text}");
}

#[test]
fn max_wall_is_configurable_and_the_report_names_the_budget_that_ended_it() {
    // Within both budgets: converged, with the defaults echoed.
    let r = helper_json(&[]);
    let d = &r["diagnostics"];
    assert_eq!(d["stopped_by"], "converged", "{d:#}");
    assert_eq!(d["budget_exhausted"], false);
    assert_eq!(d["max_runs"], 200);
    assert_eq!(d["max_wall_ms"], 5000.0);

    // A run budget too small to measure anything.
    let r = helper_json(&["--max-runs", "2"]);
    assert_eq!(r["diagnostics"]["stopped_by"], "runs", "{:#}", r["diagnostics"]);

    // A wall clock that is over before the first run.
    let r = helper_json(&["--max-wall", "0.000001"]);
    assert_eq!(r["diagnostics"]["stopped_by"], "wall", "{:#}", r["diagnostics"]);
    assert_eq!(r["diagnostics"]["max_wall_ms"], 0.001);

    // A longer budget is taken as given.
    let r = helper_json(&["--max-wall", "30"]);
    assert_eq!(r["diagnostics"]["max_wall_ms"], 30000.0);

    let path = scratch("max-wall-text", HELPER);
    let p = path.to_str().unwrap();
    let text = |extra: &[&str]| {
        let mut args = vec!["propose-edit", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--by", "25"];
        args.extend_from_slice(extra);
        args.push(p);
        let (stdout, stderr, ok) = run(&args);
        assert!(ok, "{stderr}");
        stdout.lines().next().unwrap_or("").to_string()
    };
    assert!(text(&[]).ends_with("· converged within budget"), "{}", text(&[]));
    let wall = text(&["--max-wall", "0.000001"]);
    assert!(wall.contains("stopped: wall clock (0 s; raise --max-wall)"), "{wall}");
    let runs = text(&["--max-runs", "2"]);
    assert!(runs.contains("stopped: run budget (2 runs; raise --max-runs)"), "{runs}");

    let (_, stderr, ok) = run(&[
        "propose-edit", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--by", "25", "--max-wall", "0", p,
    ]);
    assert!(!ok);
    assert!(stderr.contains("--max-wall takes a positive number of seconds"), "{stderr}");
}

#[test]
fn one_literal_is_selected_by_position_or_term_id() {
    // Level data in an imported file, with no `tunable` marker on any of it.
    let dir = std::env::temp_dir().join(format!("petal-cli-dm-v2-{}-at", std::process::id()));
    std::fs::create_dir_all(dir.join("core")).unwrap();
    std::fs::write(dir.join("core/level.ptl"), "pub let rows = [\n  {x: 10, y: 5},\n  {x: 40, y: 7},\n]\n").unwrap();
    let main = dir.join("main.ptl");
    std::fs::write(
        &main,
        "import io\nimport ./core/level\nfor r in level.rows do\n  push_output(symbol(\"shapes\"), r.x * 2 + r.y)\nend\n",
    )
    .unwrap();
    let m = main.to_str().unwrap();
    let goal = ["propose-edit", "--json", "--channel", "shapes", "--emit", "1", "--native-arg", "1", "--by", "6"];
    let with = |extra: &[&str]| {
        let mut args = goal.to_vec();
        args.extend_from_slice(extra);
        args.push(m);
        run_json(&args)
    };
    let only = |r: &Value| -> Vec<String> {
        r["alternatives"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["changes"][0]["old_text"].as_str().unwrap().to_string())
            .collect()
    };
    // Both numbers of the second row can take the drag.
    assert_eq!(only(&with(&[])), ["40", "7"]);
    // `at:FILE:LINE:COL`, anywhere inside the literal's text (`40` is
    // columns 7-8 of line 3); a path suffix names the file.
    assert_eq!(only(&with(&["--allow", "at:core/level.ptl:3:8"])), ["40"]);
    assert_eq!(only(&with(&["--allow", "at:level.ptl:3:14"])), ["7"]);
    assert_eq!(only(&with(&["--deny", "at:core/level.ptl:3:7"])), ["7"]);
    // `term:N`, the `term` the report gives each candidate and change.
    let r = with(&[]);
    let seven = r["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["value"] == 7.0)
        .map(|c| c["term"].as_u64().unwrap())
        .unwrap();
    assert_eq!(only(&with(&["--allow", &format!("term:{seven}")])), ["7"]);
    // A column between literals selects nothing.
    let (stdout, _, ok) = run(&[&goal[..], &["--allow", "at:core/level.ptl:3:6", m]].concat());
    assert!(!ok);
    assert!(stdout.contains("not among the allowed parameters"), "{stdout}");

    // `at:LINE:COL` is the entry file: `20` in `place`, a literal no knob marks.
    let path = scratch("at-entry", HELPER);
    let r = run_json(&[
        "propose-edit", "--json", "--channel", "shapes", "--emit", "2", "--native-arg", "1", "--by", "25", "--allow",
        "at:4:10", path.to_str().unwrap(),
    ]);
    assert_eq!(only(&r), ["20"]);

    for bad in ["at:3", "at:x:3", "at:f.ptl:0:1", "at::3:4", "term:x"] {
        let (_, stderr, ok) = run(&[&goal[..], &["--allow", bad, m]].concat());
        assert!(!ok, "{bad}");
        assert!(stderr.contains(&format!("selector `{bad}`")), "{bad}: {stderr}");
    }
}
