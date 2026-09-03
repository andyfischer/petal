//! Integration tests for the bloom component library as a Garden panel sees
//! it: the modules are registered (so `import bloom` works from a pushed
//! drawer with no directory of its own), the components respond to real
//! pointer and key input, and the animation settles.
//!
//! These are the library's tests, not Garden's. They live here because this is
//! the host that embeds it, and driving a `PanelHost` is the closest thing to
//! how a panel actually runs.

use garden_script::{InputEvent, PanelHost};

const W: i32 = 400;
const H: i32 = 300;
const DT: f64 = 1.0 / 60.0;

/// A panel driven the way the app drives one: frames advance one at a time and
/// the frame counter is the driver's, never the test's. bloom reads
/// `frame_count()` (for its input capture and its focus ring), so a test that
/// skipped a number would be testing a frame sequence no host produces.
struct Ui {
    host: PanelHost,
    frame: i64,
}

impl Ui {
    fn new(source: &str) -> Ui {
        let mut host = PanelHost::from_source("bloom-test", source).expect("compiles");
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

    /// Press and release at (x, y) on consecutive frames, then let the release
    /// edge clear — the shape of a real click, and what bloom's
    /// press-then-release click requires.
    fn click(&mut self, x: i32, y: i32) {
        self.host.input_event(InputEvent::MouseMove { x, y });
        self.host.input_event(InputEvent::MouseDown { button: 0 });
        self.tick(1);
        self.host.input_event(InputEvent::MouseUp { button: 0 });
        self.tick(1);
    }

    fn press(&mut self, x: i32, y: i32) {
        self.host.input_event(InputEvent::MouseMove { x, y });
        self.host.input_event(InputEvent::MouseDown { button: 0 });
        self.tick(1);
    }

    fn release_at(&mut self, x: i32, y: i32) {
        self.host.input_event(InputEvent::MouseMove { x, y });
        self.host.input_event(InputEvent::MouseUp { button: 0 });
        self.tick(1);
    }

    fn key(&mut self, key: &str) {
        self.host.input_event(InputEvent::KeyDown { key: key.into() });
        self.host.input_event(InputEvent::KeyUp { key: key.into() });
        self.tick(1);
    }

    fn text(&mut self, text: &str) {
        self.host.input_event(InputEvent::Text { text: text.into() });
        self.tick(1);
    }

    fn int(&self, name: &str) -> Option<i64> {
        self.host.state_json().get(name)?.as_i64()
    }

    fn boolean(&self, name: &str) -> Option<bool> {
        self.host.state_json().get(name)?.as_bool()
    }

    fn string(&self, name: &str) -> Option<String> {
        Some(self.host.state_json().get(name)?.as_str()?.to_string())
    }

    /// A `let` binding from the last frame, by name (Garden's own panel
    /// inspection channel).
    fn value(&self, name: &str) -> Option<i64> {
        self.host.observed_json().get(name)?.as_i64()
    }

    /// The single `state` slot whose key contains `needle` — the animators
    /// inside a component are keyed by their whole call path, and the path
    /// depends on how the component was imported, so a test matches on the
    /// animator's own variable rather than spelling the path out.
    fn anim(&self, needle: &str) -> f64 {
        let matches: Vec<f64> = self
            .host
            .state_json()
            .iter()
            .filter(|(k, _)| k.contains(needle))
            .filter_map(|(_, v)| v.as_f64())
            .collect();
        assert_eq!(matches.len(), 1, "one animator matches `{needle}`");
        matches[0]
    }
}

#[test]
fn bloom_is_importable_from_a_pushed_drawer() {
    // `from_source` has no file to resolve imports against: this passes only
    // because Garden registers the library's modules in memory.
    let ui = Ui::new("import bloom\nlet v = bloom.VERSION\n");
    assert_eq!(ui.value("v"), Some(1));
}

#[test]
fn a_button_fires_on_release_inside_it() {
    let mut ui = Ui::new(
        "import bloom: button\n\
         state hits = 0\n\
         if button(rect(20, 20, 120, 30), \"Save\") then hits += 1 end\n",
    );
    ui.click(60, 35);
    assert_eq!(ui.int("hits"), Some(1));

    // A press inside that releases outside is not a click.
    ui.press(60, 35);
    ui.release_at(300, 200);
    assert_eq!(
        ui.int("hits"),
        Some(1),
        "sliding off a button cancels the click"
    );
}

#[test]
fn a_switch_toggles_and_its_knob_settles_exactly() {
    let mut ui = Ui::new(
        "import bloom: switch\n\
         state on = false\n\
         on = switch(rect(20, 20, 46, 24), on)\n",
    );
    ui.click(40, 32);
    assert_eq!(ui.boolean("on"), Some(true));

    // The knob is a spring: still travelling right after the toggle, and
    // parked exactly on 1.0 half a second later — animation that never quite
    // arrives would make every still frame differ.
    assert!(ui.anim("bloom_motion::x") < 1.0);
    ui.tick(40);
    assert_eq!(ui.anim("bloom_motion::x"), 1.0);
}

#[test]
fn an_open_menu_captures_input_from_the_panel_under_it() {
    let mut ui = Ui::new(
        "import bloom: dropdown, button\n\
         state picked = -1\n\
         state under = 0\n\
         let c = dropdown(rect(20, 20, 140, 30), \"Actions\", [\"Rename\", \"Delete\"])\n\
         if c >= 0 then picked = c end\n\
         if button(rect(20, 150, 100, 28), \"Under\") then under += 1 end\n",
    );
    ui.click(60, 35); // opens the menu
    ui.tick(20); // let it finish opening
    ui.click(60, 105); // lands on the popup's second row, over the button below
    assert_eq!(
        ui.int("under"),
        Some(0),
        "the button under the menu never saw the click"
    );
    assert_eq!(ui.int("picked"), Some(1), "the menu row took it instead");

    // With the menu closed the same button works again.
    ui.tick(20);
    ui.click(60, 164);
    assert_eq!(ui.int("under"), Some(1));
}

#[test]
fn escape_closes_a_menu_and_gives_the_panel_its_input_back() {
    let mut ui = Ui::new(
        "import bloom: dropdown, menu_any_open\n\
         let c = dropdown(rect(20, 20, 140, 30), \"Actions\", [\"Rename\", \"Delete\"])\n\
         let open = if menu_any_open() then 1 else 0 end\n",
    );
    ui.click(60, 35);
    assert_eq!(ui.value("open"), Some(1));
    ui.key("escape");
    assert_eq!(ui.value("open"), Some(0));
}

#[test]
fn tab_moves_focus_and_return_fires_the_focused_button() {
    let mut ui = Ui::new(
        "import bloom: button\n\
         state a = 0\n\
         state b = 0\n\
         if button(rect(20, 20, 100, 28), \"A\", {id: \"a\"}) then a += 1 end\n\
         if button(rect(20, 60, 100, 28), \"B\", {id: \"b\"}) then b += 1 end\n",
    );
    // Tab is applied against the ring the previous frame built, so focus lands
    // on the first control in draw order on the frame after the key.
    ui.key("tab");
    ui.tick(1);
    ui.key("return");
    assert_eq!(ui.int("a"), Some(1));
    assert_eq!(ui.int("b"), Some(0));

    ui.key("tab");
    ui.tick(1);
    ui.key("return");
    assert_eq!(ui.int("b"), Some(1), "the ring moved on to the next control");
}

#[test]
fn a_text_field_edits_at_the_caret() {
    let mut ui = Ui::new(
        "import bloom: text_field\n\
         state text = \"ab\"\n\
         let f = text_field(rect(20, 20, 200, 30), text, {id: \"f\"})\n\
         text = f.text\n",
    );
    ui.click(190, 35); // focus, caret past the end of the text
    ui.text("c");
    assert_eq!(ui.string("text").as_deref(), Some("abc"));
    ui.key("left");
    ui.key("backspace");
    assert_eq!(
        ui.string("text").as_deref(),
        Some("ac"),
        "backspace deletes before the caret, not at the end"
    );
}

#[test]
fn toasts_expire_on_their_own_clock() {
    let mut ui = Ui::new(
        "import bloom: toast, toasts, toast_count, button\n\
         if button(rect(20, 20, 100, 28), \"Go\") then toast(\"Saved\", \"success\") end\n\
         toasts(rect(0, 0, screen_width(), screen_height()), {life: 0.5})\n\
         let live = toast_count()\n",
    );
    ui.click(60, 34);
    assert_eq!(ui.value("live"), Some(1));
    ui.tick(40); // past the 0.5s life at 60 fps
    assert_eq!(
        ui.value("live"),
        Some(0),
        "the toast expired without anyone dismissing it"
    );
}

#[test]
fn a_dialog_blocks_the_panel_and_escape_closes_it() {
    let mut ui = Ui::new(
        "import bloom: dialog, dialog_open, button\n\
         state opened = 0\n\
         state clicks = 0\n\
         if button(rect(20, 20, 100, 28), \"Open\") then dialog_open(\"d\") end\n\
         if button(rect(20, 260, 100, 28), \"Other\") then clicks += 1 end\n\
         let d = dialog(\"d\", 200, 120, \"Title\")\n\
         let is_open = if d.open then 1 else 0 end\n",
    );
    ui.click(60, 34);
    ui.tick(15);
    assert_eq!(ui.value("is_open"), Some(1));
    ui.click(60, 274); // a control behind the modal
    assert_eq!(ui.int("clicks"), Some(0), "the backdrop swallowed it");
    ui.key("escape");
    assert_eq!(ui.value("is_open"), Some(0));
    ui.tick(20);
    ui.click(60, 274);
    assert_eq!(ui.int("clicks"), Some(1), "and input comes back after");
}

#[test]
fn every_component_draws_without_erroring() {
    // The whole surface in one frame: a smoke test that keeps a renamed helper
    // or a bad arity from reaching a panel.
    let mut ui = Ui::new(include_str!("fixtures/bloom_smoke.ptl"));
    ui.tick(30);
    let cmds = ui.host.frame(DT, ui.frame).expect("a clean frame");
    assert!(cmds.len() > 40, "the smoke panel drew {} commands", cmds.len());
}
