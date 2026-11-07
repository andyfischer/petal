//! The runtime's focus and `shortcut` (src/focus.rs): who has the keyboard,
//! decided once per frame before the script runs, and which keys are theirs.

use petal_ui::harness::Headless;
use petal_ui::input::{InputEvent, Modifiers, buttons};

const CMD: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, cmd: true };
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, cmd: true };

/// A text field ("name", x 10..210, y 10..34), a plain button ("go", x 10..110,
/// y 60..84), a popup-layer rect ("menu", x 300..400) and counters for the
/// shortcuts the app answers.
const SRC: &str = r#"
state fc = focus_state()
state buf = ""
state saves = 0
state undos = 0
state vees = 0
state escapes = 0
state downs = 0
state owner_at_run = ""
owner_at_run = focus_owner()
let res = text_field(fc, "name", {x: 10, y: 10, w: 200, h: 24}, buf)
fc = res.focus
buf = res.text
interact({x: 10, y: 60, w: 100, h: 24}, "go")
interact({x: 10, y: 100, w: 100, h: 24}, "list", {focus: true, keys: ["down"]})
with_layer(UI_LAYER_POPUP, fn() interact({x: 300, y: 10, w: 100, h: 24}, "menu") end)
if shortcut("cmd+s") then saves += 1 end
if shortcut("cmd+z") then undos += 1 end
if shortcut("v") then vees += 1 end
if shortcut("escape") then escapes += 1 end
if shortcut("down", "list") then downs += 1 end
if shortcut("down") then downs += 100 end
"#;

fn app() -> Headless {
    let mut ui = Headless::with_ui(SRC).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.frame().unwrap();
    ui.frame().unwrap();
    ui
}

fn chord(ui: &mut Headless, mods: Modifiers, key: &str) {
    ui.event(InputEvent::Modifiers(mods));
    ui.key(key).unwrap();
    ui.event(InputEvent::Modifiers(Modifiers::default()));
}

fn press(ui: &mut Headless, x: i32, y: i32) {
    ui.click(x, y).unwrap();
    ui.frame().unwrap();
}

fn int(ui: &Headless, name: &str) -> i64 {
    ui.state_int(name).unwrap()
}

#[test]
fn a_press_on_a_field_gives_it_the_keyboard_on_that_frame() {
    let mut ui = app();
    assert_eq!(ui.focus(), "");
    ui.mouse_move(50, 20);
    ui.mouse_down(buttons::LEFT);
    ui.frame().unwrap();
    // Decided before the script ran: the press frame's own run already saw it.
    assert_eq!(ui.focus(), "name");
    assert_eq!(ui.state_string("owner_at_run").as_deref(), Some("name"));
    ui.mouse_up(buttons::LEFT);
    ui.frame().unwrap();
    assert_eq!(ui.focus(), "name");
}

#[test]
fn typing_keys_are_the_field_s_but_save_is_still_the_app_s() {
    let mut ui = app();
    press(&mut ui, 50, 20);
    ui.key("v").unwrap();
    ui.key("escape").unwrap();
    chord(&mut ui, CMD, "z");
    chord(&mut ui, CMD, "s");
    assert_eq!(int(&ui, "vees"), 0);
    assert_eq!(int(&ui, "escapes"), 0);
    assert_eq!(int(&ui, "undos"), 0, "Cmd+Z undoes the field's typing, not the app's");
    assert_eq!(int(&ui, "saves"), 1);
}

#[test]
fn with_nothing_focused_every_shortcut_is_the_app_s() {
    let mut ui = app();
    ui.key("v").unwrap();
    ui.key("escape").unwrap();
    chord(&mut ui, CMD, "z");
    assert_eq!((int(&ui, "vees"), int(&ui, "escapes"), int(&ui, "undos")), (1, 1, 1));
}

#[test]
fn a_chord_matches_only_its_own_modifiers() {
    let mut ui = app();
    chord(&mut ui, CMD_SHIFT, "z");
    chord(&mut ui, CMD, "v");
    assert_eq!(int(&ui, "undos"), 0);
    assert_eq!(int(&ui, "vees"), 0);
}

#[test]
fn a_press_elsewhere_takes_the_keyboard_but_one_on_a_higher_layer_does_not() {
    let mut ui = app();
    press(&mut ui, 50, 20);
    press(&mut ui, 350, 20);
    assert_eq!(ui.focus(), "name", "the popup layer took the press");
    press(&mut ui, 50, 70);
    assert_eq!(ui.focus(), "", "a button on the field's layer");
    ui.key("v").unwrap();
    assert_eq!(int(&ui, "vees"), 1);
}

#[test]
fn a_rect_that_wants_focus_gets_its_own_keys() {
    let mut ui = app();
    ui.key("down").unwrap();
    assert_eq!(int(&ui, "downs"), 100, "nobody focused: the app's");
    press(&mut ui, 50, 110);
    assert_eq!(ui.focus(), "list");
    ui.key("down").unwrap();
    assert_eq!(int(&ui, "downs"), 101, "the list's, and claimed from the app");
    ui.key("v").unwrap();
    assert_eq!(int(&ui, "vees"), 1, "the list claims only what it listed");
}

#[test]
fn focus_request_applies_on_the_next_frame_and_nil_clears() {
    let src = r#"
state n = 0
n += 1
if n == 3 then focus_request("x") end
if n == 6 then focus_request(nil) end
"#;
    let mut ui = Headless::with_ui(src).unwrap();
    ui.frames(3).unwrap();
    assert_eq!(ui.focus(), "");
    ui.frame().unwrap();
    assert_eq!(ui.focus(), "x");
    ui.frames(3).unwrap();
    assert_eq!(ui.focus(), "");
}

#[test]
fn threading_fc_by_hand_keeps_the_runtime_in_step() {
    // Old-style code: focus moved with focus_set, never by a press.
    let src = r#"
state fc = focus_state()
state n = 0
n += 1
if n == 2 then fc = focus_set(fc, "search") end
if n == 4 then fc = focus_clear(fc) end
"#;
    let mut ui = Headless::with_ui(src).unwrap();
    ui.frames(2).unwrap();
    ui.frame().unwrap();
    assert_eq!(ui.focus(), "search");
    ui.frames(2).unwrap();
    assert_eq!(ui.focus(), "");
}

#[test]
fn a_bad_chord_is_an_error_not_a_quiet_false() {
    let mut ui = Headless::with_ui("shortcut(\"hyper+k\")").unwrap();
    let err = ui.frame().unwrap_err();
    assert!(err.contains("not a chord"), "{err}");
}
