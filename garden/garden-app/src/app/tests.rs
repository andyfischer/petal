//! Unit tests for the app core, exercising the public input/command surface of
//! [`App`] against in-memory panes (no window, GPU, or terminal involved).

use super::*;
use crate::clipboard::{InMemoryClipboard, SharedClipboard};
use crate::debug::{DebugCmd, Reply};
use crate::theme::ThemeScheme;
use crate::vim::{self, Key};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// A fresh per-test scratch directory under the system temp dir.
fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("garden-app-test-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn file_with(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

/// An `App` with one editor pane per entry (`None` = pathless buffer).
fn app_with_panes(files: &[Option<&str>]) -> App {
    let children = files
        .iter()
        .map(|f| LayoutNode::Editor {
            file: f.map(str::to_string),
            line_numbers: false,
            wrap: true,
        })
        .collect();
    App::new(
        None,
        LayoutNode::Row {
            children,
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    )
}

fn dirty_all(app: &mut App) {
    for pane in &mut app.panes {
        pane.view.insert("x");
    }
}

/// What the app's (single) panel bound to `name` on its last good frame — the
/// observation channel, so the script needs no publishing call and the value
/// keeps its real type (`None` means the binding never executed). See
/// `garden_script::PanelHost::observed_json` for the naming rule.
fn panel_value(app: &App, name: &str) -> Option<serde_json::Value> {
    app.panes
        .iter()
        .find_map(|p| p.panel.as_ref())
        .and_then(|pv| pv.observed().get(name).cloned())
}

/// [`panel_value`] narrowed to an integer, for the call sites that compare or
/// do arithmetic on one rather than assert an exact value.
fn panel_int(app: &App, name: &str) -> Option<i64> {
    panel_value(app, name).and_then(|v| v.as_i64())
}

#[test]
fn petal_ide_live_binding_drives_panel_from_editor_buffer() {
    // The Petal-IDE split: an editor and a panel on the SAME .ptl file. Editing
    // the editor buffer must recompile the panel live, with no save to disk.
    let dir = temp_dir("petal-ide-live");
    let script = file_with(&dir, "app.ptl", "let v = 1\n");
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some(script.clone()),
                    line_numbers: false,
                    wrap: true,
                },
                LayoutNode::Panel {
                    script: script.clone(),
                    screens: Vec::new(),
                },
            ],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );

    // The panel renders the initial on-disk content.
    app.settle_panels();
    assert_eq!(panel_value(&app, "v"), Some(json!(1)));

    // Edit the editor buffer in memory only (not saved). The live binding, run
    // during the panel tick, must pick the new source up from the buffer.
    let editor = app.panes.iter().position(|p| !p.is_panel()).unwrap();
    app.panes[editor].view.buffer = garden_core::Buffer::from_str("let v = 42\n");
    app.settle_panels();
    assert_eq!(panel_value(&app, "v"), Some(json!(42)));

    // The change came from the buffer — disk is untouched.
    assert_eq!(fs::read_to_string(&script).unwrap(), "let v = 1\n");
}

#[test]
fn petal_ide_flags_the_error_line_in_the_paired_editor() {
    // A compile error in the live buffer highlights the offending source line in
    // the paired editor (error_line), and a fix clears it.
    let dir = temp_dir("petal-ide-errline");
    let script = file_with(&dir, "canvas.ptl", "let v = 1\n");
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some(script.clone()),
                    line_numbers: false,
                    wrap: true,
                },
                LayoutNode::Panel {
                    script: script.clone(),
                    screens: Vec::new(),
                },
            ],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    let editor = app.panes.iter().position(|p| !p.is_panel()).unwrap();
    assert_eq!(
        app.panes[editor].view.error_line, None,
        "no error initially"
    );

    // Break line 2 of the buffer (a lexer error carrying `[line 2, …]`).
    app.panes[editor].view.buffer = garden_core::Buffer::from_str("let v = 1\n?bad\n");
    app.settle_panels();
    assert_eq!(
        app.panes[editor].view.error_line,
        Some(1),
        "line 2 (0-based 1) is flagged"
    );

    // Fixing the buffer clears the highlight.
    app.panes[editor].view.buffer = garden_core::Buffer::from_str("let v = 2\n");
    app.settle_panels();
    assert_eq!(
        app.panes[editor].view.error_line, None,
        "fix clears the flag"
    );
}

/// Programming by direct manipulation, end to end: pointing at a shape on the
/// canvas highlights the `draw_*` call that drew it in the paired editor, and
/// moving off the shape clears the highlight.
#[test]
fn pointing_at_a_shape_highlights_the_code_that_drew_it() {
    let dir = temp_dir("petal-ide-trace");
    // Line 1 clears; line 2 draws a circle at the top-left of the canvas.
    let script = file_with(
        &dir,
        "canvas.ptl",
        "clear(10, 10, 10)\ndraw_circle(60, 60, 40, 200, 90, 90)\n",
    );
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some(script.clone()),
                    line_numbers: false,
                    wrap: true,
                },
                LayoutNode::Panel {
                    script: script.clone(),
                    screens: Vec::new(),
                },
            ],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    let editor = app.panes.iter().position(|p| !p.is_panel()).unwrap();
    let canvas = app.panes.iter().position(|p| p.is_panel()).unwrap();
    let rect = app.panes[canvas].rect;

    assert!(
        app.panes[canvas]
            .panel
            .as_ref()
            .is_some_and(|p| p.traces_origins()),
        "a panel paired with a live editor traces its draw calls"
    );

    // Point at the middle of the circle — panel-local (60, 60).
    app.mouse_moved(rect.x + 60.0, rect.y + 60.0);
    app.settle_panels();
    let (start, end) = app.panes[editor]
        .view
        .trace_highlight
        .expect("the circle traces back to source");
    assert_eq!(start.line, 1, "the draw_circle on line 2 (0-based 1)");
    assert_eq!(end.line, 1);
    assert!(end.col > start.col, "and covers the call, not a point");

    // Move onto bare canvas: the highlight clears rather than sticking.
    app.mouse_moved(rect.x + rect.w - 4.0, rect.y + rect.h - 4.0);
    app.settle_panels();
    assert_eq!(
        app.panes[editor].view.trace_highlight, None,
        "off a shape, nothing is highlighted"
    );
}

/// An editor|canvas pair on one file — the `garden petal-ide` shape — returning
/// the app and the two pane indices. `source`'s line numbers are what the
/// direct-manipulation tests below assert against.
fn traced_pair(name: &str, source: &str) -> (App, usize, usize, PathBuf) {
    let dir = temp_dir(name);
    let script = file_with(&dir, "canvas.ptl", source);
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some(script.clone()),
                    line_numbers: false,
                    wrap: true,
                },
                LayoutNode::Panel {
                    script: script.clone(),
                    screens: Vec::new(),
                },
            ],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    let editor = app.panes.iter().position(|p| !p.is_panel()).unwrap();
    let canvas = app.panes.iter().position(|p| p.is_panel()).unwrap();
    // Leak the temp dir for the test's lifetime by returning its path.
    (app, editor, canvas, PathBuf::from(script))
}

/// Pausing freezes the canvas — it must not freeze the *pointer*. "Freeze a
/// moment, then point at it" is the gesture pause exists for, and the frozen
/// frame is still exactly what the mouse is over. The regression this guards is
/// worse than a missing highlight: with the reconcile skipped, the last
/// highlight stayed on screen, naming a line the pointer had long left.
#[test]
fn direct_manipulation_keeps_working_while_paused() {
    let (mut app, editor, canvas, _dir) = traced_pair(
        "petal-ide-trace-paused",
        "clear(10, 10, 10)\n\
         draw_circle(60, 60, 40, 200, 90, 90)\n\
         draw_circle(200, 60, 40, 90, 200, 90)\n",
    );
    let rect = app.panes[canvas].rect;

    app.mouse_moved(rect.x + 60.0, rect.y + 60.0);
    app.settle_panels();
    assert_eq!(
        app.panes[editor].view.trace_highlight.map(|(s, _)| s.line),
        Some(1),
        "the first circle, while playing"
    );

    app.toggle_play();
    assert!(app.paused);

    // The *other* circle, while paused: the highlight must follow the pointer.
    app.mouse_moved(rect.x + 200.0, rect.y + 60.0);
    app.tick_panels();
    assert_eq!(
        app.panes[editor].view.trace_highlight.map(|(s, _)| s.line),
        Some(2),
        "paused, the highlight still tracks the pointer"
    );

    // And bare canvas still clears it, rather than stranding the last hit.
    app.mouse_moved(rect.x + rect.w - 4.0, rect.y + rect.h - 4.0);
    app.tick_panels();
    assert_eq!(
        app.panes[editor].view.trace_highlight, None,
        "paused, a miss still reads as a miss"
    );
}

/// While the buffer doesn't compile the canvas keeps showing the last good
/// frame — but that frame's spans describe the text that compiled, and the
/// editor is showing text that has moved on. Highlighting anyway points
/// confidently at whatever now occupies those lines.
#[test]
fn a_drifted_buffer_highlights_nothing_rather_than_the_wrong_line() {
    let (mut app, editor, canvas, _dir) = traced_pair(
        "petal-ide-trace-drift",
        "clear(10, 10, 10)\ndraw_circle(60, 60, 40, 200, 90, 90)\n",
    );
    let rect = app.panes[canvas].rect;
    app.mouse_moved(rect.x + 60.0, rect.y + 60.0);
    app.settle_panels();
    assert_eq!(
        app.panes[editor].view.trace_highlight.map(|(s, _)| s.line),
        Some(1)
    );

    // Break the buffer AND shift the draw call down a line. The canvas keeps
    // rendering the old frame (that is the point of the last-good-frame rule),
    // whose circle still traces to line 1 — which is now a comment.
    app.panes[editor].view.buffer = garden_core::Buffer::from_str(
        "clear(10, 10, 10)\n\
         // inserted\n\
         draw_circle(60, 60, 40, 200, 90, 90)\n\
         draw_circle(1, 2,\n",
    );
    app.settle_panels();
    assert!(
        app.panes[canvas].panel.as_ref().unwrap().source_drifted(),
        "the buffer does not compile"
    );
    assert_eq!(
        app.panes[editor].view.trace_highlight, None,
        "no highlight beats a wrong one"
    );

    // Fixing it brings the highlight back, at the call's NEW line.
    app.panes[editor].view.buffer = garden_core::Buffer::from_str(
        "clear(10, 10, 10)\n\
         // inserted\n\
         draw_circle(60, 60, 40, 200, 90, 90)\n",
    );
    app.settle_panels();
    assert_eq!(
        app.panes[editor].view.trace_highlight.map(|(s, _)| s.line),
        Some(2),
        "line 3 now (0-based 2)"
    );
}

/// Cmd-click on a shape is go-to-definition with a pixel as the symbol: the
/// cursor lands on the call, in the paired editor, which takes focus.
#[test]
fn cmd_click_on_a_shape_jumps_to_the_code_that_drew_it() {
    let (mut app, editor, canvas, _dir) = traced_pair(
        "petal-ide-trace-jump",
        "clear(10, 10, 10)\n\
         fn dot(x, y)\n\
         \x20 draw_circle(x, y, 40, 200, 90, 90)\n\
         end\n\
         dot(60, 60)\n",
    );
    let rect = app.panes[canvas].rect;
    let (cx, cy) = (rect.x + 60.0, rect.y + 60.0);

    // A plain click belongs to the script: an interactive sketch must not lose
    // its clicks — or the keyboard — to the editor.
    app.mouse_down(cx, cy, Mods::default(), 1);
    app.mouse_up();
    assert_eq!(app.focus, canvas, "a plain click focuses the canvas");
    assert_eq!(app.panes[editor].view.cursor.line, 0, "cursor untouched");

    // Cmd-click jumps — to the `draw_circle` *inside* `dot`, since that is where
    // the shape is made, and to its column, not the line's start.
    app.mouse_down(
        cx,
        cy,
        Mods {
            cmd: true,
            ..Default::default()
        },
        1,
    );
    app.mouse_up();
    assert_eq!(app.focus, editor, "focus follows you to the code");
    assert_eq!(app.panes[editor].view.cursor.line, 2);
    assert_eq!(
        app.panes[editor].view.cursor.col, 2,
        "on the call, not col 0"
    );

    // Cmd-click on bare canvas has nothing to jump to and falls through to the
    // ordinary click path rather than swallowing the press.
    app.mouse_down(
        rect.x + rect.w - 4.0,
        rect.y + rect.h - 4.0,
        Mods {
            cmd: true,
            ..Default::default()
        },
        1,
    );
    app.mouse_up();
    assert_eq!(app.focus, canvas, "a miss is an ordinary click");
}

/// Cmd-*drag* is the other half of the same gesture: pull a shape and the
/// numbers that placed it are rewritten under the pointer. The shape's position
/// is written as literals here, so the edit lands in the call itself.
#[test]
fn cmd_drag_rewrites_the_literals_that_placed_the_shape() {
    let (mut app, editor, canvas, _dir) = traced_pair(
        "petal-ide-drag-literal",
        "clear(10, 10, 10)\ndraw_rect(40, 40, 60, 60, 200, 90, 90)\n",
    );
    let rect = app.panes[canvas].rect;
    let (gx, gy) = (rect.x + 70.0, rect.y + 70.0);

    app.mouse_down(
        gx,
        gy,
        Mods {
            cmd: true,
            ..Default::default()
        },
        1,
    );
    app.mouse_moved(gx + 30.0, gy + 20.0);
    app.mouse_up();

    assert_eq!(
        app.panes[editor].view.buffer.line(1),
        "draw_rect(70, 60, 60, 60, 200, 90, 90)",
        "both coordinates moved with the pointer, and stayed integers"
    );

    // The canvas follows: the live binding recompiles from the edited buffer,
    // so the shape is now where it was dragged to.
    app.settle_panels();
    app.mouse_moved(rect.x + 100.0, rect.y + 90.0);
    app.settle_panels();
    assert_eq!(
        app.panes[editor].view.trace_highlight.map(|(s, _)| s.line),
        Some(1),
        "the moved rect is under the pointer at its new position"
    );

    // One gesture, one undo step — undoing a drag puts the shape back, rather
    // than walking it backwards a frame at a time.
    app.panes[editor].view.buffer.undo();
    assert_eq!(
        app.panes[editor].view.buffer.line(1),
        "draw_rect(40, 40, 60, 60, 200, 90, 90)"
    );
}

/// The gesture states a *goal*, not an edit — so it works where there is no
/// number to edit at all. The card's x is `x0 + i * spacing`, computed per
/// iteration; the sketch declares `spacing` a knob with `config let`, which
/// pins `x0`, so the runtime inverts the arithmetic and re-spaces the row.
#[test]
fn a_drag_solves_a_computed_position_into_the_config_knob() {
    let (mut app, editor, canvas, _dir) = traced_pair(
        "petal-ide-drag-solve",
        "config let spacing = 50\n\
         let x0 = 20\n\
         clear(10, 10, 10)\n\
         for i in range(0, 3) do\n\
         \x20 draw_rect(x0 + i * spacing, 40, 30, 30, 200, 90, 90)\n\
         end\n",
    );
    let rect = app.panes[canvas].rect;
    // The third card (i = 2) sits at 20 + 100; drag it 20px right, which the
    // solver must answer by widening `spacing` by 10.
    let (gx, gy) = (rect.x + 135.0, rect.y + 55.0);

    app.mouse_down(
        gx,
        gy,
        Mods {
            cmd: true,
            ..Default::default()
        },
        1,
    );
    app.mouse_moved(gx + 20.0, gy);
    app.mouse_up();

    assert_eq!(
        app.panes[editor].view.buffer.line(0),
        "config let spacing = 60",
        "the knob moved, not the pinned origin"
    );
    assert_eq!(
        app.panes[editor].view.buffer.line(1),
        "let x0 = 20",
        "`x0` is not a knob, so the drag left it alone"
    );
}

/// Solving a computed position inverts against the values the run *recorded*,
/// and the trace holds the last value each term took — the last iteration's. So
/// only the last shape a looping call drew can be solved; the others would get
/// a confident, wrong edit derived from a sibling's loop counter. Saying so is
/// the point of this test.
#[test]
fn a_looping_call_solves_its_last_shape_and_declines_the_rest() {
    let source = "config let spacing = 50\n\
                  let x0 = 20\n\
                  clear(10, 10, 10)\n\
                  for i in range(0, 3) do\n\
                  \x20 draw_rect(x0 + i * spacing, 40, 30, 30, 200, 90, 90)\n\
                  end\n";
    let (mut app, editor, canvas, _dir) = traced_pair("petal-ide-drag-loop", source);
    let rect = app.panes[canvas].rect;

    // The middle card (i = 1, at x = 70): its position is computed, and it is
    // not the shape whose values the trace still holds.
    let (gx, gy) = (rect.x + 85.0, rect.y + 55.0);
    app.mouse_down(
        gx,
        gy,
        Mods {
            cmd: true,
            ..Default::default()
        },
        1,
    );
    app.mouse_moved(gx + 20.0, gy);
    app.mouse_up();

    assert!(
        app.status_note
            .as_deref()
            .is_some_and(|s| s.contains("only the last one can be solved")),
        "the refusal names its reason: {:?}",
        app.status_note
    );
    assert_eq!(
        app.panes[editor].view.buffer.text(),
        source,
        "and nothing was rewritten"
    );
}

/// A drag on a shape whose position isn't editable says so instead of guessing.
/// Here every named binding is pinned by the sketch's own `config` declaration
/// and the y argument is a plain `let`, so there is nothing the runtime may
/// move.
#[test]
fn a_drag_with_nothing_to_move_reports_rather_than_guesses() {
    let (mut app, _editor, canvas, _dir) = traced_pair(
        "petal-ide-drag-refuse",
        "config let unused = 1\n\
         let top = 40\n\
         clear(10, 10, 10)\n\
         draw_rect(40, top, 60, 60, 200, 90, 90)\n",
    );
    let rect = app.panes[canvas].rect;
    let (gx, gy) = (rect.x + 70.0, rect.y + 70.0);
    app.mouse_down(
        gx,
        gy,
        Mods {
            cmd: true,
            ..Default::default()
        },
        1,
    );
    app.mouse_moved(gx, gy + 30.0);
    app.mouse_up();

    assert!(
        app.status_note
            .as_deref()
            .is_some_and(|s| s.contains("isn't directly editable")),
        "the refusal is reported, not silent: {:?}",
        app.status_note
    );
}

/// The tracing is opt-in per pane: a panel with no paired editor never turns it
/// on, so an ordinary `:Diff`/`:Git`-style drawer pays nothing for a feature it
/// cannot use.
#[test]
fn a_panel_without_a_paired_editor_does_not_trace() {
    let dir = temp_dir("petal-ide-untraced");
    let script = file_with(&dir, "solo.ptl", "draw_circle(60, 60, 40, 200, 90, 90)\n");
    let mut app = App::new(
        None,
        LayoutNode::Panel {
            script: script.clone(),
            screens: Vec::new(),
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    let canvas = app.panes.iter().position(|p| p.is_panel()).unwrap();
    assert!(!app.panes[canvas]
        .panel
        .as_ref()
        .is_some_and(|p| p.traces_origins()));
}

#[test]
fn petal_ide_live_binding_ignores_unrelated_panels() {
    // A panel whose script is NOT the editor's file must never be reloaded from
    // that editor's buffer (the pairing is by resolved path).
    let dir = temp_dir("petal-ide-unrelated");
    let edited = file_with(&dir, "edited.ptl", "let v = 1\n");
    let other = file_with(&dir, "other.ptl", "let v = 7\n");
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some(edited.clone()),
                    line_numbers: false,
                    wrap: true,
                },
                LayoutNode::Panel {
                    script: other.clone(),
                    screens: Vec::new(),
                },
            ],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    assert_eq!(panel_value(&app, "v"), Some(json!(7)));

    // Editing the unrelated editor buffer leaves the panel on its own file.
    let editor = app.panes.iter().position(|p| !p.is_panel()).unwrap();
    app.panes[editor].view.buffer = garden_core::Buffer::from_str("let v = 99\n");
    assert!(!app.sync_editor_panels());
    app.settle_panels();
    assert_eq!(panel_value(&app, "v"), Some(json!(7)));
}

