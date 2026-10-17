//! Layers, hit testing and the target registry: `ui_layer` / `with_layer` /
//! `ui_overlay`, `ui_modal`, `interact`, `pointer_captured`, `ui_target_rect`,
//! and the prelude widgets going through them.

use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;

fn ui(src: &str) -> Headless {
    Headless::with_ui(src).unwrap_or_else(|e| panic!("compile failed: {e}"))
}

fn flag(ui: &Headless, name: &str) -> bool {
    match ui.state().get(name) {
        Some(serde_json::Value::Bool(b)) => *b,
        other => panic!("state `{name}` is not a bool: {other:?}"),
    }
}

// ── interact ──────────────────────────────────────────────────────────────

const ONE: &str = "state hover = false\n\
                   state down = false\n\
                   state pressed = 0\n\
                   state released = 0\n\
                   state clicks = 0\n\
                   let s = interact(rect(10, 10, 100, 40), \"one\")\n\
                   hover = s.hover\n\
                   down = s.down\n\
                   if s.pressed then pressed = pressed + 1 end\n\
                   if s.released then released = released + 1 end\n\
                   if s.click then clicks = clicks + 1 end";

#[test]
fn interact_reports_hover_press_release_and_click() {
    let mut ui = ui(ONE);
    ui.frame().unwrap();
    assert!(!flag(&ui, "hover"));

    ui.mouse_move(20, 20);
    ui.frame().unwrap();
    assert!(flag(&ui, "hover"));
    assert!(!flag(&ui, "down"));

    ui.mouse_down(0);
    ui.frame().unwrap();
    assert!(flag(&ui, "down"));
    assert_eq!(ui.state_int("pressed"), Some(1));
    assert_eq!(ui.state_int("clicks"), Some(0), "a click is the release");

    ui.mouse_up(0);
    ui.frame().unwrap();
    assert!(!flag(&ui, "down"));
    assert_eq!(ui.state_int("released"), Some(1));
    assert_eq!(ui.state_int("clicks"), Some(1));
}

/// Pressed inside, slid off, released outside: still `down` while held (the
/// press is this rect's), and no click.
#[test]
fn a_press_that_slides_off_is_held_but_is_not_a_click() {
    let mut ui = ui(ONE);
    ui.mouse_move(20, 20);
    ui.mouse_down(0);
    ui.frame().unwrap();
    ui.mouse_move(400, 400);
    ui.frame().unwrap();
    assert!(flag(&ui, "down"), "held: the press landed inside");
    assert!(!flag(&ui, "hover"));
    ui.mouse_up(0);
    ui.frame().unwrap();
    assert_eq!(ui.state_int("clicks"), Some(0));
    assert_eq!(ui.state_int("released"), Some(0));
}

/// Pressed outside, released inside: a release over the rect, but not a click.
#[test]
fn a_release_inside_after_a_press_outside_is_not_a_click() {
    let mut ui = ui(ONE);
    ui.mouse_move(400, 400);
    ui.mouse_down(0);
    ui.frame().unwrap();
    ui.mouse_move(20, 20);
    ui.frame().unwrap();
    assert!(!flag(&ui, "down"));
    ui.mouse_up(0);
    ui.frame().unwrap();
    assert_eq!(ui.state_int("released"), Some(1));
    assert_eq!(ui.state_int("clicks"), Some(0));
}

/// A button that opens a list on its click; the list's first row sits where
/// the button was pressed, and runs after the button on the same frame.
const OPENS_A_LIST: &str = "state open = false\n\
                            state picked = 0\n\
                            if interact(rect(10, 10, 100, 30), \"open\").click then open = true end\n\
                            if open then\n\
                              let hit = with_layer(10, fn() interact(rect(0, 0, 200, 40), \"row\").click end)\n\
                              if hit then picked = picked + 1 end\n\
                            end";

/// The click that opens a panel does not also click the row of the new panel
/// that appeared under the press.
#[test]
fn a_rect_that_appears_under_a_press_is_not_clicked_by_its_release() {
    let mut ui = ui(OPENS_A_LIST);
    ui.mouse_move(20, 20);
    ui.frame().unwrap();
    ui.mouse_down(0);
    ui.frame().unwrap();
    ui.mouse_up(0);
    ui.frame().unwrap();
    assert!(flag(&ui, "open"));
    assert_eq!(ui.state_int("picked"), Some(0), "the row was not there at the press");
    ui.frame().unwrap();
    ui.click(20, 20).unwrap();
    ui.frame().unwrap();
    assert_eq!(ui.state_int("picked"), Some(1), "a press made with the row open clicks it");
}

