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
fn time_and_elapsed() {
    // `time()` reflects the host clock; `elapsed()` measures from its first call.
    let src = "state t = 0.0\n\
               state e = 0.0\n\
               t = time()\n\
               e = elapsed()";
    let mut ui = Headless::new(src).unwrap_or_else(|e| panic!("compile failed: {e}"));

    ui.time = 10.0;
    ui.frame().unwrap();
    assert_eq!(
        ui.state_float("t"),
        Some(10.0),
        "time() reads the host clock"
    );
    assert_eq!(
        ui.state_float("e"),
        Some(0.0),
        "elapsed() is 0 on its first call"
    );

    ui.time = 12.5;
    ui.frame().unwrap();
    assert_eq!(ui.state_float("t"), Some(12.5));
    assert_eq!(
        ui.state_float("e"),
        Some(2.5),
        "elapsed() = now - first-call clock, immune to dt summation"
    );

    ui.time = 100.0;
    ui.frame().unwrap();
    assert_eq!(ui.state_float("e"), Some(90.0));
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
            cmds.iter().any(|c| matches!(
                c,
                DrawCommand::Rect {
                    x: 10,
                    y: 10,
                    w: 100,
                    h: 30,
                    ..
                }
            )),
            "button draws its background rect: {cmds:?}"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DrawCommand::Text { text, .. } if text == "OK")),
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
            DrawCommand::Rect {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
                r: 0xff,
                g: 0x88,
                b: 0x00,
                a: 255,
                radius: 0
            }
        );
        assert_eq!(
            cmds[1],
            DrawCommand::Rect {
                x: 5,
                y: 6,
                w: 7,
                h: 8,
                r: 10,
                g: 20,
                b: 30,
                a: 255,
                radius: 0
            }
        );
        assert_eq!(
            cmds[2],
            DrawCommand::Text {
                text: "hi".into(),
                x: 9,
                y: 9,
                size: 14,
                r: 1,
                g: 2,
                b: 3,
                a: 255,
                font: None,
                weight: petal_ui::draw::REGULAR_WEIGHT,
                italic: false,
                spacing: 0.0,
            }
        );
    });
}

#[test]
fn styled_draw_text_reaches_the_command_stream_through_the_prelude() {
    // The prelude shadows `draw_text`, so the styled overload has to be there
    // too — otherwise a style record would be swallowed by the (text, pos,
    // size, color) arity and never reach the native.
    let src = "let BODY = {size: 15, color: {r: 200, g: 200, b: 200}, font: \"ui\"}\n\
               draw_text(\"regular\", {x: 2, y: 4}, BODY)\n\
               draw_text(\"bold\", {x: 2, y: 24}, {...BODY, weight: 700, italic: true})";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        match &cmds[0] {
            DrawCommand::Text {
                text,
                size,
                font,
                weight,
                italic,
                ..
            } => {
                assert_eq!((text.as_str(), *size), ("regular", 15));
                assert_eq!(font.as_deref(), Some("ui"));
                assert_eq!((*weight, *italic), (petal_ui::draw::REGULAR_WEIGHT, false));
            }
            other => panic!("expected a text command, got {other:?}"),
        }
        // Spread over a base style is the intended way to derive emphasis.
        match &cmds[1] {
            DrawCommand::Text {
                text,
                font,
                weight,
                italic,
                ..
            } => {
                assert_eq!(text, "bold");
                assert_eq!((font.as_deref(), *weight, *italic), (Some("ui"), 700, true));
            }
            other => panic!("expected a text command, got {other:?}"),
        }
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
        assert_eq!(
            state_field(ui, "scroll"),
            Some(0),
            "still within the window"
        );

        // Up past the top clamps at 0.
        ui.key("k").unwrap();
        ui.key("up").unwrap();
        ui.key("up").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(0));

        // End clamps to the last item and scrolls it into view.
        ui.key("end").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(19));
        assert_eq!(
            state_field(ui, "scroll"),
            Some(15),
            "19 visible in a 5-row window"
        );

        ui.key("pageup").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(14));

        ui.key("home").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(0));
        assert_eq!(
            state_field(ui, "scroll"),
            Some(0),
            "scroll follows the selection up"
        );
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
        assert_eq!(
            state_field(ui, "selected"),
            Some(0),
            "wheel leaves the selection alone"
        );

        // The wheel is still clamped to [0, item_count - visible_rows].
        ui.scroll(100.0);
        ui.frame().unwrap();
        assert_eq!(
            state_field(ui, "scroll"),
            Some(15),
            "20 items, 5 visible → max 15"
        );

        // Keyboard navigation DOES re-ensure the selection: 'down' moves sel to
        // 1 and pulls the window back so the selection is visible again.
        ui.key("down").unwrap();
        assert_eq!(state_field(ui, "selected"), Some(1));
        assert_eq!(
            state_field(ui, "scroll"),
            Some(1),
            "keyboard pulls the window to the selection"
        );
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
fn scroll_update_gates_page_keys_when_inactive() {
    // The focus-gated overload keeps the wheel hover-scoped but only pages on
    // pageup/pagedown when `active` — so PageDown in the focused region does not
    // leak into every other scroll region on screen.
    let src = "state off = 0\n\
               let r = {x: 0, y: 0, w: 100, h: 100}\n\
               off = scroll_update(off, 50, 10, r, false)";
    run_headless(src, |ui| {
        // Wheel still scrolls even when inactive (hover-scoped, not gated).
        ui.mouse_move(50, 50);
        ui.scroll(5.0);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("off"), Some(5), "wheel is not gated by focus");

        // PageDown is ignored while inactive — no leak across regions.
        ui.key("pagedown").unwrap();
        assert_eq!(
            ui.state_int("off"),
            Some(5),
            "page keys are gated off when inactive"
        );
    });
}

#[test]
fn scroll_update_pages_when_active() {
    let src = "state off = 0\n\
               let r = {x: 0, y: 0, w: 100, h: 100}\n\
               off = scroll_update(off, 50, 10, r, true)";
    run_headless(src, |ui| {
        ui.key("pagedown").unwrap();
        assert_eq!(
            ui.state_int("off"),
            Some(10),
            "active region pages on PageDown"
        );
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
fn wrap_basic_and_word_boundaries() {
    let src = "state short = \"\"\n\
               state multi = \"\"\n\
               state exact = \"\"\n\
               short = join(wrap(\"hello\", 10), \"|\")\n\
               multi = join(wrap(\"the quick brown fox\", 9), \"|\")\n\
               exact = join(wrap(\"abc def\", 3), \"|\")";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["short"], "hello", "a string that fits is one line");
        assert_eq!(st["multi"], "the quick|brown fox");
        assert_eq!(st["exact"], "abc|def", "word exactly max_chars wide");
    });
}

