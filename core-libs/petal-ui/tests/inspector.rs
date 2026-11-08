//! The inspector fields (`commit_field`, `number_field`, `color_field`,
//! `swatches`, `stepper`, `segmented`, `dropdown`), the hex colour helpers,
//! `scroll_keys`, and `text_field`'s `blurred` / `cancelled`, driven
//! headlessly with injected input.

use petal_ui::harness::Headless;
use petal_ui::input::{InputEvent, Modifiers, buttons};

fn ui(src: &str) -> Headless {
    let mut ui = Headless::with_ui(src).unwrap_or_else(|e| panic!("compile failed: {e}"));
    ui.frame().unwrap();
    ui
}

/// Click at (x, y) and run the frame that sees the release.
fn click(ui: &mut Headless, x: i32, y: i32) {
    ui.click(x, y).unwrap();
    ui.frame().unwrap();
}

fn double_click(ui: &mut Headless, x: i32, y: i32) {
    click(ui, x, y);
    click(ui, x, y);
}

// ── Hex colours ─────────────────────────────────────────────────────────────

#[test]
fn hex_round_trips_and_rejects_what_names_no_colour() {
    let mut ui = ui("state a = color_to_hex({r: 255, g: 16, b: 0})\n\
                     state b = color_to_hex({r: 1, g: 2, b: 3, a: 128})\n\
                     state c = color_to_hex(color_from_hex(\" #ABC \"))\n\
                     state d = color_to_hex(color_from_hex(\"11223344\"))\n\
                     state e = color_from_hex(\"#12345g\") == nil\n\
                     state f = color_from_hex(\"#1234\") == nil\n\
                     state g = same_color({r: 1, g: 2, b: 3}, {r: 1, g: 2, b: 3, a: 255})");
    ui.frame().unwrap();
    assert_eq!(ui.state_string("a").as_deref(), Some("#ff1000"));
    assert_eq!(ui.state_string("b").as_deref(), Some("#01020380"), "alpha only when translucent");
    assert_eq!(ui.state_string("c").as_deref(), Some("#aabbcc"), "#rgb doubles each digit");
    assert_eq!(ui.state_string("d").as_deref(), Some("#11223344"));
    for k in ["e", "f", "g"] {
        assert_eq!(ui.state()[k], true, "{k}");
    }
}

// ── text_field: blurred and cancelled ───────────────────────────────────────

#[test]
fn text_field_reports_blur_and_escape() {
    let mut ui = ui("state blurs = 0\n\
                     state cancels = 0\n\
                     let res = text_field(\"f\", {x: 10, y: 10, w: 200, h: 24}, \"x\")\n\
                     if res.blurred then blurs += 1 end\n\
                     if res.cancelled then cancels += 1 end");
    click(&mut ui, 20, 20);
    ui.key("escape").unwrap();
    assert_eq!(ui.state_int("cancels"), Some(1));
    assert_eq!(ui.state_int("blurs"), Some(0), "escape alone keeps the focus");
    click(&mut ui, 300, 300);
    ui.frame().unwrap();
    assert_eq!(ui.state_int("blurs"), Some(1), "a click away blurs once");
}

// ── commit_field ────────────────────────────────────────────────────────────

const COMMIT: &str = "state name = \"Layer\"\n\
                      state applied = 0\n\
                      let res = commit_field(\"name\", {x: 10, y: 10, w: 200, h: 24}, name)\n\
                      if res.text != nil then\n\
                        name = res.text\n\
                        applied += 1\n\
                      end";

#[test]
fn commit_field_applies_on_return_not_while_typing() {
    let mut ui = ui(COMMIT);
    click(&mut ui, 150, 20);
    ui.text("s").unwrap();
    assert_eq!(ui.state_string("name").as_deref(), Some("Layer"), "typing alone applies nothing");
    ui.key("return").unwrap();
    assert_eq!(ui.state_string("name").as_deref(), Some("Layers"));
    assert_eq!(ui.state_int("applied"), Some(1));
}

#[test]
fn commit_field_applies_on_blur_and_reverts_on_escape() {
    let mut ui = ui(COMMIT);
    click(&mut ui, 150, 20);
    ui.text("1").unwrap();
    click(&mut ui, 400, 400);
    ui.frame().unwrap();
    assert_eq!(ui.state_string("name").as_deref(), Some("Layer1"), "a click away applies");

    click(&mut ui, 150, 20);
    ui.text("zz").unwrap();
    ui.key("escape").unwrap();
    ui.frames(2).unwrap();
    assert_eq!(ui.state_string("name").as_deref(), Some("Layer1"), "escape drops the edit");
    assert_eq!(ui.state_int("applied"), Some(1));
}

// ── number_field ────────────────────────────────────────────────────────────