#[test]
fn save_all_writes_every_dirty_pane_and_skips_pathless() {
    let dir = temp_dir("save-all-writes");
    let a = file_with(&dir, "a.txt", "alpha");
    let b = file_with(&dir, "b.txt", "beta");
    let mut app = app_with_panes(&[Some(&a), Some(&b), None]);
    dirty_all(&mut app);

    let out = save_all_panes(&mut app.panes, &Default::default());
    assert!(out.first_error.is_none()); // pathless pane is no error
    assert_eq!(out.skipped_protected, 0);
    assert_eq!(fs::read_to_string(&a).unwrap(), "xalpha");
    assert_eq!(fs::read_to_string(&b).unwrap(), "xbeta");
}

#[test]
fn save_all_skips_clean_panes() {
    let dir = temp_dir("save-all-skips-clean");
    let a = file_with(&dir, "a.txt", "alpha");
    let mut app = app_with_panes(&[Some(&a)]);
    // The file changed on disk after opening; a clean pane must not
    // clobber it with its stale copy.
    fs::write(&a, "changed on disk").unwrap();

    assert!(save_all_panes(&mut app.panes, &Default::default())
        .first_error
        .is_none());
    assert_eq!(fs::read_to_string(&a).unwrap(), "changed on disk");
}

#[test]
fn save_all_reports_io_error_and_keeps_saving() {
    let dir = temp_dir("save-all-error");
    let sub = dir.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let bad = file_with(&sub, "bad.txt", "doomed");
    let good = file_with(&dir, "good.txt", "fine");
    let mut app = app_with_panes(&[Some(&bad), Some(&good)]);
    dirty_all(&mut app);
    fs::remove_dir_all(&sub).unwrap(); // writing bad.txt back now fails

    let err = save_all_panes(&mut app.panes, &Default::default())
        .first_error
        .expect("expected a save error");
    assert!(
        err.starts_with("save failed:"),
        "unexpected error text: {err}"
    );
    assert_eq!(fs::read_to_string(&good).unwrap(), "xfine"); // later pane still saved
}

#[test]
fn wqa_saves_all_panes_then_quits() {
    let dir = temp_dir("wqa");
    let a = file_with(&dir, "a.txt", "alpha");
    let b = file_with(&dir, "b.txt", "beta");
    let mut app = app_with_panes(&[Some(&a), Some(&b)]);
    dirty_all(&mut app);

    for c in ":wqa".chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
    app.apply_key(Key::Enter, Mods::default());

    assert!(app.should_quit());
    assert_eq!(fs::read_to_string(&a).unwrap(), "xalpha");
    assert_eq!(fs::read_to_string(&b).unwrap(), "xbeta");
}

#[test]
fn save_protected_file_prompts_then_saves_as_and_repoints() {
    use crate::command_line::Command;

    let dir = temp_dir("save-protected");
    let scratch = file_with(&dir, "scratch.ptl", "let v = 1\n");
    let mut app = app_with_panes(&[Some(&scratch)]);
    app.set_save_as_paths([PathBuf::from(&scratch)].into_iter().collect());

    // Edit the buffer in place, then `:w`. The scratch file must NOT be
    // overwritten; instead the command line opens pre-filled with `w ` (the
    // filename prompt).
    app.panes[0].view.insert("x");
    app.run_command(Command::Write);
    assert!(
        app.command_line.is_some(),
        "save opened the filename prompt"
    );
    assert_eq!(
        fs::read_to_string(&scratch).unwrap(),
        "let v = 1\n",
        "scratch was not overwritten"
    );

    // `:w saved.ptl` writes the edited buffer there and re-points the pane.
    let saved = dir.join("saved.ptl");
    let saved_str = saved.to_string_lossy().into_owned();
    app.run_command(Command::WriteAs(saved_str.clone()));
    assert_eq!(
        fs::read_to_string(&saved).unwrap(),
        app.panes[0].view.buffer.text(),
        "save-as wrote the edited buffer"
    );
    assert!(fs::read_to_string(&saved).unwrap().starts_with('x'));
    assert_eq!(app.panes[0].file.as_deref(), Some(saved_str.as_str()));

    // The pane is no longer protected: a plain `:w` writes to the new file
    // (no filename prompt reopens).
    app.command_line = None;
    app.panes[0].view.insert("y");
    app.run_command(Command::Write);
    assert!(
        app.command_line.is_none(),
        "no prompt — the new file saves in place"
    );
    assert_eq!(
        fs::read_to_string(&saved).unwrap(),
        app.panes[0].view.buffer.text(),
        "plain :w wrote to the re-pointed file"
    );
}

#[test]
fn save_all_skips_a_protected_pane() {
    let dir = temp_dir("save-all-protected");
    let scratch = file_with(&dir, "scratch.ptl", "orig\n");
    let good = file_with(&dir, "good.txt", "fine");
    let mut app = app_with_panes(&[Some(&scratch), Some(&good)]);
    dirty_all(&mut app);
    let protected = [PathBuf::from(&scratch)].into_iter().collect();

    let out = save_all_panes(&mut app.panes, &protected);
    assert_eq!(out.skipped_protected, 1, "the scratch pane was skipped");
    assert!(out.first_error.is_none());
    // The protected scratch keeps its on-disk content; the ordinary pane saves.
    assert_eq!(fs::read_to_string(&scratch).unwrap(), "orig\n");
    assert_eq!(fs::read_to_string(&good).unwrap(), "xfine");
}

#[test]
fn wqa_neither_overwrites_nor_quits_while_a_protected_scratch_is_dirty() {
    let dir = temp_dir("wqa-protected");
    let scratch = file_with(&dir, "scratch.ptl", "orig\n");
    let mut app = app_with_panes(&[Some(&scratch)]);
    app.set_save_as_paths([PathBuf::from(&scratch)].into_iter().collect());
    app.panes[0].view.insert("x");

    for c in ":wqa".chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
    app.apply_key(Key::Enter, Mods::default());

    assert!(
        !app.should_quit(),
        "must not quit while the scratch is unsaved"
    );
    assert_eq!(
        fs::read_to_string(&scratch).unwrap(),
        "orig\n",
        ":wqa must not overwrite the protected scratch"
    );
}

#[test]
fn save_as_repoints_a_paired_panel_to_the_new_file() {
    use crate::command_line::Command;

    // In the Petal-IDE split, `:w <new>` on the editor must also re-point the
    // panel paired with it by path, so the live binding keeps tracking the buffer.
    let dir = temp_dir("save-as-repoint-panel");
    let script = file_with(&dir, "canvas.ptl", "let v = 1\n");
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some(script.clone()),
                    line_numbers: false,
                    wrap: true,
                },
                LayoutNode::Panel {
                    script: script.clone(),
                    screens: Vec::new(),
                },
            ],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    let editor = app.panes.iter().position(|p| !p.is_panel()).unwrap();
    app.focus = editor;

    let saved = dir.join("saved.ptl");
    let saved_str = saved.to_string_lossy().into_owned();
    app.run_command(Command::WriteAs(saved_str.clone()));

    // The editor pane re-pointed to the new file...
    assert_eq!(app.panes[editor].file.as_deref(), Some(saved_str.as_str()));
    // ...and so did the panel that was paired with the old path.
    let panel = app.panes.iter().find(|p| p.is_panel()).unwrap();
    assert_eq!(
        panel.panel.as_ref().unwrap().script(),
        saved_str,
        "the paired panel now tracks the new file"
    );
}

#[test]
fn resize_divider_no_ops_on_a_degenerate_pair() {
    use crate::layout::resize_divider;

    // Three children whose middle+last ratios sum below 2*MIN_FRAC (0.05 < 0.1).
    // Dragging that divider would build an inverted clamp range and panic in
    // `f32::clamp`; the guard must make it a no-op instead.
    let mut node = LayoutNode::Row {
        children: vec![
            LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            },
            LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            },
            LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            },
        ],
        ratios: Some(vec![0.95, 0.03, 0.02]),
    };
    let changed = resize_divider(&mut node, &[], 1, 0.5);
    assert!(!changed, "a degenerate pair does not resize");
    match node {
        LayoutNode::Row {
            ratios: Some(r), ..
        } => {
            assert_eq!(r, vec![0.95, 0.03, 0.02], "ratios are left untouched");
        }
        other => panic!("expected a Row, got {other:?}"),
    }
}

#[test]
fn dragging_a_divider_resizes_the_split() {
    let dir = temp_dir("divider-drag");
    let a = file_with(&dir, "a.txt", "aaa");
    let b = file_with(&dir, "b.txt", "bbb");
    let mut app = app_with_panes(&[Some(&a), Some(&b)]);

    assert_eq!(app.dividers.len(), 1, "one divider between two panes");
    let d = app.dividers[0].clone();
    assert!(d.vertical, "a Row split gives a vertical divider");
    let cx = d.rect.x + d.rect.w / 2.0;
    let cy = d.rect.y + d.rect.h / 2.0;
    let w0_before = app.panes[0].rect.w;
    let w1_before = app.panes[1].rect.w;

    // Grab the divider and drag it 120px to the right: pane 0 grows, 1 shrinks.
    app.mouse_down(cx, cy, Mods::default(), 1);
    app.mouse_moved(cx + 120.0, cy);
    app.mouse_up();

    assert!(app.panes[0].rect.w > w0_before + 50.0, "pane 0 grew");
    assert!(app.panes[1].rect.w < w1_before - 50.0, "pane 1 shrank");
    // The resize persisted into the layout tree as explicit ratios.
    match app.layout() {
        LayoutNode::Row {
            ratios: Some(r), ..
        } => {
            assert!(r[0] > r[1], "child 0 now has the larger ratio");
        }
        other => panic!("expected a Row with ratios, got {other:?}"),
    }
    // The transient drag override is cleared after release.
    assert!(app.divider_drag.is_none());
    assert!(app.live_layout.is_none());
}

#[test]
fn state_inspector_overlays_live_state_when_toggled() {
    use crate::command_line::Command;

    let dir = temp_dir("state-inspector");
    // A `state` var and a plain `let`: the inspector's one `values` section
    // reports both, because observation names every binding alike.
    let script = file_with(&dir, "s.ptl", "state answer = 42\nlet label = 7\n");
    let mut app = App::new(
        None,
        LayoutNode::Panel {
            script: script.clone(),
            screens: Vec::new(),
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();

    let shows = |app: &App, needle: &str| {
        let needle = needle.to_string();
        app.build_scene().primitives.iter().any(
            |p| matches!(p, garden_render::Primitive::Text { text, .. } if text.contains(&needle)),
        )
    };
    let shows_state = |app: &App| shows(app, "answer = 42");

    // Off by default.
    assert!(!shows_state(&app), "inspector is off by default");

    // `:State` turns it on — the live `state answer = 42` is rendered.
    app.run_command(Command::ToggleState);
    assert!(app.show_panel_state);
    assert!(
        shows_state(&app),
        "live state var appears when the inspector is on"
    );
    assert!(shows(&app, "label = 7"), "a plain `let` is observed too");
    assert!(shows(&app, "values ("), "under one `values` section header");

    // Toggling again hides it.
    app.run_command(Command::ToggleState);
    assert!(!shows_state(&app));
}

// --- native menu dispatch --------------------------------------------

#[test]
fn menu_select_all_then_copy_matches_the_keyboard_path() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("alpha beta").unwrap();

    app.dispatch_menu(MenuAction::SelectAll);
    app.dispatch_menu(MenuAction::Copy);

    assert_eq!(app.clipboard.get().as_deref(), Some("alpha beta"));
}

#[test]
fn menu_cut_then_undo_is_one_transaction() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello world").unwrap();
    select(&mut app, (0, 0), (0, 6));

    app.dispatch_menu(MenuAction::Cut);
    assert_eq!(buffer_text(&app), "world");

    app.dispatch_menu(MenuAction::Undo);
    assert_eq!(buffer_text(&app), "hello world");
}

#[test]
fn menu_quit_sets_the_quit_flag() {
    let mut app = app_with_panes(&[None]);
    app.dispatch_menu(MenuAction::Quit);
    assert!(app.should_quit());
}

#[test]
fn menu_new_file_replaces_focused_pane_with_empty_buffer() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("scratch").unwrap();

    app.dispatch_menu(MenuAction::NewFile);

    assert_eq!(buffer_text(&app), "");
    assert!(app.panes[app.focus].file.is_none());
}

#[test]
fn menu_open_file_loads_it_into_the_focused_pane() {
    let dir = temp_dir("menu-open");
    let f = file_with(&dir, "doc.txt", "from disk");
    let mut app = app_with_panes(&[None]);

    app.dispatch_menu(MenuAction::OpenFile(std::path::PathBuf::from(&f)));

    assert_eq!(buffer_text(&app), "from disk");
    assert_eq!(app.panes[app.focus].file.as_deref(), Some(f.as_str()));
}

#[test]
fn menu_set_theme_switches_active_scheme() {
    let mut app = app_with_panes(&[None]);

    app.dispatch_menu(MenuAction::SetTheme(ThemeScheme::Brown));

    assert_eq!(app.theme_scheme(), ThemeScheme::Brown);
    assert_eq!(app.status_note.as_deref(), Some("theme: Cocoa"));
}

#[test]
fn menu_toggle_wrap_flips_the_focused_pane() {
    let mut app = app_with_panes(&[None]);
    assert!(app.panes[0].view.wrap, "wrap defaults on");

    app.dispatch_menu(MenuAction::ToggleWrap);
    assert!(!app.panes[0].view.wrap);
    assert_eq!(app.status_note.as_deref(), Some("nowrap"));

    app.dispatch_menu(MenuAction::ToggleWrap);
    assert!(app.panes[0].view.wrap);
}

#[test]
fn menu_toggle_line_numbers_flips_and_persists_the_gutter() {
    let mut app = app_with_panes(&[None]);
    assert!(!app.panes[0].view.show_line_numbers);

    app.dispatch_menu(MenuAction::ToggleLineNumbers);
    assert!(app.panes[0].view.show_line_numbers);
    // The flag is layout state: it round-trips through the pane's leaf node.
    assert_eq!(
        app.panes[0].to_layout_node(),
        LayoutNode::Editor {
            file: None,
            line_numbers: true,
            wrap: true,
        }
    );

    app.dispatch_menu(MenuAction::ToggleLineNumbers);
    assert!(!app.panes[0].view.show_line_numbers);
}

#[test]
fn menu_find_opens_the_search_prompt() {
    let mut app = app_with_panes(&[None]);
    app.dispatch_menu(MenuAction::Find);
    let cl = app.command_line.as_ref().expect("search prompt open");
    assert_eq!(cl.display(), "/");
}

#[test]
fn menu_find_next_and_prev_repeat_the_last_search() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("foo bar\nbaz foo\nfoo").unwrap();
    app.panes[0].view.cursor = garden_core::Point::new(0, 0);

    app.accept_search("foo".to_string(), true);
    assert_eq!(app.panes[0].view.cursor, garden_core::Point::new(1, 4));

    app.dispatch_menu(MenuAction::FindNext);
    assert_eq!(app.panes[0].view.cursor, garden_core::Point::new(2, 0));

    app.dispatch_menu(MenuAction::FindPrev);
    assert_eq!(app.panes[0].view.cursor, garden_core::Point::new(1, 4));
}

#[test]
fn menu_find_next_without_a_search_reports_an_error() {
    let mut app = app_with_panes(&[None]);
    app.dispatch_menu(MenuAction::FindNext);
    assert!(app
        .status_error
        .as_deref()
        .unwrap_or("")
        .contains("no previous search"));
}

#[test]
fn menu_split_and_close_manage_panes() {
    let mut app = app_with_panes(&[None]);
    assert_eq!(app.panes.len(), 1);

    app.dispatch_menu(MenuAction::SplitRight);
    assert_eq!(app.panes.len(), 2);

    app.dispatch_menu(MenuAction::SplitDown);
    assert_eq!(app.panes.len(), 3);

    app.dispatch_menu(MenuAction::ClosePane);
    assert_eq!(app.panes.len(), 2);

    app.dispatch_menu(MenuAction::CloseOtherPanes);
    assert_eq!(app.panes.len(), 1);
    assert!(!app.should_quit(), "pane management never quits");

    // Closing the last pane refuses with vim's E444 instead of quitting.
    app.dispatch_menu(MenuAction::ClosePane);
    assert_eq!(app.panes.len(), 1);
    assert!(app.status_error.as_deref().unwrap_or("").contains("E444"));
    assert!(!app.should_quit());
}

#[test]
fn menu_next_pane_cycles_focus() {
    let mut app = app_with_panes(&[None, None]);
    assert_eq!(app.focus, 0);
    app.dispatch_menu(MenuAction::NextPane);
    assert_eq!(app.focus, 1);
    app.dispatch_menu(MenuAction::NextPane);
    assert_eq!(app.focus, 0);
}

#[test]
fn menu_go_to_file_opens_the_fuzzy_finder() {
    let mut app = app_with_panes(&[None]);
    app.dispatch_menu(MenuAction::GoToFile);
    assert!(app.file_finder.is_some());
}

#[test]
fn menu_back_on_an_editor_reports_no_history() {
    let mut app = app_with_panes(&[None]);
    app.dispatch_menu(MenuAction::Back);
    assert!(app.status_note.as_deref().unwrap_or("").contains("history"));
    assert!(!app.should_quit());
}

