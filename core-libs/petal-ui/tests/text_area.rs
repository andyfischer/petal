//! `text_area`: the multi-line editor — wrapping, Return, rows under the
//! arrows and Home/End, mouse selection, the clipboard with its line breaks,
//! grouped undo, the wheel, and giving up the focus. Driven headlessly.
//!
//! Harness metrics: a glyph is `size × 0.6` px wide, so at the theme's 14 px
//! a character is 8.4 px. The area's text starts at (16, 16) and is 188 px
//! wide: 22 characters to a row.

use petal_ui::clipboard::{clipboard_text, set_clipboard_provider, set_clipboard_text};
use petal_ui::harness::Headless;
use petal_ui::input::{InputEvent, Modifiers, buttons};

fn src(buf: &str, h: i64) -> String {
    let buf = buf.replace('\n', "\\n");
    format!(
        "state fc = focus_state()\n\
         state buf = \"{buf}\"\n\
         state caret = -1\n\
         state a = -1\n\
         state b = -1\n\
         state scroll = 0\n\
         state lh = 0\n\
         state nrows = 0\n\
         state has = false\n\
         let res = text_area(fc, \"notes\", {{x: 10, y: 10, w: 200, h: {h}}}, buf)\n\
         fc = res.focus\n\
         buf = res.text\n\
         caret = res.caret\n\
         a = res.sel_start\n\
         b = res.sel_end\n\
         scroll = int(res.scroll)\n\
         lh = res.line_h\n\
         nrows = len(res.rows)\n\
         has = focused(fc, \"notes\")"
    )
}

const CMD: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, cmd: true };
const SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, cmd: false };

fn area(buf: &str, h: i64) -> Headless {
    set_clipboard_provider(None);
    let mut ui = Headless::with_ui(&src(buf, h)).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.frame().unwrap();
    ui
}

fn chord(ui: &mut Headless, mods: Modifiers, key: &str) {
    ui.event(InputEvent::Modifiers(mods));
    ui.key(key).unwrap();
    ui.event(InputEvent::Modifiers(Modifiers::default()));
}

fn int(ui: &Headless, name: &str) -> i64 {
    ui.state_int(name).unwrap_or_else(|| panic!("state `{name}` is not an int"))
}

fn buf(ui: &Headless) -> String {
    ui.state_string("buf").unwrap()
}

fn has(ui: &Headless) -> bool {
    matches!(ui.state().get("has"), Some(serde_json::Value::Bool(true)))
}

/// The x of the caret slot before column `i`, and the middle of row `row`.
fn at(ui: &Headless, i: usize, row: i64) -> (i32, i32) {
    let lh = int(ui, "lh");
    (16 + (i as f64 * 8.4).round() as i32, (16 + row * lh + lh / 2) as i32)
}

fn click(ui: &mut Headless, p: (i32, i32)) {
    ui.click(p.0, p.1).unwrap();
    ui.frame().unwrap();
}

#[test]
fn return_starts_a_new_line_and_undo_takes_a_word_at_a_time() {
    let mut ui = area("", 120);
    click(&mut ui, (100, 40));
    assert!(has(&ui));
    ui.text("one ").unwrap();
    ui.text("two").unwrap();
    ui.key("return").unwrap();
    ui.text("three").unwrap();
    assert_eq!(buf(&ui), "one two\nthree");
    assert_eq!(int(&ui, "nrows"), 2);
    chord(&mut ui, CMD, "z");
    assert_eq!(buf(&ui), "one two\n", "the word typed after the break");
    chord(&mut ui, CMD, "z");
    assert_eq!(buf(&ui), "one two", "the break is a step of its own");
    chord(&mut ui, CMD, "z");
    assert_eq!(buf(&ui), "one ");
    chord(&mut ui, Modifiers { shift: true, ..CMD }, "z");
    assert_eq!(buf(&ui), "one two");
}