const NUMBER: &str = "state v = 10.0\n\
                      let res = number_field(\"n\", {x: 10, y: 10, w: 80, h: 24}, v, {step: 0.5, places: 1, min: 0, max: 50})\n\
                      v = res.value";

#[test]
fn number_field_scrubs_by_step_per_pixel_and_clamps() {
    let mut ui = ui(NUMBER);
    ui.mouse_move(40, 20);
    ui.mouse_down(buttons::LEFT);
    ui.frame().unwrap();
    ui.mouse_move(50, 20);
    ui.frame().unwrap();
    assert_eq!(ui.state_float("v"), Some(15.0), "10 px × 0.5");

    // Shift scrubs at a tenth of the rate.
    ui.event(InputEvent::Modifiers(Modifiers { shift: true, ..Modifiers::default() }));
    ui.mouse_move(60, 20);
    ui.frame().unwrap();
    assert_eq!(ui.state_float("v"), Some(15.5));
    ui.event(InputEvent::Modifiers(Modifiers::default()));

    ui.mouse_move(400, 20);
    ui.frame().unwrap();
    assert_eq!(ui.state_float("v"), Some(50.0), "clamped to max");
    ui.mouse_up(buttons::LEFT);
    ui.frame().unwrap();
}

#[test]
fn number_field_scrubs_without_bounds() {
    // A style with no `min` or `max`: the clamp must not index them (a
    // missing key is an error), which broke every editor's scrubbing.
    let mut ui = ui("state v = 10.0\n\
                     let res = number_field(\"n\", {x: 10, y: 10, w: 80, h: 24}, v, {step: 1, places: 0})\n\
                     v = res.value");
    ui.mouse_move(40, 20);
    ui.mouse_down(buttons::LEFT);
    ui.frame().unwrap();
    ui.mouse_move(46, 20);
    ui.frame().unwrap();
    assert_eq!(ui.state_float("v"), Some(16.0));
    ui.mouse_up(buttons::LEFT);
    ui.frame().unwrap();
}

#[test]
fn number_field_types_on_double_click() {
    let mut ui = ui(NUMBER);
    double_click(&mut ui, 40, 20);
    // The field opens holding the value, all of it selectable; replace it.
    for _ in 0..6 {
        ui.key("backspace").unwrap();
    }
    ui.text("42.5").unwrap();
    assert_eq!(ui.state_float("v"), Some(10.0), "not applied while typing");
    ui.key("return").unwrap();
    assert_eq!(ui.state_float("v"), Some(42.5));

    // Escape abandons a typed value.
    double_click(&mut ui, 40, 20);
    ui.key("backspace").unwrap();
    ui.key("escape").unwrap();
    ui.frames(2).unwrap();
    assert_eq!(ui.state_float("v"), Some(42.5));
}

// ── color_field and swatches ────────────────────────────────────────────────

#[test]
fn color_field_applies_a_valid_hex_only() {
    let mut ui = ui("state c = {r: 0, g: 0, b: 0}\n\
                     state hex = \"\"\n\
                     let res = color_field(\"c\", {x: 10, y: 10, w: 160, h: 24}, c)\n\
                     if res.color != nil then c = res.color end\n\
                     hex = color_to_hex(c)");
    click(&mut ui, 100, 20);
    for _ in 0..7 {
        ui.key("backspace").unwrap();
    }
    ui.text("nope").unwrap();
    ui.key("return").unwrap();
    assert_eq!(ui.state_string("hex").as_deref(), Some("#000000"), "an invalid hex changes nothing");

    click(&mut ui, 100, 20);
    for _ in 0..7 {
        ui.key("backspace").unwrap();
    }
    ui.text("#3366ff").unwrap();
    ui.key("return").unwrap();
    ui.frame().unwrap();
    assert_eq!(ui.state_string("hex").as_deref(), Some("#3366ff"));
}

#[test]
fn swatches_return_the_chip_clicked() {
    let mut ui = ui("state hex = \"\"\n\
                     let picked = swatches({x: 10, y: 10, w: 200, h: 20}, [#ff0000, #00ff00, #0000ff], nil)\n\
                     if picked != nil then hex = color_to_hex(picked) end");
    // Chips are 20 px with a 4 px gap: the second spans x 34..54.
    click(&mut ui, 44, 20);
    assert_eq!(ui.state_string("hex").as_deref(), Some("#00ff00"));
}

// ── stepper, segmented, dropdown ────────────────────────────────────────────

