//! `text_field`'s selection, clipboard and undo, and the text-range helpers
//! (`text_wrap_rows`, `text_range_rects`), driven headlessly with injected
//! input. The buffers are non-ASCII on purpose: every offset in these APIs is
//! a character offset, and a byte offset would land inside `é`.
//!
//! Harness metrics: a glyph is `size × 0.6` px wide, so at the theme's 14 px
//! a character is 8.4 px. The field below draws its text from x = 16.

use petal_ui::clipboard::{clipboard_text, set_clipboard_provider, set_clipboard_text};
use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;
use petal_ui::input::{InputEvent, Modifiers, buttons};

const SRC: &str = "state fc = focus_state()\n\
                   state buf = \"h\u{e9}llo w\u{f6}rld\"\n\
                   state caret = -1\n\
                   state a = -1\n\
                   state b = -1\n\
                   let res = text_field(fc, \"name\", {x: 10, y: 10, w: 200, h: 24}, buf)\n\
                   fc = res.focus\n\
                   buf = res.text\n\
                   caret = res.caret\n\
                   a = res.sel_start\n\
                   b = res.sel_end";

const SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, cmd: false };
const CMD: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, cmd: true };
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, cmd: true };

/// The x of the caret slot before character `i`.
fn slot(i: usize) -> i32 {
    16 + (i as f64 * 8.4).round() as i32
}

fn field() -> Headless {
    // Each test thread starts from an empty in-memory clipboard.
    set_clipboard_provider(None);
    let mut ui = Headless::new(SRC).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.frame().unwrap();
    ui
}

/// Press `key` with `mods` held, then release the modifiers.
fn chord(ui: &mut Headless, mods: Modifiers, key: &str) {
    ui.event(InputEvent::Modifiers(mods));
    ui.key(key).unwrap();
    ui.event(InputEvent::Modifiers(Modifiers::default()));
}

fn buf(ui: &Headless) -> String {
    ui.state_string("buf").unwrap()
}

fn sel(ui: &Headless) -> (i64, i64) {
    (ui.state_int("a").unwrap(), ui.state_int("b").unwrap())
}

#[test]
fn shift_arrows_select_and_typing_replaces_the_selection() {
    let mut ui = field();
    ui.click(slot(11) + 20, 20).unwrap();
    assert_eq!(sel(&ui), (11, 11), "a plain click selects nothing");
    for _ in 0..5 {
        chord(&mut ui, SHIFT, "left");
    }
    assert_eq!(sel(&ui), (6, 11), "five characters, not five bytes");
    assert_eq!(ui.state_int("caret"), Some(6));

    // A plain arrow collapses the selection to the side it points at.
    ui.key("right").unwrap();
    assert_eq!((sel(&ui), ui.state_int("caret")), ((11, 11), Some(11)));

    chord(&mut ui, SHIFT, "home");
    assert_eq!(sel(&ui), (0, 11));
    chord(&mut ui, SHIFT, "right");
    assert_eq!(sel(&ui), (1, 11));
    ui.text("\u{65e5}\u{672c}").unwrap();
    assert_eq!(buf(&ui), "h\u{65e5}\u{672c}");
    assert_eq!((sel(&ui), ui.state_int("caret")), ((3, 3), Some(3)));

    // Backspace over a selection removes exactly the selection.
    chord(&mut ui, SHIFT, "left");
    ui.key("backspace").unwrap();
    assert_eq!(buf(&ui), "h\u{65e5}");
}

#[test]
fn shift_click_drag_and_multi_click_select() {
    let mut ui = field();
    ui.click(slot(1), 20).unwrap();
    ui.frames(40).unwrap(); // let the multi-click window lapse
    ui.event(InputEvent::Modifiers(SHIFT));
    ui.click(slot(5), 20).unwrap();
    ui.event(InputEvent::Modifiers(Modifiers::default()));
    assert_eq!(sel(&ui), (1, 5), "shift+click extends from the caret");
    ui.frames(40).unwrap();

    // Drag from before "wörld" to its end, and past the box: the caret clamps.
    ui.mouse_move(slot(6), 20);
    ui.mouse_down(buttons::LEFT);
    ui.frame().unwrap();
    ui.mouse_move(slot(9), 20);
    ui.frame().unwrap();
    assert_eq!(sel(&ui), (6, 9));
    ui.mouse_move(400, 80);
    ui.frame().unwrap();
    assert_eq!(sel(&ui), (6, 11));
    ui.mouse_up(buttons::LEFT);
    ui.frame().unwrap();
    ui.mouse_move(slot(2), 20);
    ui.frame().unwrap();
    assert_eq!(sel(&ui), (6, 11), "moving with the button up selects nothing more");
    ui.frames(40).unwrap();

    // Double click: the word under the pointer. Triple: everything.
    ui.click(slot(8), 20).unwrap();
    ui.click(slot(8), 20).unwrap();
    assert_eq!(sel(&ui), (6, 11));
    ui.click(slot(8), 20).unwrap();
    assert_eq!(sel(&ui), (0, 11));
}