#[test]
fn set_theme_persists_scheme_to_init_ptl() {
    let dir = temp_dir("set-theme-persist");
    let base = dir.join("init.ptl");
    fs::write(&base, "// my config\nlayout(editor(\"a\"))\n").unwrap();
    let host = ScriptHost::load(&base).unwrap();
    let mut app = App::new(
        Some(host),
        LayoutNode::Editor {
            file: None,
            line_numbers: false,
            wrap: true,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );

    app.dispatch_menu(MenuAction::SetTheme(ThemeScheme::Light));

    // The permanent init.ptl now selects the scheme, comment + layout intact.
    let text = fs::read_to_string(&base).unwrap();
    assert!(text.contains("color_scheme(\"light\")"), "got: {text}");
    assert!(text.contains("// my config"), "comment preserved: {text}");
    assert!(
        text.contains("layout(editor(\"a\"))"),
        "layout preserved: {text}"
    );
    assert!(
        app.status_error.is_none(),
        "no error: {:?}",
        app.status_error
    );

    // A second change updates the same call in place (no duplicate).
    app.dispatch_menu(MenuAction::SetTheme(ThemeScheme::Brown));
    let text = fs::read_to_string(&base).unwrap();
    assert!(text.contains("color_scheme(\"brown\")"), "got: {text}");
    assert_eq!(
        text.matches("color_scheme(").count(),
        1,
        "single call: {text}"
    );
}

#[test]
fn startup_reads_scheme_from_init_ptl() {
    let dir = temp_dir("startup-scheme");
    let base = dir.join("init.ptl");
    fs::write(&base, "color_scheme(\"amiga\")\nlayout(editor(\"a\"))\n").unwrap();
    let host = ScriptHost::load(&base).unwrap();
    let app = App::new(
        Some(host),
        LayoutNode::Editor {
            file: None,
            line_numbers: false,
            wrap: true,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );

    assert_eq!(app.theme_scheme(), ThemeScheme::Amiga);
}

// --- clipboard: Cmd+C / Cmd+X / Cmd+V --------------------------------

fn cmd_key(app: &mut App, c: char) {
    app.apply_key(
        Key::Char(c),
        Mods {
            cmd: true,
            ..Mods::default()
        },
    );
}

/// Select `from..to` in the focused pane, as a mouse drag would.
fn select(app: &mut App, from: (usize, usize), to: (usize, usize)) {
    let view = &mut app.panes[app.focus].view;
    view.begin_drag(garden_core::Point::new(from.0, from.1), false);
    view.drag_to(garden_core::Point::new(to.0, to.1));
    view.end_drag();
}

fn buffer_text(app: &App) -> String {
    app.panes[app.focus].view.buffer.to_string()
}

#[test]
fn ctrl_r_routes_to_vim_redo_in_normal_mode() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello").unwrap();
    for c in "0xu".chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
    assert_eq!(buffer_text(&app), "hello"); // x undone by u
    app.apply_key(
        Key::Char('r'),
        Mods {
            ctrl: true,
            ..Mods::default()
        },
    );
    assert_eq!(buffer_text(&app), "ello"); // Ctrl+R redid the x
}

/// An `App` with one pathless editor pane on the given clipboard — the
/// multi-window construction path, where each window's `App` receives a clone
/// of one process-wide [`SharedClipboard`].
fn app_on_clipboard(clipboard: Box<dyn Clipboard>) -> App {
    App::new(
        None,
        LayoutNode::Row {
            children: vec![LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            }],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        clipboard,
    )
}

/// Two `App`s (two windows) built on clones of one [`SharedClipboard`] share
/// yanks: `yy` in one window, `p` in the other pastes the same line — even
/// with the in-process fallback as the inner clipboard (no OS pasteboard).
#[test]
fn apps_built_on_shared_clipboard_clones_share_yanks() {
    let shared = SharedClipboard::new(Box::new(InMemoryClipboard::default()));
    let mut a = app_on_clipboard(Box::new(shared.clone()));
    let mut b = app_on_clipboard(Box::new(shared.clone()));

    a.insert_text("shared line").unwrap();
    for c in "yy".chars() {
        a.apply_key(Key::Char(c), Mods::default());
    }

    // Paste in the OTHER app. Its own vim register never saw a yank, so the
    // text can only arrive through the shared clipboard.
    b.apply_key(Key::Char('p'), Mods::default());
    assert_eq!(buffer_text(&b), "shared line");
}

// --- global Ctrl Mac shortcuts: Ctrl+C / X / V / A / Q ---------------

fn ctrl_key(app: &mut App, c: char) {
    app.apply_key(
        Key::Char(c),
        Mods {
            ctrl: true,
            ..Mods::default()
        },
    );
}

#[test]
fn ctrl_c_copies_selection_like_cmd_c() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello world").unwrap();
    select(&mut app, (0, 0), (0, 5));

    ctrl_key(&mut app, 'c');

    assert_eq!(app.clipboard.get().as_deref(), Some("hello"));
    assert_eq!(buffer_text(&app), "hello world"); // copy doesn't edit
}

#[test]
fn ctrl_x_cuts_selection_as_one_undo_transaction() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello world").unwrap();
    select(&mut app, (0, 0), (0, 6));

    ctrl_key(&mut app, 'x');

    assert_eq!(app.clipboard.get().as_deref(), Some("hello "));
    assert_eq!(buffer_text(&app), "world");

    cmd_key(&mut app, 'z'); // one undo restores the cut text
    assert_eq!(buffer_text(&app), "hello world");
}

#[test]
fn ctrl_v_pastes_like_cmd_v() {
    let mut app = app_with_panes(&[None]);
    app.clipboard.set("abc");

    ctrl_key(&mut app, 'v');

    assert_eq!(buffer_text(&app), "abc");
    assert_eq!(app.panes[0].view.vim.mode, vim::Mode::Normal);
}

#[test]
fn ctrl_a_selects_all_in_every_mode() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("line one\nline two").unwrap();

    ctrl_key(&mut app, 'a');

    assert_eq!(app.panes[0].view.selected_text(), "line one\nline two");
}

#[test]
fn ctrl_q_quits() {
    let mut app = app_with_panes(&[None]);
    ctrl_key(&mut app, 'q');
    assert!(app.should_quit());
}

#[test]
fn ctrl_r_still_routes_to_vim_redo() {
    // The clipboard override must not steal Ctrl+R from vim.
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello").unwrap();
    for c in "0xu".chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
    assert_eq!(buffer_text(&app), "hello");
    ctrl_key(&mut app, 'r');
    assert_eq!(buffer_text(&app), "ello");
}

#[test]
fn cmd_c_copies_selection_and_keeps_it() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello world").unwrap();
    select(&mut app, (0, 0), (0, 5));

    cmd_key(&mut app, 'c');

    assert_eq!(app.clipboard.get().as_deref(), Some("hello"));
    assert_eq!(buffer_text(&app), "hello world"); // copy doesn't edit
    assert_eq!(app.panes[0].view.selected_text(), "hello"); // selection stays
}

#[test]
fn cmd_c_without_selection_is_a_noop() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello").unwrap();

    cmd_key(&mut app, 'c');

    assert_eq!(app.clipboard.get(), None);
}

#[test]
fn cmd_x_cuts_selection_as_one_undo_transaction() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello world").unwrap();
    select(&mut app, (0, 0), (0, 6));

    cmd_key(&mut app, 'x');

    assert_eq!(app.clipboard.get().as_deref(), Some("hello "));
    assert_eq!(buffer_text(&app), "world");

    cmd_key(&mut app, 'z'); // one undo restores the cut text
    assert_eq!(buffer_text(&app), "hello world");
}

#[test]
fn cmd_x_without_selection_is_a_noop() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello").unwrap();

    cmd_key(&mut app, 'x');

    assert_eq!(app.clipboard.get(), None);
    assert_eq!(buffer_text(&app), "hello");
}

#[test]
fn cmd_v_pastes_and_clamps_cursor_in_normal_mode() {
    let mut app = app_with_panes(&[None]);
    app.clipboard.set("abc");

    cmd_key(&mut app, 'v');

    assert_eq!(buffer_text(&app), "abc");
    // Normal mode: the caret may sit at most on the last character.
    assert_eq!(app.panes[0].view.vim.mode, vim::Mode::Normal);
    assert_eq!(app.panes[0].view.cursor, garden_core::Point::new(0, 2));
}

#[test]
fn cmd_v_in_insert_mode_leaves_cursor_after_paste() {
    let mut app = app_with_panes(&[None]);
    app.apply_key(Key::Char('i'), Mods::default()); // enter Insert mode
    app.clipboard.set("abc");

    cmd_key(&mut app, 'v');

    assert_eq!(buffer_text(&app), "abc");
    assert_eq!(app.panes[0].view.cursor, garden_core::Point::new(0, 3));
}

#[test]
fn cmd_v_replaces_selection_as_one_undo_transaction() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello world").unwrap();
    select(&mut app, (0, 0), (0, 5));
    app.clipboard.set("bye");

    cmd_key(&mut app, 'v');
    assert_eq!(buffer_text(&app), "bye world");

    cmd_key(&mut app, 'z'); // one undo reverses the whole replacement
    assert_eq!(buffer_text(&app), "hello world");
}

#[test]
fn cmd_v_with_empty_clipboard_is_a_noop() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello").unwrap();

    cmd_key(&mut app, 'v');

    assert_eq!(buffer_text(&app), "hello");
}

#[test]
fn cmd_v_in_visual_mode_replaces_selection_and_returns_to_normal() {
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello").unwrap();
    app.panes[0].view.cursor = garden_core::Point::default();
    app.apply_key(Key::Char('v'), Mods::default()); // Visual mode
    app.apply_key(Key::Char('l'), Mods::default()); // selection "h"
    app.clipboard.set("X");

    cmd_key(&mut app, 'v');

    assert_eq!(buffer_text(&app), "Xello");
    assert_eq!(app.panes[0].view.vim.mode, vim::Mode::Normal);
}

#[test]
fn cmd_c_then_cmd_v_round_trips_in_process() {
    // The in-memory clipboard doubles as the fallback when no OS
    // pasteboard exists, so this is the offline round-trip guarantee.
    let mut app = app_with_panes(&[None]);
    app.insert_text("abc").unwrap();
    select(&mut app, (0, 0), (0, 3));

    cmd_key(&mut app, 'c');
    app.panes[0].view.anchor = None; // drop the selection
    app.panes[0].view.cursor = garden_core::Point::new(0, 3);
    app.apply_key(Key::Char('i'), Mods::default()); // insert at the end
    cmd_key(&mut app, 'v');

    assert_eq!(buffer_text(&app), "abcabc");
}

// --- search: the / and ? prompts, n/N, :noh ---------------------------

fn type_keys(app: &mut App, s: &str) {
    for c in s.chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
}

/// An app with one pane holding `text`, cursor at the origin.
fn app_with_text(text: &str) -> App {
    let mut app = app_with_panes(&[None]);
    app.panes[0].view.buffer = garden_core::Buffer::from_str(text);
    app
}

fn cursor(app: &App) -> garden_core::Point {
    app.panes[app.focus].view.cursor
}

#[test]
fn undo_after_join_restores_the_pre_join_cursor() {
    // Repro of bug-a3a55604: J (join) then u (undo) should put the caret back
    // where it was before the join, not at the end of the restored region.
    let mut app = app_with_text("abc\ndef");
    type_keys(&mut app, "l"); // caret -> (0, 1)
    let before = cursor(&app);
    assert_eq!(before, garden_core::Point::new(0, 1));

    type_keys(&mut app, "J"); // join the two lines
    assert_eq!(app.panes[0].view.buffer.to_string(), "abc def");

    type_keys(&mut app, "u"); // undo the join
    assert_eq!(app.panes[0].view.buffer.to_string(), "abc\ndef");
    assert_eq!(cursor(&app), before);
}

#[test]
fn set_wrap_command_toggles_the_focused_pane() {
    use crate::command_line::Command;
    let mut app = app_with_text("hello world");
    // Editor panes wrap by default.
    assert!(app.panes[0].view.wrap);

    app.run_command(Command::SetWrap(false));
    assert!(!app.panes[0].view.wrap);
    assert_eq!(app.status_note.as_deref(), Some("nowrap"));

    app.run_command(Command::SetWrap(true));
    assert!(app.panes[0].view.wrap);
    assert_eq!(app.status_note.as_deref(), Some("wrap"));
}

#[test]
fn slash_search_jumps_to_first_match_after_cursor() {
    let mut app = app_with_text("alpha\nbravo\ncharlie");
    type_keys(&mut app, "/bravo");
    app.apply_key(Key::Enter, Mods::default());

    assert!(app.command_line.is_none());
    assert_eq!(cursor(&app), garden_core::Point::new(1, 0));
    assert_eq!(app.panes[0].view.vim.last_search.as_deref(), Some("bravo"));
    assert!(app.panes[0].view.vim.search_hl);
    assert_eq!(app.status_error, None);
}

#[test]
fn slash_search_wraps_past_eof() {
    let mut app = app_with_text("target\nmiddle\nend");
    type_keys(&mut app, "G"); // last line
    type_keys(&mut app, "/target");
    app.apply_key(Key::Enter, Mods::default());

    assert_eq!(cursor(&app), garden_core::Point::new(0, 0));
}

#[test]
fn slash_search_not_found_reports_and_stays() {
    let mut app = app_with_text("alpha\nbravo");
    type_keys(&mut app, "/zzz");
    app.apply_key(Key::Enter, Mods::default());

    assert_eq!(cursor(&app), garden_core::Point::new(0, 0));
    assert_eq!(
        app.status_error.as_deref(),
        Some("E: pattern not found: zzz")
    );
}

#[test]
fn command_line_suppresses_the_status_right_slot() {
    // The right-slot error/note sits at x = w * 0.4; a long command grows
    // rightward from the left and would overlap it. While the command line is
    // open it takes over the whole status bar, so the right slot is suppressed.
    let right_slot_x = 800.0 * 0.4;
    let right_slot_texts = |app: &App| -> usize {
        app.build_scene()
            .primitives
            .iter()
            .filter(|p| matches!(p, garden_render::Primitive::Text { pos, .. } if pos.0 == right_slot_x))
            .count()
    };

    let mut app = app_with_text("alpha");
    app.status_error = Some("boom".to_string());
    // Closed command line: the error paints in the right slot.
    assert_eq!(right_slot_texts(&app), 1);

    // Open command line: the right slot is suppressed so nothing overlaps the
    // (possibly long) command text.
    app.command_line = Some(crate::command_line::CommandLine::new());
    assert_eq!(right_slot_texts(&app), 0);
}

#[test]
fn question_mark_searches_backward_and_n_follows() {
    let mut app = app_with_text("alpha\nbravo\nalpha");
    type_keys(&mut app, "j"); // cursor (1, 0)
    type_keys(&mut app, "?alpha");
    app.apply_key(Key::Enter, Mods::default());
    assert_eq!(cursor(&app), garden_core::Point::new(0, 0));

    type_keys(&mut app, "n"); // backward again, wraps to the bottom
    assert_eq!(cursor(&app), garden_core::Point::new(2, 0));
}

#[test]
fn escape_cancels_search_prompt_and_keeps_cursor() {
    let mut app = app_with_text("alpha\nbravo");
    type_keys(&mut app, "/bra");
    app.apply_key(Key::Escape, Mods::default());

    assert!(app.command_line.is_none());
    assert_eq!(cursor(&app), garden_core::Point::new(0, 0));
    assert_eq!(app.panes[0].view.vim.last_search, None); // nothing accepted
}

#[test]
fn backspace_edits_then_closes_search_prompt() {
    let mut app = app_with_text("alpha");
    type_keys(&mut app, "/a");
    app.apply_key(Key::Backspace, Mods::default());
    assert!(app.command_line.is_some()); // still open, input now empty
    app.apply_key(Key::Backspace, Mods::default());
    assert!(app.command_line.is_none()); // backspace on empty cancels
}

#[test]
fn empty_search_accept_is_a_noop() {
    let mut app = app_with_text("alpha");
    type_keys(&mut app, "/");
    app.apply_key(Key::Enter, Mods::default());

    assert!(app.command_line.is_none());
    assert_eq!(cursor(&app), garden_core::Point::new(0, 0));
    assert_eq!(app.status_error, None);
}

#[test]
fn search_prompt_shows_in_state_json() {
    let mut app = app_with_text("alpha");
    type_keys(&mut app, "/br");
    assert_eq!(app.state_json()["command_line"], json!("/br"));

    app.apply_key(Key::Escape, Mods::default());
    type_keys(&mut app, ":w");
    assert_eq!(app.state_json()["command_line"], json!(":w"));
}

#[test]
fn noh_clears_highlight_but_keeps_pattern_for_n() {
    let mut app = app_with_text("foo bar foo");
    type_keys(&mut app, "/foo");
    app.apply_key(Key::Enter, Mods::default());
    assert!(app.panes[0].view.vim.search_hl);

    type_keys(&mut app, ":noh");
    app.apply_key(Key::Enter, Mods::default());
    assert!(!app.panes[0].view.vim.search_hl);
    assert_eq!(app.panes[0].view.vim.last_search.as_deref(), Some("foo"));

    type_keys(&mut app, "n"); // pattern still repeats (and re-highlights)
    assert_eq!(cursor(&app), garden_core::Point::new(0, 0)); // wrapped around
    assert!(app.panes[0].view.vim.search_hl);
}

#[test]
fn escape_in_normal_mode_does_not_clear_highlights() {
    let mut app = app_with_text("foo bar foo");
    type_keys(&mut app, "/foo");
    app.apply_key(Key::Enter, Mods::default());
    app.apply_key(Key::Escape, Mods::default());
    assert!(app.panes[0].view.vim.search_hl); // vim default: Esc keeps :hls
}

// --- multi-click: counting and routing ---------------------------------

use std::time::{Duration, Instant};

#[test]
fn click_counter_counts_quick_clicks_in_place() {
    let mut c = ClickCounter::new();
    let t0 = Instant::now();
    assert_eq!(c.click(10.0, 10.0, t0), 1);
    assert_eq!(c.click(11.0, 10.0, t0 + Duration::from_millis(200)), 2);
    assert_eq!(c.click(10.0, 11.0, t0 + Duration::from_millis(400)), 3);
}

#[test]
fn click_counter_resets_after_the_time_window() {
    let mut c = ClickCounter::new();
    let t0 = Instant::now();
    c.click(10.0, 10.0, t0);
    assert_eq!(c.click(10.0, 10.0, t0 + Duration::from_millis(700)), 1);
}

#[test]
fn click_counter_resets_when_the_mouse_moved_away() {
    let mut c = ClickCounter::new();
    let t0 = Instant::now();
    c.click(10.0, 10.0, t0);
    assert_eq!(c.click(40.0, 10.0, t0 + Duration::from_millis(100)), 1);
}

/// Logical pixel position of the center of `(line, col)` in the test
/// app's single pane: rect (6,6), PAD 6, no gutter (line numbers default off),
/// cell (8,16).
fn cell_pos(line: usize, col: usize) -> (f32, f32) {
    (
        6.0 + 6.0 + col as f32 * 8.0,
        6.0 + 6.0 + line as f32 * 16.0 + 8.0,
    )
}

#[test]
fn double_click_selects_word_under_mouse() {
    let mut app = app_with_text("hello world");
    let (x, y) = cell_pos(0, 7);
    app.mouse_down(x, y, Mods::default(), 2);
    app.mouse_up();
    assert_eq!(app.panes[0].view.selected_text(), "world");
}

#[test]
fn triple_click_selects_whole_line_under_mouse() {
    let mut app = app_with_text("alpha\nbravo\ncharlie");
    let (x, y) = cell_pos(1, 2);
    app.mouse_down(x, y, Mods::default(), 3);
    app.mouse_up();
    assert_eq!(app.panes[0].view.selected_text(), "bravo\n");
}

#[test]
fn debug_mouse_click_carries_the_click_count() {
    let mut app = app_with_text("hello world");
    let (x, y) = cell_pos(0, 2);
    let reply = app
        .handle_debug(DebugCmd::Mouse {
            op: "click".to_string(),
            x,
            y,
            to: None,
            lines: 0.0,
            cols: 0.0,
            mods: Mods::default(),
            clicks: 2,
            button: 0,
        })
        .unwrap();
    let Reply::Json(ack) = reply else {
        panic!("expected a JSON ack")
    };
    assert_eq!(ack["selection"]["text"], json!("hello"));
}

/// `line_numbers` is a per-pane property: opening a different file in the pane
/// (`:e`, File ▸ Open) must keep the gutter on, and the layout rebuilt from the
/// live panes must still report it — otherwise the immediate `sync_layout` after
/// an `:e` would persist the gutter back off. Regression for set_editor.
#[test]
fn opening_a_file_preserves_line_numbers_config() {
    let mut app = app_with_panes(&[None]);
    app.panes[0].view.show_line_numbers = true;
    app.open_path("README.md");
    assert!(
        app.panes[0].view.show_line_numbers,
        "gutter must survive :e"
    );
    assert_eq!(
        app.panes[0].to_layout_node(),
        LayoutNode::Editor {
            file: Some("README.md".into()),
            line_numbers: true,
            wrap: true
        }
    );
}