#[test]
fn stepper_steps_both_ways_and_wraps() {
    let mut ui = ui("state v = \"b\"\n\
                     v = stepper({x: 10, y: 10, w: 120, h: 24}, [\"a\", \"b\", \"c\"], v)");
    click(&mut ui, 120, 20); // ›
    assert_eq!(ui.state_string("v").as_deref(), Some("c"));
    click(&mut ui, 120, 20);
    assert_eq!(ui.state_string("v").as_deref(), Some("a"), "wraps forward");
    click(&mut ui, 20, 20); // ‹
    assert_eq!(ui.state_string("v").as_deref(), Some("c"), "wraps back");
    click(&mut ui, 70, 20); // the value steps forward
    assert_eq!(ui.state_string("v").as_deref(), Some("a"));
}

#[test]
fn segmented_selects_the_segment_clicked() {
    let mut ui = ui("state i = 0\n\
                     i = segmented({x: 0, y: 0, w: 300, h: 24}, [\"Left\", \"Centre\", \"Right\"], i)");
    click(&mut ui, 250, 12);
    assert_eq!(ui.state_int("i"), Some(2));
    click(&mut ui, 150, 12);
    assert_eq!(ui.state_int("i"), Some(1));
}

#[test]
fn dropdown_opens_a_menu_and_returns_the_choice() {
    let mut ui = ui("state v = \"Inter\"\n\
                     v = dropdown({x: 10, y: 10, w: 160, h: 24}, [\"Inter\", \"Georgia\", \"Menlo\"], v)");
    click(&mut ui, 50, 20);
    // The menu opens under the button; rows are 24 px after 6 px of padding,
    // so "Menlo" (row 2) sits at y = 36 + 6 + 48 + 12.
    click(&mut ui, 60, 102);
    assert_eq!(ui.state_string("v").as_deref(), Some("Menlo"));
}

// ── scroll_keys ─────────────────────────────────────────────────────────────

#[test]
fn scroll_keys_move_by_line_page_and_end() {
    let mut ui = ui("state off = 0\n\
                     off = scroll_keys(off, 100, 10)");
    ui.key("j").unwrap();
    ui.key("down").unwrap();
    assert_eq!(ui.state_int("off"), Some(2));
    ui.key("space").unwrap();
    assert_eq!(ui.state_int("off"), Some(12));
    ui.key("end").unwrap();
    assert_eq!(ui.state_int("off"), Some(90), "clamped so the last page fills the view");
    ui.key("k").unwrap();
    assert_eq!(ui.state_int("off"), Some(89));
    ui.key("home").unwrap();
    assert_eq!(ui.state_int("off"), Some(0));
}

// ── What the editors' copies did that ui now does too ───────────────────────

#[test]
fn commit_field_with_start_takes_the_keyboard_when_it_shows() {
    let mut ui = ui("state name = \"a\"\n\
                     let res = commit_field(\"rename\", {x: 10, y: 10, w: 200, h: 24}, name, {start: true})\n\
                     if res.text != nil then name = res.text end");
    ui.text("b").unwrap();
    ui.key("return").unwrap();
    assert_eq!(ui.state_string("name").as_deref(), Some("ab"), "typed with no click first");
}

#[test]
fn color_field_shows_a_nil_colour_empty() {
    let mut ui = ui("state hex = \"none\"\n\
                     let res = color_field(\"c\", {x: 10, y: 10, w: 160, h: 24}, nil)\n\
                     if res.color != nil then hex = color_to_hex(res.color) end");
    click(&mut ui, 100, 20);
    ui.text("#123").unwrap();
    ui.key("return").unwrap();
    assert_eq!(ui.state_string("hex").as_deref(), Some("#112233"));
}

#[test]
fn swatches_leave_out_chips_that_do_not_fit() {
    let mut ui = ui("state n = 0\n\
                     let picked = swatches({x: 0, y: 0, w: 50, h: 20}, [#ff0000, #00ff00, #0000ff], nil)\n\
                     if picked != nil then n += 1 end");
    click(&mut ui, 49, 10); // where the third chip (48..68) would start
    assert_eq!(ui.state_int("n"), Some(0));
    assert!(ui.target("swatch-0-0-2").is_none(), "the third chip is not drawn");
    assert!(ui.target("swatch-0-0-1").is_some(), "the second, 24..44, fits");
}

#[test]
fn text_area_select_all_selects_everything_on_that_frame() {
    let mut ui = ui("state asked = false\n\
                     if !asked then\n\
                       focus_request(\"t\")\n\
                       asked = true\n\
                     end\n\
                     state buf = \"Your text\"\n\
                     state fresh = true\n\
                     let res = text_area(\"t\", {x: 10, y: 10, w: 200, h: 80}, buf, {select_all: fresh})\n\
                     fresh = false\n\
                     buf = res.text");
    ui.text("Hi").unwrap();
    assert_eq!(ui.state_string("buf").as_deref(), Some("Hi"), "typing replaced the selected text");
}
