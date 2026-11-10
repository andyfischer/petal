//! bloom's fields (`core-runtime/bloom/src/fields.ptl`) as a Garden panel sees
//! them: the text field and text area on `ui`'s editing logic (selection,
//! clipboard, undo, blur, cancel), the inspector fields that apply on Return
//! or blur, and the one keyboard owner bloom's Tab ring shares with `ui`.
//!
//! Positions avoid depending on glyph widths: a click lands well inside a
//! field, and the caret is moved with keys.

use garden_script::{InputEvent, Modifiers, PanelHost};

const W: i32 = 400;
const H: i32 = 300;
const DT: f64 = 1.0 / 60.0;

const CMD: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, cmd: true };
const CMD_SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, cmd: true };
const SHIFT: Modifiers = Modifiers { shift: true, ctrl: false, alt: false, cmd: false };

struct Ui {
    host: PanelHost,
    frame: i64,
}

impl Ui {
    fn new(source: &str) -> Ui {
        let mut host =
            PanelHost::from_source("bloom-fields-test", &format!("import ui; {source}")).expect("compiles");
        host.set_dimensions(W, H);
        let mut ui = Ui { host, frame: 0 };
        ui.tick(1);
        ui
    }

    fn tick(&mut self, n: usize) {
        for _ in 0..n {
            self.host.frame(DT, self.frame).expect("frame");
            self.frame += 1;
        }
    }

    fn click(&mut self, x: i32, y: i32) {
        self.host.input_event(InputEvent::MouseMove { x, y });
        self.host.input_event(InputEvent::MouseDown { button: 0 });
        self.tick(1);
        self.host.input_event(InputEvent::MouseUp { button: 0 });
        self.tick(1);
    }

    fn key(&mut self, key: &str) {
        self.host.input_event(InputEvent::KeyDown { key: key.into() });
        self.host.input_event(InputEvent::KeyUp { key: key.into() });
        self.tick(1);
    }

    fn chord(&mut self, mods: Modifiers, key: &str) {
        self.host.input_event(InputEvent::Modifiers(mods));
        self.key(key);
        self.host.input_event(InputEvent::Modifiers(Modifiers::default()));
        self.tick(1);
    }

    fn text(&mut self, text: &str) {
        self.host.input_event(InputEvent::Text { text: text.into() });
        self.tick(1);
    }

    fn string(&self, name: &str) -> Option<String> {
        Some(self.host.state_json().get(name)?.as_str()?.to_string())
    }

    fn int(&self, name: &str) -> Option<i64> {
        self.host.state_json().get(name)?.as_i64()
    }

    fn float(&self, name: &str) -> Option<f64> {
        self.host.state_json().get(name)?.as_f64()
    }

    fn value(&self, name: &str) -> Option<i64> {
        self.host.observed_json().get(name)?.as_i64()
    }
}

const FIELD: &str = "import bloom: text_field\n\
                     state text = \"hello\"\n\
                     state submits = 0\n\
                     state cancels = 0\n\
                     let f = text_field(rect(20, 20, 300, 30), text, {id: \"f\"})\n\
                     text = f.text\n\
                     if f.submitted then submits += 1 end\n\
                     if f.cancelled then cancels += 1 end\n\
                     let sel = f.sel_end - f.sel_start\n\
                     let has = if f.focused then 1 else 0 end\n";

#[test]
fn typing_inserts_and_return_submits() {
    let mut ui = Ui::new(FIELD);
    ui.click(300, 35);
    assert_eq!(ui.value("has"), Some(1), "a click gives the field the keyboard");
    ui.key("end");
    ui.text("!");
    assert_eq!(ui.string("text").as_deref(), Some("hello!"));
    ui.key("return");
    assert_eq!(ui.int("submits"), Some(1));
    ui.key("escape");
    assert_eq!(ui.int("cancels"), Some(1));
}

#[test]
fn shift_arrows_select_and_typing_replaces_the_selection() {
    let mut ui = Ui::new(FIELD);
    ui.click(300, 35);
    ui.key("end");
    for _ in 0..3 {
        ui.chord(SHIFT, "left");
    }
    assert_eq!(ui.value("sel"), Some(3));
    ui.text("p");
    assert_eq!(ui.string("text").as_deref(), Some("hep"));
}

#[test]
fn select_all_copy_and_paste_go_through_the_clipboard() {
    let mut ui = Ui::new(FIELD);
    ui.click(300, 35);
    ui.chord(CMD, "a");
    assert_eq!(ui.value("sel"), Some(5), "cmd+a selects everything");
    ui.chord(CMD, "c");
    ui.key("end");
    ui.chord(CMD, "v");
    assert_eq!(ui.string("text").as_deref(), Some("hellohello"));
}

#[test]
fn undo_and_redo_step_through_the_edits() {
    let mut ui = Ui::new(FIELD);
    ui.click(300, 35);
    ui.key("end");
    ui.text("x");
    assert_eq!(ui.string("text").as_deref(), Some("hellox"));
    ui.chord(CMD, "z");
    assert_eq!(ui.string("text").as_deref(), Some("hello"));
    ui.chord(CMD_SHIFT, "z");
    assert_eq!(ui.string("text").as_deref(), Some("hellox"));
}