#[test]
fn wrap_hard_breaks_long_tokens() {
    let src = "state lone = \"\"\n\
               state mixed = \"\"\n\
               lone = join(wrap(\"abcdefgh\", 3), \"|\")\n\
               mixed = join(wrap(\"x abcdefgh yz\", 4), \"|\")";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["lone"], "abc|def|gh");
        // The oversized token starts fresh lines; its remainder takes the
        // following word when it fits.
        assert_eq!(st["mixed"], "x|abcd|efgh|yz");
    });
}

#[test]
fn wrap_unicode_multibyte_is_safe() {
    // len/slice are byte-based; slice snaps to UTF-8 char boundaries so
    // multibyte input must not panic (lines may run short in chars).
    let src = "state uni = \"\"\n\
               state glued = \"\"\n\
               let lines = wrap(\"héllo wörld ünïcödé\", 8)\n\
               uni = join(lines, \"|\")\n\
               glued = join(lines, \"\")";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["uni"], "héllo|wörld|ünïcö|dé");
        // No content lost: lines minus the breaks reconstruct the input
        // minus its (consumed) word separators.
        assert_eq!(st["glued"], "héllowörldünïcödé");
    });
}

#[test]
fn preview_short_input_is_one_untruncated_line() {
    let src = "state lines = \"\"\n\
               state count = 0\n\
               state trunc = true\n\
               let pv = preview(\"hello world\", 20, 3)\n\
               lines = join(pv.lines, \"|\")\n\
               count = len(pv.lines)\n\
               trunc = pv.truncated";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["lines"], "hello world");
        assert_eq!(ui.state_int("count"), Some(1));
        assert_eq!(st["trunc"], false, "a string that fits is not truncated");
    });
}

#[test]
fn preview_wraps_to_max_lines_with_trailing_ellipsis() {
    // Six 3-char words wrap 2-per-line at width 7; capped at 2 lines, so the
    // second kept line ends in an explicit "…".
    let src = "state lines = \"\"\n\
               state count = 0\n\
               state trunc = false\n\
               let pv = preview(\"aaa bbb ccc ddd eee fff\", 7, 2)\n\
               lines = join(pv.lines, \"|\")\n\
               count = len(pv.lines)\n\
               trunc = pv.truncated";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["lines"], "aaa bbb|ccc dd…");
        assert_eq!(ui.state_int("count"), Some(2), "capped at max_lines");
        assert_eq!(st["trunc"], true);
    });
}

#[test]
fn preview_cost_is_bounded_by_the_clip_window() {
    // The input is pre-clipped to (max_lines+1)*max_chars = 30 bytes before
    // wrapping, so a marker sitting far beyond that window can never appear in
    // the result no matter how long the input is.
    let src = "state lines = \"\"\n\
               state count = 0\n\
               let long = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaDEEPMARKER\"\n\
               let pv = preview(long, 10, 2)\n\
               lines = join(pv.lines, \"|\")\n\
               count = len(pv.lines)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert!(
            !st["lines"].as_str().unwrap().contains("DEEPMARKER"),
            "content beyond the clip window is never wrapped: {:?}",
            st["lines"]
        );
        assert_eq!(
            ui.state_int("count"),
            Some(2),
            "a huge input still returns at most max_lines"
        );
    });
}

#[test]
fn preview_max_lines_one() {
    // max_lines = 1: a wrapping input collapses to a single ellipsised line.
    let src = "state lines = \"\"\n\
               state count = 0\n\
               state trunc = false\n\
               let pv = preview(\"aaa bbb ccc\", 7, 1)\n\
               lines = join(pv.lines, \"|\")\n\
               count = len(pv.lines)\n\
               trunc = pv.truncated";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["lines"], "aaa bb…");
        assert_eq!(ui.state_int("count"), Some(1));
        assert_eq!(st["trunc"], true);
    });
}

#[test]
fn preview_tiny_max_chars_is_safe() {
    // The callers guard max_chars with max(8, …); still, a tiny positive width
    // must neither panic nor exceed max_lines (hard-broken tokens included).
    let src = "state count = 0\n\
               state trunc = false\n\
               let pv = preview(\"hello world foo\", 3, 2)\n\
               count = len(pv.lines)\n\
               trunc = pv.truncated";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert!(
            ui.state_int("count").unwrap() <= 2,
            "never exceeds max_lines"
        );
        assert_eq!(st["trunc"], true);
    });
}

// ── fit_parts ─────────────────────────────────────────────────────────────
// text_width in the headless harness is round(chars × size × 0.6). At size 10
// each char is 6px; the "   ·   " separator is 7 chars = 42px. Three 3-char
// parts: "aaa" = 18px, "aaa   ·   bbb" = 78px, full join = 138px.

#[test]
fn fit_parts_all_segments_fit() {
    let src = "state text = \"\"\n\
               state count = 0\n\
               let r = fit_parts_n([\"aaa\", \"bbb\", \"ccc\"], 200, 10)\n\
               text = r.text\n\
               count = r.count";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["text"], "aaa   ·   bbb   ·   ccc");
        assert_eq!(ui.state_int("count"), Some(3));
    });
}

#[test]
fn fit_parts_drops_rightmost_until_it_fits() {
    // avail 100px: "aaa   ·   bbb" (78) fits, the full join (138) does not, so
    // the third segment is dropped.
    let src = "state text = \"\"\n\
               state count = 0\n\
               let r = fit_parts_n([\"aaa\", \"bbb\", \"ccc\"], 100, 10)\n\
               text = r.text\n\
               count = r.count\n\
               state plain = \"\"\n\
               plain = fit_parts([\"aaa\", \"bbb\", \"ccc\"], 100, 10)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["text"], "aaa   ·   bbb");
        assert_eq!(ui.state_int("count"), Some(2));
        // fit_parts is the plain-text convenience over fit_parts_n.
        assert_eq!(st["plain"], "aaa   ·   bbb");
    });
}