// --- :s substitution ---------------------------------------------------

/// Type `cmd` after a `:` and press Enter.
fn run_ex(app: &mut App, cmd: &str) {
    app.apply_key(Key::Char(':'), Mods::default());
    type_keys(app, cmd);
    app.apply_key(Key::Enter, Mods::default());
}

// --- :N jump-to-line ---------------------------------------------------

#[test]
fn colon_number_jumps_to_first_non_blank_of_that_line() {
    let mut app = app_with_text("alpha\nbravo\n  charlie");
    run_ex(&mut app, "3");
    assert_eq!(cursor(&app), garden_core::Point::new(2, 2));
    run_ex(&mut app, "1");
    assert_eq!(cursor(&app), garden_core::Point::new(0, 0));
}

#[test]
fn colon_number_clamps_to_the_buffer_and_dollar_hits_the_end() {
    let mut app = app_with_text("alpha\nbravo\ncharlie");
    run_ex(&mut app, "999");
    assert_eq!(cursor(&app), garden_core::Point::new(2, 0));
    run_ex(&mut app, "1");
    run_ex(&mut app, "$");
    assert_eq!(cursor(&app), garden_core::Point::new(2, 0));
}

// --- status-bar message lifecycle ---------------------------------------

#[test]
fn transient_status_messages_clear_on_the_next_key() {
    let mut app = app_with_text("alpha");
    run_ex(&mut app, "bogus");
    assert_eq!(app.status_error.as_deref(), Some("E: not a command: bogus"));
    // The very next key starts a new action; the old message goes away.
    app.apply_key(Key::Char('j'), Mods::default());
    assert_eq!(app.status_error, None);

    app.status_note = Some("wrote something".to_string());
    app.apply_key(Key::Char('k'), Mods::default());
    assert_eq!(app.status_note, None);
}

#[test]
fn script_errors_survive_keypresses() {
    // A broken layout script stays broken until it reloads cleanly, so its
    // error must outlive the keypress-clearing of transient messages.
    let mut app = app_with_text("alpha");
    app.script_error = Some("parse error".to_string());
    app.apply_key(Key::Char('j'), Mods::default());
    assert_eq!(app.script_error.as_deref(), Some("parse error"));
}

#[test]
fn substitute_current_line_first_match_only() {
    let mut app = app_with_text("foo foo\nfoo foo");
    run_ex(&mut app, "s/foo/bar/");
    assert_eq!(buffer_text(&app), "bar foo\nfoo foo");
}

#[test]
fn substitute_current_line_global() {
    let mut app = app_with_text("foo foo\nfoo foo");
    run_ex(&mut app, "s/foo/bar/g");
    assert_eq!(buffer_text(&app), "bar bar\nfoo foo");
}

#[test]
fn substitute_whole_buffer_global() {
    let mut app = app_with_text("foo foo\nfoo foo\nbaz");
    run_ex(&mut app, "%s/foo/bar/g");
    assert_eq!(buffer_text(&app), "bar bar\nbar bar\nbaz");
}

#[test]
fn substitute_reports_when_pattern_not_found() {
    let mut app = app_with_text("alpha\nbravo");
    run_ex(&mut app, "%s/zzz/x/g");
    assert_eq!(buffer_text(&app), "alpha\nbravo"); // untouched
    assert!(app
        .status_error
        .as_deref()
        .unwrap_or("")
        .contains("not found"));
}

#[test]
fn substitute_is_one_undo_transaction() {
    let mut app = app_with_text("foo\nfoo\nfoo");
    run_ex(&mut app, "%s/foo/bar/g");
    assert_eq!(buffer_text(&app), "bar\nbar\nbar");
    app.apply_key(Key::Char('u'), Mods::default()); // single undo reverses all
    assert_eq!(buffer_text(&app), "foo\nfoo\nfoo");
}

#[test]
fn substitute_empty_pattern_reuses_last_search() {
    let mut app = app_with_text("foo foo\nfoo");
    app.panes[0].view.vim.last_search = Some("foo".to_string());
    run_ex(&mut app, "%s//bar/g");
    assert_eq!(buffer_text(&app), "bar bar\nbar");
}

#[test]
fn substitute_puts_cursor_on_the_last_changed_line() {
    let mut app = app_with_text("foo\nplain\nfoo");
    run_ex(&mut app, "%s/foo/bar/");
    assert_eq!(cursor(&app).line, 2);
}

#[test]
fn substitute_reports_the_count_in_a_status_note() {
    let mut app = app_with_text("foo foo\nfoo");
    run_ex(&mut app, "%s/foo/bar/g");
    let note = app.status_note.as_deref().unwrap_or("");
    assert!(note.contains('3'), "expected a count of 3 in: {note}");
}

#[test]
fn substitute_line_range_touches_only_those_lines() {
    let mut app = app_with_text("foo\nfoo\nfoo\nfoo");
    run_ex(&mut app, "2,3s/foo/bar/");
    assert_eq!(buffer_text(&app), "foo\nbar\nbar\nfoo");
}

#[test]
fn substitute_range_addresses_resolve_dot_and_dollar() {
    let mut app = app_with_text("foo\nfoo\nfoo");
    app.panes[0].view.cursor = garden_core::Point::new(1, 0);
    run_ex(&mut app, ".,$s/foo/bar/");
    assert_eq!(buffer_text(&app), "foo\nbar\nbar");
}

#[test]
fn substitute_range_clamps_and_reorders() {
    // 9,2 → lines 2..=3 (clamped to the buffer, reordered).
    let mut app = app_with_text("foo\nfoo\nfoo");
    run_ex(&mut app, "9,2s/foo/bar/");
    assert_eq!(buffer_text(&app), "foo\nbar\nbar");
}

#[test]
fn substitute_ignore_case_flag() {
    let mut app = app_with_text("FOO Foo foo");
    run_ex(&mut app, "s/foo/bar/gi");
    assert_eq!(buffer_text(&app), "bar bar bar");
}

#[test]
fn substitute_is_case_sensitive_without_the_flag() {
    let mut app = app_with_text("FOO foo");
    run_ex(&mut app, "s/foo/bar/g");
    assert_eq!(buffer_text(&app), "FOO bar");
}

// --- external file refresh (poll_files) --------------------------------

#[test]
fn poll_files_reloads_clean_pane_on_external_change() {
    let dir = temp_dir("refresh-clean");
    let a = file_with(&dir, "a.txt", "alpha\n");
    let mut app = app_with_panes(&[Some(&a)]);
    assert!(!app.panes[0].view.buffer.is_dirty());

    fs::write(&a, "changed on disk\n").unwrap();
    app.poll_files();

    assert_eq!(buffer_text(&app), "changed on disk\n");
    assert!(!app.panes[0].view.buffer.is_dirty());
    assert!(app.panes[0].external_conflict.is_none());
}

#[test]
fn poll_files_keeps_dirty_pane_and_warns() {
    let dir = temp_dir("refresh-dirty");
    let a = file_with(&dir, "a.txt", "alpha\n");
    let mut app = app_with_panes(&[Some(&a)]);
    app.panes[0].view.insert("x"); // now dirty: "xalpha\n"

    fs::write(&a, "external edit\n").unwrap();
    app.poll_files();

    assert_eq!(buffer_text(&app), "xalpha\n"); // unsaved edit kept
    assert!(app.panes[0].external_conflict.is_some());
    let note = app.status_note.as_deref().unwrap_or("");
    assert!(note.contains("changed on disk"), "unexpected note: {note}");
}

#[test]
fn poll_files_warns_once_per_disk_version() {
    let dir = temp_dir("refresh-dedupe");
    let a = file_with(&dir, "a.txt", "alpha\n");
    let mut app = app_with_panes(&[Some(&a)]);
    app.panes[0].view.insert("x");

    fs::write(&a, "external edit\n").unwrap();
    app.poll_files();
    app.status_note = None; // clear; a second poll with no new change is silent
    app.poll_files();
    assert_eq!(app.status_note, None);
}

#[test]
fn poll_files_reloads_after_dirty_buffer_becomes_clean() {
    let dir = temp_dir("refresh-becomes-clean");
    let a = file_with(&dir, "a.txt", "alpha\n");
    let mut app = app_with_panes(&[Some(&a)]);
    app.panes[0].view.insert("x"); // dirty

    fs::write(&a, "external edit\n").unwrap();
    app.poll_files(); // conflict: kept, warned
    assert_eq!(buffer_text(&app), "xalpha\n");

    app.panes[0].view.undo(); // back to "alpha\n": clean again
    assert!(!app.panes[0].view.buffer.is_dirty());
    app.poll_files(); // now safe to reload

    assert_eq!(buffer_text(&app), "external edit\n");
    assert!(app.panes[0].external_conflict.is_none());
}

#[test]
fn poll_files_reloads_again_after_edit_undo_following_a_prior_reload() {
    // Cross-feature: a buffer that was already reloaded once, then edited,
    // conflicted, and undone back to clean, must still reload on the next
    // poll (the dirty flag and disk stamp have to be consistent post-undo).
    let dir = temp_dir("refresh-reload-twice");
    let a = file_with(&dir, "a.txt", "v1\n");
    let mut app = app_with_panes(&[Some(&a)]);

    fs::write(&a, "EXTERNAL ONE\n").unwrap();
    app.poll_files(); // first reload (clean)
    assert_eq!(buffer_text(&app), "EXTERNAL ONE\n");
    assert!(!app.panes[0].view.buffer.is_dirty());

    app.panes[0].view.insert("X"); // dirty again
    fs::write(&a, "DISK TWO\n").unwrap();
    app.poll_files(); // conflict: kept + warned
    assert_eq!(buffer_text(&app), "XEXTERNAL ONE\n");

    app.panes[0].view.undo(); // back to "EXTERNAL ONE\n"
    assert!(
        !app.panes[0].view.buffer.is_dirty(),
        "should be clean after undo"
    );
    app.poll_files(); // free to reload the newest disk content
    assert_eq!(buffer_text(&app), "DISK TWO\n");
}

#[test]
fn poll_files_clamps_cursor_after_reload_shrinks_file() {
    let dir = temp_dir("refresh-clamp");
    let a = file_with(&dir, "a.txt", "l0\nl1\nl2\nl3\nl4\n");
    let mut app = app_with_panes(&[Some(&a)]);
    app.panes[0].view.cursor = garden_core::Point::new(4, 1);

    fs::write(&a, "only\n").unwrap();
    app.poll_files();

    assert_eq!(buffer_text(&app), "only\n");
    let c = app.panes[0].view.cursor;
    assert!(c.line < app.panes[0].view.buffer.line_count());
}

#[test]
fn cmd_shift_s_saves_all_panes() {
    let dir = temp_dir("cmd-shift-s");
    let a = file_with(&dir, "a.txt", "alpha");
    let b = file_with(&dir, "b.txt", "beta");
    let mut app = app_with_panes(&[Some(&a), Some(&b), None]);
    dirty_all(&mut app);

    app.apply_key(
        Key::Char('s'),
        Mods {
            cmd: true,
            shift: true,
            ..Mods::default()
        },
    );

    assert!(!app.should_quit());
    assert_eq!(fs::read_to_string(&a).unwrap(), "xalpha");
    assert_eq!(fs::read_to_string(&b).unwrap(), "xbeta"); // unfocused pane saved too
}

// --- Ctrl+W window navigation ------------------------------------------

/// Press `Ctrl+W` then `c` as a complete window command.
fn window_cmd(app: &mut App, c: char) {
    ctrl_key(app, 'w');
    app.apply_key(Key::Char(c), Mods::default());
}

#[test]
fn ctrl_w_l_and_h_move_focus_across_a_row() {
    // Three editors in a row: focus starts at 0 (leftmost).
    let mut app = app_with_panes(&[None, None, None]);
    assert_eq!(app.focus, 0);

    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 1);
    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 2);
    window_cmd(&mut app, 'h');
    assert_eq!(app.focus, 1);
}

#[test]
fn ctrl_w_at_the_edge_keeps_focus() {
    let mut app = app_with_panes(&[None, None]);
    window_cmd(&mut app, 'h'); // already leftmost
    assert_eq!(app.focus, 0);

    app.focus = 1;
    window_cmd(&mut app, 'l'); // already rightmost
    assert_eq!(app.focus, 1);
}

#[test]
fn ctrl_w_w_cycles_through_panes() {
    let mut app = app_with_panes(&[None, None, None]);
    window_cmd(&mut app, 'w');
    assert_eq!(app.focus, 1);
    window_cmd(&mut app, 'w');
    assert_eq!(app.focus, 2);
    window_cmd(&mut app, 'w'); // wraps back to 0
    assert_eq!(app.focus, 0);
}

#[test]
fn ctrl_w_direction_may_hold_ctrl() {
    // vim accepts both `Ctrl+W l` and `Ctrl+W Ctrl+L`.
    let mut app = app_with_panes(&[None, None]);
    ctrl_key(&mut app, 'w');
    ctrl_key(&mut app, 'l');
    assert_eq!(app.focus, 1);
}

#[test]
fn ctrl_w_then_unrelated_key_cancels_without_editing() {
    // The pending prefix must swallow the next key so it neither navigates
    // nor reaches the vim layer (where `x` would delete a character).
    let mut app = app_with_panes(&[None]);
    app.insert_text("hello").unwrap();
    app.panes[0].view.cursor = garden_core::Point::default();

    ctrl_key(&mut app, 'w');
    assert!(app.window_cmd_pending);
    app.apply_key(Key::Char('x'), Mods::default());

    assert!(!app.window_cmd_pending);
    assert_eq!(buffer_text(&app), "hello"); // not deleted
    assert_eq!(app.focus, 0);
}

#[test]
fn ctrl_w_does_not_reach_vim_as_a_regular_key() {
    // The single-pane case: Ctrl+W sets the prefix and a following motion
    // key is consumed by it, leaving the buffer untouched.
    let mut app = app_with_panes(&[None]);
    app.insert_text("abc").unwrap();
    window_cmd(&mut app, 'l'); // no pane to the right; harmless no-op
    assert_eq!(app.focus, 0);
    assert_eq!(buffer_text(&app), "abc");
}

#[test]
fn window_cmd_pending_shows_in_state_json() {
    let mut app = app_with_panes(&[None, None]);
    ctrl_key(&mut app, 'w');
    assert_eq!(app.state_json()["window_cmd_pending"], json!(true));
    app.apply_key(Key::Char('l'), Mods::default());
    assert_eq!(app.state_json()["window_cmd_pending"], json!(false));
}

#[test]
fn ctrl_w_navigates_a_two_row_column_layout() {
    // A column of two editors: pane 0 on top, pane 1 below.
    let mut app = App::new(
        None,
        LayoutNode::Column {
            children: vec![
                LayoutNode::Editor {
                    file: None,
                    line_numbers: false,
                    wrap: true,
                },
                LayoutNode::Editor {
                    file: None,
                    line_numbers: false,
                    wrap: true,
                },
            ],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );

    window_cmd(&mut app, 'j');
    assert_eq!(app.focus, 1);
    window_cmd(&mut app, 'k');
    assert_eq!(app.focus, 0);
}

// --- Ctrl+W o (expand focused pane to fill the window) -----------------

#[test]
fn ctrl_w_o_collapses_to_focused_pane_no_script() {
    // Three editors in a row; focus the middle one, then expand it.
    let a = "a.txt";
    let mut app = app_with_panes(&[Some(a), Some("b.txt"), Some("c.txt")]);
    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 1);

    window_cmd(&mut app, 'o');

    assert_eq!(app.panes.len(), 1, "only the focused pane should remain");
    assert_eq!(app.panes[0].file.as_deref(), Some("b.txt"));
    // The in-memory fallback layout now describes the single pane.
    assert_eq!(
        app.fallback_layout,
        LayoutNode::Editor {
            file: Some("b.txt".into()),
            line_numbers: false,
            wrap: true
        }
    );
}

#[test]
fn ctrl_w_o_on_single_pane_is_noop() {
    let mut app = app_with_panes(&[Some("only.txt")]);
    window_cmd(&mut app, 'o');
    assert_eq!(app.panes.len(), 1);
    assert_eq!(app.panes[0].file.as_deref(), Some("only.txt"));
}

#[test]
fn ctrl_w_o_persists_to_transient_script() {
    let dir = temp_dir("ctrl-w-o-transient");
    let base = dir.join("init.ptl");
    fs::write(
        &base,
        "// my layout\nlayout(row([editor(\"left.rs\"), editor(\"right.rs\")], [0.5, 0.5]))\n",
    )
    .unwrap();
    let host = ScriptHost::load(&base).unwrap();
    let mut app = App::new(
        Some(host),
        LayoutNode::Editor {
            file: None,
            line_numbers: false,
            wrap: true,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    assert_eq!(app.panes.len(), 2);

    // Focus the right pane, then collapse to it.
    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 1);
    window_cmd(&mut app, 'o');

    // Layout collapsed in memory…
    assert_eq!(app.panes.len(), 1);
    assert_eq!(app.panes[0].file.as_deref(), Some("right.rs"));

    // …and persisted to the transient sibling, preserving the comment, with the
    // base file left untouched.
    let transient = dir.join("init.transient.ptl");
    assert!(transient.exists(), "transient script should be written");
    let written = fs::read_to_string(&transient).unwrap();
    assert!(
        written.contains("// my layout"),
        "comment preserved: {written}"
    );
    assert!(
        written.contains("layout(editor(\"right.rs\"))"),
        "got: {written}"
    );
    let base_text = fs::read_to_string(&base).unwrap();
    assert!(
        base_text.contains("row("),
        "base file untouched: {base_text}"
    );
}

// --- Ctrl+W s / Ctrl+W v (split the focused pane) ----------------------

#[test]
fn ctrl_w_s_stacks_a_new_pane_below_the_focused_one() {
    // A single editor; `Ctrl+W s` should produce two stacked panes side by side
    // vertically (a Column), the new one showing the same file.
    let mut app = app_with_panes(&[Some("main.rs")]);
    assert_eq!(app.panes.len(), 1);

    window_cmd(&mut app, 's');

    assert_eq!(app.panes.len(), 2, "split adds a pane");
    // Stacked → the two panes share a column (same x, different y).
    assert_eq!(app.panes[0].rect.x, app.panes[1].rect.x);
    assert!(app.panes[1].rect.y > app.panes[0].rect.y);
    // Both editors are on the focused pane's file.
    assert_eq!(app.panes[0].file.as_deref(), Some("main.rs"));
    assert_eq!(app.panes[1].file.as_deref(), Some("main.rs"));
    // Focus stays on the original (top) pane.
    assert_eq!(app.focus, 0);
}

#[test]
fn ctrl_w_v_places_a_new_pane_to_the_right() {
    let mut app = app_with_panes(&[Some("main.rs")]);

    window_cmd(&mut app, 'v');

    assert_eq!(app.panes.len(), 2);
    // Side by side → same y, different x.
    assert_eq!(app.panes[0].rect.y, app.panes[1].rect.y);
    assert!(app.panes[1].rect.x > app.panes[0].rect.x);
}

#[test]
fn ctrl_w_v_splits_only_the_focused_pane_of_a_row() {
    // Two editors in a row; focus the right one and split it vertically. The
    // left pane is untouched; the right becomes a nested row of two.
    let mut app = app_with_panes(&[Some("left.rs"), Some("right.rs")]);
    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 1);

    window_cmd(&mut app, 'v');

    assert_eq!(app.panes.len(), 3, "only the focused pane split");
    assert_eq!(app.panes[0].file.as_deref(), Some("left.rs"));
    assert_eq!(app.panes[1].file.as_deref(), Some("right.rs"));
    assert_eq!(app.panes[2].file.as_deref(), Some("right.rs"));
    assert_eq!(
        app.fallback_layout,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some("left.rs".into()),
                    line_numbers: false,
                    wrap: true
                },
                LayoutNode::Row {
                    children: vec![
                        LayoutNode::Editor {
                            file: Some("right.rs".into()),
                            line_numbers: false,
                            wrap: true
                        },
                        LayoutNode::Editor {
                            file: Some("right.rs".into()),
                            line_numbers: false,
                            wrap: true
                        },
                    ],
                    ratios: None,
                },
            ],
            ratios: None,
        }
    );
}