/// A widget that shows only under the pointer, moved onto and pressed in one
/// frame (what `Headless::click` does), registered on the press frame — the
/// press is still its own.
#[test]
fn a_widget_shown_on_the_press_frame_still_takes_the_click() {
    let mut ui = ui("state clicks = 0\n\
                     if hovered(rect(0, 0, 100, 100)) then\n\
                       if interact(rect(10, 10, 50, 20), \"x\").click then clicks = clicks + 1 end\n\
                     end\n\
                     ui_block(rect(200, 200, 10, 10))");
    ui.mouse_move(400, 400);
    ui.frame().unwrap();
    ui.click(20, 15).unwrap();
    ui.frame().unwrap();
    assert_eq!(ui.state_int("clicks"), Some(1));
}

// ── Layers ────────────────────────────────────────────────────────────────

/// A button under a panel on a higher layer. `show` turns the panel on.
const COVERED: &str = "state show = false\n\
                       state under = 0\n\
                       state over = 0\n\
                       state under_hover = false\n\
                       state captured = false\n\
                       if key_pressed(\"p\") then show = !show end\n\
                       let s = interact(rect(0, 0, 100, 40), \"under\")\n\
                       under_hover = s.hover\n\
                       if s.pressed then under = under + 1 end\n\
                       if show then\n\
                         let hit = with_layer(10, fn()\n\
                           ui_block(rect(0, 0, 200, 200), \"panel\")\n\
                           interact(rect(10, 10, 50, 20), \"over\").pressed\n\
                         end)\n\
                         if hit then over = over + 1 end\n\
                       end\n\
                       captured = pointer_captured()";

#[test]
fn a_rect_on_a_higher_layer_takes_the_pointer_from_the_one_below() {
    let mut ui = ui(COVERED);
    ui.mouse_move(20, 15);
    ui.frame().unwrap();
    assert!(flag(&ui, "under_hover"));

    ui.key("p").unwrap(); // the panel appears on this frame
    ui.frame().unwrap(); // and is in the registry from this one
    assert!(!flag(&ui, "under_hover"), "covered: not hot");
    ui.click(20, 15).unwrap();
    assert_eq!(ui.state_int("under"), Some(0), "the click went to the panel");
    assert_eq!(ui.state_int("over"), Some(1));

    ui.frame().unwrap();
    ui.key("p").unwrap(); // the panel goes
    ui.frame().unwrap();
    assert!(flag(&ui, "under_hover"), "uncovered again a frame later");
}

/// Rects on one layer do not block each other — what overlapping widgets
/// have always done.
#[test]
fn rects_on_the_same_layer_are_both_hot() {
    let mut ui = ui("state a = false\nstate b = false\n\
                     a = ui_hot(rect(0, 0, 100, 100))\n\
                     b = ui_hot(rect(50, 50, 100, 100))");
    ui.mouse_move(60, 60);
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert!(flag(&ui, "a"));
    assert!(flag(&ui, "b"));
}

/// The cover runs *before* what it covers (z order is not code order): the
/// lower rect is blocked on the very frame the cover appears, because the
/// cover has already turned out hot this run.
#[test]
fn a_cover_that_runs_first_blocks_on_its_first_frame() {
    let mut ui = ui("state under = 0\n\
                     state show = false\n\
                     if key_pressed(\"p\") then show = true end\n\
                     if show then with_layer(5, fn() ui_block(rect(0, 0, 200, 200)) end) end\n\
                     if interact(rect(0, 0, 100, 40)).pressed then under = under + 1 end");
    ui.mouse_move(20, 15);
    ui.frame().unwrap();
    // The key that opens the panel and the press arrive on the same frame.
    ui.event(petal_ui::input::InputEvent::KeyDown { key: "p".into() });
    ui.mouse_down(0);
    ui.frame().unwrap();
    assert_eq!(ui.state_int("under"), Some(0));
}

// ── pointer_captured ──────────────────────────────────────────────────────

#[test]
fn pointer_captured_is_true_over_registered_rects_only() {
    let mut ui = ui(COVERED);
    ui.mouse_move(20, 15);
    ui.frame().unwrap();
    assert!(flag(&ui, "captured"), "over the button");
    ui.mouse_move(400, 400);
    ui.frame().unwrap();
    assert!(!flag(&ui, "captured"), "over the world");
}