#[test]
fn select_all_cut_copy_and_paste_go_through_the_clipboard() {
    let mut ui = field();
    ui.click(slot(11) + 20, 20).unwrap();
    chord(&mut ui, CMD, "a");
    assert_eq!(sel(&ui), (0, 11));
    chord(&mut ui, CMD, "c");
    assert_eq!(clipboard_text(), "h\u{e9}llo w\u{f6}rld");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld", "copy leaves the text alone");

    // Paste replaces the selection; at a caret it inserts.
    ui.key("end").unwrap();
    ui.text(" ").unwrap();
    chord(&mut ui, CMD, "v");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld h\u{e9}llo w\u{f6}rld");
    assert_eq!(ui.state_int("caret"), Some(23));

    // Cut the last word.
    chord(&mut ui, Modifiers { shift: true, alt: true, ..Default::default() }, "left");
    assert_eq!(sel(&ui), (18, 23));
    chord(&mut ui, CMD, "x");
    assert_eq!(clipboard_text(), "w\u{f6}rld");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld h\u{e9}llo ");

    // Text from outside the program; its line breaks become spaces. Ctrl
    // works where Cmd does.
    set_clipboard_text("one\ntwo\r\nthree");
    chord(&mut ui, CMD, "a");
    chord(&mut ui, Modifiers { ctrl: true, ..Default::default() }, "v");
    assert_eq!(buf(&ui), "one two three");

    // The shortcut's letter is not typed, even from a host that sends it.
    ui.event(InputEvent::Modifiers(CMD));
    ui.event(InputEvent::Text { text: "c".to_string() });
    ui.key("c").unwrap();
    ui.event(InputEvent::Modifiers(Modifiers::default()));
    assert_eq!(buf(&ui), "one two three");
}

#[test]
fn undo_and_redo_step_through_edits() {
    let mut ui = field();
    ui.click(slot(11) + 20, 20).unwrap();
    // A run of typing is one step, ending with the word.
    ui.text(" a").unwrap();
    ui.text("b ").unwrap();
    ui.text("\u{e7}d").unwrap();
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld ab \u{e7}d");
    ui.key("backspace").unwrap();
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld ab \u{e7}");

    chord(&mut ui, CMD, "z");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld ab \u{e7}d", "the backspace");
    chord(&mut ui, CMD, "z");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld ab ", "the second word");
    chord(&mut ui, CMD, "z");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld", "the first word");
    assert_eq!(ui.state_int("caret"), Some(11), "the caret goes back with the text");
    chord(&mut ui, CMD, "z");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld", "nothing left to undo");

    chord(&mut ui, CMD_SHIFT, "z");
    chord(&mut ui, CMD, "y");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld ab \u{e7}d");

    // A new edit after an undo drops the redo branch.
    chord(&mut ui, CMD, "z");
    ui.text("!").unwrap();
    chord(&mut ui, CMD_SHIFT, "z");
    assert_eq!(buf(&ui), "h\u{e9}llo w\u{f6}rld ab !");
}