#[test]
fn fit_parts_single_segment_too_long_is_dropped() {
    // A first segment that already overflows yields nothing (retro.ptl drops it
    // rather than truncating — the value survives in the detail panel).
    let src = "state text = \"x\"\n\
               state count = -1\n\
               let r = fit_parts_n([\"aaaaaaaaaa\"], 30, 10)\n\
               text = r.text\n\
               count = r.count";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["text"], "");
        assert_eq!(ui.state_int("count"), Some(0));
    });
}

#[test]
fn fit_parts_empty_list() {
    let src = "state text = \"x\"\n\
               state count = -1\n\
               let r = fit_parts_n([], 100, 10)\n\
               text = r.text\n\
               count = r.count";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["text"], "");
        assert_eq!(ui.state_int("count"), Some(0));
    });
}

// ── ensure_visible_px ───────────────────────────────────────────────────────

#[test]
fn ensure_visible_px_keeps_selected_row_in_view() {
    // A viewport 200px tall over rows of assorted heights.
    let src = "state above = 0\n\
               state below = 0\n\
               state stay = 0\n\
               state top_first = 0\n\
               state bottom_last = 0\n\
               // selection above the viewport scrolls up to its offset\n\
               above = ensure_visible_px(300, 100, 80, 200)\n\
               // selection below the viewport scrolls down to reveal its bottom\n\
               below = ensure_visible_px(0, 500, 80, 200)\n\
               // an already-visible row leaves scroll untouched\n\
               stay = ensure_visible_px(50, 100, 80, 200)\n\
               // the first row (offset 0) pins the top at 0\n\
               top_first = ensure_visible_px(120, 0, 80, 200)\n\
               // a last row whose bottom is past the viewport scrolls to show it\n\
               bottom_last = ensure_visible_px(0, 920, 80, 200)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("above"), Some(100), "scroll up to the row top");
        assert_eq!(
            ui.state_int("below"),
            Some(380),
            "scroll down so 500+80 = 580 is the viewport bottom (580-200)"
        );
        assert_eq!(ui.state_int("stay"), Some(50), "already visible: unchanged");
        assert_eq!(ui.state_int("top_first"), Some(0), "first row pins top");
        assert_eq!(ui.state_int("bottom_last"), Some(800), "920+80-200");
    });
}

#[test]
fn ensure_visible_px_row_taller_than_viewport_pins_top() {
    // A 300px row in a 200px viewport can't fit; ensure-visible pins its top so
    // reading starts from the beginning (the overflow runs off the bottom).
    let src = "state up = 0\n\
               state down = 0\n\
               up = ensure_visible_px(350, 100, 300, 200)\n\
               down = ensure_visible_px(0, 100, 300, 200)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("up"), Some(100), "scrolling up pins the top");
        assert_eq!(
            ui.state_int("down"),
            Some(100),
            "scrolling down pins the top"
        );
    });
}

#[test]
fn wrap_edge_cases_and_newlines() {
    let src = "state nempty = 0\n\
               state empty = \"\"\n\
               state zero = \"\"\n\
               state paras = \"\"\n\
               let e = wrap(\"\", 5)\n\
               nempty = len(e)\n\
               empty = join(e, \"|\")\n\
               zero = join(wrap(\"hello world\", 0), \"|\")\n\
               paras = join(wrap(\"ab cd\\nef\\n\\ngh\", 5), \"|\")";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(
            ui.state_int("nempty"),
            Some(1),
            "empty string is one empty line"
        );
        assert_eq!(st["empty"], "");
        assert_eq!(st["zero"], "hello world", "max_chars <= 0 returns [s]");
        // Explicit newlines split first; each segment wraps independently,
        // and a blank segment stays a blank line.
        assert_eq!(st["paras"], "ab cd|ef||gh");
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
        ui.event(InputEvent::Modifiers(Modifiers {
            shift: true,
            ..Default::default()
        }));
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

// ── Scrollbar + section label widgets ───────────────────────────────────────

#[test]
fn scrollbar_draws_nothing_when_everything_fits() {
    // count <= rows: the whole list is visible, so no scrollbar is drawn.
    let src = "draw_scrollbar({x: 0, y: 0, w: 100, h: 200}, 5, 5, 0)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap();
        assert!(cmds.is_empty(), "no scrollbar when content fits: {cmds:?}");
    });
}

#[test]
fn scrollbar_draws_track_and_proportional_thumb() {
    // 20 items, 5 visible → thumb covers a quarter of the track; scrolled to
    // the bottom (scroll = count - rows = 15) the thumb sits at the track end.
    let top = "draw_scrollbar({x: 0, y: 0, w: 100, h: 200}, 20, 5, 0)";
    run_headless(top, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let rects: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Rect { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect();
        assert_eq!(rects.len(), 2, "track + thumb: {cmds:?}");
        // Track runs the full height along the right edge.
        let (tx, ty, tw, th) = rects[0];
        assert_eq!((ty, th), (0, 200), "track spans the region height");
        assert_eq!(tx + tw as i32, 100, "track hugs the right edge");
        // Thumb is at the top and a quarter of the height (5/20 * 200 = 50).
        let (_, thumb_y, _, thumb_h) = rects[1];
        assert_eq!(thumb_y, 0, "at scroll 0 the thumb is at the top");
        assert_eq!(thumb_h, 50, "thumb height is rows/count of the track");
    });

    let bottom = "draw_scrollbar({x: 0, y: 0, w: 100, h: 200}, 20, 5, 15)";
    run_headless(bottom, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let thumb = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Rect { y, h, .. } => Some((*y, *h)),
                _ => None,
            })
            .nth(1)
            .expect("thumb rect");
        assert_eq!(
            thumb.0 + thumb.1 as i32,
            200,
            "fully scrolled: thumb bottom at track end"
        );
    });
}

#[test]
fn section_label_underlines_and_accents_only_when_active() {
    // Inactive: dim text, no underline.
    run_headless("section_label(\"Files\", 10, 20, false)", |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert!(
            cmds.iter().any(|c| matches!(c,
                DrawCommand::Text { text, r: 138, g: 147, b: 162, .. } if text == "Files")),
            "inactive label is dim text: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|c| matches!(c, DrawCommand::Line { .. })),
            "inactive label has no underline: {cmds:?}"
        );
    });

    // Active: accent text plus an underline in the accent color starting at x.
    run_headless("section_label(\"Files\", 10, 20, true)", |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert!(
            cmds.iter().any(|c| matches!(c,
                DrawCommand::Text { text, r: 60, g: 140, b: 255, .. } if text == "Files")),
            "active label is accent text: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                DrawCommand::Line {
                    x1: 10,
                    r: 60,
                    g: 140,
                    b: 255,
                    ..
                }
            )),
            "active label draws an accent underline from x: {cmds:?}"
        );
    });
}

