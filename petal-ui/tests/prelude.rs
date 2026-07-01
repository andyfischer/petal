//! Behavioral tests for the `ui` prelude module, driven headlessly through
//! the standard host frame contract. The interaction tests run under both
//! backends (graph + bytecode) via `both_backends`.

use petal::backend::Backend;
use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;

fn both_backends(source: &str, check: impl Fn(&mut Headless)) {
    for backend in [Backend::Graph, Backend::Bytecode] {
        let mut ui = Headless::new(source)
            .unwrap_or_else(|e| panic!("compile failed: {e}"))
            .with_backend(backend);
        check(&mut ui);
    }
}

#[test]
fn hovered_and_clicked_edges() {
    let src = "state hovers = 0\n\
               state hits = 0\n\
               let r = {x: 100, y: 100, w: 50, h: 20}\n\
               if hovered(r) then hovers = hovers + 1 end\n\
               if clicked(r) then hits = hits + 1 end";
    both_backends(src, |ui| {
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
    both_backends(src, |ui| {
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
    both_backends(src, |ui| {
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
    both_backends(LIST_SRC, |ui| {
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
    both_backends(LIST_SRC, |ui| {
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
fn list_wheel_scrolls_when_hovered_and_clamps() {
    both_backends(LIST_SRC, |ui| {
        ui.frame().unwrap();
        // Wheel outside the list: no scroll.
        ui.mouse_move(300, 300);
        ui.scroll(2.0);
        ui.frame().unwrap();
        assert_eq!(state_field(ui, "scroll"), Some(0));

        // Wheel over the list scrolls, but never past the end and never off
        // the selection (selection stays visible).
        ui.mouse_move(50, 150);
        ui.scroll(3.0);
        ui.frame().unwrap();
        // Selection 0 must stay visible: scroll snaps back to 0.
        assert_eq!(state_field(ui, "scroll"), Some(0));

        // Move the selection down first, then wheel scrolling has room.
        for _ in 0..10 {
            ui.key("j").unwrap();
        }
        assert_eq!(state_field(ui, "selected"), Some(10));
        assert_eq!(state_field(ui, "scroll"), Some(6));
    });
}

#[test]
fn scroll_update_wheel_pages_and_clamps() {
    let src = "state off = 0\n\
               let r = {x: 0, y: 0, w: 100, h: 100}\n\
               off = scroll_update(off, 50, 10, r)";
    both_backends(src, |ui| {
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
    both_backends(src, |ui| {
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
    both_backends(src, |ui| {
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
    both_backends(src, |ui| {
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
    both_backends(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state()["inside"], true);
    });

    let src2 = "import ui: truncate_tail\n\
                state t = \"\"\n\
                t = truncate_tail(\"hello world\", 6)";
    both_backends(src2, |ui| {
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
    both_backends(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("got"), Some(42));
    });
}

/// Read a field of the `lst` state record.
fn state_field(ui: &Headless, field: &str) -> Option<i64> {
    ui.state().get("lst")?.get(field)?.as_i64()
}