#[test]
fn ctrl_w_s_persists_to_transient_script() {
    let dir = temp_dir("ctrl-w-s-transient");
    let base = dir.join("init.ptl");
    fs::write(&base, "// my layout\nlayout(editor(\"only.rs\"))\n").unwrap();
    let mut host = ScriptHost::load(&base).unwrap();
    // Mirror the app wiring: the overlay goes into a per-window state dir, here
    // under a temp dir so the test never touches the real ~/.garden.
    let transient = dir.join("state").join("window-1").join("window.ptl");
    host.set_transient_path(transient.clone());
    let mut app = App::new(
        Some(host),
        LayoutNode::Editor {
            file: None,
            line_numbers: false,
            wrap: true,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );

    window_cmd(&mut app, 's');

    assert_eq!(app.panes.len(), 2);
    // Persisted to the per-window overlay (dir created on demand), comment kept.
    assert!(
        transient.exists(),
        "transient script should be written into the window dir"
    );
    let written = fs::read_to_string(&transient).unwrap();
    assert!(
        written.contains("// my layout"),
        "comment preserved: {written}"
    );
    assert!(
        written.contains("column("),
        "split serialized as a column: {written}"
    );
    // Base file untouched.
    assert_eq!(
        fs::read_to_string(&base).unwrap(),
        "// my layout\nlayout(editor(\"only.rs\"))\n"
    );
}

// --- Ctrl+W c / q and :q (close the focused pane) ----------------------

#[test]
fn ctrl_w_c_closes_the_focused_pane() {
    let mut app = app_with_panes(&[Some("a.txt"), Some("b.txt"), Some("c.txt")]);
    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 1);

    window_cmd(&mut app, 'c');

    assert_eq!(app.panes.len(), 2, "the focused pane closed");
    assert_eq!(app.panes[0].file.as_deref(), Some("a.txt"));
    assert_eq!(app.panes[1].file.as_deref(), Some("c.txt"));
    // Focus keeps its index, landing on the next pane in solver order.
    assert_eq!(app.focus, 1);
    assert!(!app.should_quit());
    // The persisted layout collapsed to the two survivors.
    assert_eq!(
        app.fallback_layout,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some("a.txt".into()),
                    line_numbers: false,
                    wrap: true
                },
                LayoutNode::Editor {
                    file: Some("c.txt".into()),
                    line_numbers: false,
                    wrap: true
                },
            ],
            ratios: None,
        },
    );
}

#[test]
fn ctrl_w_c_closing_the_last_in_a_row_pulls_focus_back() {
    let mut app = app_with_panes(&[Some("a.txt"), Some("b.txt")]);
    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 1);

    window_cmd(&mut app, 'c');

    assert_eq!(app.panes.len(), 1);
    assert_eq!(app.focus, 0, "focus clamps onto the survivor");
    assert_eq!(app.panes[0].file.as_deref(), Some("a.txt"));
}

#[test]
fn ctrl_w_c_refuses_the_last_pane() {
    let mut app = app_with_panes(&[Some("only.txt")]);
    window_cmd(&mut app, 'c');
    assert_eq!(app.panes.len(), 1, "the last pane cannot be closed");
    assert!(!app.should_quit());
    assert!(
        app.status_error
            .as_deref()
            .is_some_and(|e| e.contains("cannot close last pane")),
        "vim-style error expected, got {:?}",
        app.status_error,
    );
}

#[test]
fn wq_command_writes_then_closes_the_pane() {
    let dir = temp_dir("wq-closes-pane");
    let a = file_with(&dir, "a.txt", "alpha");
    let mut app = app_with_panes(&[Some(&a), None]);
    app.panes[0].view.insert("x");
    assert_eq!(app.focus, 0);

    run_ex(&mut app, "wq");

    assert_eq!(
        fs::read_to_string(&a).unwrap(),
        "xalpha",
        ":wq wrote the buffer"
    );
    assert_eq!(app.panes.len(), 1, ":wq closed the pane");
    assert!(!app.should_quit(), "another pane remains open");
}

// --- Runtime content changes stay in sync with the layout --------------

#[test]
fn open_path_syncs_the_layout_source_of_truth() {
    // `:e` swaps a pane's content in place; the persisted layout must follow so
    // it never drifts from what is actually on screen.
    let mut app = app_with_panes(&[Some("a.txt"), Some("b.txt")]);
    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 1);

    run_ex(&mut app, "e c.txt");

    assert_eq!(app.panes[1].file.as_deref(), Some("c.txt"));
    assert_eq!(
        app.fallback_layout,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some("a.txt".into()),
                    line_numbers: false,
                    wrap: true
                },
                LayoutNode::Editor {
                    file: Some("c.txt".into()),
                    line_numbers: false,
                    wrap: true
                },
            ],
            ratios: None,
        },
        "opening a file must update the layout, not leave it stale",
    );
}

#[test]
fn splitting_one_pane_keeps_another_panes_freshly_opened_file() {
    // The reported bug: open a file in one pane, then split a *different* pane.
    // The split must not resurrect the other pane's old content from a stale
    // layout snapshot, and the live (edited) buffer must move across untouched.
    let mut app = app_with_panes(&[Some("a.txt"), Some("b.txt")]);

    // In pane 1, open c.txt and type into it.
    window_cmd(&mut app, 'l');
    assert_eq!(app.focus, 1);
    run_ex(&mut app, "e c.txt");
    assert_eq!(app.panes[1].file.as_deref(), Some("c.txt"));
    app.insert_text("hello").unwrap();

    // Focus pane 0 and split it side by side.
    window_cmd(&mut app, 'h');
    assert_eq!(app.focus, 0);
    window_cmd(&mut app, 'v');

    // Three panes: the split pair on the left (a.txt), then the untouched c.txt.
    assert_eq!(app.panes.len(), 3);
    let files: Vec<_> = app.panes.iter().map(|p| p.file.as_deref()).collect();
    assert_eq!(
        files,
        [Some("a.txt"), Some("a.txt"), Some("c.txt")],
        "the split must not turn pane 2 back into the stale b.txt",
    );
    // The live buffer (with the typed text) moved across, not a fresh one.
    assert!(
        app.panes[2].view.buffer.to_string().contains("hello"),
        "the edited c.txt buffer must survive the split of another pane",
    );
}

#[test]
fn collapsing_to_a_pane_uses_live_sibling_content() {
    // `Ctrl+W o` keeps the focused pane; the focused pane's *current* file (after
    // a `:e`) must be what survives, not a stale snapshot.
    let mut app = app_with_panes(&[Some("a.txt"), Some("b.txt")]);
    window_cmd(&mut app, 'l');
    run_ex(&mut app, "e c.txt");
    window_cmd(&mut app, 'o');

    assert_eq!(app.panes.len(), 1);
    assert_eq!(app.panes[0].file.as_deref(), Some("c.txt"));
    assert_eq!(
        app.fallback_layout,
        LayoutNode::Editor {
            file: Some("c.txt".into()),
            line_numbers: false,
            wrap: true
        }
    );
}

// --- fuzzy file finder ---

#[test]
fn ctrl_p_opens_and_escape_closes_the_file_finder() {
    let mut app = app_with_panes(&[None]);
    assert!(app.file_finder.is_none());

    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    app.apply_key(Key::Char('p'), ctrl);
    assert!(app.file_finder.is_some(), "Ctrl+P should open the finder");

    app.apply_key(Key::Escape, Mods::default());
    assert!(app.file_finder.is_none(), "Escape should close the finder");
}

#[test]
fn file_finder_enter_opens_the_selected_file() {
    let dir = temp_dir("file-finder-open");
    fs::create_dir_all(dir.join("sub")).unwrap();
    file_with(&dir, "alpha.txt", "A");
    fs::write(dir.join("sub/beta.txt"), "B").unwrap();

    let mut app = app_with_panes(&[None]);
    // Seed the finder directly (open_file_finder's gather is exercised against
    // the real tree elsewhere); here we drive the modal keys deterministically.
    app.file_finder_root = dir.clone();
    app.file_finder = Some(crate::file_finder::FileFinder::new(vec![
        "alpha.txt".to_string(),
        "sub/beta.txt".to_string(),
    ]));

    // Type "beta" to select the nested file, then open it with Enter.
    for c in "beta".chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
    app.apply_key(Key::Enter, Mods::default());

    assert!(app.file_finder.is_none(), "opening closes the finder");
    let pane = &app.panes[app.focus];
    let expected = dir.join("sub/beta.txt").to_string_lossy().into_owned();
    assert_eq!(pane.file.as_deref(), Some(expected.as_str()));
    assert_eq!(pane.view.buffer.to_string().trim_end(), "B");
}

#[test]
fn file_finder_query_keys_filter_not_edit_the_buffer() {
    // While the finder is modal, character keys build the query and must not
    // leak into the focused editor's buffer.
    let mut app = app_with_panes(&[None]);
    app.file_finder_root = std::path::PathBuf::from(".");
    app.file_finder = Some(crate::file_finder::FileFinder::new(vec![
        "main.rs".to_string(),
        "lib.rs".to_string(),
    ]));

    for c in "main".chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
    assert_eq!(app.file_finder.as_ref().unwrap().query(), "main");
    assert_eq!(
        app.file_finder.as_ref().unwrap().selected_path(),
        Some("main.rs")
    );
    assert!(
        app.panes[0].view.buffer.to_string().is_empty(),
        "buffer untouched"
    );
}

// ── G5: panel navigation whitelist (path-traversal defense) ────────────────

/// A single-panel `App` whose panel's origin script is `script`, settled once so
/// the panel is loaded and has run its first frame.
fn app_with_panel(script: &str) -> App {
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![LayoutNode::Panel {
                script: script.to_string(),
                screens: Vec::new(),
            }],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    app
}

/// Index of the app's (single) panel pane.
fn panel_idx(app: &App) -> usize {
    app.panes.iter().position(|p| p.is_panel()).unwrap()
}

/// The origin/current screen of the app's panel pane.
fn panel_screen(app: &App) -> String {
    let idx = panel_idx(app);
    app.panes[idx]
        .panel
        .as_ref()
        .unwrap()
        .current_screen()
        .to_string()
}

/// Drive one `navigate(...)` intent through the client-event handler.
fn navigate(app: &mut App, intent: garden_script::NavIntent) {
    let idx = panel_idx(app);
    app.handle_client_events(idx, vec![crate::panel_view::ClientEvent::Navigate(intent)]);
}

#[test]
fn navigate_swaps_to_an_in_directory_screen() {
    use garden_script::NavIntent;
    let base = temp_dir("g5-nav-accept");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let a = file_with(&screens, "a.ptl", "let s = 1\n");
    file_with(&screens, "b.ptl", "let s = 2\n");

    let mut app = app_with_panel(&a);
    let idx = panel_idx(&app);
    assert_eq!(panel_value(&app, "s"), Some(json!(1)), "starts on a.ptl");
    assert_eq!(app.panes[idx].panel.as_ref().unwrap().history_len(), 1);

    // A sibling `.ptl` in the same directory is allowed: the screen swaps and a
    // history entry is pushed.
    navigate(
        &mut app,
        NavIntent::Push("b.ptl".into(), serde_json::Value::Null),
    );
    assert_eq!(panel_screen(&app), "b.ptl", "current screen is now b.ptl");
    assert_eq!(app.panes[idx].panel.as_ref().unwrap().history_len(), 2);
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "s"),
        Some(json!(2)),
        "b.ptl is now running"
    );

    // Back returns to the origin screen; forward returns to b.
    navigate(&mut app, NavIntent::Back);
    assert_eq!(panel_screen(&app), a, "back restores the origin screen");
    navigate(&mut app, NavIntent::Forward);
    assert_eq!(panel_screen(&app), "b.ptl", "forward returns to b.ptl");
}

/// The host affordances — `Ctrl+[` / `Ctrl+]` and the `:back` / `:forward` ex
/// commands — drive the focused panel's history stack, the same stack a script
/// builds. (Phase 5 of G5: back/forward affordances.)
#[test]
fn ctrl_bracket_and_ex_commands_drive_panel_history() {
    use crate::command_line::Command;
    use garden_script::NavIntent;
    let base = temp_dir("g5-nav-affordance");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let a = file_with(&screens, "a.ptl", "let s = 1\n");
    file_with(&screens, "b.ptl", "let s = 2\n");

    let mut app = app_with_panel(&a);

    // Push a second screen so there is history to walk.
    navigate(
        &mut app,
        NavIntent::Push("b.ptl".into(), serde_json::Value::Null),
    );
    assert_eq!(panel_screen(&app), "b.ptl");

    // Ctrl+[ steps back to the origin; Ctrl+] steps forward again — the host
    // keyboard affordance driving the same stack the script builds.
    ctrl_key(&mut app, '[');
    assert_eq!(panel_screen(&app), a, "Ctrl+[ steps back");
    ctrl_key(&mut app, ']');
    assert_eq!(panel_screen(&app), "b.ptl", "Ctrl+] steps forward");

    // The `:back` / `:forward` ex commands drive the same stack.
    app.run_command(Command::Back);
    assert_eq!(panel_screen(&app), a, ":back steps back");
    app.run_command(Command::Forward);
    assert_eq!(panel_screen(&app), "b.ptl", ":forward steps forward");

    // Forward at the end of history is a no-op, reported in the status note.
    app.status_note = None;
    app.run_command(Command::Forward);
    assert_eq!(panel_screen(&app), "b.ptl", "forward at the end is a no-op");
    assert!(
        app.status_note
            .as_deref()
            .is_some_and(|s| s.contains("nothing forward")),
        "a no-op forward reports it in the status note"
    );
}

/// `:back` on a pane with no panel is a graceful no-op with an explanatory note,
/// never a crash or a silent swallow.
#[test]
fn history_command_on_a_non_panel_pane_reports_gracefully() {
    use crate::command_line::Command;
    let mut app = app_with_text("hello\n");
    app.run_command(Command::Back);
    assert!(
        app.status_note
            .as_deref()
            .is_some_and(|s| s.contains("no history")),
        "an editor pane has no history to navigate"
    );
}

#[test]
fn navigate_rejects_traversal_absolute_missing_and_non_ptl() {
    use garden_script::NavIntent;
    let base = temp_dir("g5-nav-reject");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let a = file_with(&screens, "a.ptl", "let s = 1\n");
    file_with(&screens, "b.ptl", "let s = 2\n");
    // A `.txt` next to the screens (must be refused for its extension) and a real
    // `.ptl` OUTSIDE the screen dir (must be refused via `..` even though it
    // exists and would otherwise be readable).
    file_with(&screens, "b.txt", "let s = 9\n");
    file_with(&base, "secret.ptl", "let s = 99\n");

    let mut app = app_with_panel(&a);
    let idx = panel_idx(&app);

    // Each of these is an untrusted target the whitelist must refuse: no swap, no
    // new history entry, and a rejection surfaced in the status note.
    for bad in [
        "../secret.ptl",   // `..` traversal to a real file outside the dir
        "/etc/passwd",     // absolute path
        "nonexistent.ptl", // does not exist
        "b.txt",           // not a .ptl
    ] {
        app.status_note = None;
        navigate(
            &mut app,
            NavIntent::Push(bad.into(), serde_json::Value::Null),
        );
        assert_eq!(
            panel_screen(&app),
            a,
            "'{bad}' must not swap the screen (still on origin)"
        );
        assert_eq!(
            app.panes[idx].panel.as_ref().unwrap().history_len(),
            1,
            "'{bad}' must not push a history entry"
        );
        assert!(
            app.status_note.as_deref().is_some_and(|m| m.contains(bad)),
            "'{bad}' rejection is surfaced in the status note, got {:?}",
            app.status_note
        );
    }

    // Sanity: after all the rejects the panel still runs its origin screen.
    app.settle_panels();
    assert_eq!(panel_value(&app, "s"), Some(json!(1)));
}

#[test]
fn navigate_intent_is_drained_by_the_real_tick_loop() {
    // Regression: an in-process `panel(...)` pane has no subprocess client, so its
    // `navigate()` intents must be drained by the ordinary panel tick loop
    // (`tick_panels`/`settle_panels`) — not only by a direct `handle_client_events`
    // call as the other tests do. A script that navigates on its first frame must
    // swap screens through the normal drive path, with nothing injected by hand.
    let base = temp_dir("g5-nav-ticked");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let home = file_with(
        &screens,
        "home.ptl",
        "state gone = false\nlet s = 1\nif !gone then\n  navigate(\"detail.ptl\")\n  gone = true\nend\n",
    );
    file_with(&screens, "detail.ptl", "let s = 2\n");

    // `app_with_panel` already settles once; settle again to render the swapped-in
    // screen. The swap happens entirely inside the tick loop.
    let mut app = app_with_panel(&home);
    app.settle_panels();
    assert_eq!(
        panel_screen(&app),
        "detail.ptl",
        "navigate() must be acted on by the tick loop, swapping to detail"
    );
    assert_eq!(
        panel_value(&app, "s"),
        Some(json!(2)),
        "the swapped-in detail screen runs after the ticked navigation"
    );
}

// ── G5 (phase 3): explicit `screens: [...]` allowlist narrows the default ───

/// A single-panel `App` whose panel declares an explicit `screens` allowlist.
/// Mirrors [`app_with_panel`] but sets the `panel(script, { screens: [...] })`
/// list on the layout node.
fn app_with_panel_screens(script: &str, screens: &[&str]) -> App {
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![LayoutNode::Panel {
                script: script.to_string(),
                screens: screens.iter().map(|s| s.to_string()).collect(),
            }],
            ratios: None,
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    app
}

/// A declared `screens: ["b.ptl"]` list lets navigation reach the listed screen
/// (it still passes every path-safety check) — the allowlist narrows, it does
/// not break, the in-directory default for a member.
#[test]
fn declared_screen_navigation_succeeds() {
    use garden_script::NavIntent;
    let base = temp_dir("g5-screens-accept");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let a = file_with(&screens, "a.ptl", "let s = 1\n");
    file_with(&screens, "b.ptl", "let s = 2\n");

    let mut app = app_with_panel_screens(&a, &["b.ptl"]);
    let idx = panel_idx(&app);
    // The allowlist threaded onto the live panel.
    assert_eq!(app.panes[idx].panel.as_ref().unwrap().screens(), ["b.ptl"]);

    navigate(
        &mut app,
        NavIntent::Push("b.ptl".into(), serde_json::Value::Null),
    );
    assert_eq!(panel_screen(&app), "b.ptl", "listed screen swaps in");
    assert_eq!(app.panes[idx].panel.as_ref().unwrap().history_len(), 2);
    app.settle_panels();
    assert_eq!(panel_value(&app, "s"), Some(json!(2)));
}