#[test]
fn record_overloads_carry_alpha_and_width() {
    // The record forms used to stop short of alpha, so every translucent UI had
    // to fall back to the flat 8- and 9-argument calls. These overloads are what
    // let a whole panel vocabulary stay in record form.
    let src = "let R = rect(4, 6, 20, 10)\n\
               let C = {r: 10, g: 20, b: 30}\n\
               draw_rect_outline(R, C, 128)\n\
               draw_rect_outline(R, C, 128, 3)\n\
               draw_circle({x: 5, y: 7}, 9, C, 64)\n\
               draw_text(\"hi\", {x: 1, y: 2}, 12, C, 200)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert!(
            matches!(
                cmds[0],
                DrawCommand::RectOutline {
                    x: 4,
                    y: 6,
                    a: 128,
                    width: 1,
                    ..
                }
            ),
            "outline takes alpha, keeping the default width: {cmds:?}"
        );
        assert!(
            matches!(
                cmds[1],
                DrawCommand::RectOutline {
                    a: 128,
                    width: 3,
                    ..
                }
            ),
            "outline takes alpha and width: {cmds:?}"
        );
        assert!(
            matches!(
                cmds[2],
                DrawCommand::Circle {
                    cx: 5,
                    cy: 7,
                    radius: 9,
                    a: 64,
                    ..
                }
            ),
            "circle takes alpha: {cmds:?}"
        );
        assert!(
            matches!(cmds[3], DrawCommand::Text { a: 200, .. }),
            "text takes alpha: {cmds:?}"
        );
    });
}

#[test]
fn draw_text_center_puts_the_midpoint_at_cx() {
    // The counterpart to draw_text_right. Both measure with text_width, so the
    // run's own width — not a guess at it — decides the origin.
    let src = "draw_text_center(\"abcd\", 100, 20, 10, {r: 1, g: 2, b: 3})\n\
               state w = 0\n\
               w = text_width(\"abcd\", 10)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let width = ui.state_int("w").expect("text_width recorded");
        match &cmds[0] {
            DrawCommand::Text { x, y, .. } => {
                assert_eq!((*x, *y), (100 - (width as i32) / 2, 20));
            }
            other => panic!("expected text, got {other:?}"),
        }
    });
}

#[test]
fn ellipsize_fits_the_pixel_budget_and_terminates() {
    // The loop appends its "…" once at the end and never measures it back into
    // the string being trimmed — doing so is a fixed point that never returns,
    // because len/slice are byte-indexed while text_width counts characters.
    let src = "state short = \"\"\n\
               state long = \"\"\n\
               state empty = \"\"\n\
               state uni = \"\"\n\
               short = ellipsize(\"ab\", 1000, 10)\n\
               long = ellipsize(\"abcdefghijklmnop\", 30, 10)\n\
               empty = ellipsize(\"abcdefghijklmnop\", 0, 10)\n\
               uni = ellipsize(\"héllo wörld ünïcode\", 30, 10)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state_string("short").as_deref(),
            Some("ab"),
            "a string that already fits is returned untouched"
        );
        let long = ui.state_string("long").expect("long");
        assert!(long.ends_with('…'), "clipped text is marked: {long:?}");
        assert!(long.len() < "abcdefghijklmnop".len(), "it actually shrank");
        assert_eq!(
            ui.state_string("empty").as_deref(),
            Some("…"),
            "a zero budget trims to the bare ellipsis rather than looping"
        );
        // The real hazard: trimming multi-byte characters must land on
        // character boundaries, not split a codepoint.
        let uni = ui.state_string("uni").expect("uni");
        assert!(uni.ends_with('…') && uni.is_char_boundary(uni.len() - '…'.len_utf8()));
    });
}

#[test]
fn ellipsize_tail_keeps_the_end_and_terminates() {
    // The mirror of ellipsize: the "…" goes in front and the *tail* survives,
    // which is what a file path wants. Trimming from the front is where slice
    // snapping a start index **up** earns its keep — each pass drops a whole
    // character, so the loop shrinks even through multi-byte text.
    let src = "state short = \"\"\n\
               state long = \"\"\n\
               state empty = \"\"\n\
               state uni = \"\"\n\
               short = ellipsize_tail(\"ab\", 1000, 10)\n\
               long = ellipsize_tail(\"src/panels/git_panel.ptl\", 60, 10)\n\
               empty = ellipsize_tail(\"abcdefghijklmnop\", 0, 10)\n\
               uni = ellipsize_tail(\"héllo wörld ünïcode\", 30, 10)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state_string("short").as_deref(),
            Some("ab"),
            "a string that already fits is returned untouched"
        );
        let long = ui.state_string("long").expect("long");
        assert!(long.starts_with('…'), "clipped text is marked: {long:?}");
        assert!(
            long.len() < "src/panels/git_panel.ptl".len(),
            "it actually shrank"
        );
        assert!(
            "src/panels/git_panel.ptl".ends_with(long.trim_start_matches('…')),
            "the tail is what survived, not the head: {long:?}"
        );
        assert_eq!(
            ui.state_string("empty").as_deref(),
            Some("…"),
            "a zero budget trims to the bare ellipsis rather than looping"
        );
        // Trimming from the front must land on character boundaries: a start
        // index inside "é" snaps up to the next whole character.
        let uni = ui.state_string("uni").expect("uni");
        assert!(uni.starts_with('…'));
        assert!(
            "héllo wörld ünïcode".ends_with(uni.trim_start_matches('…')),
            "multi-byte text kept a whole-character tail: {uni:?}"
        );
    });
}

