//! `petal-ui-propose-edit`: `petal propose-edit` for scripts that draw
//! through `ui`, which the core CLI cannot load (it has no petal-ui natives).
//! Shells out to the built binary on a small HUD.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value as Json;

const BIN: &str = env!("CARGO_BIN_EXE_petal-ui-propose-edit");

const HUD: &str = "\
import ui

config let margin = 12
config let bar_h = 18

fn health_bar(x, y, frac)
  draw_rect(x, y, 120, bar_h, #303030)
  draw_rect(x, y, 120 * frac, bar_h, #40c040)
end

fn hud()
  health_bar(margin, margin, 0.75)
  draw_text(\"SCORE 120\", margin, margin + bar_h + 6, #ffffff)
end

hud()
";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petal-ui-propose-edit-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hud.ptl");
    std::fs::write(&path, HUD).unwrap();
    path
}

fn run(args: &[&str]) -> (Json, i32) {
    let out = Command::new(BIN).args(args).env_remove("PETAL_SEED").output().expect("runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("not JSON ({e}):\n{stdout}\nstderr:\n{}", String::from_utf8_lossy(&out.stderr))
    });
    (json, out.status.code().unwrap_or(-1))
}

#[test]
fn trace_emits_lists_the_draw_commands() {
    let path = scratch("trace");
    let (emits, code) = run(&["--trace-emits", path.to_str().unwrap()]);
    assert_eq!(code, 0);
    let draws = emits["draw_commands"].as_array().expect("draw_commands");
    assert_eq!(draws.len(), 3, "{emits:#}");
    // draw_text("SCORE 120", x, y, color): y is margin + bar_h + 6.
    assert_eq!(draws[2]["native_args"][2].as_f64(), Some(36.0), "{emits:#}");
}

#[test]
fn drags_a_hud_layout_and_applies_it() {
    let path = scratch("drag");
    let p = path.to_str().unwrap();
    // Drag the text's baseline down by 6 without moving the bar's height.
    let goal = ["--json", "--channel", "draw_commands", "--emit", "2", "--native-arg", "2", "--to", "42", "--deny", "bar_h"];
    let (report, code) = run(&[&goal[..], &[p]].concat());
    assert_eq!(code, 0, "{report:#}");
    let best = &report["alternatives"][0];
    assert_eq!(best["verified"], Json::Bool(true), "{report:#}");
    let changes = best["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "{report:#}");
    assert_eq!(changes[0]["binding"], "margin", "{report:#}");

    // The same request with --apply writes the top alternative.
    let (applied, code) = run(&[&goal[..], &["--apply", "--pick", "1", p]].concat());
    assert_eq!(code, 0, "{applied:#}");
    assert_eq!(applied["applied"]["pick"], 1, "{applied:#}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_ne!(text, HUD, "the file was not edited");
    assert!(text.contains("config let bar_h = 18"), "{text}");

    // And the edited file now draws the text where it was dragged to.
    let (emits, _) = run(&["--trace-emits", p]);
    assert_eq!(emits["draw_commands"][2]["native_args"][2].as_f64(), Some(42.0), "{emits:#}");
}

#[test]
fn errors_come_back_in_the_cli_shape() {
    let path = scratch("err");
    let (err, code) =
        run(&["--json", "--channel", "nope", "--emit", "0", "--native-arg", "0", "--to", "1", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert_eq!(err["error"], Json::Bool(true), "{err:#}");
    assert_eq!(err["phase"], "goal", "{err:#}");
    assert!(err["message"].as_str().unwrap().contains("draw_commands"), "{err:#}");
}
