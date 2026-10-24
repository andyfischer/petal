//! Direct manipulation v2 on a Garden panel (docs/dev/direct-manipulation-v2-plan.md
//! §4.4, M4 acceptance item 3): a drag whose position flows through function
//! calls is solved by re-running the frame, the client can say which literals
//! may move, and the answer, applied, puts the shape where the pointer left it.

use std::fs;

use garden_script::{
    apply_rewrites_to, DragFilter, DragOutcomeV2, DragSelector, PanelCmd, PanelHost, SourceRewrite,
};

/// A panel on `src` (with the panel imports), traced and checkpointed the
/// way Petal IDE runs it, one frame in.
fn traced(src: &str) -> (PanelHost, String) {
    let source = format!("import ui; import garden; {src}");
    let mut host = PanelHost::from_source("sketch.ptl", &source).unwrap();
    host.set_trace_origins(true);
    host.set_dm_checkpoint(true);
    host.set_dimensions(400, 300);
    host.frame(0.0, 0).unwrap();
    (host, source)
}

fn cmds(host: &mut PanelHost) -> Vec<PanelCmd> {
    host.frame(0.0, 1).unwrap()
}

fn circle_x(cmd: &PanelCmd) -> i32 {
    match cmd {
        PanelCmd::Circle { cx, .. } => *cx,
        other => panic!("not a circle: {other:?}"),
    }
}

/// The top alternative of a v2 outcome, which must have verified.
fn top(outcome: &DragOutcomeV2) -> &[SourceRewrite] {
    match outcome {
        DragOutcomeV2::Alternatives { alternatives, .. } => {
            assert!(alternatives[0].verified, "{alternatives:#?}");
            &alternatives[0].rewrites
        }
        other => panic!("expected alternatives, got {other:?}"),
    }
}

/// Apply `rewrites` to `source`, reload the panel from it, and return the
/// next frame's commands.
fn apply(host: &mut PanelHost, source: &str, rewrites: &[SourceRewrite]) -> (String, Vec<PanelCmd>) {
    let edited = apply_rewrites_to(source, rewrites).unwrap();
    host.reload_source(&edited).unwrap();
    let cmds = cmds(host);
    (edited, cmds)
}

const HELPER: &str = "\n\
fn place(i)\n\
\x20 20 + i * 40\n\
end\n\
clear(0, 0, 0)\n\
for i in range(0, 4) do\n\
\x20 draw_circle(place(i), 50, 10, 200, 90, 90)\n\
end\n";

/// The acceptance case: the circle's x is `place(i)`, a call v1 refuses to
/// look into. v2 solves it into the helper's literals and the shape lands
/// 25px to the right.
#[test]
fn drag_through_helper_v2() {
    let (mut host, source) = traced(HELPER);
    let before = cmds(&mut host);
    // Command 0 is the clear; 2 is the second circle (i = 1, x = 60).
    assert_eq!(circle_x(&before[2]), 60);

    let outcome = host.propose_drag_edits_v2(2, 25.0, 0.0, &DragFilter::default());
    let rewrites = top(&outcome).to_vec();
    assert!(
        rewrites.iter().all(|r| r.span.start_line == 2 && r.file.is_none()),
        "the edit is in the helper's body (line 3): {rewrites:#?}"
    );
    let (_, after) = apply(&mut host, &source, &rewrites);
    let moved = circle_x(&after[2]) - circle_x(&before[2]);
    assert!((moved - 25).abs() <= 1, "moved by {moved}");
}

/// v1 could only solve the last shape a looping call drew, because the trace
/// holds the last iteration's values. v2 keys emits by occurrence, so the
/// first circle of the loop is as draggable as the last.
#[test]
fn any_iteration_of_a_loop_is_draggable() {
    let src = "config let spacing = 50\n\
               let x0 = 20\n\
               clear(10, 10, 10)\n\
               for i in range(0, 3) do\n\
               \x20 draw_rect(x0 + i * spacing, 40, 30, 30, 200, 90, 90)\n\
               end\n";
    let (mut host, source) = traced(src);
    // The middle card (i = 1, at x = 70) — the one v1 refused.
    let outcome = host.propose_drag_edits_v2(2, 20.0, 0.0, &DragFilter::default());
    let rewrites = top(&outcome).to_vec();
    let (_, after) = apply(&mut host, &source, &rewrites);
    match &after[2] {
        PanelCmd::Rect { x, y, .. } => {
            assert!((x - 90).abs() <= 1, "x = {x}");
            assert_eq!(*y, 40, "the drag was horizontal");
        }
        other => panic!("{other:?}"),
    }
    // The config knob is preferred (softly): it is what moves.
    assert!(
        rewrites.iter().any(|r| r.config),
        "the config knob moves: {rewrites:#?}"
    );
}