/// `rect()` builds the built-in `Rect` class, not a bare record: the geometry
/// methods work on anything the prelude hands back, and `type()` says so. The
/// record shape is unchanged, which is what keeps every hand-written
/// `{x, y, w, h}` and every `r.x` read in existing apps working.
#[test]
fn rect_constructs_the_builtin_class() {
    let src = "state kind = \"\"\n\
               state cx = 0\n\
               state right = 0\n\
               state inset_w = 0\n\
               let r = rect(10, 20, 100, 40)\n\
               kind = type(r)\n\
               cx = r.center_x()\n\
               right = r.right()\n\
               inset_w = r.inset(5).w";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_string("kind").as_deref(), Some("Rect"));
        assert_eq!(ui.state_int("cx"), Some(60));
        assert_eq!(ui.state_int("right"), Some(110));
        assert_eq!(ui.state_int("inset_w"), Some(90));
    });
}

/// A bad argument to `rect()` is the *app author's* mistake, so it is reported
/// at their call. `rect` is the constructor itself rather than a prelude
/// function wrapping it — a wrapper puts the caret on a line of ui.ptl and
/// leaves the author reading library source for a typo in their own file.
#[test]
fn a_bad_rect_argument_is_reported_at_the_call_site() {
    let mut ui = Headless::new("let r = rect(\"a\", 0, 0, 0)").expect("compiles");
    let err = ui.frame().unwrap_err();
    let head = err.lines().next().unwrap_or_default().to_string();
    assert!(head.contains("field `x`"), "{err}");
    assert!(
        head.contains("[line 1, column 9]"),
        "the caret should sit on the caller's own rect(...): {err}"
    );
}

/// Sub-pixel geometry survives `rect()`. Layout and animation compute
/// fractional positions all the time; the constructor stores what it was
/// given, so nothing is quantized on the way in. Truncation happens once, at
/// the draw call, where pixels actually are integers.
#[test]
fn rect_keeps_fractional_geometry() {
    let src = "state x = 0.0\n\
               state w = 0.0\n\
               state cx = 0.0\n\
               state right = 0.0\n\
               state moved = 0.0\n\
               let r = rect(10.5, 20.9, 100.4, 40.6)\n\
               x = r.x\n\
               w = r.w\n\
               cx = r.center_x()\n\
               right = r.right()\n\
               moved = r.offset(0.25, 0.0).x";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_float("x"), Some(10.5), "rect() kept x");
        assert_eq!(ui.state_float("w"), Some(100.4), "rect() kept w");
        assert_eq!(ui.state_float("cx"), Some(10.5 + 100.4 / 2.0));
        assert_eq!(ui.state_float("right"), Some(10.5 + 100.4));
        assert_eq!(ui.state_float("moved"), Some(10.75));
    });
}

/// A fractional rect still draws: the draw layer is where a coordinate becomes
/// a pixel, and it truncates there (as it always has for float args).
#[test]
fn draw_overloads_accept_a_fractional_rect() {
    let src = "draw_rect(rect(4.5, 6.5, 20.5, 10.5), {r: 10, g: 20, b: 30})";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert!(
            matches!(
                cmds[0],
                DrawCommand::Rect {
                    x: 4,
                    y: 6,
                    w: 20,
                    h: 10,
                    ..
                }
            ),
            "{:?}",
            cmds[0]
        );
    });
}

/// The record-arg draw overloads keep taking a `rect()` result — the class tag
/// changes nothing about field access, which is all they use.
#[test]
fn draw_overloads_still_accept_a_rect() {
    let src = "draw_rect(rect(4, 6, 20, 10), {r: 10, g: 20, b: 30})";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert!(
            matches!(
                cmds[0],
                DrawCommand::Rect {
                    x: 4,
                    y: 6,
                    w: 20,
                    h: 10,
                    ..
                }
            ),
            "unexpected command: {:?}",
            cmds[0]
        );
    });
}

// ── Context menu ────────────────────────────────────────────────────────────

/// A row at (0,0)-(200,40) that opens a three-item menu (with a separator and
/// one disabled entry) on right-click. `chose` records the last index taken and
/// `took` the label, so a test can assert what the menu resolved to.
const MENU_SRC: &str = "state menu = menu_state()\n\
                        state chose = -1\n\
                        state took = \"\"\n\
                        state tag = \"\"\n\
                        state clicks = 0\n\
                        let r = {x: 0, y: 0, w: 200, h: 40}\n\
                        if !menu_blocking(menu) && mouse_pressed(0) && point_in(mouse_x(), mouse_y(), r) then\n\
                          clicks = clicks + 1\n\
                        end\n\
                        menu = menu_open_on_right_click(menu, r, \"row-7\")\n\
                        let res = context_menu(menu, [menu_item(\"Only this commit\"), menu_sep(), menu_item(\"Revert\", false), menu_item(\"Copy SHA\")])\n\
                        menu = res.menu\n\
                        if res.index >= 0 then\n\
                          chose = res.index\n\
                          took = res.label\n\
                          tag = res.tag\n\
                        end";

/// Right-click opens the menu at the pointer; a left click on a row resolves to
/// that row's index, label and the tag the menu was opened with, and closes.
#[test]
fn context_menu_opens_on_right_click_and_resolves_a_choice() {
    run_headless(MENU_SRC, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state().get("menu").unwrap()["open"], false);

        // Right press inside the row opens it at (30, 20).
        ui.mouse_move(30, 20);
        ui.mouse_down(1);
        ui.frame().unwrap();
        ui.mouse_up(1);
        ui.frame().unwrap();
        let menu = ui.state().get("menu").unwrap().clone();
        assert_eq!(menu["open"], true, "right-click opens the menu");
        assert_eq!(menu["x"], 30);
        assert_eq!(menu["y"], 20);

        // Rows start 6px below the menu top and are 24px tall, so item 0 spans
        // y 26..50 and the first click lands squarely in it.
        ui.click(60, 35).unwrap();
        assert_eq!(ui.state_int("chose"), Some(0));
        assert_eq!(ui.state_string("took").as_deref(), Some("Only this commit"));
        assert_eq!(
            ui.state_string("tag").as_deref(),
            Some("row-7"),
            "the choice carries the tag the menu was opened with"
        );
        assert_eq!(
            ui.state().get("menu").unwrap()["open"],
            false,
            "choosing closes the menu"
        );
        assert_eq!(
            ui.state_int("clicks"),
            Some(0),
            "the click that picked a menu item must not also reach the row under it"
        );
    });
}

