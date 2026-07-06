//! Behavioral tests for the `ui` prelude module, driven headlessly through
//! the standard host frame contract.

use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;
use petal_ui::input::{InputEvent, Modifiers};

fn run_headless(source: &str, check: impl Fn(&mut Headless)) {
    let mut ui = Headless::new(source).unwrap_or_else(|e| panic!("compile failed: {e}"));
    check(&mut ui);
}

#[test]
fn hovered_and_clicked_edges() {
    let src = "state hovers = 0\n\
               state hits = 0\n\
               let r = {x: 100, y: 100, w: 50, h: 20}\n\
               if hovered(r) then hovers = hovers + 1 end\n\
               if clicked(r) then hits = hits + 1 end";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("hovers"), Some(0));

        ui.mouse_move(120, 110);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("hovers"), Some(1));
        assert_eq!(ui.state_int("hits"), Some(0), "hover alone is not a click");

        ui.click(120, 110).unwrap();
        assert_eq!(ui.state_int("hits"), Some(1));

        // The press edge lasts exactly one frame.
        ui.frame().unwrap();
        assert_eq!(ui.state_int("hits"), Some(1));

        // A click outside the rect misses.
        ui.click(10, 10).unwrap();
        assert_eq!(ui.state_int("hits"), Some(1));
    });
}

#[test]
fn button_draws_and_reports_click() {
    let src = "state pressed = 0\n\
               if button({x: 10, y: 10, w: 100, h: 30}, \"OK\") then\n\
                 pressed = pressed + 1\n\
               end";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap();
        assert!(
            cmds.iter().any(|c| matches!(c, DrawCommand::Rect { x: 10, y: 10, w: 100, h: 30, .. })),
            "button draws its background rect: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| matches!(c, DrawCommand::Text { text, .. } if text == "OK")),
            "button draws its label: {cmds:?}"
        );
        assert_eq!(ui.state_int("pressed"), Some(0));

        ui.click(50, 25).unwrap();
        assert_eq!(ui.state_int("pressed"), Some(1));
    });
}

#[test]
fn record_draw_overloads_emit_flat_commands() {
    // Both the record form and the flat form work with the prelude loaded.
    let src = "draw_rect({x: 1, y: 2, w: 3, h: 4}, #ff8800)\n\
               draw_rect(5, 6, 7, 8, 10, 20, 30)\n\
               draw_text(\"hi\", {x: 9, y: 9}, 14, {r: 1, g: 2, b: 3})";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert_eq!(
            cmds[0],
            DrawCommand::Rect { x: 1, y: 2, w: 3, h: 4, r: 0xff, g: 0x88, b: 0x00 }
        );
        assert_eq!(
            cmds[1],
            DrawCommand::Rect { x: 5, y: 6, w: 7, h: 8, r: 10, g: 20, b: 30 }
        );
        assert_eq!(
            cmds[2],
            DrawCommand::Text { text: "hi".into(), x: 9, y: 9, size: 14, r: 1, g: 2, b: 3 }
        );
    });
}

const LIST_SRC: &str = "state lst = list_state()\n\
                        let r = {x: 0, y: 100, w: 200, h: 100}\n\
                        lst = list_update(lst, 20, 5, r)";

#[test]
fn list_keyboard_navigation_and_clamping() {
    run_headless(LIST_SRC, |ui| {
        ui.frame().unwrap();
        assert_eq!(state_field(ui, "selected"), Some(0));

        ui.key("j").unwrap();
        ui.key("down").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(2));
        assert_eq!(state_field(ui, "scroll"), Some(0), "still within the window");

        // Up past the top clamps at 0.
        ui.key("k").unwrap();
        ui.key("up").unwrap();
        ui.key("up").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(0));

        // End clamps to the last item and scrolls it into view.
        ui.key("end").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(19));
        assert_eq!(state_field(ui, "scroll"), Some(15), "19 visible in a 5-row window");

        ui.key("pageup").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(14));

        ui.key("home").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(0));
        assert_eq!(state_field(ui, "scroll"), Some(0), "scroll follows the selection up");
    });
}