/// The client's choice is exact: allowing only `x0` moves `x0`, even though
/// the engine would prefer the config knob.
#[test]
fn an_allow_list_chooses_the_variable() {
    let src = "config let gap = 30\n\
               let x0 = 40\n\
               fn slot(i)\n\
               \x20 x0 + i * gap\n\
               end\n\
               clear(0, 0, 0)\n\
               for i in range(0, 3) do\n\
               \x20 draw_circle(slot(i), 60, 8, 255, 255, 255)\n\
               end\n";
    let (mut host, source) = traced(src);
    let start = host.begin_drag_v2(3, &DragFilter::default()).unwrap();
    let names: Vec<&str> = start
        .candidates
        .iter()
        .filter(|c| c.eligible && c.moves)
        .map(|c| c.label.as_str())
        .collect();
    assert!(names.contains(&"gap") && names.contains(&"x0"), "{:#?}", start.candidates);
    assert_eq!(start.base_x, vec![100.0], "slot(2) = 40 + 2 * 30");
    host.cancel_drag_v2();

    let only_x0 = DragFilter {
        allow: vec![DragSelector::parse("x0").unwrap()],
        deny: Vec::new(),
    };
    let outcome = host.propose_drag_edits_v2(3, 12.0, 0.0, &only_x0);
    let rewrites = top(&outcome).to_vec();
    assert_eq!(rewrites.len(), 1, "{rewrites:#?}");
    assert_eq!(rewrites[0].variable.as_deref(), Some("x0"));
    assert_eq!(rewrites[0].new_text, "52");
    let (edited, _) = apply(&mut host, &source, &rewrites);
    assert!(edited.contains("let x0 = 52"), "{edited}");
}

/// Several alternatives come back, ranked, each one verified on its own.
#[test]
fn alternatives_are_listed() {
    let src = "let a = 30\n\
               let b = 20\n\
               fn f()\n\
               \x20 a + b\n\
               end\n\
               draw_circle(f(), 60, 8, 255, 255, 255)\n";
    let (mut host, _) = traced(src);
    match host.propose_drag_edits_v2(0, 10.0, 0.0, &DragFilter::default()) {
        DragOutcomeV2::Alternatives { alternatives, .. } => {
            assert!(alternatives.len() >= 2, "{alternatives:#?}");
            let vars: Vec<Option<&str>> = alternatives
                .iter()
                .map(|a| a.rewrites[0].variable.as_deref())
                .collect();
            assert!(vars.contains(&Some("a")) && vars.contains(&Some("b")), "{vars:?}");
            assert!(alternatives.windows(2).all(|w| w[0].score <= w[1].score || !w[1].verified));
        }
        other => panic!("{other:?}"),
    }
}

/// A drag in progress is previewed with no runs, and the release agrees with
/// the preview.
#[test]
fn preview_then_finish() {
    let (mut host, source) = traced(HELPER);
    host.pin_dm_checkpoint();
    host.begin_drag_v2(2, &DragFilter::default()).unwrap();
    assert!(host.drag_v2_active());
    let preview = host.preview_drag_v2(25.0, 0.0);
    assert!(!preview.is_empty(), "something is predicted to move");
    let previewed = apply_rewrites_to(&source, &preview).unwrap();
    let outcome = host.finish_drag_v2(25.0, 0.0);
    assert!(!host.drag_v2_active(), "the release closes the drag");
    let rewrites = top(&outcome).to_vec();
    let finished = apply_rewrites_to(&source, &rewrites).unwrap();
    // Both edit the helper; the release's numbers are the verified ones.
    let helper_line = |s: &str| s.lines().nth(2).unwrap().to_string();
    assert_ne!(helper_line(&previewed), helper_line(&source));
    assert_ne!(helper_line(&finished), helper_line(&source));
    // A second release has nothing open.
    assert_eq!(host.finish_drag_v2(25.0, 0.0), DragOutcomeV2::Stale);
}

