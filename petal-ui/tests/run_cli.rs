//! End-to-end tests for the `petal-ui-run` CLI: the JSONL trace, scenario
//! playback, determinism under `--seed`, and the error/exit-code contract.
//!
//! These drive the real binary (via `CARGO_BIN_EXE_*`) rather than the library,
//! because the contract the refactor verifier depends on is the *process*
//! contract — argv, stdout/stderr, exit codes, one JSON line per frame.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_petal-ui-run");

/// A scratch directory unique to one test, removed when the test finishes.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("petal-ui-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the binary, returning (exit code, stdout, stderr).
fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN).args(args).output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn records(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad JSONL line {l:?}: {e}")))
        .collect()
}

fn repo_root() -> PathBuf {
    // tests run with CARGO_MANIFEST_DIR = <repo>/petal-ui
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

// ── The trace ────────────────────────────────────────────────────────────

#[test]
fn writes_one_record_per_frame_and_replays_a_scenario_click() {
    let s = Scratch::new("scenario");
    let app = s.write(
        "app.ptl",
        "state hits = 0\n\
         state saw = 0\n\
         saw = frame_count()\n\
         if clicked({x: 10, y: 10, w: 100, h: 40}) then hits = hits + 1 end\n\
         draw_rect({x: 10, y: 10, w: 100, h: 40}, #ffffff)\n",
    );
    let scenario = s.write("s.json", r#"{"events": [{"at": 3, "click": [40, 20]}]}"#);
    let out = s.path("trace.jsonl");
    let (code, _, err) = run(&[
        app.to_str().unwrap(),
        "--frames",
        "8",
        "--scenario",
        scenario.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {err}");

    let recs = records(&out);
    assert_eq!(recs.len(), 8, "one JSONL record per frame");
    for (i, r) in recs.iter().enumerate() {
        assert_eq!(r["frame"], i as i64);
        assert!(r["error"].is_null());
        assert!(!r["commands"].as_array().unwrap().is_empty(), "draws");
    }
    // `clicked` fires on the press edge, which the scenario delivers to frame 3.
    let hits = |i: usize| recs[i]["state"]["hits"].as_i64().unwrap();
    assert_eq!(hits(2), 0, "no click yet");
    assert_eq!(hits(3), 1, "the click lands on its scheduled frame");
    assert_eq!(hits(7), 1, "and does not repeat");
}

#[test]
fn scenario_file_supplies_size_and_frames_and_flags_override_them() {
    let s = Scratch::new("size");
    let app = s.write(
        "app.ptl",
        "state w = 0\nstate h = 0\nw = screen_width()\nh = screen_height()\n",
    );
    let scenario = s.write(
        "s.json",
        r#"{"size": [321, 123], "frames": 4, "events": []}"#,
    );

    let out = s.path("a.jsonl");
    let (code, _, err) = run(&[
        app.to_str().unwrap(),
        "--scenario",
        scenario.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let recs = records(&out);
    assert_eq!(recs.len(), 4);
    assert_eq!(recs[0]["state"]["w"], 321);
    assert_eq!(recs[0]["state"]["h"], 123);

    let out = s.path("b.jsonl");
    let (code, _, err) = run(&[
        app.to_str().unwrap(),
        "--scenario",
        scenario.to_str().unwrap(),
        "--size",
        "640x480",
        "--frames",
        "2",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let recs = records(&out);
    assert_eq!(recs.len(), 2, "--frames wins over the scenario");
    assert_eq!(recs[0]["state"]["w"], 640, "--size wins over the scenario");
}

#[test]
fn prints_are_captured_per_frame() {
    let s = Scratch::new("prints");
    let app = s.write("app.ptl", "state n = 0\nn = n + 1\nprint(\"tick {n}\")\n");
    let out = s.path("t.jsonl");
    let (code, _, err) = run(&[
        app.to_str().unwrap(),
        "--frames",
        "3",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let recs = records(&out);
    assert_eq!(recs[0]["prints"], serde_json::json!(["tick 1"]));
    assert_eq!(recs[2]["prints"], serde_json::json!(["tick 3"]));
}

/// `print` must not echo to the process's stdout: the driver turns echoing off
/// (`Env::set_echo(false)`), so a printing app's trace on stdout is still
/// nothing but JSONL — one parseable object per line, no interleaved text.
#[test]
fn printing_app_leaves_stdout_clean_jsonl() {
    let s = Scratch::new("print-echo");
    let app = s.write("app.ptl", "state n = 0\nn = n + 1\nprint(\"tick {n}\")\n");
    let (code, stdout, err) = run(&[app.to_str().unwrap(), "--frames", "3"]);
    assert_eq!(code, 0, "stderr: {err}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "one line per frame, got: {stdout:?}");
    for (i, line) in lines.iter().enumerate() {
        let rec: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("line {i} is not JSON: {e}"));
        assert_eq!(rec["frame"], i);
        assert_eq!(
            rec["prints"],
            serde_json::json!([format!("tick {}", i + 1)])
        );
    }
}

// ── Determinism ──────────────────────────────────────────────────────────

#[test]
fn same_seed_gives_byte_identical_output() {
    let s = Scratch::new("seed");
    let app = s.write(
        "app.ptl",
        "state r = 0.0\nstate acc = 0.0\nr = random(0.0, 1.0)\nacc = acc + r\n",
    );
    let go = |name: &str, seed: &str| {
        let out = s.path(name);
        let (code, _, err) = run(&[
            app.to_str().unwrap(),
            "--frames",
            "10",
            "--seed",
            seed,
            "--out",
            out.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {err}");
        std::fs::read(&out).unwrap()
    };
    assert_eq!(go("a.jsonl", "3"), go("b.jsonl", "3"), "--seed 3 twice");
    assert_ne!(
        go("a.jsonl", "3"),
        go("c.jsonl", "4"),
        "a different seed draws different numbers"
    );
}

#[test]
fn monkey_scenarios_are_reproducible_from_their_seed() {
    let s = Scratch::new("monkey");
    let app = s.write(
        "app.ptl",
        "state hits = 0\n\
         state typed = \"\"\n\
         if mouse_pressed(0) then hits = hits + 1 end\n\
         typed = typed ++ text_input()\n",
    );
    let go = |name: &str, scenario: &str| {
        let out = s.path(name);
        let (code, _, err) = run(&[
            app.to_str().unwrap(),
            "--frames",
            "40",
            "--seed",
            "1",
            "--scenario",
            scenario,
            "--out",
            out.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {err}");
        std::fs::read(&out).unwrap()
    };
    let a = go("a.jsonl", "monkey:7");
    assert_eq!(a, go("b.jsonl", "monkey:7"), "monkey:7 twice");
    assert_ne!(a, go("c.jsonl", "monkey:8"), "a different monkey seed");

    // The monkey actually drives the app rather than idling.
    let recs = records(&s.path("a.jsonl"));
    let last = recs.last().unwrap();
    assert!(
        last["state"]["hits"].as_i64().unwrap() > 0,
        "monkey clicked at least once"
    );
}

// ── Failure modes ────────────────────────────────────────────────────────

#[test]
fn a_runtime_error_writes_its_frame_then_exits_one() {
    let s = Scratch::new("error");
    let app = s.write(
        "app.ptl",
        "state n = 0\n\
         n = n + 1\n\
         let xs = [1, 2]\n\
         if n == 3 then\n\
             print(str(xs[9]))\n\
         end\n",
    );
    let out = s.path("t.jsonl");
    let (code, _, _) = run(&[
        app.to_str().unwrap(),
        "--frames",
        "10",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    let recs = records(&out);
    assert_eq!(recs.len(), 3, "the run stops at the failing frame");
    let err = recs[2]["error"].as_str().unwrap();
    assert!(err.contains("out of bounds"), "{err}");
    assert!(
        err.contains("line 5"),
        "full format keeps the position: {err}"
    );

    // `--error-format bare` drops the position and the echoed source line, so
    // a re-indenting refactor cannot change the message.
    let out = s.path("bare.jsonl");
    let (code, _, _) = run(&[
        app.to_str().unwrap(),
        "--frames",
        "10",
        "--error-format",
        "bare",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    let err = records(&out)[2]["error"].as_str().unwrap().to_string();
    assert!(err.contains("out of bounds"), "{err}");
    assert!(!err.contains("line 5"), "no position: {err}");
    assert!(!err.contains('|'), "no echoed source line: {err}");
}

#[test]
fn a_compile_error_exits_two_with_a_message_on_stderr() {
    let s = Scratch::new("compile");
    let app = s.write("app.ptl", "let x = (((\n");
    let (code, stdout, stderr) = run(&[app.to_str().unwrap(), "--frames", "2"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty(), "no trace was written");
    assert!(!stderr.is_empty(), "the compile error goes to stderr");
}

#[test]
fn a_bad_scenario_file_exits_two() {
    let s = Scratch::new("badscenario");
    let app = s.write("app.ptl", "state n = 0\nn = n + 1\n");
    let scenario = s.write("s.json", r#"{"events": [{"at": 1, "key": "ArrowLeft"}]}"#);
    let (code, _, stderr) = run(&[
        app.to_str().unwrap(),
        "--scenario",
        scenario.to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    assert!(stderr.contains("canonical"), "{stderr}");
}

// ── host_data fixtures ───────────────────────────────────────────────────

#[test]
fn host_data_fixtures_answer_matching_questions_and_nil_otherwise() {
    let s = Scratch::new("hostdata");
    let app = s.write(
        "app.ptl",
        "state title = \"\"\n\
         state n = 0\n\
         state missing = 0\n\
         let r = host_data(\"commit\", \"abc\")\n\
         title = r.title\n\
         n = r.n\n\
         if host_data(\"commit\", \"zzz\") == nil then missing = 1 end\n",
    );
    let fixtures = s.write(
        "f.json",
        r#"[{"kind": "commit", "arg": "abc", "value": {"title": "first", "n": 42}}]"#,
    );
    let out = s.path("t.jsonl");
    let (code, _, err) = run(&[
        app.to_str().unwrap(),
        "--frames",
        "1",
        "--host-data",
        fixtures.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let r = &records(&out)[0];
    assert_eq!(r["state"]["title"], "first");
    assert_eq!(r["state"]["n"], 42);
    assert_eq!(
        r["state"]["missing"], 1,
        "an unmatched question answers nil"
    );
}

// ── The checked-in example apps ──────────────────────────────────────────

#[test]
fn example_apps_run_headlessly() {
    let root = repo_root();
    for rel in [
        "examples/games/snake/app.ptl",
        "examples/productivity/kanban/app.ptl",
    ] {
        let app = root.join(rel);
        assert!(
            app.exists(),
            "example app missing: {} (moved or renamed?)",
            app.display()
        );
        let s = Scratch::new(&rel.replace('/', "-"));
        let out = s.path("t.jsonl");
        let (code, _, err) = run(&[
            app.to_str().unwrap(),
            "--frames",
            "20",
            "--seed",
            "1",
            "--size",
            "1280x850",
            "--scenario",
            "monkey:5",
            "--out",
            out.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "{rel} failed: {err}");
        let recs = records(&out);
        assert_eq!(recs.len(), 20, "{rel}");
        assert!(
            recs.iter()
                .all(|r| !r["commands"].as_array().unwrap().is_empty()),
            "{rel} drew nothing on some frame"
        );
    }
}

// ── Garden panel-native stubs ────────────────────────────────────────────

/// The driver registers `petal_ui::panel_stubs`, so a Garden panel drawer's
/// host natives (`palette`, `query`, `mutate`, the stores and text-view
/// regions) answer deterministically instead of `Unknown builtin`.
#[test]
fn garden_panel_natives_answer_as_deterministic_stubs() {
    let s = Scratch::new("panel-stubs");
    let app = s.write(
        "app.ptl",
        "state loading = 0\n\
         state handle = 0\n\
         state stored = \"\"\n\
         state no_nav = 0\n\
         let P = palette()\n\
         clear(P.window_bg.r, P.window_bg.g, P.window_bg.b)\n\
         let rd = query(\"doc\", {id: 7})\n\
         if is_loading(rd) then loading = loading + 1 end\n\
         handle = mutate(\"apply\", {n: 1})\n\
         stored = panel_store_get(\"k\") ?? \"empty\"\n\
         panel_store_set(\"k\", \"v\")\n\
         text_view(1, 0, 0, 100, 50, \"body\")\n\
         emit(\"status\", {text: \"hi\"})\n\
         if nav_arg() == nil then no_nav = 1 end\n",
    );
    let out = s.path("t.jsonl");
    let (code, _, err) = run(&[
        app.to_str().unwrap(),
        "--frames",
        "3",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let recs = records(&out);
    assert_eq!(recs.len(), 3);
    // `query` never resolves: the loading branch runs every frame.
    assert_eq!(recs[2]["state"]["loading"], 3);
    // `mutate` hands out env-lifetime-unique handles, like Garden's native.
    assert_eq!(recs[0]["state"]["handle"], 1);
    assert_eq!(recs[2]["state"]["handle"], 3);
    // The store is inert: `panel_store_set` persists nothing.
    assert_eq!(recs[2]["state"]["stored"], "empty");
    assert_eq!(recs[0]["state"]["no_nav"], 1, "nav_arg answers nil");
    // `text_view` emits the same Host draw command Garden's native does.
    let cmds = serde_json::to_string(&recs[0]["commands"]).unwrap();
    assert!(cmds.contains("text_view"), "no text_view command: {cmds}");
}