/// With a `screens` list declared, an in-directory sibling that is NOT on the
/// list — which the implicit default (G2b) would happily allow — is refused. The
/// explicit list narrows; it never widens past its own members.
#[test]
fn undeclared_in_directory_screen_is_rejected_when_list_declared() {
    use garden_script::NavIntent;
    let base = temp_dir("g5-screens-narrow");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let a = file_with(&screens, "a.ptl", "let s = 1\n");
    file_with(&screens, "b.ptl", "let s = 2\n");
    // `c.ptl` exists in the very same directory but is absent from the allowlist.
    file_with(&screens, "c.ptl", "let s = 3\n");

    let mut app = app_with_panel_screens(&a, &["b.ptl"]);
    let idx = panel_idx(&app);

    app.status_note = None;
    navigate(
        &mut app,
        NavIntent::Push("c.ptl".into(), serde_json::Value::Null),
    );
    assert_eq!(
        panel_screen(&app),
        a,
        "an off-list but in-directory screen must not swap"
    );
    assert_eq!(
        app.panes[idx].panel.as_ref().unwrap().history_len(),
        1,
        "an off-list target pushes no history entry"
    );
    assert!(
        app.status_note
            .as_deref()
            .is_some_and(|m| m.contains("c.ptl") && m.contains("declared screens")),
        "the rejection names the off-list screen, got {:?}",
        app.status_note
    );

    // The listed screen still works — narrowing left the member reachable.
    navigate(
        &mut app,
        NavIntent::Push("b.ptl".into(), serde_json::Value::Null),
    );
    assert_eq!(panel_screen(&app), "b.ptl");
}

/// A listed screen that fails a path-safety check is still refused — the
/// allowlist NARROWS the safety checks, never bypasses them. Here the declared
/// entry uses `..` traversal, which the layered defense rejects even though it is
/// "on the list".
#[test]
fn a_listed_screen_still_must_pass_safety_checks() {
    use garden_script::NavIntent;
    let base = temp_dir("g5-screens-unsafe");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let a = file_with(&screens, "a.ptl", "let s = 1\n");
    // A real .ptl outside the screen dir, named in the allowlist via `..`.
    file_with(&base, "secret.ptl", "let s = 99\n");

    let mut app = app_with_panel_screens(&a, &["../secret.ptl"]);
    app.status_note = None;
    navigate(
        &mut app,
        NavIntent::Push("../secret.ptl".into(), serde_json::Value::Null),
    );
    assert_eq!(
        panel_screen(&app),
        a,
        "a `..` target is refused even when it is on the declared list"
    );
    assert!(
        app.status_note
            .as_deref()
            .is_some_and(|m| m.contains("escapes the screen directory")),
        "the traversal defense — not the allowlist — refuses it, got {:?}",
        app.status_note
    );
}

// ── G5: pane-reuse identity across a rebuild (the linchpin) ────────────────

/// After navigation the panel's **live** `script()` tracks the current screen
/// while `origin_script()` stays the layout-declared origin; a `rebuild_panes`
/// (what a split / resize / reload triggers) reuses the SAME panel keyed on that
/// origin, carrying the whole history stack across intact, and the persisted
/// layout still records the ORIGIN screen — never the navigated one.
#[test]
fn navigated_panel_survives_rebuild_and_persists_by_origin() {
    use garden_script::NavIntent;
    let base = temp_dir("g5-nav-rebuild");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let a = file_with(&screens, "a.ptl", "state n = 0\nn = n + 1\nlet s = 1\n");
    file_with(&screens, "b.ptl", "let s = 2\n");

    let mut app = app_with_panel(&a);
    let idx = panel_idx(&app);

    // Navigate to a sibling screen: the live script now tracks b, the origin
    // stays a, and a history entry is pushed.
    navigate(
        &mut app,
        NavIntent::Push("b.ptl".into(), serde_json::Value::Null),
    );
    {
        let pv = app.panes[idx].panel.as_ref().unwrap();
        assert_eq!(
            pv.script(),
            "b.ptl",
            "live script tracks the navigated screen"
        );
        assert_eq!(
            pv.origin_script(),
            a,
            "origin stays the layout-declared screen"
        );
        assert_eq!(pv.history_len(), 2);
        assert_eq!(pv.history_cursor(), 1);
    }
    // Run frames so the panel has a non-zero frame_count witnessing its identity
    // across the rebuild (a fresh load would reset it to 0).
    app.settle_panels();
    let frames_before = app.panes[idx].panel.as_ref().unwrap().frame_count();
    assert!(frames_before > 0);

    // The persisted layout records the ORIGIN screen, not the navigated b.
    assert_eq!(
        app.panes[idx].to_layout_node(),
        LayoutNode::Panel {
            script: a.clone(),
            screens: Vec::new(),
        },
        "layout persists the panel by its origin, never the navigated screen"
    );

    // A rebuild must REUSE the same panel: history + cursor + live screen
    // preserved, not a fresh load of the origin screen.
    app.rebuild_panes();
    let idx = panel_idx(&app);
    let pv = app.panes[idx].panel.as_ref().unwrap();
    assert_eq!(pv.frame_count(), frames_before, "the SAME panel was reused");
    assert_eq!(pv.history_len(), 2, "history survived the rebuild");
    assert_eq!(pv.history_cursor(), 1);
    assert_eq!(
        pv.current_screen(),
        "b.ptl",
        "still on the navigated screen"
    );
    assert_eq!(pv.origin_script(), a);
}

/// Regression guard: a panel that was never navigated still reuses correctly
/// across a rebuild (same instance, frame_count preserved), with its origin
/// equal to its current screen.
#[test]
fn non_navigated_panel_reuses_across_rebuild() {
    let base = temp_dir("g5-no-nav-rebuild");
    let a = file_with(&base, "a.ptl", "state n = 0\nn = n + 1\nlet s = 1\n");
    let mut app = app_with_panel(&a);
    let idx = panel_idx(&app);
    app.settle_panels();
    let frames_before = app.panes[idx].panel.as_ref().unwrap().frame_count();
    assert!(frames_before > 0);
    assert_eq!(app.panes[idx].panel.as_ref().unwrap().history_len(), 1);

    app.rebuild_panes();
    let idx = panel_idx(&app);
    let pv = app.panes[idx].panel.as_ref().unwrap();
    assert_eq!(
        pv.frame_count(),
        frames_before,
        "same non-navigated panel reused"
    );
    assert_eq!(pv.history_len(), 1);
    assert_eq!(pv.origin_script(), a);
    assert_eq!(pv.script(), a);
}

/// A second-level navigation (a → b → c, all siblings) must resolve `c` against
/// the panel's **origin** directory, not the bare current screen name. Guards
/// the whitelist root against keying on the live (post-navigation) script, whose
/// bare `.ptl` name has no directory to resolve siblings within.
#[test]
fn navigate_from_a_navigated_screen_resolves_against_the_origin_dir() {
    use garden_script::NavIntent;
    let base = temp_dir("g5-nav-second-level");
    let screens = base.join("screens");
    fs::create_dir_all(&screens).unwrap();
    let a = file_with(&screens, "a.ptl", "let s = 1\n");
    file_with(&screens, "b.ptl", "let s = 2\n");
    file_with(&screens, "c.ptl", "let s = 3\n");

    let mut app = app_with_panel(&a);
    let idx = panel_idx(&app);

    navigate(
        &mut app,
        NavIntent::Push("b.ptl".into(), serde_json::Value::Null),
    );
    navigate(
        &mut app,
        NavIntent::Push("c.ptl".into(), serde_json::Value::Null),
    );
    assert_eq!(
        panel_screen(&app),
        "c.ptl",
        "second-level navigate resolved"
    );
    assert_eq!(app.panes[idx].panel.as_ref().unwrap().history_len(), 3);
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "s"),
        Some(json!(3)),
        "c.ptl is now running"
    );
}

/// A stub client whose `navigate` handler has a **side effect**: it
/// answers `b.ptl` with a fresh source every time, numbering the visit. The
/// numbering stands in for the real thing an app's `on_mutation("navigate")`
/// handler does — priming the data the target screen reads — and makes it
/// observable whether the host asked again.
const NAV_REPLAY_CLIENT: &str = r#"
read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":2,"name":"replay-stub"}}'
visits=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"screen":"b.ptl"'*)
      visits=$((visits+1))
      printf '{"jsonrpc":"2.0","id":%s,"result":{"screen":"b.ptl","source":"let visits = %s"}}\n' "$id" "$visits"
      ;;
    *'"method":"navigate"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":1,"message":"no such screen"}}\n' "$id"
      ;;
  esac
done
"#;

/// Back/forward re-issue the restored entry's `navigate` mutation on a
/// subprocess panel, so the client's own handler re-primes the screen.
///
/// This is the half that per-entry `nav_arg` could not fix on its own: the host
/// restores its record of a visit faithfully, but the *provider* holds the data
/// the screen draws, and without a replay it is never told the user came back.
#[test]
fn back_and_forward_re_issue_the_navigate_mutation() {
    use crate::process_pane::ProcessPane;
    use garden_script::NavIntent;
    let base = temp_dir("g5-nav-replay");
    let a = file_with(&base, "a.ptl", "let home = 1\n");

    let mut app = app_with_panel(&a);
    let idx = panel_idx(&app);
    let client = ProcessPane::spawn(
        "bash",
        &["-c".to_string(), NAV_REPLAY_CLIENT.to_string()],
        2,
        24,
        80,
    )
    .expect("spawn stub client");
    app.panes[idx].panel.as_mut().unwrap().attach_client(
        client,
        crate::script_client::new_shared(),
        "replay-stub".into(),
    );

    // The first navigation is visit 1.
    navigate(
        &mut app,
        NavIntent::Push("b.ptl".into(), serde_json::Value::Null),
    );
    assert_eq!(panel_screen(&app), "b.ptl");
    app.settle_panels();
    assert_eq!(panel_value(&app, "visits"), Some(json!(1)));

    // Back lands on the seed. Nothing navigated to it, so it is *not* replayed —
    // its screen name is the pane's own origin, which the client never declared.
    navigate(&mut app, NavIntent::Back);
    assert_eq!(panel_screen(&app), a, "back restores the origin");
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "home"),
        Some(json!(1)),
        "the seed rebuilt from its own file, not from the client"
    );

    // Forward re-asks the client for b.ptl: visit 2, and the fresh source is
    // what the panel now runs. Without the replay this would still read 1.
    navigate(&mut app, NavIntent::Forward);
    assert_eq!(panel_screen(&app), "b.ptl");
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "visits"),
        Some(json!(2)),
        "forward re-issued the navigate mutation"
    );

    // And again on a second round trip, so the replay is not a one-shot.
    navigate(&mut app, NavIntent::Back);
    navigate(&mut app, NavIntent::Forward);
    app.settle_panels();
    assert_eq!(panel_value(&app, "visits"), Some(json!(3)));
}

/// The same stub, but it serves `b.ptl` exactly once and refuses every later
/// request — a client that has moved on, or lost the data the screen needs.
const NAV_REPLAY_ONCE_CLIENT: &str = r#"
read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocol":2,"name":"once-stub"}}'
served=0
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"screen":"b.ptl"'*)
      if [ "$served" = "0" ]; then
        served=1
        printf '{"jsonrpc":"2.0","id":%s,"result":{"screen":"b.ptl","source":"let visits = 1"}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":1,"message":"screen is gone"}}\n' "$id"
      fi
      ;;
    *'"method":"navigate"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":1,"message":"no such screen"}}\n' "$id"
      ;;
  esac
done
"#;

/// A client that refuses the replay leaves the restored entry showing its
/// cached screen: the user asked to go forward, so going forward must not fail.
/// The reason surfaces in the status note instead.
#[test]
fn a_refused_navigate_replay_keeps_the_cached_screen() {
    use crate::process_pane::ProcessPane;
    use garden_script::NavIntent;
    let base = temp_dir("g5-nav-replay-refused");
    let a = file_with(&base, "a.ptl", "let home = 1\n");

    let mut app = app_with_panel(&a);
    let idx = panel_idx(&app);
    let client = ProcessPane::spawn(
        "bash",
        &["-c".to_string(), NAV_REPLAY_ONCE_CLIENT.to_string()],
        2,
        24,
        80,
    )
    .expect("spawn stub client");
    app.panes[idx].panel.as_mut().unwrap().attach_client(
        client,
        crate::script_client::new_shared(),
        "once-stub".into(),
    );

    navigate(
        &mut app,
        NavIntent::Push("b.ptl".into(), serde_json::Value::Null),
    );
    app.settle_panels();
    assert_eq!(panel_value(&app, "visits"), Some(json!(1)));

    // The replay is refused, but the navigation itself still happens.
    navigate(&mut app, NavIntent::Back);
    navigate(&mut app, NavIntent::Forward);
    assert_eq!(
        panel_screen(&app),
        "b.ptl",
        "forward still moved the cursor"
    );
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "visits"),
        Some(json!(1)),
        "the entry still runs its cached source"
    );
    let note = app.status_note.clone().unwrap_or_default();
    assert!(
        note.contains("screen is gone"),
        "the client's reason is surfaced, got {note:?}"
    );
}

// ── Petal-IDE: toolbar + play/pause ───────────────────────────────────────────

#[test]
fn ide_toolbar_absent_until_enabled_then_hit_tests() {
    // No toolbar in a normal window; enabling IDE mode reserves the band and
    // lays out its buttons, which hit-test to the same actions they draw.
    let mut app = app_with_panes(&[None]);
    assert!(
        app.toolbar_buttons().is_empty(),
        "no toolbar without IDE mode"
    );
    assert!(app.toolbar_at(4.0, 40.0).is_none());

    app.enable_ide(
        PathBuf::from("/x/sketch.ptl"),
        PathBuf::from("/x/ir_view.ptl"),
    );
    let buttons = app.toolbar_buttons();
    assert_eq!(buttons.len(), 4, "play/pause, IR, state, reset");
    // A press at each button's center resolves to that button's action.
    for b in &buttons {
        let cx = b.rect.x + b.rect.w / 2.0;
        let cy = b.rect.y + b.rect.h / 2.0;
        assert_eq!(app.toolbar_at(cx, cy), Some(b.action), "hit {:?}", b.action);
    }
    // A press below the band hits no button.
    assert!(app.toolbar_at(4.0, 400.0).is_none());
}