#[test]
fn list_click_selects_row_under_pointer() {
    run_headless(LIST_SRC, |ui| {
        ui.frame().unwrap();
        // Rows are 20px tall (h 100 / 5 rows) starting at y=100; row 3 spans
        // y 160..180.
        ui.click(50, 165).unwrap();
        assert_eq!(state_field(ui, "selected"), Some(3));

        // A click outside the list rect changes nothing.
        ui.click(50, 20).unwrap();
        assert_eq!(state_field(ui, "selected"), Some(3));
    });
}

#[test]
fn list_wheel_scrolls_freely_and_keyboard_re_ensures_selection() {
    run_headless(LIST_SRC, |ui| {
        ui.frame().unwrap();
        // Wheel outside the list: no scroll.
        ui.mouse_move(300, 300);
        ui.scroll(2.0);
        ui.frame().unwrap();
        assert_eq!(state_field(ui, "scroll"), Some(0));

        // Wheel over the list moves the window even though selection 0 scrolls
        // off-screen — the window is NOT clamped to the selection (like gitk /
        // native list widgets). No snap-back to the selection.
        ui.mouse_move(50, 150);
        ui.scroll(3.0);
        ui.frame().unwrap();
        assert_eq!(state_field(ui, "scroll"), Some(3), "wheel scrolls freely");
        assert_eq!(state_field(ui, "selected"), Some(0), "wheel leaves the selection alone");

        // The wheel is still clamped to [0, item_count - visible_rows].
        ui.scroll(100.0);
        ui.frame().unwrap();
        assert_eq!(state_field(ui, "scroll"), Some(15), "20 items, 5 visible → max 15");

        // Keyboard navigation DOES re-ensure the selection: 'down' moves sel to
        // 1 and pulls the window back so the selection is visible again.
        ui.key("down").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(1));
        assert_eq!(state_field(ui, "scroll"), Some(1), "keyboard pulls the window to the selection");
    });
}

#[test]
fn scroll_update_wheel_pages_and_clamps() {
    let src = "state off = 0\n\
               let r = {x: 0, y: 0, w: 100, h: 100}\n\
               off = scroll_update(off, 50, 10, r)";
    run_headless(src, |ui| {
        ui.mouse_move(50, 50);
        ui.scroll(5.0);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("off"), Some(5));

        ui.key("pagedown").unwrap();
        assert_eq!(ui.state_int("off"), Some(15));

        ui.scroll(100.0);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("off"), Some(40), "clamped to total - visible");

        for _ in 0..6 {
            ui.key("pageup").unwrap();
        }
        assert_eq!(ui.state_int("off"), Some(0), "clamped at 0");
    });
}

#[test]
fn truncate_helpers() {
    let src = "state tail = \"\"\n\
               state head = \"\"\n\
               state short = \"\"\n\
               tail = truncate_tail(\"src/app/mouse.rs\", 8)\n\
               head = truncate_head(\"src/app/mouse.rs\", 8)\n\
               short = truncate_tail(\"abc\", 8)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        // The ellipsis counts toward max_chars: results are exactly 8 chars.
        assert_eq!(st["tail"], "…ouse.rs");
        assert_eq!(st["head"], "src/app…");
        assert_eq!(st["short"], "abc");
    });
}

#[test]
fn released_edges_and_ui_version() {
    let src = "state releases = 0\n\
               state version = 0\n\
               version = ui_version()\n\
               if mouse_released(0) then releases = releases + 1 end\n\
               if key_released(\"a\") then releases = releases + 10 end";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("version"), Some(petal_ui::UI_VERSION));

        ui.mouse_down(0);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("releases"), Some(0));
        ui.mouse_up(0);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("releases"), Some(1));

        ui.key("a").unwrap(); // down+up in one frame: released fires with it
        assert_eq!(ui.state_int("releases"), Some(11));
    });
}

#[test]
fn drag_and_click_count_reach_scripts() {
    let src = "state dragging = false\n\
               state from_x = -1\n\
               state doubles = 0\n\
               if drag_active() then\n\
                 dragging = true\n\
                 from_x = drag_start_x()\n\
               end\n\
               if click_count() == 2 then doubles = doubles + 1 end";
    run_headless(src, |ui| {
        ui.mouse_move(100, 100);
        ui.mouse_down(0);
        ui.frame().unwrap();
        assert_eq!(ui.state()["dragging"], false);

        ui.mouse_move(120, 100);
        ui.frame().unwrap();
        assert_eq!(ui.state()["dragging"], true);
        assert_eq!(ui.state_int("from_x"), Some(100));

        ui.mouse_up(0);
        ui.frames(2).unwrap();

        // Two quick clicks in place → click_count() == 2 on the second.
        ui.mouse_down(0);
        ui.frame().unwrap();
        ui.mouse_up(0);
        ui.frame().unwrap();
        ui.mouse_down(0);
        ui.frame().unwrap();
        ui.mouse_up(0);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("doubles"), Some(1));
    });
}