#[test]
fn text_wraps_and_the_arrows_and_home_end_move_by_row() {
    // 22 characters to a row: "aaaa bbbb cccc dddd" fits, "eeee" wraps.
    let mut ui = area("aaaa bbbb cccc dddd eeee ffff\nxy", 120);
    assert_eq!(int(&ui, "nrows"), 3);
    // Click in row 0, column 2.
    let p = at(&ui, 2, 0);
    click(&mut ui, p);
    assert_eq!(int(&ui, "caret"), 2);
    ui.key("down").unwrap();
    assert_eq!(int(&ui, "caret"), 22, "row 1 starts at 20; column 2");
    ui.key("down").unwrap();
    assert_eq!(int(&ui, "caret"), 32, "row 2 (\"xy\") starts at 30; column 2 is its end");
    ui.key("up").unwrap();
    assert_eq!(int(&ui, "caret"), 22, "back up, the column was kept");
    ui.key("end").unwrap();
    assert_eq!(int(&ui, "caret"), 29, "the end of row 1, before the break");
    ui.key("home").unwrap();
    assert_eq!(int(&ui, "caret"), 20);
    ui.key("up").unwrap();
    ui.key("up").unwrap();
    assert_eq!(int(&ui, "caret"), 0, "up from the top row goes to the start");
    chord(&mut ui, CMD, "down");
    assert_eq!(int(&ui, "caret"), 32);
    // Shift+up selects back a row.
    chord(&mut ui, SHIFT, "up");
    assert_eq!((int(&ui, "a"), int(&ui, "b")), (22, 32));
}

#[test]
fn mouse_selection_and_the_clipboard_keep_line_breaks() {
    let mut ui = area("alpha beta\ngamma", 120);
    // A double click selects a word, and stops at the line break.
    let p = at(&ui, 7, 0);
    click(&mut ui, p);
    let p = at(&ui, 7, 0);
    click(&mut ui, p);
    assert_eq!((int(&ui, "a"), int(&ui, "b")), (6, 10));
    // A triple click, everything.
    let p = at(&ui, 7, 0);
    click(&mut ui, p);
    assert_eq!((int(&ui, "a"), int(&ui, "b")), (0, 16));
    ui.frames(40).unwrap();
    // A drag from row 0 to row 1 selects across the break.
    let (x0, y0) = at(&ui, 6, 0);
    let (x1, y1) = at(&ui, 2, 1);
    ui.mouse_move(x0, y0);
    ui.mouse_down(buttons::LEFT);
    ui.frame().unwrap();
    ui.mouse_move(x1, y1);
    ui.frame().unwrap();
    ui.mouse_up(buttons::LEFT);
    ui.frame().unwrap();
    assert_eq!((int(&ui, "a"), int(&ui, "b")), (6, 13));
    chord(&mut ui, CMD, "c");
    assert_eq!(clipboard_text(), "beta\nga");
    // Paste keeps the breaks (a CR of a CRLF goes).
    set_clipboard_text("1\r\n2");
    ui.frame().unwrap();
    chord(&mut ui, CMD, "v");
    assert_eq!(buf(&ui), "alpha 1\n2mma");
}

#[test]
fn the_wheel_scrolls_an_overflowing_area_and_the_caret_is_followed() {
    let mut ui = area("1\n2\n3\n4\n5\n6\n7\n8\n9\n10", 60);
    let lh = int(&ui, "lh");
    assert_eq!(int(&ui, "scroll"), 0);
    ui.mouse_move(100, 30);
    ui.scroll(2.0);
    ui.frame().unwrap();
    assert_eq!(int(&ui, "scroll"), 2 * lh, "unfocused, the wheel still scrolls");
    ui.scroll(-5.0);
    ui.frame().unwrap();
    assert_eq!(int(&ui, "scroll"), 0, "clamped at the top");
    // Focus, then cmd+down to the end: the view follows the caret.
    click(&mut ui, (100, 30));
    chord(&mut ui, CMD, "down");
    let inner_h = 60 - 12;
    assert_eq!(int(&ui, "scroll"), 10 * lh - inner_h);
    // While focused the wheel can scroll the caret out of view.
    ui.scroll(-1.0);
    ui.frame().unwrap();
    ui.frame().unwrap();
    assert_eq!(int(&ui, "scroll"), 10 * lh - inner_h - lh);
}

#[test]
fn a_press_outside_the_area_gives_up_the_focus() {
    let mut ui = area("x", 60);
    click(&mut ui, (100, 30));
    assert!(has(&ui));
    click(&mut ui, (100, 300));
    assert!(!has(&ui));
    ui.text("zz").unwrap();
    assert_eq!(buf(&ui), "x");
}