/// The checkpoint is the stack *before* the frame on screen ran: an animated
/// sketch re-runs to the position the user sees, not one frame further on.
#[test]
fn the_checkpoint_is_the_frame_on_screen() {
    let src = "state t = 0\n\
               t = t + 10\n\
               draw_circle(t + 20, 50, 8, 255, 255, 255)\n";
    let (mut host, _) = traced(src);
    let shown = cmds(&mut host);
    let start = host.begin_drag_v2(0, &DragFilter::default()).unwrap();
    assert_eq!(
        start.base_x,
        vec![circle_x(&shown[0]) as f64],
        "the re-run draws the frame on screen"
    );
    host.cancel_drag_v2();
}

/// The `rect`-taking overload: one record argument carries both axes, which
/// v1 cannot drag. v2 reads the native's record fields.
#[test]
fn a_record_positioned_rect_is_draggable() {
    let src = "let card = {x: 30, y: 40, w: 50, h: 20}\n\
               draw_rect(card, {r: 200, g: 90, b: 90})\n";
    let (mut host, source) = traced(src);
    let outcome = host.propose_drag_edits_v2(0, 15.0, -10.0, &DragFilter::default());
    let rewrites = top(&outcome).to_vec();
    let (edited, after) = apply(&mut host, &source, &rewrites);
    match &after[0] {
        PanelCmd::Rect { x, y, .. } => assert_eq!((*x, *y), (45, 30), "{edited}"),
        other => panic!("{other:?}"),
    }
}

/// Editing the program under an open drag makes its release stale rather
/// than writing spans that no longer describe the text.
#[test]
fn a_reload_under_the_drag_is_stale() {
    let (mut host, source) = traced(HELPER);
    host.begin_drag_v2(2, &DragFilter::default()).unwrap();
    host.reload_source(&source.replace("20 + i", "21 + i")).unwrap();
    assert_eq!(host.finish_drag_v2(25.0, 0.0), DragOutcomeV2::Stale);
}

/// A shape placed by a helper in an imported module: the edit names that
/// file, so a host can write it where it lives.
#[test]
fn an_edit_in_an_imported_file_names_it() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("layout.ptl"),
        "pub fn column_x(i)\n  30 + i * 25\nend\n",
    )
    .unwrap();
    let entry = dir.path().join("main.ptl");
    fs::write(
        &entry,
        "import ui; import garden; import ./layout\n\
         for i in range(0, 3) do\n\
         \x20 draw_circle(layout.column_x(i), 50, 6, 255, 255, 255)\n\
         end\n",
    )
    .unwrap();
    let mut host = PanelHost::load(&entry).unwrap();
    host.set_trace_origins(true);
    host.set_dm_checkpoint(true);
    host.set_dimensions(300, 200);
    host.frame(0.0, 0).unwrap();
    let outcome = host.propose_drag_edits_v2(1, 10.0, 0.0, &DragFilter::default());
    let rewrites = top(&outcome);
    let file = rewrites[0].file.as_ref().expect("the edit is in the module");
    assert!(file.ends_with("layout.ptl"), "{file:?}");
    assert_eq!(rewrites[0].span.start_line, 1);
}