#[test]
fn explicit_and_selective_imports_also_work() {
    // The prelude is a real module: qualified and selective access work too.
    // (Qualified access reaches module exports; natives like text_width stay
    // global.)
    let src = "import ui as u\n\
               state inside = false\n\
               inside = u.point_in(5, 5, u.rect(0, 0, 10, 10))";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state()["inside"], true);
    });

    let src2 = "import ui: truncate_tail\n\
                state t = \"\"\n\
                t = truncate_tail(\"hello world\", 6)";
    run_headless(src2, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state()["t"], "…world");
    });
}

#[test]
fn scripts_shadow_prelude_names() {
    // Implicit imports are weak: a script's own `button` wins silently.
    let src = "fn button(a, b) 42 end\n\
               state got = 0\n\
               got = button(1, 2)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("got"), Some(42));
    });
}

/// Read a field of the `lst` state record.
fn state_field(ui: &Headless, field: &str) -> Option<i64> {
    ui.state().get("lst")?.get(field)?.as_i64()
}

// ── Focus registry ──────────────────────────────────────────────────────────

#[test]
fn focus_set_focused_and_clear() {
    let src = "state fc = focus_state()\n\
               state none = true\n\
               state a = false\n\
               state b = false\n\
               none = focused(fc, \"a\")\n\
               fc = focus_set(fc, \"a\")\n\
               a = focused(fc, \"a\")\n\
               b = focused(fc, \"b\")";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        // Nothing focused by default.
        assert_eq!(ui.state()["none"], false);
        // After focus_set("a"), a is focused and b is not.
        assert_eq!(ui.state()["a"], true);
        assert_eq!(ui.state()["b"], false);

        // Clearing focus un-focuses everything (survives across frames via state).
        let mut ui2 = Headless::new(
            "state fc = focus_set(focus_state(), \"a\")\n\
             state cleared = true\n\
             fc = focus_clear(fc)\n\
             cleared = focused(fc, \"a\")",
        )
        .unwrap();
        ui2.frame().unwrap();
        assert_eq!(ui2.state()["cleared"], false);
        let _ = ui;
    });
}

const CYCLE_SRC: &str = "state fc = focus_state()\n\
                         let ids = [\"a\", \"b\", \"c\"]\n\
                         if key_pressed(\"n\") then fc = focus_next(fc, ids) end\n\
                         if key_pressed(\"p\") then fc = focus_prev(fc, ids) end\n\
                         state cur = \"\"\n\
                         cur = fc.id";

#[test]
fn focus_next_and_prev_cycle_and_wrap() {
    run_headless(CYCLE_SRC, |ui| {
        // From nothing, next → first, prev → last.
        ui.key("n").unwrap();
        assert_eq!(ui.state()["cur"], "a");
        ui.key("n").unwrap();
        assert_eq!(ui.state()["cur"], "b");
        ui.key("n").unwrap();
        assert_eq!(ui.state()["cur"], "c");
        // Wrap forward.
        ui.key("n").unwrap();
        assert_eq!(ui.state()["cur"], "a");
        // Wrap backward past the first.
        ui.key("p").unwrap();
        assert_eq!(ui.state()["cur"], "c");
        ui.key("p").unwrap();
        assert_eq!(ui.state()["cur"], "b");
    });

    // prev from nothing focused → last.
    run_headless(CYCLE_SRC, |ui| {
        ui.key("p").unwrap();
        assert_eq!(ui.state()["cur"], "c");
    });
}