#[test]
fn the_selection_is_painted_and_a_long_field_scrolls_to_its_caret() {
    let mut ui = field();
    ui.click(slot(11) + 20, 20).unwrap();
    chord(&mut ui, SHIFT, "left");
    chord(&mut ui, SHIFT, "left");
    ui.frame().unwrap();
    // "héllo wör" is 9 glyphs: round(75.6) = 76; the run is round(92.4) = 92.
    let selection = ui.commands.iter().find_map(|c| match c {
        DrawCommand::Rect { x, w, .. } if *x == 16 + 76 => Some(*w),
        _ => None,
    });
    assert_eq!(selection, Some(92 - 76), "one rect behind the last two characters");
    assert!(
        !ui.commands.iter().any(|c| matches!(c, DrawCommand::ClipPush { .. })),
        "text that fits is not clipped"
    );

    // 188 px of room holds 22 glyphs; type past it.
    ui.key("end").unwrap();
    ui.text(" and then quite a lot more").unwrap();
    ui.frame().unwrap();
    assert!(ui.commands.iter().any(|c| matches!(c, DrawCommand::ClipPush { .. })));
    let caret_x = ui
        .commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Line { x1, x2, .. } if x1 == x2 => Some(*x1),
            _ => None,
        })
        .expect("caret drawn");
    assert_eq!(caret_x, 10 + 200 - 6, "the caret sits at the right inset, inside the box");
    // And a click still lands on the character under it.
    ui.click(caret_x - 6, 20).unwrap();
    assert_eq!(ui.state_int("caret"), Some(36), "37 characters; the click is inside the last");
}

#[test]
fn wrap_rows_and_range_rects_work_in_characters() {
    // Size 10: 6 px glyphs, so a 60 px box holds ten. Line height comes from
    // the caller here, so the numbers do not depend on the face's metrics.
    let src = "state rows = \"\"\n\
               state rects = \"\"\n\
               state lines = \"\"\n\
               let s = \"h\u{e9}llo w\u{f6}rld \u{65e5}\u{672c}\\n\\nfin\"\n\
               let rs = text_wrap_rows(s, 10, 60.0)\n\
               rows = rs |> map(fn(r) \"{r.text}@{r.start}-{r.end}\" end) |> join(\"|\")\n\
               let fmt = fn(q) \"{q.x},{q.y},{q.w},{q.h}\" end\n\
               rects = text_range_rects(rs, {size: 10}, 8, 18, 12) |> map(fmt) |> join(\"|\")\n\
               lines = text_range_rects(\"\u{e9}\u{e9}\\nabc\", 10, 1, 4, 12) |> map(fmt) |> join(\"|\")";
    let mut ui = Headless::new(src).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.frame().unwrap();
    assert_eq!(
        ui.state_string("rows").as_deref(),
        Some("h\u{e9}llo@0-5|w\u{f6}rld \u{65e5}\u{672c}@6-14|@15-15|fin@16-19")
    );
    // [8, 18): "rld 日本" on row 1 (from x = 12, six glyphs plus the break),
    // the empty row (the break alone), and "fi" on the last.
    assert_eq!(
        ui.state_string("rects").as_deref(),
        Some("12,12,39,12|0,24,3,12|0,36,12,12")
    );
    // A plain string is one row per hard newline.
    assert_eq!(ui.state_string("lines").as_deref(), Some("6,0,9,12|0,12,6,12"));
}

/// The email client's search box is `text_field_update` under the app's own
/// focus string and its own paint: the widget's selection, clipboard and undo
/// work there with no editing code in the app.
#[test]
fn the_email_client_search_field_selects_copies_and_undoes() {
    set_clipboard_provider(None);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app = root.join("examples/productivity/email-client/app.ptl");
    let mut ui = Headless::from_file_with_paths(&app, 1268, 778, &[root.join("core-runtime")])
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    petal_ui::panel_stubs::register_panel_stubs(&mut ui.env);
    let q = |ui: &Headless| ui.state_string("q").unwrap();
    ui.frame().unwrap();
    ui.key("/").unwrap();
    ui.text("caf\u{e9} from:m\u{e9}l").unwrap();
    assert_eq!(q(&ui), "caf\u{e9} from:m\u{e9}l");

    chord(&mut ui, Modifiers { shift: true, alt: true, ..Default::default() }, "left");
    chord(&mut ui, CMD, "x");
    assert_eq!(q(&ui), "caf\u{e9} ");
    assert_eq!(clipboard_text(), "from:m\u{e9}l");
    chord(&mut ui, CMD, "z");
    assert_eq!(q(&ui), "caf\u{e9} from:m\u{e9}l");

    // The search text starts at x = 278; a double click on "café" selects it,
    // and typing replaces it.
    ui.frames(40).unwrap();
    ui.click(278 + 12, 34).unwrap();
    ui.click(278 + 12, 34).unwrap();
    ui.text("t\u{e9}").unwrap();
    assert_eq!(q(&ui), "t\u{e9} from:m\u{e9}l");
    chord(&mut ui, CMD, "a");
    chord(&mut ui, CMD, "v");
    assert_eq!(q(&ui), "from:m\u{e9}l");
}