/// The debrief's leak: a panel opens under the pointer on the same frame as
/// a press. Asked after the UI has drawn, `pointer_captured()` already says
/// yes on that frame — the world does not get the click.
#[test]
fn a_panel_that_opens_under_the_pointer_captures_the_same_frame() {
    let mut ui = ui("state show = false\n\
                     state world_clicks = 0\n\
                     if key_pressed(\"p\") then show = true end\n\
                     if show then ui_block(rect(300, 300, 200, 200)) end\n\
                     if mouse_pressed(0) && !pointer_captured() then world_clicks = world_clicks + 1 end");
    ui.mouse_move(350, 350);
    ui.frame().unwrap();
    ui.click(350, 350).unwrap();
    assert_eq!(ui.state_int("world_clicks"), Some(1), "nothing there yet");
    ui.frame().unwrap();

    ui.event(petal_ui::input::InputEvent::KeyDown { key: "p".into() });
    ui.mouse_down(0);
    ui.frame().unwrap();
    assert_eq!(ui.state_int("world_clicks"), Some(1), "the new panel took it");
}

/// A HUD that goes away stops capturing: a run that registers nothing
/// replaces the registry rather than leaving last frame's rects in it.
#[test]
fn a_hidden_hud_stops_capturing() {
    let mut ui = ui("state show = true\n\
                     state captured = false\n\
                     if key_pressed(\"h\") then show = false end\n\
                     if show then ui_block(rect(0, 0, 200, 200)) end\n\
                     captured = pointer_captured()");
    ui.mouse_move(50, 50);
    ui.frame().unwrap();
    assert!(flag(&ui, "captured"));
    ui.key("h").unwrap();
    ui.frame().unwrap();
    assert!(!flag(&ui, "captured"));
}

#[test]
fn pointer_captured_above_a_layer_ignores_that_layer_and_below() {
    let mut ui = ui("state mine = false\nstate above = false\n\
                     with_layer(3, fn() ui_block(rect(0, 0, 100, 100)) end)\n\
                     mine = pointer_captured(3)\n\
                     above = pointer_captured(2)");
    ui.mouse_move(50, 50);
    ui.frame().unwrap();
    assert!(!flag(&ui, "mine"), "nothing above layer 3");
    assert!(flag(&ui, "above"), "layer 3 is above layer 2");
}

// ── Modal ─────────────────────────────────────────────────────────────────

#[test]
fn a_modal_layer_makes_everything_below_inert_everywhere() {
    let mut ui = ui("state open = false\n\
                     state under = 0\n\
                     state inside = 0\n\
                     state captured = false\n\
                     if key_pressed(\"m\") then open = !open end\n\
                     if interact(rect(0, 0, 100, 40)).pressed then under = under + 1 end\n\
                     if open then\n\
                       let hit = with_layer(50, fn()\n\
                         ui_modal()\n\
                         interact(rect(300, 300, 80, 30)).pressed\n\
                       end)\n\
                       if hit then inside = inside + 1 end\n\
                     end\n\
                     captured = pointer_captured()");
    ui.frame().unwrap();
    ui.click(20, 20).unwrap();
    assert_eq!(ui.state_int("under"), Some(1));
    ui.frame().unwrap();

    ui.key("m").unwrap();
    ui.frame().unwrap();
    // Far from the dialog's own rect, and still inert.
    ui.click(20, 20).unwrap();
    assert_eq!(ui.state_int("under"), Some(1), "inert under a modal");
    assert!(flag(&ui, "captured"), "a modal captures the pointer everywhere");
    ui.frame().unwrap();
    ui.click(310, 310).unwrap();
    assert_eq!(ui.state_int("inside"), Some(1), "the modal's own controls work");
}

/// The prelude's `modal` declares itself: a plain `button` drawn before it is
/// inert while it is open, with no `modal_blocking` guard in the app, and a
/// button drawn after it (inside the dialog) is live.
#[test]
fn the_prelude_modal_blocks_the_buttons_under_it() {
    let mut ui = ui("state md = modal_state()\n\
                     state under = 0\n\
                     state ok = 0\n\
                     if key_pressed(\"m\") then md = modal_open(md) end\n\
                     if button(rect(0, 0, 100, 30), \"Under\") then under = under + 1 end\n\
                     let res = modal(md, 300, 200, \"Sure?\")\n\
                     md = res.modal\n\
                     if res.open then\n\
                       if button(rect(res.content.x, res.content.y, 80, 30), \"OK\") then ok = ok + 1 end\n\
                     end");
    ui.frame().unwrap();
    ui.click_target("Under").unwrap();
    assert_eq!(ui.state_int("under"), Some(1));
    ui.frame().unwrap();

    ui.key("m").unwrap();
    ui.frame().unwrap();
    let under = ui.target("Under").unwrap().center();
    ui.click(under.0, under.1).unwrap();
    assert_eq!(ui.state_int("under"), Some(1), "the dialog is modal");
    ui.frame().unwrap();
    ui.key("m").unwrap();
    ui.frame().unwrap();
    ui.click_target("OK").unwrap();
    assert_eq!(ui.state_int("ok"), Some(1));
}