/// A click outside dismisses without choosing, and — the point of
/// `menu_blocking` — the underlying widget does not act on that press either.
#[test]
fn context_menu_dismisses_without_choosing() {
    run_headless(MENU_SRC, |ui| {
        ui.frame().unwrap();
        ui.mouse_move(30, 20);
        ui.mouse_down(1);
        ui.frame().unwrap();
        ui.mouse_up(1);
        ui.frame().unwrap();

        // Far outside the menu, but still inside the row that opened it.
        ui.click(180, 10).unwrap();
        assert_eq!(ui.state_int("chose"), Some(-1), "nothing was chosen");
        assert_eq!(ui.state().get("menu").unwrap()["open"], false);
        assert_eq!(
            ui.state_int("clicks"),
            Some(0),
            "the dismissing click is swallowed, not passed through to the row"
        );

        // With the menu closed, the row takes clicks normally again.
        ui.click(180, 10).unwrap();
        assert_eq!(ui.state_int("clicks"), Some(1));
    });
}

/// Escape closes it, and a disabled row cannot be picked by pointer or by
/// keyboard — Down skips straight past it to the next choosable entry.
#[test]
fn context_menu_skips_disabled_rows_and_closes_on_escape() {
    run_headless(MENU_SRC, |ui| {
        ui.frame().unwrap();
        ui.mouse_move(30, 20);
        ui.mouse_down(1);
        ui.frame().unwrap();
        ui.mouse_up(1);
        ui.frame().unwrap();

        // Item 2 ("Revert") is disabled: item 0 spans 26..50, the separator
        // 50..59, item 2 spans 59..83. Clicking it chooses nothing but still
        // dismisses, like any other click that hits no row.
        ui.click(60, 70).unwrap();
        assert_eq!(ui.state_int("chose"), Some(-1));
        assert_eq!(ui.state().get("menu").unwrap()["open"], false);

        // Reopen and drive it from the keyboard: Down lands on item 0, a second
        // Down skips the separator *and* the disabled row to reach item 3.
        ui.mouse_move(30, 20);
        ui.mouse_down(1);
        ui.frame().unwrap();
        ui.mouse_up(1);
        // Park the pointer off the menu so hover doesn't override the keyboard.
        ui.mouse_move(400, 400);
        ui.frame().unwrap();
        ui.key("down").unwrap();
        assert_eq!(ui.state().get("menu").unwrap()["hover"], 0);
        ui.key("down").unwrap();
        assert_eq!(
            ui.state().get("menu").unwrap()["hover"],
            3,
            "Down skips the separator and the disabled row"
        );
        ui.key("return").unwrap();
        assert_eq!(ui.state_string("took").as_deref(), Some("Copy SHA"));

        // Escape dismisses an open menu without choosing.
        ui.mouse_move(30, 20);
        ui.mouse_down(1);
        ui.frame().unwrap();
        ui.mouse_up(1);
        ui.frame().unwrap();
        assert_eq!(ui.state().get("menu").unwrap()["open"], true);
        ui.key("escape").unwrap();
        assert_eq!(ui.state().get("menu").unwrap()["open"], false);
    });
}

/// A menu opened near an edge flips to the other side of the pointer rather
/// than being clipped: the harness screen is 800×600, so a right-click at the
/// bottom-right corner must place the menu above and left of it.
#[test]
fn context_menu_flips_away_from_the_screen_edges() {
    // A trigger rect covering the whole 800x600 screen, so the corner press
    // that would clip a naively-placed menu still opens one.
    let src = MENU_SRC.replace(
        "let r = {x: 0, y: 0, w: 200, h: 40}",
        "let r = {x: 0, y: 0, w: 800, h: 600}",
    );
    run_headless(&src, |ui| {
        ui.frame().unwrap();
        ui.mouse_move(790, 590);
        ui.mouse_down(1);
        ui.frame().unwrap();
        ui.mouse_up(1);
        let cmds = ui.frame().unwrap().to_vec();
        // The menu's own frame is the last opaque rect drawn; every one of its
        // rows must sit inside the screen.
        let rects: Vec<(i32, i32, u32, u32)> = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Rect { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect();
        assert!(!rects.is_empty(), "an open menu draws something");
        for (x, y, w, h) in rects {
            assert!(
                x >= 0 && y >= 0 && x + w as i32 <= 800 && y + h as i32 <= 600,
                "menu geometry {x},{y} {w}x{h} escapes the 800x600 screen"
            );
        }
    });
}

// ── Regression coverage for the prelude gaps found by the testbed apps ──────

/// `draw_line`'s record overload had only the 3-argument form, so a translucent
/// or thick stroke — what a checkmark, a strikethrough and a caret all want —
/// fell through to the flat native and died at *runtime* with
/// "draw_line() expects 7 or 8 or 9 or 3 arguments, got 5".
#[test]
fn draw_line_record_overload_takes_alpha_and_width() {
    let src = "draw_line({x: 1, y: 2}, {x: 3, y: 4}, #ff0000)\n\
               draw_line({x: 5, y: 6}, {x: 7, y: 8}, #00ff00, 128)\n\
               draw_line({x: 9, y: 10}, {x: 11, y: 12}, #0000ff, 64, 3)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let lines: Vec<(i32, u8, u32)> = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Line { x1, a, width, .. } => Some((*x1, *a, *width)),
                _ => None,
            })
            .collect();
        assert_eq!(
            lines,
            vec![(1, 255, 1), (5, 128, 1), (9, 64, 3)],
            "the 4- and 5-argument record forms carry alpha and stroke width"
        );
    });
}

/// The menu's landing rect — width, height and the flip-when-offscreen rule —
/// is public, so an app that must know where the menu sits stops copying
/// `_menu_rect` and the four metric constants into its own file.
#[test]
fn menu_rect_is_public_and_matches_what_context_menu_draws() {
    let src = "state mx = 0\n\
               state my = 0\n\
               state mw = 0\n\
               state cx = 0\n\
               state cy = 0\n\
               state menu = menu_state()\n\
               let items = [menu_item(\"Copy\"), menu_sep(), menu_item(\"Delete\")]\n\
               menu = menu_open_on_right_click(menu, {x: 0, y: 0, w: 800, h: 600}, \"row\")\n\
               let mr = menu_rect(menu, items)\n\
               mx = mr.x\n\
               my = mr.y\n\
               mw = mr.w\n\
               let picked = context_menu(menu, items)\n\
               menu = picked.menu\n\
               cx = picked.rect.x\n\
               cy = picked.rect.y";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        // Open near the bottom-right corner: the menu must flip up and left.
        ui.mouse_move(790, 590);
        ui.mouse_down(1);
        ui.frame().unwrap();
        ui.mouse_up(1);
        ui.frame().unwrap();
        let (x, y, w) = (
            ui.state_int("mx").unwrap(),
            ui.state_int("my").unwrap(),
            ui.state_int("mw").unwrap(),
        );
        assert!(x < 790 && y < 590, "menu_rect flips away from the edge");
        assert!(x + w <= 800, "flipped menu stays on screen");
        assert_eq!(
            (ui.state_int("cx"), ui.state_int("cy")),
            (Some(x), Some(y)),
            "context_menu hands back the same rect it drew at"
        );
    });
}

