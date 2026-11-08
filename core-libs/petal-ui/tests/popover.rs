//! One open popover, kept by the runtime (src/popover.rs): opened by
//! request, closed by Escape or a press outside before the frame runs.

use petal_ui::harness::Headless;
use petal_ui::input::buttons;

/// Two buttons that open two popovers; counters for what got through.
/// "a" (x 10..110, y 10..34) opens popover "pa" under itself; "b" (x 200..300)
/// opens "pb". A plain button "under" (x 10..110, y 40..64) sits where pa's
/// menu opens.
const SRC: &str = r#"
state chosen = ""
state unders = 0
state escapes = 0
state saves = 0
state menu_ups = 0
let ra = {x: 10, y: 10, w: 100, h: 24}
let rb = {x: 200, y: 10, w: 100, h: 24}
if interact(ra, "a").click then popover_open("pa") end
if interact(rb, "b").click then popover_open("pb") end
if interact({x: 10, y: 40, w: 100, h: 24}, "under").click then unders += 1 end
let pa = popover("pa", ra, [menu_item("One"), menu_item("Two")])
if pa.index >= 0 then chosen = pa.label end
let pb = popover("pb", rb, [menu_item("Three")])
if pb.index >= 0 then chosen = pb.label end
if shortcut("escape") then escapes += 1 end
if shortcut("cmd+s") then saves += 1 end
if shortcut("up", "pa") then menu_ups += 1 end
"#;

fn app() -> Headless {
    let mut ui = Headless::with_ui(SRC).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.frames(2).unwrap();
    ui
}

fn click(ui: &mut Headless, x: i32, y: i32) {
    ui.click(x, y).unwrap();
    ui.frames(2).unwrap();
}

#[test]
fn a_click_opens_it_and_a_click_on_an_item_chooses_and_closes() {
    let mut ui = app();
    click(&mut ui, 50, 20);
    assert_eq!(ui.popover(), "pa");
    // pa's menu: top at y 36, rows from y 42, 24 tall.
    click(&mut ui, 50, 50);
    assert_eq!(ui.state_string("chosen").as_deref(), Some("One"));
    assert_eq!(ui.popover(), "");
    assert_eq!(ui.state_int("unders"), Some(0), "the row's press was the menu's");
}

#[test]
fn a_press_outside_closes_it_and_reaches_nothing_underneath() {
    let mut ui = app();
    click(&mut ui, 50, 20);
    assert_eq!(ui.popover(), "pa");
    ui.mouse_move(500, 300);
    ui.mouse_down(buttons::LEFT);
    ui.frame().unwrap();
    // Closed by the roll, before the press frame ran.
    assert_eq!(ui.popover(), "");
    ui.mouse_up(buttons::LEFT);
    ui.frames(2).unwrap();
    // The button the dismissing press was on gets no click from it.
    click(&mut ui, 50, 20);
    click(&mut ui, 250, 200);
    assert_eq!(ui.popover(), "");
    assert_eq!(ui.state_int("unders"), Some(0));
}

#[test]
fn the_dismissing_press_on_a_button_is_not_its_click() {
    let mut ui = app();
    click(&mut ui, 50, 20);
    // "b" is outside pa's menu: the press closes pa and is swallowed.
    click(&mut ui, 250, 20);
    assert_eq!(ui.popover(), "");
}

#[test]
fn opening_one_closes_the_other() {
    let mut ui = app();
    ui.frame().unwrap();
    // Open pa, then (by request, no press) pb.
    let src = r#"
state n = 0
n += 1
if n == 2 then popover_open("pa") end
if n == 4 then popover_open("pb") end
popover("pa", {x: 10, y: 10}, [menu_item("One")])
popover("pb", {x: 300, y: 10}, [menu_item("Two")])
"#;
    let mut ui = Headless::with_ui(src).unwrap();
    ui.frames(3).unwrap();
    assert_eq!(ui.popover(), "pa");
    ui.frames(2).unwrap();
    assert_eq!(ui.popover(), "pb");
}

#[test]
fn escape_closes_it_and_is_not_also_the_app_s() {
    let mut ui = app();
    ui.key("escape").unwrap();
    assert_eq!(ui.state_int("escapes"), Some(1), "nothing open: the app's");
    click(&mut ui, 50, 20);
    assert_eq!(ui.popover(), "pa");
    ui.key("escape").unwrap();
    assert_eq!(ui.popover(), "");
    assert_eq!(ui.state_int("escapes"), Some(1));
}

#[test]
fn while_it_is_open_it_has_the_keys() {
    let mut ui = app();
    click(&mut ui, 50, 20);
    let cmd = petal_ui::input::Modifiers { shift: false, ctrl: false, alt: false, cmd: true };
    ui.event(petal_ui::input::InputEvent::Modifiers(cmd));
    ui.key("s").unwrap();
    ui.event(petal_ui::input::InputEvent::Modifiers(Default::default()));
    ui.key("up").unwrap();
    assert_eq!(ui.state_int("saves"), Some(0));
    assert_eq!(ui.state_int("menu_ups"), Some(1));
}

#[test]
fn a_context_menu_shown_every_frame_still_takes_its_click() {
    // The Phase 0 bug: re-showing a menu every frame re-armed its frame
    // skip, so it never took input.
    let src = r#"
state menu = menu_state()
state took = ""
menu = menu_show(menu, 20, 20, "t")
let res = context_menu(menu, [menu_item("Keep")])
menu = res.menu
if res.index >= 0 then took = res.label end
"#;
    let mut ui = Headless::with_ui(src).unwrap();
    ui.frames(3).unwrap();
    ui.click(50, 35).unwrap();
    ui.frames(1).unwrap();
    assert_eq!(ui.state_string("took").as_deref(), Some("Keep"));
}