// ── Clip ──────────────────────────────────────────────────────────────────

/// A row scrolled half out of its list is not hot over the part the clip
/// hides.
#[test]
fn interact_respects_the_clip_in_force() {
    let mut ui = ui("state hot = false\n\
                     clip_push(rect(0, 0, 100, 50))\n\
                     hot = ui_hot(rect(0, 40, 100, 40), \"row\")\n\
                     clip_pop()");
    ui.mouse_move(50, 45);
    ui.frame().unwrap();
    assert!(flag(&ui, "hot"), "the visible part");
    ui.mouse_move(50, 60);
    ui.frame().unwrap();
    assert!(!flag(&ui, "hot"), "the clipped part");
    // And the registry holds the visible part, so a click on the target's
    // centre lands on something the user could have clicked.
    let row = ui.target("row").unwrap();
    assert_eq!((row.y, row.h), (40.0, 10.0));
}

#[test]
fn nested_clips_intersect_and_pop_restores() {
    let mut ui = ui("state a = false\nstate b = false\n\
                     clip_push(rect(0, 0, 100, 100))\n\
                     clip_push(rect(50, 50, 100, 100))\n\
                     a = ui_hot(rect(0, 0, 200, 200))\n\
                     clip_pop()\n\
                     b = ui_hot(rect(0, 0, 200, 200))\n\
                     clip_pop()");
    ui.mouse_move(20, 20); // inside the outer clip only
    ui.frame().unwrap();
    assert!(!flag(&ui, "a"));
    assert!(flag(&ui, "b"));
}

// ── Targets ───────────────────────────────────────────────────────────────

#[test]
fn widgets_register_ids_a_script_and_the_host_can_look_up() {
    let mut ui = ui("state found = false\n\
                     state x = -1\n\
                     button(rect(40, 60, 100, 30), \"Save\")\n\
                     button(rect(40, 100, 100, 30), \"Quit\", {id: \"quit-button\"})\n\
                     ui_target(\"drop-zone\", rect(300, 300, 50, 50))\n\
                     let r = ui_target_rect(\"Save\")\n\
                     if r != nil then\n\
                       found = true\n\
                       x = r.x\n\
                     end");
    ui.frame().unwrap();
    assert!(!flag(&ui, "found"), "last frame's registry: nothing yet");
    ui.frame().unwrap();
    assert!(flag(&ui, "found"));
    assert_eq!(ui.state_int("x"), Some(40));

    // The host side: every registered rect, by id.
    let ids: Vec<String> = ui.targets().into_iter().filter_map(|t| t.id).collect();
    assert_eq!(ids, ["Save", "quit-button", "drop-zone"]);
    assert_eq!(ui.target("quit-button").unwrap().center(), (90, 115));
    assert!(ui.target("Quit").is_none(), "the id replaces the label");
    // A named target does not block the pointer.
    assert!(!ui.target("drop-zone").unwrap().blocks);
}

#[test]
fn click_target_clicks_a_widget_by_name() {
    let mut ui = ui("state n = 0\n\
                     if button(rect(500, 400, 80, 24), \"Go\") then n = n + 1 end");
    ui.frame().unwrap();
    ui.click_target("Go").unwrap();
    assert_eq!(ui.state_int("n"), Some(1));
    assert!(ui.click_target("Stop").is_err());
}

// ── Draw order ────────────────────────────────────────────────────────────

fn rect_x(cmds: &[DrawCommand]) -> Vec<i32> {
    cmds.iter()
        .filter_map(|c| match c {
            DrawCommand::Rect { x, .. } => Some(*x),
            _ => None,
        })
        .collect()
}