#[test]
fn a_text_area_takes_line_breaks() {
    let mut ui = Ui::new(
        "import bloom: text_area\n\
         state notes = \"a\"\n\
         let res = text_area(rect(20, 20, 300, 120), notes, {id: \"notes\"})\n\
         notes = res.text\n\
         let rows = len(res.rows)\n",
    );
    ui.click(200, 80);
    ui.chord(CMD, "down"); // to the end of the text
    ui.key("return");
    ui.text("b");
    assert_eq!(ui.string("notes").as_deref(), Some("a\nb"));
    assert_eq!(ui.value("rows"), Some(2));
}

#[test]
fn a_commit_field_applies_on_return_and_on_blur_and_escape_reverts() {
    let mut ui = Ui::new(
        "import bloom: commit_field, button\n\
         state name = \"old\"\n\
         let res = commit_field(rect(20, 20, 300, 30), name, {id: \"name\"})\n\
         if res.text != nil then name = res.text end\n\
         if button(rect(20, 200, 100, 28), \"Elsewhere\") then end\n",
    );
    // Return applies.
    ui.click(300, 35);
    ui.key("end");
    ui.text("1");
    assert_eq!(ui.string("name").as_deref(), Some("old"), "nothing applies mid-edit");
    ui.key("return");
    assert_eq!(ui.string("name").as_deref(), Some("old1"));

    // A click elsewhere applies too.
    ui.click(300, 35);
    ui.key("end");
    ui.text("2");
    ui.click(60, 214);
    ui.tick(1);
    assert_eq!(ui.string("name").as_deref(), Some("old12"));

    // Escape drops the edit.
    ui.click(300, 35);
    ui.key("end");
    ui.text("3");
    ui.key("escape");
    ui.tick(1);
    assert_eq!(ui.string("name").as_deref(), Some("old12"));
}

#[test]
fn a_number_field_scrubs_and_takes_a_typed_value() {
    let mut ui = Ui::new(
        "import bloom: number_field\n\
         state v = 10.0\n\
         let res = number_field(rect(20, 20, 100, 24), v, {id: \"n\", tip: \"\"})\n\
         v = res.value\n",
    );
    // Drag 15 px right: 15 steps of 1.0.
    ui.host.input_event(InputEvent::MouseMove { x: 60, y: 32 });
    ui.host.input_event(InputEvent::MouseDown { button: 0 });
    ui.tick(1);
    ui.host.input_event(InputEvent::MouseMove { x: 75, y: 32 });
    ui.tick(1);
    ui.host.input_event(InputEvent::MouseUp { button: 0 });
    ui.tick(2);
    assert_eq!(ui.float("v"), Some(25.0));

    // A double click opens it for typing; Return applies the typed number.
    for _ in 0..2 {
        ui.host.input_event(InputEvent::MouseDown { button: 0 });
        ui.tick(1);
        ui.host.input_event(InputEvent::MouseUp { button: 0 });
        ui.tick(1);
    }
    ui.tick(1);
    ui.chord(CMD, "a");
    ui.text("3.5");
    ui.key("return");
    ui.tick(1);
    assert_eq!(ui.float("v"), Some(3.5));
}

#[test]
fn tab_reaches_a_text_field_and_the_focus_is_uis() {
    let mut ui = Ui::new(
        "import bloom: button, text_field\n\
         state text = \"\"\n\
         if button(rect(20, 20, 100, 28), \"A\", {id: \"a\"}) then end\n\
         let f = text_field(rect(20, 60, 300, 30), text, {id: \"f\"})\n\
         text = f.text\n\
         let owner_is_f = if has_focus(\"f\") then 1 else 0 end\n",
    );
    ui.key("tab");
    ui.tick(1);
    ui.key("tab");
    ui.tick(1);
    assert_eq!(ui.value("owner_is_f"), Some(1), "Tab walked the ring onto the field");
    ui.text("hi");
    assert_eq!(ui.string("text").as_deref(), Some("hi"));

    // `ui.focus_request` moves bloom's focus, because it is the same owner.
    let mut ui = Ui::new(
        "import bloom: focused\n\
         if frame_count() == 2 then focus_request(\"x\") end\n\
         let b = if focused(\"x\") then 1 else 0 end\n",
    );
    ui.tick(3);
    assert_eq!(ui.value("b"), Some(1));
}

#[test]
fn swatches_and_a_color_field_pick_colours() {
    let mut ui = Ui::new(
        "import bloom: swatches, color_field\n\
         state c = #ff0000\n\
         let p = swatches(rect(20, 20, 200, 20), [#ff0000, #00ff00, #0000ff], c)\n\
         if p != nil then c = p end\n\
         let res = color_field(rect(20, 80, 200, 26), c, {id: \"hex\"})\n\
         if res.color != nil then c = res.color end\n\
         let g = c.g\n\
         let b = c.b\n",
    );
    ui.click(20 + 24 + 10, 30); // the second chip
    assert_eq!(ui.value("g"), Some(255));
    ui.click(150, 93);
    ui.chord(CMD, "a");
    ui.text("#0000ff");
    ui.key("return");
    ui.tick(1);
    assert_eq!(ui.value("b"), Some(255));
    assert_eq!(ui.value("g"), Some(0));
}