#[test]
fn ide_pause_freezes_panel_ticks_but_keeps_editor_live() {
    // Pausing stops panel frames (the canvas holds), yet the editor buffer still
    // edits; resuming lets the panel run again.
    let dir = temp_dir("ide-pause");
    let script = file_with(&dir, "canvas.ptl", "state n = 0\nn = n + 1\n");
    let mut app = App::new(
        None,
        LayoutNode::Panel {
            script: script.clone(),
            screens: Vec::new(),
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.enable_ide(PathBuf::from(&script), PathBuf::from("/x/ir_view.ptl"));

    // Run a few frames; the counter advances.
    app.settle_panels();
    let before = panel_int(&app, "n").unwrap();
    app.tick_panels();
    app.tick_panels();
    let running = panel_int(&app, "n").unwrap();
    assert!(
        running > before,
        "panel advances while playing ({before} -> {running})"
    );

    // Pause: no further frames run, so the counter holds even across ticks.
    app.toggle_play();
    assert!(app.paused);
    let paused_at = panel_int(&app, "n").unwrap();
    for _ in 0..5 {
        app.tick_panels();
    }
    assert_eq!(
        panel_int(&app, "n"),
        Some(paused_at),
        "panel frozen while paused"
    );

    // Resume: frames run again.
    app.toggle_play();
    assert!(!app.paused);
    app.tick_panels();
    app.tick_panels();
    assert!(
        panel_int(&app, "n").unwrap() > paused_at,
        "panel resumes after play"
    );
}

#[test]
fn ide_toggle_ir_opens_and_closes_the_inspector_pane() {
    // The IR toolbar button opens a panel on the seeded ir_view.ptl (with an IR
    // provider attached, so it renders the target's IR), and toggling closes it.
    let dir = temp_dir("ide-ir");
    let target = file_with(&dir, "sketch.ptl", "let a = 1 + 2\n");
    let ir_view = file_with(&dir, "ir_view.ptl", crate::petal_ide::IR_VIEW_SCRIPT);
    let mut app = App::new(
        None,
        LayoutNode::Editor {
            file: Some(target.clone()),
            line_numbers: false,
            wrap: true,
        },
        true,
        Viewport {
            size: (900.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.enable_ide(PathBuf::from(&target), PathBuf::from(&ir_view));

    assert!(app.ir_panel_index().is_none(), "no IR pane initially");
    app.toggle_ir_panel();
    let ir_idx = app.ir_panel_index().expect("IR pane opened");
    // Its bindings say stage_count == 3 (IR/Bytecode/AST) and non-empty IR.
    app.refresh_ir_source();
    app.settle_panels();
    assert_eq!(panel_value(&app, "stage_count"), Some(json!(3)));
    assert!(panel_int(&app, "body_len").unwrap() > 0, "IR text rendered");
    // A real bool, not an int encoding of one — observation keeps the type.
    assert_eq!(panel_value(&app, "has_error"), Some(json!(false)));
    let _ = ir_idx;

    app.toggle_ir_panel();
    assert!(app.ir_panel_index().is_none(), "IR pane closed");
}

// --- close-window vs quit (MWI phase 2) ---------------------------------
//
// With multiple OS windows per process, "close this window" and "quit the
// whole process" are different signals. The App core reports them through two
// flags: `should_close()` (this window goes away, the process may live on)
// and `should_quit()` (the process exits). Exactly one of the two fires for
// any given gesture — never both.

#[test]
fn cmd_w_signals_close_window_not_quit() {
    let mut app = app_with_panes(&[None]);
    cmd_key(&mut app, 'w');
    assert!(app.should_close(), "Cmd+W closes the window");
    assert!(!app.should_quit(), "Cmd+W must not quit the process");
}

#[test]
fn cmd_w_closes_the_whole_window_regardless_of_pane_count() {
    // Cmd+W is the macOS close-window chord, not a pane close: with three
    // panes open it still closes the whole window (leaving the panes alone —
    // the frontend tears the window down, not the pane list).
    let mut app = app_with_panes(&[Some("a.txt"), Some("b.txt"), Some("c.txt")]);
    cmd_key(&mut app, 'w');
    assert!(
        app.should_close(),
        "Cmd+W closes the window even with splits"
    );
    assert!(!app.should_quit());
    assert_eq!(app.panes.len(), 3, "Cmd+W does not close individual panes");
}

#[test]
fn cmd_q_signals_quit_not_close_window() {
    let mut app = app_with_panes(&[None]);
    cmd_key(&mut app, 'q');
    assert!(app.should_quit(), "Cmd+Q quits the process");
    assert!(!app.should_close(), "a quit is not a window close");
}

#[test]
fn ctrl_q_signals_quit_not_close_window() {
    let mut app = app_with_panes(&[None]);
    ctrl_key(&mut app, 'q');
    assert!(app.should_quit(), "Ctrl+Q quits the process");
    assert!(!app.should_close(), "a quit is not a window close");
}

#[test]
fn q_command_on_last_pane_closes_the_window_not_the_process() {
    let mut app = app_with_panes(&[Some("a.txt"), Some("b.txt")]);

    // With a split, `:q` still just closes the focused pane.
    run_ex(&mut app, "q");
    assert_eq!(app.panes.len(), 1, ":q closes the focused pane first");
    assert!(!app.should_close());
    assert!(!app.should_quit());

    // On the last pane, `:q` closes this window — other windows live on.
    run_ex(&mut app, "q");
    assert!(app.should_close(), ":q on the last pane closes the window");
    assert!(!app.should_quit(), ":q must not quit the whole process");
}

#[test]
fn wq_command_on_last_pane_writes_then_closes_the_window() {
    let dir = temp_dir("wq-closes-window");
    let a = file_with(&dir, "a.txt", "alpha");
    let mut app = app_with_panes(&[Some(&a)]);
    app.panes[0].view.insert("x");

    run_ex(&mut app, "wq");

    assert_eq!(fs::read_to_string(&a).unwrap(), "xalpha", ":wq wrote first");
    assert!(app.should_close(), ":wq on the last pane closes the window");
    assert!(!app.should_quit(), ":wq must not quit the whole process");
}

#[test]
fn ctrl_w_q_on_last_pane_closes_the_window_not_the_process() {
    let mut app = app_with_panes(&[None, None]);

    window_cmd(&mut app, 'q');
    assert_eq!(app.panes.len(), 1, "Ctrl+W q closes a split pane");
    assert!(!app.should_close());
    assert!(!app.should_quit());

    window_cmd(&mut app, 'q');
    assert!(
        app.should_close(),
        "Ctrl+W q on the last pane closes the window"
    );
    assert!(!app.should_quit(), "Ctrl+W q must not quit the process");
}

#[test]
fn wqa_still_quits_the_whole_process() {
    // `:wqa` means "write everything and exit" — it stays a process quit,
    // not a per-window close.
    let dir = temp_dir("wqa-still-quits");
    let a = file_with(&dir, "a.txt", "alpha");
    let mut app = app_with_panes(&[Some(&a)]);
    app.panes[0].view.insert("x");

    run_ex(&mut app, "wqa");

    assert!(app.should_quit(), ":wqa quits the process");
    assert!(!app.should_close(), ":wqa is a quit, not a window close");
}

#[test]
fn ctrl_w_c_pane_close_signals_neither_close_nor_quit() {
    // Pane management is untouched by the window/quit split: with a split,
    // `Ctrl+W c` closes just the pane; on the last pane it still refuses
    // with E444. Neither case touches the window/process flags.
    let mut app = app_with_panes(&[Some("a.txt"), Some("b.txt")]);

    window_cmd(&mut app, 'c');
    assert_eq!(app.panes.len(), 1, "Ctrl+W c closed the focused pane");
    assert!(!app.should_close());
    assert!(!app.should_quit());

    window_cmd(&mut app, 'c');
    assert_eq!(app.panes.len(), 1, "the last pane cannot be closed");
    assert!(
        app.status_error.as_deref().unwrap_or("").contains("E444"),
        "vim-style error expected, got {:?}",
        app.status_error,
    );
    assert!(!app.should_close());
    assert!(!app.should_quit());
}

#[test]
fn menu_close_window_signals_close_not_quit() {
    let mut app = app_with_panes(&[None]);
    app.dispatch_menu(MenuAction::CloseWindow);
    assert!(app.should_close(), "File > Close Window closes the window");
    assert!(
        !app.should_quit(),
        "the menu close must not quit the process"
    );
}

#[test]
fn menu_quit_signals_quit_not_close() {
    let mut app = app_with_panes(&[None]);
    app.dispatch_menu(MenuAction::Quit);
    assert!(app.should_quit(), "File > Quit quits the process");
    assert!(!app.should_close(), "a quit is not a window close");
}

// --- new window intent (MWI phase 3) -------------------------------------
//
// The App core cannot create OS windows — only the windowed frontend can.
// So "open a new window" is surfaced as an intent: `:windownew` and
// File > New Window set a request flag the frontend drains with the
// take-style `take_new_window_request()` (true once, then false — the same
// contract as `take_redraw`). `:new` is deliberately NOT this command: vim
// reserves `:new` for split-with-empty-buffer.

#[test]
fn new_window_request_defaults_to_false() {
    let mut app = app_with_panes(&[None]);
    assert!(
        !app.take_new_window_request(),
        "a fresh app has no pending new-window request"
    );
}

#[test]
fn windownew_command_requests_a_new_window() {
    let mut app = app_with_panes(&[None]);
    run_ex(&mut app, "windownew");

    assert!(
        app.take_new_window_request(),
        ":windownew must request a new window"
    );
    assert!(
        !app.take_new_window_request(),
        "the request is take-style: drained after the first read"
    );
    assert!(!app.should_close(), ":windownew must not close this window");
    assert!(!app.should_quit(), ":windownew must not quit the process");
    assert_eq!(
        app.panes.len(),
        1,
        ":windownew opens a window, not a pane split"
    );
}

#[test]
fn menu_new_window_requests_a_new_window() {
    let mut app = app_with_panes(&[None]);
    app.dispatch_menu(MenuAction::NewWindow);

    assert!(
        app.take_new_window_request(),
        "File > New Window must request a new window"
    );
    assert!(
        !app.take_new_window_request(),
        "the request is take-style: drained after the first read"
    );
    assert!(!app.should_close());
    assert!(!app.should_quit());
    assert_eq!(app.panes.len(), 1, "the menu action is not a pane split");
}

#[test]
fn windownew_is_not_vim_new() {
    // `:new` is vim's split-with-empty-buffer command; it is not implemented
    // here (today it parses as Unknown), and it must never alias the
    // new-window intent.
    let mut app = app_with_panes(&[None]);
    run_ex(&mut app, "new");

    assert!(
        !app.take_new_window_request(),
        ":new must not request a new window — vim reserves it for split-new-buffer"
    );
    assert!(!app.should_close());
    assert!(!app.should_quit());
}

// ---------------------------------------------------------------------------
// G7 Phase 2 — LSP document sync off the poll tick
// ---------------------------------------------------------------------------

/// The `lsp` block of `/state`, which is how document sync is observed
/// headlessly.
fn lsp_state(app: &App) -> serde_json::Value {
    app.lsp_state_json()
}

/// Path of the petal language server, or `None` to skip: Garden path-depends
/// on the petal *crates*, not on an installed `petal` *binary*.
fn petal_server_or_skip() -> Option<String> {
    let command = crate::lsp::registry::REGISTRY[0].resolved_command();
    let ok = std::process::Command::new(&command)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();
    if !ok {
        eprintln!("skipping: no `{command}` binary (set GARDEN_PETAL_LSP_BIN)");
        return None;
    }
    Some(command)
}

#[test]
fn a_file_with_no_language_server_is_never_synced() {
    let dir = temp_dir("lsp-ineligible");
    let path = file_with(&dir, "notes.txt", "plain text\n");
    let mut app = app_with_panes(&[Some(&path)]);

    app.poll_lsp();

    let state = lsp_state(&app);
    assert_eq!(state["documents"].as_array().unwrap().len(), 0);
    assert_eq!(
        state["servers"].as_array().unwrap().len(),
        0,
        "a .txt file must not start any server"
    );
}

#[test]
fn a_missing_server_is_reported_once_and_not_retried() {
    let dir = temp_dir("lsp-missing");
    let path = file_with(&dir, "a.ptl", "let x = 1\n");
    let mut app = app_with_panes(&[Some(&path)]);

    // SAFETY: single-threaded test.
    unsafe {
        std::env::set_var(
            "GARDEN_PETAL_LSP_BIN",
            dir.join("definitely-not-a-server")
                .to_string_lossy()
                .as_ref(),
        )
    };
    app.poll_lsp();
    app.poll_lsp();
    app.poll_lsp();
    unsafe { std::env::remove_var("GARDEN_PETAL_LSP_BIN") };

    let state = lsp_state(&app);
    let failures = state["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 1, "one entry, however many ticks ran");
    let error = failures[0]["error"].as_str().unwrap();
    assert!(
        error.contains("GARDEN_PETAL_LSP_BIN"),
        "the hint must name the override: {error}"
    );
    assert_eq!(
        state["documents"].as_array().unwrap().len(),
        0,
        "nothing is synced to a server that never started"
    );
}

#[test]
fn document_sync_opens_changes_and_closes() {
    let Some(command) = petal_server_or_skip() else {
        return;
    };
    // SAFETY: single-threaded test.
    unsafe { std::env::set_var("GARDEN_PETAL_LSP_BIN", &command) };

    let dir = temp_dir("lsp-sync");
    let path = file_with(&dir, "a.ptl", "let x = 1\n");
    let mut app = app_with_panes(&[Some(&path)]);

    // didOpen on first sight.
    app.poll_lsp();
    let state = lsp_state(&app);
    let docs = state["documents"].as_array().unwrap();
    assert_eq!(docs.len(), 1, "the .ptl buffer should be open: {state}");
    assert_eq!(docs[0]["language_id"], "petal");
    assert_eq!(docs[0]["version"], 1);
    assert!(docs[0]["uri"].as_str().unwrap().starts_with("file:///"));
    assert_eq!(state["servers"].as_array().unwrap(), &["petal"]);

    // An untouched buffer must not re-send anything.
    app.poll_lsp();
    assert_eq!(
        lsp_state(&app)["documents"][0]["version"],
        1,
        "an unedited buffer must not bump the document version"
    );

    // An edit bumps the revision, so the next tick sends didChange.
    app.panes[0].view.insert("y");
    app.poll_lsp();
    assert_eq!(
        lsp_state(&app)["documents"][0]["version"],
        2,
        "editing should have sent one didChange"
    );

    // The pane stops showing the file -> didClose.
    app.panes[0].view = crate::editor_view::EditorView::open(None);
    app.panes[0].file = None;
    app.poll_lsp();
    assert_eq!(
        lsp_state(&app)["documents"].as_array().unwrap().len(),
        0,
        "a file no longer in any pane should be closed"
    );

    unsafe { std::env::remove_var("GARDEN_PETAL_LSP_BIN") };
}

#[test]
fn one_server_is_shared_by_two_panes_on_the_same_language() {
    let Some(command) = petal_server_or_skip() else {
        return;
    };
    // SAFETY: single-threaded test.
    unsafe { std::env::set_var("GARDEN_PETAL_LSP_BIN", &command) };

    let dir = temp_dir("lsp-shared");
    let a = file_with(&dir, "a.ptl", "let a = 1\n");
    let b = file_with(&dir, "b.ptl", "let b = 2\n");
    let mut app = app_with_panes(&[Some(&a), Some(&b)]);

    app.poll_lsp();

    let state = lsp_state(&app);
    assert_eq!(state["documents"].as_array().unwrap().len(), 2);
    assert_eq!(
        state["servers"].as_array().unwrap().len(),
        1,
        "two .ptl panes share one server, not one server each"
    );

    unsafe { std::env::remove_var("GARDEN_PETAL_LSP_BIN") };
}

/// Keystrokes inside a panel go to its focused editable region, not the pane's
/// own (placeholder) buffer — so the status bar has to report that region's vim
/// mode and cursor, or it sits frozen at `NORMAL … 1:1` while the user types.
#[test]
fn the_status_bar_follows_a_panels_focused_region() {
    let dir = temp_dir("status-region");
    let script = file_with(
        &dir,
        "edit.ptl",
        "edit_view(1, 0, 0, 400, 300, \"alpha\\nbeta\\ngamma\")\n",
    );
    let mut app = app_with_panel(&script);
    let idx = panel_idx(&app);

    let status = |app: &App| {
        app.build_scene()
            .primitives
            .iter()
            .find_map(|p| match p {
                garden_render::Primitive::Text { text, pos, .. } if pos.1 > 560.0 => {
                    Some(text.clone())
                }
                _ => None,
            })
            .expect("a status line")
    };
    assert!(status(&app).contains("1:1"), "{}", status(&app));

    // Focus the region on its third line, then enter insert mode.
    let rect = app.panes[idx].rect;
    let cell = app.viewport.cell;
    app.panes[idx].panel.as_mut().unwrap().region_press(
        1,
        rect,
        cell,
        rect.x + 2.0 * cell.0,
        rect.y + 2.0 * cell.1,
        false,
        1,
    );
    app.settle_panels();
    let line = status(&app);
    let c = app.panes[idx]
        .panel
        .as_ref()
        .unwrap()
        .region_view(1)
        .unwrap()
        .cursor;
    assert!(c.line > 0, "the press should have left line 1");
    assert!(
        line.contains(&format!("{}:{}", c.line + 1, c.col + 1)),
        "the bar should mirror the region's cursor ({c:?}): {line}"
    );
    assert!(line.starts_with("NORMAL"), "{line}");

    let mut clip = InMemoryClipboard::default();
    app.panes[idx]
        .panel
        .as_mut()
        .unwrap()
        .region_key(1, rect, cell, Key::Char('i'), &mut clip);
    app.settle_panels();
    assert!(
        status(&app).starts_with("INSERT"),
        "the region's vim mode is the pane's mode: {}",
        status(&app)
    );
}

// ── right button: the context gesture ──────────────────────────────────────

/// A panel script that accumulates every button edge it sees into a `state` var
/// (one-frame edges are cleared by the next idle tick, so they have to be
/// counted rather than sampled) and records the pointer the press arrived with.
/// The counters need no publishing step: a `state` var is a named term, so the
/// host observes it under its own name.
const BUTTON_PROBE: &str = "\
state rights = 0
state lefts = 0
state at_x = -1
if mouse_pressed(1) then
  rights = rights + 1
  at_x = mouse_x()
end
if mouse_pressed(0) then lefts = lefts + 1 end
";

/// A right click reaches the panel script as `petal-ui` button 1, carrying the
/// pane-local pointer — and is *not* also delivered as a left press, which
/// would fire whatever the script does on a plain click alongside the menu.
#[test]
fn a_right_click_reaches_a_panel_as_button_one() {
    let dir = temp_dir("right-click-panel");
    let script = file_with(&dir, "probe.ptl", BUTTON_PROBE);
    let mut app = app_with_panel(&script);

    app.mouse_down_right(120.0, 80.0);
    app.mouse_up_right();
    assert_eq!(panel_value(&app, "rights"), Some(json!(1)));
    assert_eq!(panel_value(&app, "lefts"), Some(json!(0)));
    // The script reads the pointer in *pane-local* pixels, so the press has to
    // arrive already translated by the pane's origin.
    let pane_x = app.panes[panel_idx(&app)].rect.x;
    assert_eq!(panel_int(&app, "at_x"), Some((120.0 - pane_x) as i64));

    // And the left button still only fires the left edge.
    app.mouse_down(120.0, 80.0, Mods::default(), 1);
    app.mouse_up();
    assert_eq!(panel_value(&app, "rights"), Some(json!(1)));
    assert_eq!(panel_value(&app, "lefts"), Some(json!(1)));
}

/// The release goes to the pane that took the press even after the pointer has
/// left it, so a script never sees a button that went down and never came up.
#[test]
fn a_right_release_follows_the_pane_that_took_the_press() {
    let dir = temp_dir("right-release-follows");
    let script = file_with(
        &dir,
        "probe.ptl",
        "state downs = 0\nstate ups = 0\n\
         if mouse_pressed(1) then downs = downs + 1 end\n\
         if mouse_released(1) then ups = ups + 1 end\n",
    );
    let mut app = app_with_panel(&script);

    app.mouse_down_right(50.0, 50.0);
    // Outside every pane (the viewport is 800×600).
    app.mouse_moved(2000.0, 2000.0);
    app.mouse_up_right();
    assert_eq!(panel_value(&app, "downs"), Some(json!(1)));
    assert_eq!(
        panel_value(&app, "ups"),
        Some(json!(1)),
        "the press must be balanced even when the pointer left the pane"
    );
}

/// Right-clicking asks for a menu, not for a new focus: a region the user was
/// editing keeps the keyboard. (A *left* press outside a region clears it —
/// that is the existing behavior this must not copy.)
#[test]
fn a_right_click_leaves_a_focused_region_alone() {
    let dir = temp_dir("right-click-keeps-focus");
    let script = file_with(
        &dir,
        "probe.ptl",
        "edit_view(1, 0, 0, 400, 200, \"one\\ntwo\\nthree\")\n",
    );
    let mut app = app_with_panel(&script);
    let idx = panel_idx(&app);

    // Focus the region with a left press inside it.
    app.mouse_down(10.0, 10.0, Mods::default(), 1);
    app.mouse_up();
    assert_eq!(
        app.panes[idx].panel.as_ref().unwrap().focused_region(),
        Some(1)
    );

    // A right press well outside the region must not take it away.
    app.mouse_down_right(600.0, 500.0);
    app.mouse_up_right();
    assert_eq!(
        app.panes[idx].panel.as_ref().unwrap().focused_region(),
        Some(1),
        "a context gesture must not end an edit session"
    );
}

// ── `/` search inside a panel region ───────────────────────────────────────

/// `/` in a focused, editable panel region opens the host's search prompt and
/// the accepted pattern searches **that region's** buffer — the gesture that
/// makes the diff reviewer's unified view searchable. `n` then repeats it
/// inside the region, since the region's own vim holds the last pattern.
#[test]
fn slash_searches_the_focused_panel_region() {
    let dir = temp_dir("region-search");
    let script = file_with(
        &dir,
        "probe.ptl",
        "edit_view(1, 0, 0, 600, 400, \"alpha\\nbeta\\ngamma\\nbeta again\\n\")\n",
    );
    let mut app = app_with_panel(&script);
    let idx = panel_idx(&app);

    // Click into the region so it holds the keyboard.
    let r = app.panes[idx].rect;
    app.mouse_down(r.x + 10.0, r.y + 10.0, Mods::default(), 1);
    app.mouse_up();
    assert_eq!(
        app.panes[idx].panel.as_ref().unwrap().focused_region(),
        Some(1)
    );

    // `/` opens the prompt rather than being typed into the buffer.
    app.apply_key(Key::Char('/'), Mods::default());
    assert!(
        app.command_line.is_some(),
        "`/` in a region opens the host search prompt"
    );

    for c in "beta".chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
    app.apply_key(Key::Enter, Mods::default());
    assert!(app.command_line.is_none(), "Return closes the prompt");

    let cursor = |app: &App| {
        app.panes[panel_idx(app)]
            .panel
            .as_ref()
            .unwrap()
            .region_view(1)
            .unwrap()
            .cursor
    };
    assert_eq!(cursor(&app).line, 1, "the cursor moved to the first `beta`");
    assert_eq!(app.status_error, None);

    // `n` repeats inside the region and reaches the second match.
    let mut clip = InMemoryClipboard::default();
    let (rect, cell) = (app.panes[idx].rect, app.viewport.cell);
    app.panes[idx]
        .panel
        .as_mut()
        .unwrap()
        .region_key(1, rect, cell, Key::Char('n'), &mut clip);
    assert_eq!(cursor(&app).line, 3, "`n` repeats the region's own search");
}

/// A pattern that is not in the region is reported, and does not silently
/// search the pane behind it instead.
#[test]
fn a_region_search_that_misses_says_so() {
    let dir = temp_dir("region-search-miss");
    let script = file_with(
        &dir,
        "probe.ptl",
        "edit_view(1, 0, 0, 600, 400, \"alpha\\nbeta\\n\")\n",
    );
    let mut app = app_with_panel(&script);
    let idx = panel_idx(&app);
    let r = app.panes[idx].rect;
    app.mouse_down(r.x + 10.0, r.y + 10.0, Mods::default(), 1);
    app.mouse_up();

    app.apply_key(Key::Char('/'), Mods::default());
    for c in "zzz".chars() {
        app.apply_key(Key::Char(c), Mods::default());
    }
    app.apply_key(Key::Enter, Mods::default());
    assert_eq!(
        app.status_error.as_deref(),
        Some("E: pattern not found: zzz")
    );
}

/// A panel script that doesn't compile must announce itself. Before this was
/// reported, the pane silently degraded to an ordinary editor — `kind` read
/// `editor`, `panel` was null, `status_error` was clean — so a syntax error, a
/// bad layout, and a wrong path all looked identical, and only `petal check`
/// told them apart. The pane now stays a *panel* (running an empty stub) and the
/// compile error is reported through `panel_error`/`status_error`.
#[test]
fn a_panel_that_does_not_compile_reports_the_error() {
    let dir = temp_dir("panel-broken");
    // Unbalanced parens: a parse error, not a runtime one.
    let script = file_with(&dir, "broken.ptl", "clear(0,0,0)\nlet x = (((\n");
    let app = app_with_panel(&script);

    // The pane is still a panel, so it can come back to life on a fixed save.
    let panel = app.panes[0]
        .panel
        .as_ref()
        .expect("a script that fails to compile keeps its panel pane");
    let panel_err = panel.error().expect("the panel carries the compile error");
    assert!(
        panel_err.contains("line 2"),
        "the panel error should carry the compiler's position: {panel_err}"
    );

    let err = app
        .effective_status_error()
        .expect("a failed panel load must reach status_error");
    assert!(
        err.contains("panel") && err.contains("broken.ptl"),
        "status_error should name the failing script: {err}"
    );
    assert_eq!(app.panel_error.as_deref(), Some(err));
}

/// A panel whose script is simply missing must report just as loudly — this is
/// the other half of the same confusion (bad path vs bad syntax).
#[test]
fn a_panel_with_a_missing_script_reports_the_error() {
    let dir = temp_dir("panel-missing");
    let missing = dir.join("nope.ptl").display().to_string();
    let app = app_with_panel(&missing);

    let err = app
        .effective_status_error()
        .expect("a missing panel script must reach status_error");
    assert!(
        err.contains("nope.ptl"),
        "status_error should name the missing script: {err}"
    );
}

/// A panel that was broken **at load** used to be unrecoverable: the pane had
/// been replaced by an editor, so fixing the file changed nothing and only a
/// restart brought the canvas back. It must heal itself instead.
#[test]
fn a_panel_broken_at_load_recovers_when_the_script_is_fixed() {
    let dir = temp_dir("panel-load-recover");
    let script = file_with(&dir, "recover.ptl", "let x = (((\n");
    let mut app = app_with_panel(&script);
    assert!(app.panel_error.is_some(), "starts broken");

    fs::write(&script, "let ok = 1 + 1\n").unwrap();
    app.settle_panels();

    assert_eq!(
        app.panel_error, None,
        "a fixed script must clear the reported panel error"
    );
    assert_eq!(app.effective_status_error(), None);
    assert_eq!(
        panel_value(&app, "ok"),
        Some(json!(2)),
        "the repaired script is actually running"
    );
}

/// A **hot reload** that doesn't compile keeps the last good program on screen —
/// correct, but silent: you edit, see no change, and conclude the edit had no
/// effect. The failure has to be reported.
#[test]
fn a_hot_reload_that_does_not_compile_is_reported() {
    let dir = temp_dir("panel-reload-broken");
    let script = file_with(&dir, "live.ptl", "let ok = 1 + 1\n");
    let mut app = app_with_panel(&script);
    assert_eq!(app.panel_error, None, "starts healthy");

    fs::write(&script, "let ok = (((\n").unwrap();
    app.settle_panels();

    let err = app
        .effective_status_error()
        .expect("a failed hot reload must reach status_error");
    assert!(
        err.contains("live.ptl"),
        "the report should name the script: {err}"
    );
    // The old program is still the one running — that part was always right.
    assert_eq!(panel_value(&app, "ok"), Some(json!(2)));

    // …and fixing it again clears the report.
    fs::write(&script, "let ok = 1 + 1 + 1\n").unwrap();
    app.settle_panels();
    assert_eq!(app.panel_error, None);
    assert_eq!(panel_value(&app, "ok"), Some(json!(3)));
}

/// A **runtime** error lived only in `panes[].panel.error` plus a banner painted
/// into the canvas; `status_error` — where the docs send people — stayed null.
/// It must be reported in the same one place, and recover on a fixed save.
#[test]
fn a_runtime_error_reaches_status_error_and_recovers() {
    let dir = temp_dir("panel-runtime-error");
    let script = file_with(&dir, "boom.ptl", "let xs = [1, 2]\nlet bad = xs[10]\n");
    let mut app = app_with_panel(&script);

    let err = app
        .effective_status_error()
        .expect("a panel runtime error must reach status_error");
    assert!(
        err.contains("boom.ptl"),
        "the report should name the script: {err}"
    );
    // One line, whatever the Petal error's source excerpt looks like: the status
    // bar has one line to give it.
    assert_eq!(
        err.lines().count(),
        1,
        "status text is a single line: {err}"
    );

    fs::write(&script, "let xs = [1, 2]\nlet bad = xs[1]\n").unwrap();
    app.settle_panels();
    assert_eq!(
        app.panel_error, None,
        "a panel must recover from a runtime error on a fixed save"
    );
}

/// `print(...)` from a panel script reached stdout and nowhere else, so a
/// headless client had no debug channel at all. It must show up in `/state`'s
/// `script.output`.
#[test]
fn panel_print_output_reaches_the_debug_state() {
    let dir = temp_dir("panel-print");
    let script = file_with(&dir, "printer.ptl", "print(\"DBG hello\")\n");
    let mut app = app_with_panel(&script);

    let state = app.state_json();
    let output = state["script"]["output"]
        .as_array()
        .expect("script.output is a list")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(
        output.iter().any(|l| l.contains("DBG hello")),
        "panel print should appear in script.output: {output:?}"
    );

    // Drained: a second read does not repeat the same lines forever.
    let again = app.state_json();
    assert!(!again["script"]["output"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap_or_default().contains("DBG hello")));
}

// ── panel input fidelity ────────────────────────────────────────────────
//
// A panel used to have almost no keyboard namespace and a lossy pointer: every
// Cmd/Ctrl chord but `Ctrl+S` was swallowed, `Mods` had no alt bit at all,
// `/mouse` delivered only shift, `click_count()` was permanently 1, and the one
// forwarded chord arrived with its character attached. Apps shipped `esc`-then-
// `z` for undo and moved Select All onto a bare `A` to work around it. These
// pin down the fixed contract end to end, through the same `App` entry points
// the frontends and the debug server use.

/// A panel over a script that records everything the input contract delivers.
fn app_with_input_probe(test: &str, prelude: &str) -> App {
    let dir = temp_dir(test);
    let script = file_with(
        &dir,
        "probe.ptl",
        &format!(
            "{prelude}\
             state typed = \"\"\n\
             state chords = 0\n\
             state clicks = 0\n\
             state alt_seen = 0\n\
             if text_input() != \"\" then typed = concat(typed, text_input()) end\n\
             if key_pressed(\"z\") then chords = chords + 1 end\n\
             if click_count() > clicks then clicks = click_count() end\n\
             if mod_alt() then alt_seen = 1 end\n\
             let held_shift = key_down(\"shift\")\n\
             let held_w = key_down(\"w\")\n\
             draw_rect(0, 0, 2, 2, 1, 2, 3)\n"
        ),
    );
    app_with_panel(&script)
}

/// `Ctrl+S` is forwarded to the script — and must arrive as a *chord*, with no
/// character attached. It used to be fed through `panel_key_text`, so the first
/// save also typed an "s" into the document.
#[test]
fn a_forwarded_chord_delivers_no_text_input() {
    let mut app = app_with_input_probe("panel-chord-text", "");
    app.apply_key(
        Key::Char('s'),
        Mods {
            ctrl: true,
            ..Default::default()
        },
    );
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "typed"),
        Some(json!("")),
        "a modified chord must not also type its character"
    );
    // A plain key still types.
    app.apply_key(Key::Char('a'), Mods::default());
    app.settle_panels();
    assert_eq!(panel_value(&app, "typed"), Some(json!("a")));
}

/// `claim_key("z", "cmd")` hands the panel a chord the host would otherwise keep
/// (Garden's global Undo). Without it a panel has no command keyspace at all.
#[test]
fn a_claimed_chord_reaches_the_panel_script() {
    let mut app = app_with_input_probe("panel-claim", "claim_key(\"z\", \"cmd\")\n");
    app.apply_key(
        Key::Char('z'),
        Mods {
            cmd: true,
            ..Default::default()
        },
    );
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "chords"),
        Some(json!(1)),
        "the claimed Cmd+Z must reach the script"
    );
    assert_eq!(
        panel_value(&app, "typed"),
        Some(json!("")),
        "…and still carry no typed text"
    );
}

/// An unclaimed Cmd chord stays host-global — claiming is opt-in, so an
/// existing panel keeps every editor shortcut it always had.
#[test]
fn an_unclaimed_chord_stays_with_the_host() {
    let mut app = app_with_input_probe("panel-unclaimed", "");
    app.apply_key(
        Key::Char('z'),
        Mods {
            cmd: true,
            ..Default::default()
        },
    );
    app.settle_panels();
    assert_eq!(panel_value(&app, "chords"), Some(json!(0)));
}

/// Every modifier reaches the script on a mouse press, not just shift: alt-drag
/// ("scale about the center") and cmd-click were unimplementable and untestable.
#[test]
fn a_mouse_press_carries_every_modifier() {
    let mut app = app_with_input_probe("panel-mouse-mods", "");
    let idx = panel_idx(&app);
    let rect = app.panes[idx].rect;
    app.mouse_down(
        rect.x + 20.0,
        rect.y + 20.0,
        Mods {
            alt: true,
            ..Default::default()
        },
        1,
    );
    app.mouse_up();
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "alt_seen"),
        Some(json!(1)),
        "mod_alt() must be true under an alt-press"
    );
}