#[test]
fn a_higher_layer_draws_later_whatever_the_code_order() {
    let mut ui = ui("draw_rect(1, 0, 5, 5, #ff0000)\n\
                     with_layer(10, fn() draw_rect(2, 0, 5, 5, #ff0000) end)\n\
                     draw_rect(3, 0, 5, 5, #ff0000)\n\
                     with_layer(5, fn() draw_rect(4, 0, 5, 5, #ff0000) end)\n\
                     draw_rect(5, 0, 5, 5, #ff0000)");
    let cmds = ui.frame().unwrap().to_vec();
    assert_eq!(rect_x(&cmds), [1, 3, 5, 4, 2]);
    assert!(
        !cmds.iter().any(|c| matches!(c, DrawCommand::Layer { .. })),
        "the markers never reach a host"
    );
}

/// A run moved by the sort takes its clip with it, and leaves the clip of
/// what follows as it was.
#[test]
fn a_layer_keeps_the_clip_it_was_drawn_under() {
    let mut ui = ui("clip_push(rect(0, 0, 50, 50))\n\
                     with_layer(10, fn() draw_rect(1, 0, 5, 5, #ff0000) end)\n\
                     draw_rect(2, 0, 5, 5, #ff0000)\n\
                     clip_pop()\n\
                     draw_rect(3, 0, 5, 5, #ff0000)");
    let cmds = ui.frame().unwrap().to_vec();
    let push = DrawCommand::ClipPush { x: 0, y: 0, w: 50, h: 50, radius: 0 };
    let x = |c: &DrawCommand| match c {
        DrawCommand::Rect { x, .. } => *x,
        _ => 0,
    };
    let shape: Vec<String> = cmds
        .iter()
        .map(|c| match c {
            DrawCommand::ClipPush { .. } => {
                assert_eq!(c, &push);
                "push".to_string()
            }
            DrawCommand::ClipPop => "pop".to_string(),
            other => x(other).to_string(),
        })
        .collect();
    // Layer 0 in order (2 clipped, 3 not), then layer 10 under its clip,
    // and the clip popped again: the frame ends with the stack as it began.
    assert_eq!(shape, ["push", "2", "pop", "3", "push", "1", "pop"]);
}

/// `ui_overlay` lifts the clip: a popup opened from inside a scrolled list.
#[test]
fn an_overlay_is_drawn_outside_every_clip_and_the_clip_comes_back() {
    let mut ui = ui("state hot = false\n\
                     clip_push(rect(0, 0, 50, 50))\n\
                     hot = ui_overlay(10, fn()\n\
                       draw_rect(1, 0, 5, 5, #ff0000)\n\
                       ui_hot(rect(200, 200, 50, 50))\n\
                     end)\n\
                     draw_rect(2, 0, 5, 5, #ff0000)\n\
                     clip_pop()");
    ui.mouse_move(210, 210);
    let cmds = ui.frame().unwrap().to_vec();
    assert!(flag(&ui, "hot"), "the overlay's rects are not clipped either");
    let shape: Vec<&str> = cmds
        .iter()
        .map(|c| match c {
            DrawCommand::ClipPush { .. } => "push",
            DrawCommand::ClipPop => "pop",
            DrawCommand::Rect { x: 1, .. } => "overlay",
            DrawCommand::Rect { .. } => "list",
            _ => "?",
        })
        .collect();
    assert_eq!(shape, ["push", "pop", "push", "list", "pop", "overlay"]);
}

/// A tooltip asked for in the middle of a frame still comes out on top.
#[test]
fn a_tooltip_draws_over_what_is_drawn_after_it() {
    let mut ui = ui("tooltip(rect(0, 0, 100, 40), \"hello\", {delay: 0.0})\n\
                     draw_rect(7, 7, 300, 300, #112233)");
    ui.mouse_move(20, 20);
    ui.frame().unwrap();
    ui.frames(20).unwrap();
    let cmds = ui.commands.clone();
    let late = cmds
        .iter()
        .position(|c| matches!(c, DrawCommand::Rect { x: 7, y: 7, .. }))
        .expect("the late rect");
    let tip = cmds
        .iter()
        .position(|c| matches!(c, DrawCommand::Text { text, .. } if text == "hello"))
        .expect("the tooltip's text");
    assert!(tip > late, "tooltip at {tip}, late rect at {late}");
}

// ── Memoized scopes and the frame gate ────────────────────────────────────

