//! Integration tests for `ScriptHost`: load a `.ptl` file from a temp dir,
//! assert the extracted `LayoutNode` tree, exercise error paths and the
//! mtime/size-polling hot reload.
//!
//! Note on change detection: `poll_reload` compares (mtime, size). To stay
//! fast (no mtime-granularity sleeps) the rewritten scripts below always
//! differ in byte length from what they replace.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use garden_script::{LayoutNode, ScriptHost};

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

/// Create a fresh temp dir and write `source` into `init.ptl` inside it.
fn write_script(source: &str) -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("garden-script-test-{}-{}", std::process::id(), id));
    fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("init.ptl");
    fs::write(&path, source).expect("write script");
    path
}

fn editor(file: &str) -> LayoutNode {
    LayoutNode::Editor {
        file: Some(file.to_string()),
        line_numbers: false,
        wrap: true,
    }
}

#[test]
fn loads_nested_layout_with_ratios() {
    let path = write_script(
        r#"
layout(
    row([
        column([ editor("a.rs"), editor() ], [0.7, 0.3]),
        editor("b.md"),
    ], [0.6, 0.4])
)
"#,
    );
    let host = ScriptHost::load(&path).expect("load should succeed");

    let expected = LayoutNode::Row {
        children: vec![
            LayoutNode::Column {
                children: vec![
                    editor("a.rs"),
                    LayoutNode::Editor {
                        file: None,
                        line_numbers: false,
                        wrap: true,
                    },
                ],
                ratios: Some(vec![0.7, 0.3]),
            },
            editor("b.md"),
        ],
        ratios: Some(vec![0.6, 0.4]),
    };
    assert_eq!(host.layout(), &expected);
}

#[test]
fn editor_config_sets_line_numbers() {
    let path = write_script(
        r#"
layout(row([
    editor("a.rs", { line_numbers: true }),
    editor("b.rs"),
]))
"#,
    );
    let host = ScriptHost::load(&path).expect("load should succeed");

    let expected = LayoutNode::Row {
        children: vec![
            LayoutNode::Editor {
                file: Some("a.rs".into()),
                line_numbers: true,
                wrap: true,
            },
            LayoutNode::Editor {
                file: Some("b.rs".into()),
                line_numbers: false,
                wrap: true,
            },
        ],
        ratios: None,
    };
    assert_eq!(host.layout(), &expected);
}

#[test]
fn editor_config_sets_wrap() {
    let path = write_script(
        r#"
layout(row([
    editor("a.rs", { wrap: false }),
    editor("b.rs"),
]))
"#,
    );
    let host = ScriptHost::load(&path).expect("load should succeed");

    let expected = LayoutNode::Row {
        children: vec![
            LayoutNode::Editor {
                file: Some("a.rs".into()),
                line_numbers: false,
                wrap: false, // explicitly disabled
            },
            LayoutNode::Editor {
                file: Some("b.rs".into()),
                line_numbers: false,
                wrap: true, // defaults on
            },
        ],
        ratios: None,
    };
    assert_eq!(host.layout(), &expected);
}

#[test]
fn editor_wrap_must_be_a_bool() {
    let path = write_script("layout(editor(\"a.rs\", { wrap: 5 }))");
    let err = ScriptHost::load(&path).expect_err("non-bool wrap should fail");
    assert!(err.contains("'wrap' must be a bool"), "got: {err}");
}

#[test]
fn editor_config_must_be_a_record() {
    let path = write_script("layout(editor(\"a.rs\", 5))");
    let err = ScriptHost::load(&path).expect_err("non-record config should fail");
    assert!(err.contains("config must be a record"), "got: {err}");
}

#[test]
fn syntax_error_fails_load() {
    let path = write_script("layout(row([editor(\"a\")]");
    let err = ScriptHost::load(&path).expect_err("bad syntax should fail");
    assert!(!err.is_empty());
}

#[test]
fn layout_rejects_non_record() {
    let path = write_script("layout(5)");
    let err = ScriptHost::load(&path).expect_err("non-record layout should fail");
    assert!(err.contains("layout"), "unexpected message: {err}");
}