/// A double click reaches the script as `click_count() == 2`. The host counted
/// the chain and then threw it away, so a panel saw 1 forever.
#[test]
fn a_double_click_reaches_the_script_as_click_count_two() {
    let mut app = app_with_input_probe("panel-click-count", "");
    let idx = panel_idx(&app);
    let rect = app.panes[idx].rect;
    app.mouse_down(rect.x + 30.0, rect.y + 30.0, Mods::default(), 2);
    app.mouse_up();
    app.settle_panels();
    assert_eq!(panel_value(&app, "clicks"), Some(json!(2)));
}

/// `key_down("shift")` used to return false forever — not an error, just a
/// keybinding that quietly did nothing. Modifiers are published as held keys too.
#[test]
fn a_held_modifier_is_readable_as_a_key() {
    let mut app = app_with_input_probe("panel-modifier-key", "");
    app.apply_key(
        Key::Char('a'),
        Mods {
            shift: true,
            ..Default::default()
        },
    );
    app.settle_panels();
    assert_eq!(panel_value(&app, "held_shift"), Some(json!(true)));
}

/// `{"op": "down"}` / `{"op": "up"}`: a key can be *held* across frames, so
/// `key_down(k)` is observable from a later read and hold-to-do-X is drivable.
#[test]
fn a_key_can_be_held_across_frames() {
    let mut app = app_with_input_probe("panel-held-key", "");
    app.apply_key_phase(Key::Char('w'), Mods::default(), KeyPhase::Down);
    app.settle_panels();
    assert_eq!(
        panel_value(&app, "held_w"),
        Some(json!(true)),
        "a held key stays down across frames"
    );
    // Several frames later it is *still* held — this is the whole point.
    app.settle_panels();
    assert_eq!(panel_value(&app, "held_w"), Some(json!(true)));

    app.apply_key_phase(Key::Char('w'), Mods::default(), KeyPhase::Up);
    app.settle_panels();
    assert_eq!(panel_value(&app, "held_w"), Some(json!(false)));
}

/// A plain key press is still a tap: down and up in one frame, so nothing that
/// depended on the old behavior starts seeing a stuck key.
#[test]
fn a_tapped_key_does_not_stay_held() {
    let mut app = app_with_input_probe("panel-tap-key", "");
    app.apply_key(Key::Char('w'), Mods::default());
    app.settle_panels();
    assert_eq!(panel_value(&app, "held_w"), Some(json!(false)));
}

// --- Recording what the user opened ------------------------------------

/// An `App` with one empty pane whose recents lists live in `dir`, plus a
/// second handle on the same database to read back what was recorded (the
/// `App`'s own handle is private to it).
fn app_with_recents(dir: &Path) -> (App, crate::recents::Recents) {
    let mut app = app_with_panes(&[None]);
    app.set_recents(Some(crate::recents::Recents::open(dir).unwrap()));
    (app, crate::recents::Recents::open(dir).unwrap())
}

#[test]
fn opening_a_file_records_it_and_its_project() {
    let dir = temp_dir("recents-open-file");
    let repo = dir.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let file = file_with(&repo, "a.txt", "hello\n");
    let (mut app, recents) = app_with_recents(&dir);

    run_ex(&mut app, &format!("e {file}"));

    let files = recents.recent_files(10).unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].path.ends_with("a.txt"));
    assert!(
        files[0].project_path.as_deref().unwrap().ends_with("repo"),
        "the file's repo root is recorded alongside it"
    );
    let projects = recents.recent_projects(10).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "repo");
}

/// `:e newfile` opens a buffer for a file that may never be written; only
/// files that exist on disk belong in the recents list.
#[test]
fn opening_a_path_that_is_not_a_file_records_nothing() {
    let dir = temp_dir("recents-open-missing");
    let (mut app, recents) = app_with_recents(&dir);

    run_ex(&mut app, "e /nonexistent/never-written.txt");

    assert!(recents.recent_files(10).unwrap().is_empty());
}

/// Opening a folder records the project even though no file was opened, and a
/// folder *inside* a repo records the repo root rather than the subdirectory.
#[test]
fn opening_a_folder_records_its_project_root() {
    let dir = temp_dir("recents-open-folder");
    let repo = dir.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join("src")).unwrap();
    let (mut app, recents) = app_with_recents(&dir);

    app.record_project_opened(&repo.join("src").to_string_lossy());

    let projects = recents.recent_projects(10).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "repo");
}

/// `:PR 42` records the PR; `:PR` with no number has no identity to record
/// until `gh` resolves the branch's PR, so it records nothing.
#[test]
fn opening_a_pr_records_it_only_with_a_number() {
    let dir = temp_dir("recents-open-pr");
    let (mut app, recents) = app_with_recents(&dir);

    app.record_pr_opened(&dir.to_string_lossy(), 42);
    let prs = recents.recent_prs(10).unwrap();
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].number, 42);
    assert_eq!(
        prs[0].title, "",
        "the title is filled in later, not by `gh`"
    );

    // The `--pr` form the ex command builds, minus the number.
    app.open_garden_diff(vec!["--pr".to_string()]);
    assert_eq!(recents.recent_prs(10).unwrap().len(), 1);
}

/// The host-intercepted `mutate` names (see `App::host_mutation`) — the only
/// way an in-process `panel(...)` screen, which has no subprocess and whose
/// `emit(...)` calls are dropped, can ask Garden to act.
#[test]
fn a_panel_mutation_can_open_a_file_in_the_focused_pane() {
    let dir = temp_dir("mutate-open-path");
    let file = file_with(&dir, "a.txt", "hello\n");
    let mut app = app_with_panes(&[None]);

    app.mutate_panel(0, "open_path", json!({ "path": file }), 1);

    assert_eq!(app.panes[0].file.as_deref(), Some(file.as_str()));
    assert_eq!(app.panes[0].view.buffer.text(), "hello\n");
}

/// A `mutate(...)` hands the script back a handle, and the outcome comes back
/// under it — the only way a drawer (or a test reading `panel.values`) can tell
/// a mutation that worked from one that failed, since the frame that makes the
/// request cannot see its own answer.
#[test]
fn a_panel_mutation_reports_its_outcome_back_under_its_handle() {
    let dir = temp_dir("mutate-handle");
    let script = file_with(
        &dir,
        "p.ptl",
        "state h = 0\n\
         if h == 0 then\n\
           h = mutate(\"apply\", {edits: []})\n\
         end\n\
         let reply = mutate_result(h)\n\
         let ok = reply.ok ?? \"pending\"\n",
    );
    let mut app = App::new(
        None,
        LayoutNode::Panel {
            script,
            screens: Vec::new(),
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );

    // Frame 1 issues the mutation; the handle it returns is a real, non-zero id
    // and no reply exists yet.
    app.tick_panels();
    fn panel(app: &App) -> serde_json::Map<String, serde_json::Value> {
        app.panes[0].panel.as_ref().unwrap().observed().clone()
    }
    let handle = panel(&app).get("h").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        handle > 0,
        "mutate() returned a usable handle, got {handle}"
    );
    assert_eq!(panel(&app).get("ok"), Some(&json!("pending")));

    // This panel has no subprocess, so `apply` cannot be delivered — and that
    // refusal is exactly what must reach the script rather than silence.
    app.mutate_panel(0, "apply", json!({ "edits": [] }), handle);
    app.tick_panels();
    assert_eq!(
        panel(&app).get("ok"),
        Some(&json!(false)),
        "a mutation that could not be delivered reports ok: false"
    );
    let err = panel(&app)
        .get("reply")
        .and_then(|r| r.get("error"))
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_string();
    assert!(
        err.contains("no subprocess"),
        "the reply carries the reason it failed: {err:?}"
    );
}

/// A name the host does not own keeps the old behavior: it is forwarded to the
/// pane's subprocess. This panel has none, so the forwarding path is what
/// produces the error — proof the mutation was not swallowed by the host.
#[test]
fn an_unknown_panel_mutation_still_goes_to_the_client() {
    let dir = temp_dir("mutate-unknown");
    let script = file_with(&dir, "p.ptl", "draw_rect(0, 0, 1, 1, 255, 255, 255)\n");
    let mut app = App::new(
        None,
        LayoutNode::Panel {
            script,
            screens: Vec::new(),
        },
        true,
        Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );

    app.mutate_panel(0, "apply", json!({ "edits": [] }), 1);

    let err = app.status_error.clone().unwrap_or_default();
    assert!(
        err.contains("apply") && err.contains("no subprocess"),
        "unknown mutations are relayed to the client, not handled here: {err:?}"
    );
}

/// The arg is JSON a script built, so every field is untrusted: a missing or
/// non-numeric PR number is a status error, not a panic (and opens nothing).
#[test]
fn a_malformed_open_pr_mutation_is_a_status_error() {
    let mut app = app_with_panes(&[None]);

    app.mutate_panel(0, "open_pr", json!({ "number": "forty-two" }), 1);
    assert!(app.status_error.as_deref().unwrap().contains("open_pr"));

    app.status_error = None;
    app.mutate_panel(0, "open_pr", json!({}), 1);
    assert!(app.status_error.as_deref().unwrap().contains("open_pr"));

    assert!(!app.panes[0].is_panel(), "nothing was opened");
}

/// A native modal would block this thread forever with no window to dismiss it
/// from, so a non-windowed App refuses the picker instead of opening one — if
/// this regressed, the test would hang rather than fail.
#[test]
fn the_file_dialog_is_refused_without_a_window() {
    let mut app = app_with_panes(&[None]);

    app.mutate_panel(0, "open_file_dialog", json!({ "mode": "file" }), 1);

    assert_eq!(
        app.status_error.as_deref(),
        Some("open_file_dialog: no native file picker without a window")
    );
}