/// Rows built on the layered hit test are replayed like rows built on the
/// raw `hovered`: a pointer move that changes no row's answer re-runs none of
/// them, and one that crosses an edge re-runs exactly the two it touches.
#[test]
fn layered_widgets_are_still_replayed_by_the_memo() {
    // The rows sit inside a clip and on a layer their caller set — the usual
    // shape of a scrolled list in a panel — and read both.
    let mut ui = ui("fn row(i)\n\
                       let r = rect(0, i * 20, 100, 20)\n\
                       let c = if ui_hot(r, \"row-{i}\") then #090909 else #010101 end\n\
                       draw_rect(r, c)\n\
                       draw_text(\"row {i}\", 4, i * 20, 12, #ffffff)\n\
                     end\n\
                     let prev = ui_layer(4)\n\
                     clip_push(rect(0, 0, 100, 100))\n\
                     for i in range(0, 5) do row(i) end\n\
                     clip_pop()\n\
                     ui_layer(prev)\n");
    ui.env.set_echo(false);
    ui.set_policy(petal::policy::RunPolicy::REPLAY);
    ui.mouse_move(50, 30); // row 1
    ui.frame().unwrap();
    ui.frame().unwrap();
    ui.frame().unwrap();
    let first = ui.commands.clone();
    let s0 = ui.memo_stats();
    ui.mouse_move(60, 35); // still row 1
    ui.frame().unwrap();
    let s = ui.memo_stats();
    assert_eq!(s.misses - s0.misses, 0, "nothing's answer changed: {s:?}");
    assert!(s.hits - s0.hits >= 5, "every row replayed: {s0:?} then {s:?}");
    assert_eq!(ui.commands, first, "a replayed frame draws what a run would");
    assert_eq!(ui.targets().len(), 5, "and registers what a run would");

    ui.mouse_move(60, 70); // row 3
    ui.frame().unwrap();
    let s2 = ui.memo_stats();
    // The row left and the row entered re-run, and nothing else: a miss is
    // counted per scope, and a row is three deep here (`row`, `ui_hot`, and
    // the prelude function under it), so two rows are six. A row that
    // depended on which *other* row has the pointer would add to that.
    assert_eq!(s2.misses - s.misses, 6, "{s:?} then {s2:?}");
    assert!(s2.hits - s.hits >= 3, "the other three rows replayed");
}

/// The gate still skips a frame nothing changed in, and the registry
/// survives the skip: an empty buffer is "no run", not "nothing registered".
#[test]
fn a_skipped_frame_keeps_the_registry() {
    let mut ui = ui("state captured = false\n\
                     ui_block(rect(0, 0, 100, 100), \"panel\")\n\
                     captured = pointer_captured()");
    ui.mouse_move(50, 50);
    ui.frame().unwrap();
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert!(ui.last_frame_skipped, "a quiet frame is skipped");
    ui.frame().unwrap();
    assert!(ui.target("panel").is_some());
    // The pointer leaves and comes back; the floor follows it.
    ui.mouse_move(500, 500);
    ui.frame().unwrap();
    assert!(!flag(&ui, "captured"));
    ui.mouse_move(50, 50);
    ui.frame().unwrap();
    assert!(flag(&ui, "captured"));
}

/// A panel that opens with the pointer resting under it: the frame after it
/// opens re-runs (the floor moved), so what it covers stops looking hovered
/// without the pointer having to move.
#[test]
fn what_a_new_panel_covers_stops_hovering_without_a_pointer_move() {
    let mut ui = ui(COVERED);
    ui.mouse_move(20, 15);
    ui.frame().unwrap();
    assert!(flag(&ui, "under_hover"));
    ui.key("p").unwrap();
    assert!(flag(&ui, "under_hover"), "the one-frame gap, on the frame it opens");
    ui.frame().unwrap();
    assert!(!ui.last_frame_skipped, "the gate saw the floor move");
    assert!(!flag(&ui, "under_hover"));
}

// ── Measuring and flow layout ─────────────────────────────────────────────

