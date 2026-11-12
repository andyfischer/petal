//! End-to-end tests for `garden-panel-run`: the generic `petal-ui-run` CLI
//! (tested in `core-libs/petal-ui-headless/tests/run_cli.rs`) with Garden's
//! panel stubs, so the examples, most of which `import garden` through their
//! `layout.ptl`, run headlessly.
//!
//! These drive the real binary (via `CARGO_BIN_EXE_*`) rather than the library,
//! because the contract the refactor verifier depends on is the *process*
//! contract — argv, stdout/stderr, exit codes, one JSON line per frame.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_garden-panel-run");

/// A scratch directory unique to one test, removed when the test finishes.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("garden-panel-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.0.join(name);
        // Every app here is a petal-ui script: give it its `import ui`
        // (on its own first line, so line numbers are the body's).
        let contents = if name.ends_with(".ptl") {
            format!("import ui; {contents}")
        } else {
            contents.to_string()
        };
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
    // tests run with CARGO_MANIFEST_DIR = <repo>/garden/garden-panel-run
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
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

/// The finance dashboard hit-tests the pointer against the donut's slices
/// from before the frame's input and then drew the focused one from the list
/// after it: on the frame of a drill-down the hover index belonged to
/// another list ("Index 7 out of bounds (len 5)", frame 26 of this run).
#[test]
fn the_finance_dashboard_survives_a_drill_down_under_the_pointer() {
    let rel = "examples/dashboards/finance-dashboard/app.ptl";
    let app = repo_root().join(rel);
    assert!(app.exists(), "example app missing: {}", app.display());
    let s = Scratch::new("finance-drill-down");
    let out = s.path("t.jsonl");
    let (code, _, err) = run(&[
        app.to_str().unwrap(),
        "--frames",
        "60",
        "--seed",
        "1",
        "--scenario",
        "monkey:3",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{rel} failed: {err}");
    let recs = records(&out);
    assert_eq!(recs.len(), 60, "{rel}");
    assert!(
        recs.iter().all(|r| r.get("error").is_none_or(|e| e.is_null())),
        "{rel} raised an error on some frame"
    );
}

// ── Garden panel-native stubs ────────────────────────────────────────────

/// The driver registers `garden_script::headless`'s stubs, so a Garden panel drawer's
/// host natives (`palette`, `query`, `mutate`, the stores and text-view
/// regions) answer deterministically, and `import garden` resolves.
#[test]
fn garden_panel_natives_answer_as_deterministic_stubs() {
    let s = Scratch::new("panel-stubs");
    let app = s.write(
        "app.ptl",
        "import garden\n\
         state loading = 0\n\
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

/// `--query-fixtures` answers the stub `query(kind, arg)` from a table, so a
/// drawer takes its loaded path; a `(kind, arg)` with no fixture still loads.
#[test]
fn query_fixtures_answer_the_stub_query() {
    let s = Scratch::new("query-fixtures");
    let app = s.write(
        "app.ptl",
        "import garden\n\
         state got = \"\"\n\
         state other_loading = false\n\
         let rd = query(\"doc\", {id: 7})\n\
         if !is_loading(rd) then got = rd.title end\n\
         other_loading = is_loading(query(\"doc\", {id: 8}))\n",
    );
    let fixtures = s.write(
        "q.json",
        r#"[{"kind": "doc", "arg": {"id": 7}, "value": {"title": "seven"}}]"#,
    );
    let out = s.path("t.jsonl");
    let (code, _, err) = run(&[
        app.to_str().unwrap(),
        "--frames",
        "1",
        "--query-fixtures",
        fixtures.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let recs = records(&out);
    assert_eq!(recs[0]["state"]["got"], "seven");
    assert_eq!(recs[0]["state"]["other_loading"], true);
}
