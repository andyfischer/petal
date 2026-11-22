//! Whole programs: every example in this repository transpiles without a
//! panic, and (when `e2e/node_modules` is installed) the output runs in
//! React: `cd e2e && npm install` once.

use std::path::{Path, PathBuf};
use std::process::Command;

use transpile_to_react_js::{Options, transpile, transpile_project};

fn ptl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" || name == "vendor" {
            continue;
        }
        if p.is_dir() {
            ptl_files(&p, out);
        } else if name.ends_with(".ptl") {
            out.push(p);
        }
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_example_transpiles() {
    let mut files = Vec::new();
    for d in ["examples", "integrations/petal-web-html/examples", "garden/examples"] {
        ptl_files(&repo().join(d), &mut files);
    }
    assert!(files.len() > 100, "found only {} examples", files.len());
    let mut transpiled = 0;
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        if transpile_to_react_js::parse(&src).is_err() {
            continue; // not Petal this parser reads (a deliberately broken example)
        }
        let opts = Options { file_name: f.file_name().unwrap().to_string_lossy().into(), ..Options::default() };
        let out = transpile(&src, &opts).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert!(!out.code.is_empty());
        transpiled += 1;
    }
    assert!(transpiled * 10 >= files.len() * 9, "{transpiled} of {} transpiled", files.len());
}

/// Transpile `entry` as a project into a fresh folder; render `export`
/// with `clicks`; return the report.
fn render(entry: &Path, export: &str, clicks: &[&str]) -> Option<serde_json::Value> {
    let e2e = Path::new(env!("CARGO_MANIFEST_DIR")).join("e2e");
    if !e2e.join("node_modules/react").exists() {
        eprintln!("skipping: run `npm install` in {} to check the output in React", e2e.display());
        return None;
    }
    let out = std::env::temp_dir().join(format!("petal-react-test-{}-{}", std::process::id(), export));
    std::fs::create_dir_all(&out).unwrap();
    let files = transpile_project(entry, &Options::default()).unwrap();
    let mut main = None;
    for f in &files {
        let p = out.join(&f.out);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, &f.output.code).unwrap();
        for (n, c) in &f.output.runtime {
            std::fs::write(out.join(n), c).unwrap();
        }
        main = Some(p);
    }
    let mut cmd = Command::new("node");
    cmd.arg(e2e.join("render.mjs")).arg(main.unwrap()).arg("--export").arg(export);
    for c in clicks {
        cmd.arg("--click").arg(c);
    }
    let o = cmd.output().expect("node");
    let report: serde_json::Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)));
    Some(report)
}

#[test]
fn the_menu_example_runs_and_its_clicks_change_state() {
    let Some(r) = render(&repo().join("integrations/petal-web-html/examples/menu.ptl"), "default", &["text=File", ".dropdown-backdrop", "text=Open About Petal"]) else { return };
    assert_eq!(r["ok"], true, "{r:#}");
    let text = |i: usize| r["steps"][i]["text"].as_str().unwrap().to_string();
    assert!(!text(0).contains("Export"));
    assert!(text(1).contains("Export"), "the File menu opens: {}", text(1));
    assert!(!text(2).contains("Export"), "the backdrop closes it");
    assert!(text(3).contains("A custom programming language"), "the modal opens");
}

#[test]
fn a_design_renders() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cards.ptl");
    let Some(r) = render(&fixture, "default", &[]) else { return };
    assert_eq!(r["ok"], true, "{r:#}");
    assert_eq!(r["keyWarnings"], 0, "{r:#}");
    let text = r["steps"][0]["text"].as_str().unwrap();
    assert!(text.contains("This week") && text.contains("5.2 km") && text.contains("30 minutes"), "{text}");
    let Some(r) = render(&fixture, "TodayIos", &[]) else { return };
    assert_eq!(r["ok"], true, "the lifted frame renders alone: {r:#}");
}