/// A closed menu reports an empty rect rather than a stale one.
#[test]
fn context_menu_reports_an_empty_rect_when_closed() {
    let src = "state w = -1\n\
               let picked = context_menu(menu_state(), [menu_item(\"Copy\")])\n\
               w = picked.rect.w";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("w"), Some(0));
    });
}

/// `theme_set` repaints every prelude widget, so a light-themed panel stops
/// reimplementing buttons, fields, scrollbars and the menu just to change color.
#[test]
fn theme_set_repaints_every_widget() {
    // Away from the pointer's resting (0, 0) so the button paints its face,
    // not its hover color.
    let src = "theme_set({panel: #ffffff, text: #101010, outline: #cccccc})\n\
               button({x: 100, y: 100, w: 80, h: 24}, \"Go\")";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                DrawCommand::Rect {
                    r: 255,
                    g: 255,
                    b: 255,
                    ..
                }
            )),
            "the button's face takes the themed panel color: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| matches!(c,
                DrawCommand::Text { text, r: 16, g: 16, b: 16, .. } if text == "Go")),
            "the button's label takes the themed text color: {cmds:?}"
        );
    });
}

/// `theme_set` merges: the keys it leaves out keep their current value, and
/// `theme_reset` puts the built-in dark palette back.
#[test]
fn theme_set_merges_and_theme_reset_restores() {
    let src = "state accent_r = 0\n\
               state panel_r = 0\n\
               state n = 0\n\
               n = n + 1\n\
               if n == 1 then\n\
                 theme_set({panel: #ffffff})\n\
               end\n\
               if n == 3 then\n\
                 theme_reset()\n\
               end\n\
               accent_r = ui_theme().accent.r\n\
               panel_r = ui_theme().panel.r";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("panel_r"), Some(255), "the set key applies");
        assert_eq!(
            ui.state_int("accent_r"),
            Some(60),
            "an unmentioned key keeps the default accent"
        );
        ui.frame().unwrap();
        assert_eq!(
            ui.state_int("panel_r"),
            Some(255),
            "the palette survives the frame that set it"
        );
        ui.frame().unwrap();
        assert_eq!(ui.state_int("panel_r"), Some(29), "theme_reset restores");
    });
}

/// Every widget also takes a per-call style record, and a style may omit any
/// key — what it leaves out falls through to the live theme.
#[test]
fn widget_style_records_may_be_partial() {
    let src = "button({x: 100, y: 100, w: 80, h: 24}, \"Go\", {bg: #ff0000})\n\
               draw_scrollbar({x: 0, y: 40, w: 100, h: 200}, 20, 5, 0, {thumb: #00ff00})\n\
               section_label(\"Files\", 10, 260, true, {active: #0000ff})\n\
               draw_text_field({x: 0, y: 300, w: 120, h: 24}, \"hi\", false, {bg: #ffff00})";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let rect_colors: Vec<(u8, u8, u8)> = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Rect { r, g, b, .. } => Some((*r, *g, *b)),
                _ => None,
            })
            .collect();
        assert!(rect_colors.contains(&(255, 0, 0)), "button bg override");
        assert!(rect_colors.contains(&(0, 255, 0)), "scrollbar thumb override");
        assert!(rect_colors.contains(&(255, 255, 0)), "text field bg override");
        assert!(
            rect_colors.contains(&(58, 64, 76)),
            "the scrollbar track it did NOT override still comes from the theme: {rect_colors:?}"
        );
        assert!(
            cmds.iter().any(|c| matches!(c,
                DrawCommand::Text { text, r: 0, g: 0, b: 255, .. } if text == "Files")),
            "section_label active color override: {cmds:?}"
        );
    });
}

/// `ellipsize` / `ellipsize_tail` / `fit_parts` accept the same style record
/// `draw_text` takes, so what is measured is what gets drawn.
#[test]
fn ellipsize_accepts_a_style_record() {
    let src = "state a = \"\"\n\
               state b = \"\"\n\
               a = ellipsize(\"a long enough label\", 40, 14)\n\
               b = ellipsize(\"a long enough label\", 40, {size: 14})";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let a = ui.state_string("a").unwrap();
        assert!(a.ends_with('…'), "the bare-size form still clips: {a}");
        assert_eq!(
            ui.state_string("b").unwrap(),
            a,
            "a style record measures the same as the bare size it carries"
        );
    });

    let src = "state a = \"\"\n\
               a = ellipsize_tail(\"/some/deep/path/file.ptl\", 40, {size: 14})";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert!(ui.state_string("a").unwrap().starts_with('…'));
    });
}

/// Right- and center-aligned text take a style record too, so bold or spaced
/// runs land where they are measured.
#[test]
fn aligned_text_takes_a_style_record() {
    let src = "draw_text_right(\"hi\", 100, 10, {size: 14, color: #ff0000})\n\
               draw_text_center(\"hi\", 100, 30, {size: 14, color: #00ff00})";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let xs: Vec<i32> = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { x, .. } => Some(*x),
                _ => None,
            })
            .collect();
        assert_eq!(xs.len(), 2, "both aligned forms drew: {cmds:?}");
        assert!(xs[0] < 100, "right-aligned text sits left of its right edge");
        assert!(
            xs[1] > xs[0],
            "centered text starts further right than right-aligned text at the same edge"
        );
    });
}