/// Shapes that are not a position pair say so.
#[test]
fn a_polygon_is_refused() {
    let (mut host, _) = traced("fill_polygon([[10, 10], [50, 10], [30, 40]], 255, 255, 255)\n");
    match host.propose_drag_edits_v2(0, 5.0, 5.0, &DragFilter::default()) {
        DragOutcomeV2::Refused { why, .. } => assert!(why.contains("no position"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn selectors_parse() {
    assert_eq!(DragSelector::parse("gap"), Some(DragSelector::Variable("gap".into())));
    assert_eq!(DragSelector::parse("line:3"), Some(DragSelector::Line(2)));
    assert_eq!(DragSelector::parse("fn:slot"), Some(DragSelector::Function("slot".into())));
    assert_eq!(DragSelector::parse("config"), Some(DragSelector::ConfigOnly));
    assert_eq!(DragSelector::parse("knob"), Some(DragSelector::ConfigOnly));
    assert_eq!(DragSelector::ConfigOnly.label(), "config");
    assert_eq!(DragSelector::parse("line:0"), None);
    assert_eq!(DragSelector::parse("a b"), None);
    for s in ["gap", "line:3", "fn:slot", "config"] {
        assert_eq!(DragSelector::parse(s).unwrap().label(), s);
    }
}

/// The interaction budget: opening a drag on a sketch of a few hundred
/// shapes stays within the open cap (plus one probe and two baselines), a
/// preview makes no runs, and a release stays within its cap. Prints the
/// timings, and what the per-frame checkpoint costs, for the docs.
#[test]
fn a_drag_stays_interactive() {
    use std::time::{Duration, Instant};
    let src = "config let gap = 3\n\
               let x0 = 10\n\
               fn cell(i)\n\
               \x20 let col = i % 20\n\
               \x20 let row = (i - col) / 20\n\
               \x20 [x0 + col * gap * 6, 20 + row * gap * 6]\n\
               end\n\
               clear(0, 0, 0)\n\
               for i in range(0, 300) do\n\
               \x20 let p = cell(i)\n\
               \x20 draw_circle(p[0], p[1], 4, 255, 255, 255)\n\
               end\n";
    let (mut host, _) = traced(src);
    let frame = |host: &mut PanelHost, on: bool| {
        host.set_dm_checkpoint(on);
        host.invalidate_frame();
        let t = Instant::now();
        for k in 0..20 {
            host.invalidate_frame();
            host.frame(0.0, 10 + k).unwrap();
        }
        t.elapsed() / 20
    };
    let plain = frame(&mut host, false);
    let armed = frame(&mut host, true);
    let t = Instant::now();
    let start = host.begin_drag_v2(45, &DragFilter::default()).unwrap();
    let open = t.elapsed();
    let t = Instant::now();
    let preview = host.preview_drag_v2(12.0, 6.0);
    let previewed = t.elapsed();
    let t = Instant::now();
    let outcome = host.finish_drag_v2(12.0, 6.0);
    let finish = t.elapsed();
    eprintln!(
        "frame {plain:?} (checkpoint armed {armed:?}); open {open:?} ({} runs); preview {previewed:?}; finish {finish:?}",
        start.runs
    );
    assert!(!preview.is_empty());
    assert!(matches!(outcome, DragOutcomeV2::Alternatives { .. }), "{outcome:?}");
    let cap = |wall: Duration| wall + Duration::from_millis(600);
    assert!(open < cap(garden_script::OPEN_WALL), "open took {open:?}");
    assert!(finish < cap(garden_script::FINISH_WALL), "finish took {finish:?}");
    assert!(previewed < Duration::from_millis(50), "a preview is linear: {previewed:?}");
}

/// The shipped demo (`examples/panels/drag-to-edit.ptl`): a loop card that
/// is not the last one, and a dot placed through a trig helper, both solve,
/// verify, and land where the drag left them.
#[test]
fn the_drag_to_edit_demo_solves() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/panels/drag-to-edit.ptl");
    let source = fs::read_to_string(&path).unwrap();
    let mut host = PanelHost::load(&path).unwrap();
    host.set_trace_origins(true);
    host.set_dm_checkpoint(true);
    host.set_dimensions(640, 420);
    host.frame(0.0, 0).unwrap();
    let before = cmds(&mut host);
    // The second card of section 3, and the third dot of section 5.
    let card = before
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c, PanelCmd::Rect { w: 44, .. }))
        .nth(1)
        .unwrap()
        .0;
    let dot = before
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c, PanelCmd::Circle { radius: 7, .. }))
        .nth(2)
        .unwrap()
        .0;
    let at = |cmds: &[PanelCmd], i: usize| match &cmds[i] {
        PanelCmd::Rect { x, y, .. } => (*x, *y),
        PanelCmd::Circle { cx, cy, .. } => (*cx, *cy),
        other => panic!("{other:?}"),
    };
    for (index, (dx, dy)) in [(card, (10, 0)), (dot, (12, -8))] {
        let outcome = host.propose_drag_edits_v2(index, dx as f64, dy as f64, &DragFilter::default());
        let rewrites = top(&outcome).to_vec();
        let (_, after) = apply(&mut host, &source, &rewrites);
        let (b, a) = (at(&before, index), at(&after, index));
        assert!(
            (a.0 - b.0 - dx).abs() <= 1 && (a.1 - b.1 - dy).abs() <= 1,
            "command {index} moved from {b:?} to {a:?}, wanted ({dx}, {dy}): {rewrites:#?}"
        );
        host.reload_source(&source).unwrap();
        cmds(&mut host);
    }
}

