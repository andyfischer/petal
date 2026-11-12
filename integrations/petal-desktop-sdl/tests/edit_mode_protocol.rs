//! Edit mode over the headless agent protocol: the `petal-sdl` binary driven
//! by scripted stdin commands, the way an agent or a test harness drives it
//! (see `src/edit_mode.rs`). A best-effort drag writes the sketch, the
//! watcher's hot reload replays the frozen frame through the edit, an
//! unreachable drag writes nothing, and undo puts the file back.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use serde_json::Value;

struct Agent {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Agent {
    fn start(sketch: &std::path::Path) -> Agent {
        let mut child = Command::new(env!("CARGO_BIN_EXE_petal-sdl"))
            .arg("--headless")
            .arg(sketch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn petal-sdl");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut agent = Agent { child, stdin, stdout };
        let hello = agent.read();
        assert_eq!(hello["ok"], true, "{hello}");
        agent
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read a response");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
    }

    fn send(&mut self, cmd: Value) -> Value {
        writeln!(self.stdin, "{cmd}").unwrap();
        self.stdin.flush().unwrap();
        self.read()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn sketch(name: &str, src: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("petal-sdl-proto-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sketch.ptl");
    std::fs::write(&path, src).unwrap();
    (dir, path)
}

const ROW: &str = "import ui\n\
let gap = tunable(40)\n\
\n\
fn card(i)\n  \
  draw_rect(60 + i * gap, 100, 30, 30, 70, 165, 150)\n\
end\n\
\n\
clear(16, 18, 26)\n\
for i in range(0, 4) do\n  \
  card(i)\n\
end\n";

#[test]
fn a_best_effort_drag_edits_the_file_and_reloads() {
    let (dir, path) = sketch("drag", ROW);
    let mut agent = Agent::start(&path);
    assert_eq!(agent.send(serde_json::json!({"cmd": "step", "n": 3}))["ok"], true);

    let on = agent.send(serde_json::json!({"cmd": "edit_mode", "on": true}));
    assert_eq!(on["edit"]["on"], true, "{on}");
    assert_eq!(on["timeline"]["frozen"], true, "edit mode freezes the timeline: {on}");

    // The fourth card (i = 3) is at 60 + 3 * 40 = 180.
    let picked = agent.send(serde_json::json!({"cmd": "edit_pick", "x": 190, "y": 110}));
    let label = picked["edit"]["picked"].as_str().unwrap_or_default().to_string();
    assert!(label.contains("`draw_rect` at line 5"), "the call is named: {picked}");

    // Drag it 30 to the right: only `gap` is a knob, so it carries the
    // whole move (3 * gap grows by 30, gap by 10).
    let dragged = agent.send(serde_json::json!({"cmd": "edit_drag", "x": 190, "y": 110, "dx": 30.0, "dy": 0.0}));
    assert_eq!(dragged["ok"], true, "{dragged}");
    let applied = &dragged["edit"]["applied"];
    assert!(applied["description"].as_str().unwrap().contains("gap"), "{dragged}");
    let edited = std::fs::read_to_string(&path).unwrap();
    assert!(edited.contains("let gap = tunable(50)"), "{edited}");
    assert!(edited.contains("draw_rect(60 + i * gap"), "the plain literal stays: {edited}");

    // The watcher's hot reload replays the frozen frame through the edit:
    // the card is now where it was dropped.
    let mut reloaded = false;
    for _ in 0..200 {
        let t = agent.send(serde_json::json!({"cmd": "timeline"}));
        if t["timeline"]["generation"].as_u64() == Some(1) {
            reloaded = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(reloaded, "the edit was hot reloaded");
    let moved = agent.send(serde_json::json!({"cmd": "edit_pick", "x": 220, "y": 110}));
    assert_eq!(moved["ok"], true, "the moved card is at its new place: {moved}");

    // A drag nothing can follow (vertical: every y literal is plain, but
    // `rigid`) is an error, and writes nothing.
    let before = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, before.replace("draw_rect(60 + i * gap, 100,", "draw_rect(60 + i * gap, rigid(100),")).unwrap();
    for _ in 0..200 {
        let t = agent.send(serde_json::json!({"cmd": "timeline"}));
        if t["timeline"]["generation"].as_u64() == Some(2) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let rigid = std::fs::read_to_string(&path).unwrap();
    let refused = agent.send(serde_json::json!({"cmd": "edit_drag", "x": 220, "y": 110, "dx": 0.0, "dy": 40.0}));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(refused["edit"]["error"].is_string(), "the reason is shown: {refused}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), rigid, "nothing was written");

    // Leaving edit mode resumes the game.
    let off = agent.send(serde_json::json!({"cmd": "edit_mode", "on": false}));
    assert_eq!(off["edit"]["on"], false, "{off}");
    assert_eq!(off["timeline"]["frozen"], false, "{off}");
    drop(agent);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undo_puts_the_file_back() {
    let (dir, path) = sketch("undo", ROW);
    let mut agent = Agent::start(&path);
    agent.send(serde_json::json!({"cmd": "step", "n": 2}));
    let dragged = agent.send(serde_json::json!({"cmd": "edit_drag", "x": 70, "y": 110, "dx": 0.0, "dy": 20.0}));
    assert_eq!(dragged["ok"], true, "{dragged}");
    assert_ne!(std::fs::read_to_string(&path).unwrap(), ROW);
    assert!(dragged["edit"]["undo"].is_string(), "{dragged}");
    let undone = agent.send(serde_json::json!({"cmd": "edit_undo"}));
    assert_eq!(undone["ok"], true, "{undone}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), ROW);
    let again = agent.send(serde_json::json!({"cmd": "edit_undo"}));
    assert_eq!(again["ok"], false, "one undo: {again}");
    drop(agent);
    let _ = std::fs::remove_dir_all(&dir);
}