/// One function is the measuring pass and the drawing pass: measured, it
/// draws nothing, registers nothing and is hot nowhere; drawn, it does all
/// three — and reports the same size both times.
#[test]
fn ui_measure_runs_a_panel_without_drawing_or_registering_it() {
    let mut ui = ui("state measured_h = 0\n\
                     state drawn_h = 0\n\
                     state hot_while_measuring = false\n\
                     fn panel(r)\n\
                       flow_begin(r, {pad: 8, gap: 4})\n\
                       var hot = false\n\
                       for i in range(0, 3) do\n\
                         let row = flow_next(20)\n\
                         draw_rect(row, #303030)\n\
                         if ui_hot(row, \"row-{i}\") then set hot = true end\n\
                       end\n\
                       {size: flow_end(), hot: get hot}\n\
                     end\n\
                     let m = ui_measure(fn() panel(rect(0, 0, 200, 0)) end)\n\
                     measured_h = m.size.h\n\
                     hot_while_measuring = m.hot\n\
                     draw_rect(1, 1, 1, 1, #ffffff)\n\
                     let d = panel(rect(300, 100, 200, m.size.h))\n\
                     drawn_h = d.size.h");
    ui.mouse_move(20, 20); // over where the measured copy would have been
    let cmds = ui.frame().unwrap().to_vec();
    // 3 rows of 20 with two gaps of 4, inside 8 of padding.
    assert_eq!(ui.state_int("measured_h"), Some(84));
    assert_eq!(ui.state_int("drawn_h"), Some(84));
    assert!(!flag(&ui, "hot_while_measuring"));
    // Only the marker rect and the drawn copy's rows reached the host.
    assert_eq!(rect_x(&cmds), [1, 308, 308, 308]);
    assert!(!cmds.iter().any(|c| matches!(c, DrawCommand::Mute { .. })));
    // And only the drawn copy's rows are in the registry.
    let rows: Vec<f64> = ui.targets().iter().map(|t| t.x).collect();
    assert_eq!(rows, [308.0, 308.0, 308.0]);
    assert_eq!(ui.target("row-1").unwrap().y, 132.0);
}

#[test]
fn a_flow_hands_out_rects_in_order_and_nests() {
    let mut ui = ui("flow_begin(rect(10, 10, 200, 300), {pad: 5, gap: 2})\n\
                     ui_target(\"title\", flow_next(20))\n\
                     flow_begin(flow_next(24), {axis: \"x\", gap: 6})\n\
                     ui_target(\"ok\", flow_next(50))\n\
                     ui_target(\"cancel\", flow_next(70))\n\
                     let row = flow_end()\n\
                     flow_space(10)\n\
                     ui_target(\"small\", flow_next(40, 12))\n\
                     ui_target(\"rest\", flow_rest())\n\
                     let used = flow_end()\n\
                     state row_w = 0\nstate used_h = 0\n\
                     row_w = row.w\n\
                     used_h = used.h");
    ui.frame().unwrap();
    let at = |ui: &mut Headless, id: &str| {
        let t = ui.target(id).unwrap();
        (t.x, t.y, t.w, t.h)
    };
    assert_eq!(at(&mut ui, "title"), (15.0, 15.0, 190.0, 20.0));
    assert_eq!(at(&mut ui, "ok"), (15.0, 37.0, 50.0, 24.0));
    assert_eq!(at(&mut ui, "cancel"), (71.0, 37.0, 70.0, 24.0));
    assert_eq!(at(&mut ui, "small"), (15.0, 73.0, 40.0, 12.0));
    assert_eq!(at(&mut ui, "rest"), (15.0, 87.0, 190.0, 218.0));
    assert_eq!(ui.state_int("row_w"), Some(126));
    // 20 + 2 + 24 + 10 + 2 + 12 = 70, plus 5 of padding top and bottom.
    assert_eq!(ui.state_int("used_h"), Some(80));
}

#[test]
fn a_flow_call_with_no_flow_open_says_so() {
    let err = ui("flow_next(10)").frame().unwrap_err();
    assert!(err.contains("flow_begin"), "{err}");
}

// ── Scenarios ─────────────────────────────────────────────────────────────

/// A scenario clicks a widget by its id: the position is looked up when the
/// frame is about to run, so the scenario does not know the layout.
#[test]
fn a_scenario_clicks_a_target_by_name() {
    use petal_ui::scenario::Scenario;
    let scenario = Scenario::from_json_str(
        r#"{"events": [{"at": 2, "click_target": "Go"}, {"at": 5, "click_target": "Nope"}]}"#,
    )
    .unwrap();
    // It survives the trip through the normalized JSON a host reads.
    assert_eq!(Scenario::from_json(&scenario.to_json()).unwrap(), scenario);

    let mut ui = ui("state n = 0\nstate held = false\n\
                     if button(rect(500, 400, 80, 24), \"Go\") then n = n + 1 end\n\
                     held = mouse_down(0)");
    let mut missing = Vec::new();
    for frame in 0..7 {
        missing.extend(scenario.apply(&mut ui, frame));
        ui.frame().unwrap();
        if frame == 2 {
            assert!(flag(&ui, "held"), "pressed on its frame");
            assert_eq!((ui.input.mouse_x, ui.input.mouse_y), (540, 412));
        }
        if frame == 3 {
            assert!(!flag(&ui, "held"), "released on the next");
        }
    }
    assert_eq!(ui.state_int("n"), Some(1));
    // The target that is not there is reported, and presses nothing.
    assert_eq!(missing, ["Nope"]);
    assert!(!flag(&ui, "held"));
}