/// The drag-mode examples (`examples/panels/dm-v2/`): the main shape of each
/// solves, verifies, and lands where the drag left it, as the header says.
#[test]
fn the_drag_mode_examples_solve() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/panels/dm-v2");
    type Pick = fn(&[PanelCmd]) -> usize;
    let last_card: Pick = |c| c.iter().rposition(|c| matches!(c, PanelCmd::Rect { w: 56, .. })).unwrap();
    let tip: Pick = |c| c.iter().position(|c| matches!(c, PanelCmd::Circle { radius: 7, .. })).unwrap();
    let rightmost_leaf: Pick = |c| {
        c.iter()
            .enumerate()
            .filter_map(|(i, c)| match c {
                PanelCmd::Circle { cx, radius: 4, .. } => Some((*cx, i)),
                _ => None,
            })
            .max()
            .unwrap()
            .1
    };
    let last_button: Pick = |c| c.iter().rposition(|c| matches!(c, PanelCmd::Rect { h: 28, .. })).unwrap();
    let third_bar: Pick = |c| {
        c.iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, PanelCmd::Rect { w: 36, .. }))
            .nth(2)
            .unwrap()
            .0
    };
    let marker: Pick = |c| c.iter().position(|c| matches!(c, PanelCmd::Circle { radius: 7, .. })).unwrap();
    let cases: [(&str, Pick, (i32, i32)); 6] = [
        ("01-helper-row.ptl", last_card, (20, 0)),
        ("02-clock-and-orbit.ptl", tip, (0, 80)),
        ("03-fractal-tree.ptl", rightmost_leaf, (15, 0)),
        ("04-flex-row.ptl", last_button, (20, 0)),
        ("05-projectile.ptl", marker, (40, 0)),
        ("06-bar-chart.ptl", third_bar, (0, -16)),
    ];
    let at = |cmds: &[PanelCmd], i: usize| match &cmds[i] {
        PanelCmd::Rect { x, y, .. } | PanelCmd::RectOutline { x, y, .. } | PanelCmd::Text { x, y, .. } => (*x, *y),
        PanelCmd::Circle { cx, cy, .. } => (*cx, *cy),
        other => panic!("{other:?}"),
    };
    for (file, pick, (dx, dy)) in cases {
        let path = dir.join(file);
        let source = fs::read_to_string(&path).unwrap();
        let mut host = PanelHost::load(&path).unwrap();
        host.set_trace_origins(true);
        host.set_dm_checkpoint(true);
        host.set_dimensions(633, 744);
        host.frame(0.0, 0).unwrap();
        let before = cmds(&mut host);
        let index = pick(&before);
        let outcome = host.propose_drag_edits_v2(index, dx as f64, dy as f64, &DragFilter::default());
        if let DragOutcomeV2::Alternatives { alternatives, .. } = &outcome {
            assert!(alternatives[0].verified, "{file}: {outcome:?}");
        }
        let rewrites = top(&outcome).to_vec();
        let (_, after) = apply(&mut host, &source, &rewrites);
        let (b, a) = (at(&before, index), at(&after, index));
        assert!(
            (a.0 - b.0 - dx).abs() <= 1 && (a.1 - b.1 - dy).abs() <= 1,
            "{file}: command {index} moved from {b:?} to {a:?}, wanted ({dx}, {dy}): {rewrites:#?}"
        );
    }
}