#[test]
fn focus_update_reads_tab_and_shift_tab() {
    let src = "state fc = focus_state()\n\
               let ids = [\"a\", \"b\", \"c\"]\n\
               fc = focus_update(fc, ids)\n\
               state cur = \"\"\n\
               cur = fc.id";
    run_headless(src, |ui| {
        // A frame with no tab leaves focus untouched.
        ui.frame().unwrap();
        assert_eq!(ui.state()["cur"], "");

        // Tab advances.
        ui.key("tab").unwrap();
        assert_eq!(ui.state()["cur"], "a");
        ui.key("tab").unwrap();
        assert_eq!(ui.state()["cur"], "b");

        // Shift+Tab goes back.
        ui.event(InputEvent::Modifiers(Modifiers { shift: true, ..Default::default() }));
        ui.key("tab").unwrap();
        assert_eq!(ui.state()["cur"], "a");
        ui.event(InputEvent::Modifiers(Modifiers::default()));
    });
}

// ── Text field widget ───────────────────────────────────────────────────────

const FIELD_SRC: &str = "state fc = focus_state()\n\
                         state buf = \"\"\n\
                         state submits = 0\n\
                         let r = {x: 10, y: 10, w: 200, h: 24}\n\
                         let res = text_field(fc, \"name\", r, buf)\n\
                         fc = res.focus\n\
                         buf = res.text\n\
                         if res.submitted then submits = submits + 1 end";

#[test]
fn text_field_ignores_typing_until_focused() {
    run_headless(FIELD_SRC, |ui| {
        // Typing before focus is dropped.
        ui.text("hi").unwrap();
        assert_eq!(ui.state()["buf"], "");

        // Click focuses the field.
        ui.click(50, 20).unwrap();
        assert_eq!(ui.state().get("fc").unwrap()["id"], "name");

        // Now typing lands in the buffer.
        ui.text("hel").unwrap();
        ui.text("lo").unwrap();
        assert_eq!(ui.state()["buf"], "hello");
    });
}

#[test]
fn text_field_backspace_and_submit() {
    run_headless(FIELD_SRC, |ui| {
        ui.click(50, 20).unwrap();
        ui.text("abc").unwrap();
        assert_eq!(ui.state()["buf"], "abc");

        ui.key("backspace").unwrap();
        assert_eq!(ui.state()["buf"], "ab");

        // Backspace on empty is a no-op (no underflow).
        ui.key("backspace").unwrap();
        ui.key("backspace").unwrap();
        ui.key("backspace").unwrap();
        assert_eq!(ui.state()["buf"], "");

        // Return submits.
        ui.text("x").unwrap();
        ui.key("return").unwrap();
        assert_eq!(ui.state_int("submits"), Some(1));
    });
}

#[test]
fn text_field_draws_caret_only_when_focused() {
    // A focused field draws a caret line after its text; an unfocused one
    // draws no line.
    run_headless(FIELD_SRC, |ui| {
        let cmds = ui.frame().unwrap();
        assert!(
            !cmds.iter().any(|c| matches!(c, DrawCommand::Line { .. })),
            "unfocused field has no caret: {cmds:?}"
        );

        ui.click(50, 20).unwrap();
        let cmds = ui.frame().unwrap();
        assert!(
            cmds.iter().any(|c| matches!(c, DrawCommand::Line { .. })),
            "focused field draws a caret: {cmds:?}"
        );
    });
}

// ── Focus-gated list ────────────────────────────────────────────────────────

const GATED_LIST_SRC: &str = "state lst = list_state()\n\
                              state fc = focus_state()\n\
                              let r = {x: 0, y: 100, w: 200, h: 100}\n\
                              if mouse_pressed(0) && point_in(mouse_x(), mouse_y(), r) then\n\
                                fc = focus_set(fc, \"list\")\n\
                              end\n\
                              lst = list_update(lst, 20, 5, r, focused(fc, \"list\"))";

#[test]
fn focus_gated_list_ignores_keys_until_focused() {
    run_headless(GATED_LIST_SRC, |ui| {
        ui.frame().unwrap();
        // Keyboard nav is inert while the list is unfocused.
        ui.key("j").unwrap();
        ui.key("j").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(0));

        // Clicking a row both focuses the list and selects that row.
        // Rows are 20px tall from y=100; row 2 spans y 140..160.
        ui.click(50, 145).unwrap();
        assert_eq!(state_field(ui, "selected"), Some(2));
        assert_eq!(ui.state().get("fc").unwrap()["id"], "list");

        // Now keyboard nav works.
        ui.key("j").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(3));
    });
}