// ── State restore ─────────────────────────────────────────────────────────

/// A host that restores a saved state map into a fresh program before its
/// first frame (Garden's back/forward does) gets a working `ui` module: the
/// module's own per-run cells are restored as cells, and reset as usual.
#[test]
fn a_restored_state_map_leaves_the_module_working() {
    const SRC: &str = "state n = 0\n\
                       n = n + 1\n\
                       with_layer(7, fn() ui_block(rect(0, 0, 50, 50)) end)\n\
                       clip_push(rect(0, 0, 10, 10))";
    let mut a = ui(SRC);
    a.mouse_move(5, 5);
    a.frame().unwrap();
    a.frame().unwrap();
    // Saved mid-flight: a floor raised, a clip left pushed, the layer set.
    let saved = a.state();
    assert_eq!(saved["ui::_floor"], 7);

    let mut b = ui(SRC);
    let (program, stack) = (b.program_id(), b.stack_id());
    b.env.set_state_map_from_json(program, stack, &saved);
    b.frame().unwrap();
    assert_eq!(b.state_int("n"), Some(3), "the app's state was restored");
    // The pointer is at (0, 0) here, off nothing registered last frame: the
    // restored floor did not survive the run's reset.
    b.mouse_move(400, 400);
    b.frame().unwrap();
    assert_eq!(b.state()["ui::_floor"], serde_json::Value::Null);
}

// ── The worked example ────────────────────────────────────────────────────

/// `examples/ui/layers` end to end, driven by name: a click on the world
/// picks a cell, a click on the panel does not, the dialog makes everything
/// under it inert, and a hidden panel gives the world its clicks back.
#[test]
fn the_layers_example_keeps_ui_clicks_from_the_world() {
    use petal_ui::scenario::Scenario;
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ui/layers/app.ptl");
    let mut ui = Headless::from_file_with_size(&app, 800, 600)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    let scenario = Scenario::from_json_str(
        r#"{"events": [
            {"at": 2,  "click": [100, 100]},
            {"at": 5,  "click_target": "Palette"},
            {"at": 8,  "click_target": "panel"},
            {"at": 11, "click_target": "confirm"},
            {"at": 14, "click": [100, 100]},
            {"at": 17, "click_target": "confirm"},
            {"at": 20, "click_target": "clear"},
            {"at": 23, "click_target": "toggle-panel"},
            {"at": 26, "click": [400, 120]}
        ]}"#,
    )
    .unwrap();
    let state = |ui: &Headless, name: &str| ui.state()[name].clone();
    for frame in 0..30 {
        let missing = scenario.apply(&mut ui, frame);
        assert!(missing.is_empty(), "frame {frame}: no target {missing:?}");
        ui.frame().unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        match frame {
            3 => {
                assert_eq!(state(&ui, "world_clicks"), 1, "a click on the world");
                assert_ne!(state(&ui, "picked"), -1);
            }
            6 => assert_eq!(state(&ui, "palette_ix"), 1, "the button, by name"),
            9 => {
                // The panel lies over the grid: the click was on a cell, too.
                let panel = ui.target("panel").unwrap().center();
                assert!(panel.0 < 24 + 32 * 14 && panel.1 < 24 + 20 * 14);
                assert_eq!(state(&ui, "world_clicks"), 1, "the panel took the click");
            }
            12 => assert_eq!(state(&ui, "md")["open"], true),
            15 => {
                assert_eq!(state(&ui, "md")["open"], false, "a click outside closes it");
                assert_eq!(state(&ui, "world_clicks"), 1, "and is not a world click");
            }
            21 => {
                assert_eq!(state(&ui, "cleared"), 1, "the dialog's own button works");
                assert_eq!(state(&ui, "picked"), -1);
            }
            24 => assert_eq!(state(&ui, "panel_open"), false),
            27 => assert_eq!(state(&ui, "world_clicks"), 2, "where the panel was is world again"),
            _ => {}
        }
    }
    // The grid is recorded once per palette: one palette change, two bodies.
    let grid_rects = ui
        .commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Rect { w: 13, h: 13, .. }))
        .count();
    assert_eq!(grid_rects, 640, "the retained grid is drawn every frame");
}