#[test]
fn unknown_kind_fails_load() {
    let path = write_script(r#"layout({ kind: "tabs" })"#);
    let err = ScriptHost::load(&path).expect_err("unknown kind should fail");
    assert!(
        err.contains("unknown layout kind"),
        "unexpected message: {err}"
    );
}

#[test]
fn empty_children_fails_load() {
    let path = write_script("layout(row([]))");
    let err = ScriptHost::load(&path).expect_err("empty row should fail");
    assert!(
        err.contains("at least one child"),
        "unexpected message: {err}"
    );
}

#[test]
fn missing_layout_falls_back_to_default() {
    let path = write_script(r#"print("no layout here")"#);
    let host = ScriptHost::load(&path).expect("missing layout() should fall back");
    assert_eq!(
        host.layout(),
        &LayoutNode::Editor {
            file: None,
            line_numbers: false,
            wrap: true
        }
    );
}

#[test]
fn ratios_length_mismatch_degrades_to_none_with_warning() {
    let path = write_script(r#"layout(row([ editor("a"), editor("b") ], [0.5, 0.25, 0.25]))"#);
    let mut host = ScriptHost::load(&path).expect("load should succeed");
    assert_eq!(
        host.layout(),
        &LayoutNode::Row {
            children: vec![editor("a"), editor("b")],
            ratios: None,
        }
    );
    let output = host.take_output();
    assert!(
        output
            .iter()
            .any(|line| line.contains("warning") && line.contains("ratios")),
        "expected a ratios warning, got: {output:?}"
    );
}

#[test]
fn print_output_is_collected() {
    let path = write_script(
        r#"
print("hello from petal")
layout(editor("a"))
"#,
    );
    let mut host = ScriptHost::load(&path).expect("load should succeed");
    let output = host.take_output();
    assert!(
        output.contains(&"hello from petal".to_string()),
        "got: {output:?}"
    );
    // Drained: a second take returns nothing new.
    assert!(host.take_output().is_empty());
}

#[test]
fn hot_reload_picks_up_layout_change() {
    let path = write_script(r#"layout(editor("first.rs"))"#);
    let mut host = ScriptHost::load(&path).expect("load should succeed");
    assert_eq!(host.layout(), &editor("first.rs"));

    // Unchanged file: no reload.
    assert_eq!(host.poll_reload(), Ok(false));

    // Rewrite with a different layout (and different byte length).
    fs::write(
        &path,
        r#"layout(column([ editor("second.rs"), editor() ], [0.8, 0.2]))"#,
    )
    .expect("rewrite script");

    assert_eq!(host.poll_reload(), Ok(true), "layout should have changed");
    assert_eq!(
        host.layout(),
        &LayoutNode::Column {
            children: vec![
                editor("second.rs"),
                LayoutNode::Editor {
                    file: None,
                    line_numbers: false,
                    wrap: true
                }
            ],
            ratios: Some(vec![0.8, 0.2]),
        }
    );

    // And quiet again afterwards.
    assert_eq!(host.poll_reload(), Ok(false));
}

#[test]
fn hot_reload_error_keeps_old_layout_and_reports_once() {
    let path = write_script(r#"layout(editor("keep.rs"))"#);
    let mut host = ScriptHost::load(&path).expect("load should succeed");

    // Break the script (different length than the original).
    fs::write(&path, "layout(row([editor(").expect("rewrite script");
    let err = host
        .poll_reload()
        .expect_err("broken script should report an error");
    assert!(!err.is_empty());
    assert_eq!(host.layout(), &editor("keep.rs"), "old layout must be kept");

    // Same broken content: no re-report on subsequent polls.
    assert_eq!(host.poll_reload(), Ok(false));
    assert_eq!(host.poll_reload(), Ok(false));

    // Fix the file: reload succeeds again.
    fs::write(&path, r#"layout(editor("fixed.rs"))"#).expect("rewrite script");
    assert_eq!(host.poll_reload(), Ok(true));
    assert_eq!(host.layout(), &editor("fixed.rs"));
}

#[test]
fn hot_reload_preserves_petal_state() {
    // `state` survives hot_reload; the layout is derived from it. After a
    // reload that bumps the counter, the preserved state is incremented
    // rather than reinitialized.
    let path = write_script(
        r#"
state runs = 0
runs = runs + 1
layout(editor("run-{runs}.txt"))
"#,
    );
    let mut host = ScriptHost::load(&path).expect("load should succeed");
    assert_eq!(host.layout(), &editor("run-1.txt"));

    // Rewrite with trailing comment to change the file size; same logic.
    fs::write(
        &path,
        r#"
state runs = 0
runs = runs + 1
layout(editor("run-{runs}.txt"))
// edited
"#,
    )
    .expect("rewrite script");

    assert_eq!(host.poll_reload(), Ok(true));
    assert_eq!(
        host.layout(),
        &editor("run-2.txt"),
        "state var should survive the hot reload"
    );
}

#[test]
fn color_theme_captures_colors() {
    let path = write_script(
        r##"
color_theme({ window_bg: "#102030", selection: "#01020304" })
layout(editor("a.rs"))
"##,
    );
    let host = ScriptHost::load(&path).expect("load should succeed");

    let theme = host.theme();
    assert!(!theme.is_empty());

    let bg = theme.get("window_bg").expect("window_bg set");
    assert!((bg[0] - 0x10 as f32 / 255.0).abs() < 1e-4);
    assert!((bg[1] - 0x20 as f32 / 255.0).abs() < 1e-4);
    assert!((bg[2] - 0x30 as f32 / 255.0).abs() < 1e-4);
    assert_eq!(bg[3], 1.0);

    // The 8-digit form carries an explicit alpha.
    let sel = theme.get("selection").expect("selection set");
    assert!((sel[3] - 0x04 as f32 / 255.0).abs() < 1e-4);

    // Layout still loads normally alongside the theme.
    assert_eq!(host.layout(), &editor("a.rs"));
}

#[test]
fn colors_only_edit_bumps_theme_rev_without_changing_layout() {
    let path = write_script(
        r##"
color_theme({ window_bg: "#102030" })
layout(editor("a.rs"))
"##,
    );
    let mut host = ScriptHost::load(&path).expect("load should succeed");
    let rev0 = host.theme_rev();

    // Edit only the colors; the layout is byte-for-byte the same node.
    fs::write(
        &path,
        r##"
color_theme({ window_bg: "#405060" })
layout(editor("a.rs"))
"##,
    )
    .expect("rewrite script");

    // The layout is unchanged, so poll_reload reports Ok(false)...
    assert_eq!(host.poll_reload(), Ok(false), "layout is identical");
    // ...but the theme changed, so its revision advanced and the new color is
    // visible — this is what lets the app restyle on a colors-only hot edit.
    assert_ne!(host.theme_rev(), rev0, "theme revision should advance");
    let bg = host.theme().get("window_bg").expect("window_bg set");
    assert!((bg[0] - 0x40 as f32 / 255.0).abs() < 1e-4);
}

#[test]
fn no_color_theme_yields_empty_default_theme() {
    let path = write_script(r#"layout(editor("only.rs"))"#);
    let host = ScriptHost::load(&path).expect("load should succeed");
    assert!(host.theme().is_empty());
    assert_eq!(host.layout(), &editor("only.rs"));
}

#[test]
fn color_theme_skips_malformed_color_with_warning() {
    let path = write_script(
        r##"
color_theme({ window_bg: "#102030", text: "not-a-color" })
layout(editor("a.rs"))
"##,
    );
    let mut host = ScriptHost::load(&path).expect("load should succeed");
    // The good key survives; the bad one is skipped.
    assert!(host.theme().get("window_bg").is_some());
    assert!(host.theme().get("text").is_none());
    let output = host.take_output();
    assert!(
        output
            .iter()
            .any(|l| l.contains("warning") && l.contains("text")),
        "expected a malformed-color warning, got: {output:?}"
    );
}