/// `text_field_update` is the input half with no pixels: an app with its own
/// palette keeps focus, typing, backspace and Return and draws the box itself.
#[test]
fn text_field_update_edits_without_drawing() {
    let src = "state fc = focus_state()\n\
               state buf = \"\"\n\
               state submitted = false\n\
               let r = {x: 0, y: 0, w: 200, h: 24}\n\
               let res = text_field_update(fc, \"name\", r, buf)\n\
               fc = res.focus\n\
               buf = res.text\n\
               submitted = res.submitted";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert!(cmds.is_empty(), "the input half draws nothing: {cmds:?}");
        ui.click(10, 10).unwrap();
        ui.text("ab").unwrap();
        assert_eq!(ui.state_string("buf").as_deref(), Some("ab"));
        ui.key("backspace").unwrap();
        assert_eq!(ui.state_string("buf").as_deref(), Some("a"));
        ui.key("return").unwrap();
        assert_eq!(ui.state().get("submitted").unwrap(), true);
        assert!(
            ui.commands.is_empty(),
            "still no pixels of its own: {:?}",
            ui.commands
        );
    });
}

/// The stock look is still one call, and the draw half can be used on its own.
#[test]
fn draw_text_field_renders_the_stock_box() {
    let src = "draw_text_field({x: 0, y: 0, w: 200, h: 24}, \"hi\", true)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DrawCommand::Line { r: 60, g: 140, b: 255, .. })),
            "a focused field draws the accent caret: {cmds:?}"
        );
    });
}

/// The drag primitive every kanban / reorderable-list / tab-strip app was
/// hand-rolling: press to pick up, offset while held, one frame of `dropped`.
#[test]
fn drag_update_tracks_a_press_drag_release_cycle() {
    let src = "state dg = drag_state()\n\
               state dropped_id = \"\"\n\
               state dx = 0\n\
               state dy = 0\n\
               state active = \"\"\n\
               dg = drag_update(dg, \"a\", {x: 0, y: 0, w: 50, h: 20})\n\
               dg = drag_update(dg, \"b\", {x: 0, y: 40, w: 50, h: 20})\n\
               active = if dg.dragging then dg.id else \"\" end\n\
               dx = dg.dx\n\
               dy = dg.dy\n\
               if dg.dropped then dropped_id = dg.id end";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_string("active").as_deref(), Some(""));

        // Press inside item "b".
        ui.mouse_move(10, 50);
        ui.mouse_down(0);
        ui.frame().unwrap();
        assert_eq!(
            ui.state_string("active").as_deref(),
            Some("b"),
            "the press claims the item under the pointer"
        );
        assert_eq!(ui.state_int("dx"), Some(0), "offset starts at zero");

        // Drag up and to the right.
        ui.mouse_move(35, 5);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("dx"), Some(25));
        assert_eq!(ui.state_int("dy"), Some(-45));
        assert_eq!(
            ui.state_string("active").as_deref(),
            Some("b"),
            "a drag in flight is not stolen by the item now under the pointer"
        );

        // Release: `dropped` is reported once, with the final offset, and it
        // survives the rest of the frame's drag_update calls.
        ui.mouse_up(0);
        ui.frame().unwrap();
        assert_eq!(ui.state_string("dropped_id").as_deref(), Some("b"));
        assert_eq!(ui.state_int("dx"), Some(25));
        assert_eq!(ui.state_string("active").as_deref(), Some(""));

        // ...and is cleared on the next frame.
        let src_dropped = ui.state().get("dg").unwrap()["dropped"].clone();
        assert_eq!(src_dropped, true, "dropped is still set entering the frame");
        ui.frame().unwrap();
        assert_eq!(ui.state().get("dg").unwrap()["dropped"], false);
    });
}

/// `insertion_index` turns a drop position into the list index an insert wants.
#[test]
fn insertion_index_counts_rows_above_the_drop() {
    let src = "state before = 0\n\
               state middle = 0\n\
               state after = 0\n\
               let rows = [{x: 0, y: 0, w: 50, h: 20},\n\
                           {x: 0, y: 20, w: 50, h: 20},\n\
                           {x: 0, y: 40, w: 50, h: 20}]\n\
               before = insertion_index(rows, 2)\n\
               middle = insertion_index(rows, 35)\n\
               after = insertion_index(rows, 100)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("before"), Some(0), "above every midpoint");
        assert_eq!(ui.state_int("middle"), Some(2), "past two midpoints");
        assert_eq!(ui.state_int("after"), Some(3), "below every midpoint");
    });

    let src = "state i = 0\n\
               let cols = [{x: 0, y: 0, w: 40, h: 10}, {x: 40, y: 0, w: 40, h: 10}]\n\
               i = insertion_index_x(cols, 65)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("i"), Some(2));
    });
}

/// The color helpers several apps hand-rolled.
#[test]
fn color_helpers_mix_and_measure_luma() {
    let src = "state m = 0\n\
               state l = 0\n\
               state dark = 0\n\
               state light = 0\n\
               m = mix(#000000, #ffffff, 0.5).r\n\
               l = luma(#ffffff)\n\
               dark = contrast_text(#ffffff).r\n\
               light = contrast_text(#000000).r";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("m"), Some(128), "mix is a midpoint blend");
        assert_eq!(ui.state_int("l"), Some(255), "white is full luma");
        assert!(
            ui.state_int("dark").unwrap() < 64,
            "a light surface gets dark ink"
        );
        assert!(
            ui.state_int("light").unwrap() > 192,
            "a dark surface gets light ink"
        );
    });

    let src = "state a = 0\n a = lerp_color(#000000, #ff0000, 1.0).r";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("a"), Some(255));
    });
}

/// The `clamp` builtin preserves int-ness now, which is why the prelude's
/// private `_clamp` is gone. A float creeping back into the menu's width would
/// put every menu on a half-pixel boundary.
#[test]
fn menu_geometry_stays_integral_on_the_clamp_builtin() {
    let src = "state w = 0.5\n\
               w = menu_rect(menu_show(menu_state(), 10, 10, \"t\"),\n\
                             [menu_item(\"Copy\")]).w";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state().get("w").unwrap(),
            &serde_json::json!(120),
            "clamp() keeps the width an int, not 120.0"
        );
    });
}

/// A script that already had its own `mix` / `luma` (several testbed apps do)
/// keeps its own — the new prelude names are shadowed, not fought over.
#[test]
fn script_definitions_shadow_the_new_color_helpers() {
    let src = "fn mix(a, b, t)\n\
                 {r: 7, g: 7, b: 7}\n\
               end\n\
               state m = 0\n\
               m = mix(#000000, #ffffff, 0.5).r";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("m"), Some(7), "the script's own mix wins");
    });
}
