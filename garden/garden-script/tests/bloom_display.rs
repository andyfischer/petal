//! Tests for bloom's display and pane components (bloom/display, bloom/panes)
//! and the button's `fill` / `ink` options — the bloom replacements for `ui`'s
//! deprecated badge, card, hint bar, scrollbar, load view, tab bar, stepper,
//! splitter and table. Driven through a `PanelHost`, as `tests/bloom.rs` is.

use garden_script::{InputEvent, PanelCmd, PanelHost};

const W: i32 = 400;
const H: i32 = 300;
const DT: f64 = 1.0 / 60.0;

struct Ui {
    host: PanelHost,
    frame: i64,
    last: Vec<PanelCmd>,
}

impl Ui {
    fn new(source: &str) -> Ui {
        let mut host = PanelHost::from_source("bloom-display-test", &format!("import ui; {source}"))
            .expect("compiles");
        host.set_dimensions(W, H);
        let mut ui = Ui { host, frame: 0, last: Vec::new() };
        ui.tick(1);
        ui
    }

    fn tick(&mut self, n: usize) {
        for _ in 0..n {
            self.last = self.host.frame(DT, self.frame).expect("frame");
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

    fn int(&self, name: &str) -> Option<i64> {
        self.host.state_json().get(name)?.as_i64()
    }

    fn float(&self, name: &str) -> Option<f64> {
        self.host.state_json().get(name)?.as_f64()
    }

    fn boolean(&self, name: &str) -> Option<bool> {
        self.host.state_json().get(name)?.as_bool()
    }

    /// A `let` binding from the last frame.
    fn value(&self, name: &str) -> Option<serde_json::Value> {
        self.host.observed_json().get(name).cloned()
    }

    fn has_fill(&self, (r0, g0, b0): (u8, u8, u8)) -> bool {
        self.last.iter().any(|c| matches!(c, PanelCmd::Rect { r, g, b, .. } if (*r, *g, *b) == (r0, g0, b0)))
    }

    fn has_text_color(&self, (r0, g0, b0): (u8, u8, u8)) -> bool {
        self.last
            .iter()
            .any(|c| matches!(c, PanelCmd::Text { r, g, b, .. } if (*r, *g, *b) == (r0, g0, b0)))
    }

    fn rects_in(&self, x0: i32, x1: i32) -> usize {
        self.last
            .iter()
            .filter(|c| matches!(c, PanelCmd::Rect { x, .. } if *x >= x0 && *x < x1))
            .count()
    }
}

#[test]
fn tabs_switch_on_click() {
    let mut ui = Ui::new(
        "import bloom: tabs\n\
         state at = 0\n\
         at = tabs(rect(0, 0, 400, 30), [\"One\", \"Two\", \"Three\"], at)\n",
    );
    // "One" is about 24 + its label wide; the second tab starts after it.
    ui.click(80, 15);
    assert_eq!(ui.int("at"), Some(1));
    ui.click(10, 15);
    assert_eq!(ui.int("at"), Some(0));
}

#[test]
fn cycle_steps_forward_back_and_wraps() {
    let mut ui = Ui::new(
        "import bloom: cycle\n\
         state at = 0\n\
         at = cycle(rect(10, 10, 160, 26), [\"a\", \"b\", \"c\"], at)\n",
    );
    ui.click(160, 22); // the right arrow
    assert_eq!(ui.int("at"), Some(1));
    ui.click(90, 22); // the value steps forward too
    assert_eq!(ui.int("at"), Some(2));
    ui.click(160, 22); // wraps
    assert_eq!(ui.int("at"), Some(0));
    ui.click(20, 22); // the left arrow wraps backwards
    assert_eq!(ui.int("at"), Some(2));
}

#[test]
fn a_splitter_drags_its_fraction_and_clamps_it() {
    let mut ui = Ui::new(
        "import bloom: splitter\n\
         state f = 0.5\n\
         let s = splitter(rect(0, 0, 400, 300), f, {max: 0.9})\n\
         f = s.frac\n\
         let aw = s.a.w\n",
    );
    ui.host.input_event(InputEvent::MouseMove { x: 200, y: 100 });
    ui.host.input_event(InputEvent::MouseDown { button: 0 });
    ui.tick(1);
    ui.host.input_event(InputEvent::MouseMove { x: 100, y: 100 });
    ui.tick(1);
    assert_eq!(ui.float("f"), Some(0.25));
    // Off the end: the fraction stops at `max`, and the pixel floor keeps the
    // second pane at least 80 px wide.
    ui.host.input_event(InputEvent::MouseMove { x: 399, y: 100 });
    ui.tick(1);
    assert_eq!(ui.float("f"), Some(0.9));
    assert_eq!(ui.value("aw").and_then(|v| v.as_i64()), Some(320 - 4));
    ui.host.input_event(InputEvent::MouseUp { button: 0 });
    ui.tick(1);
    // Released: moving the pointer changes nothing.
    ui.host.input_event(InputEvent::MouseMove { x: 50, y: 100 });
    ui.tick(1);
    assert_eq!(ui.float("f"), Some(0.9));
}

#[test]
fn a_table_selects_the_pressed_row_and_records_the_sort_request() {
    let mut ui = Ui::new(
        "import bloom: table\n\
         state sel = -1\n\
         state col = -1\n\
         state asc = true\n\
         let tb = table(rect(0, 0, 400, 300), [{label: \"Name\"}, {label: \"Size\", w: 90}],\n\
           [[\"a\", 1], [\"b\", 2], [\"c\", 3], [\"d\", 4]])\n\
         sel = tb.selected\n\
         col = tb.sort_col\n\
         asc = tb.sort_asc\n",
    );
    assert_eq!(ui.int("sel"), Some(0));
    // Header 28 px, rows 26 px: row 2 spans y 80..106.
    ui.click(50, 90);
    assert_eq!(ui.int("sel"), Some(2));
    // The Size header (the last 90 px).
    ui.click(350, 14);
    assert_eq!(ui.int("col"), Some(1));
    assert_eq!(ui.boolean("asc"), Some(true));
    ui.click(350, 14);
    assert_eq!(ui.boolean("asc"), Some(false));
}

#[test]
fn a_scrollbar_draws_only_when_the_content_overflows() {
    let fits = Ui::new("import bloom: scrollbar\nscrollbar(rect(0, 0, 200, 100), 5, 10, 0)\n");
    assert_eq!(fits.rects_in(150, 200), 0);
    let over = Ui::new("import bloom: scrollbar\nscrollbar(rect(0, 0, 200, 100), 50, 10, 0)\n");
    assert_eq!(over.rects_in(150, 200), 2, "a track and a thumb");
}

#[test]
fn load_view_is_true_only_once_the_value_is_ready() {
    let ui = Ui::new(
        "import bloom: load_view\n\
         let idle = load_view(rect(0, 0, 200, 100), load_state())\n\
         let busy = load_view(rect(0, 0, 200, 100), load_begin(load_state(), \"k\"))\n\
         let bad = load_view(rect(0, 0, 200, 100), load_fail(load_begin(load_state(), \"k\"), \"nope\"))\n\
         let ok = load_view(rect(0, 0, 200, 100), load_ok(load_begin(load_state(), \"k\"), 7))\n",
    );
    let b = |n: &str| ui.value(n).and_then(|v| v.as_bool());
    assert_eq!(b("idle"), Some(false));
    assert_eq!(b("busy"), Some(false));
    assert_eq!(b("bad"), Some(false));
    assert_eq!(b("ok"), Some(true));
    assert!(
        ui.last.iter().any(|c| matches!(c, PanelCmd::Text { text, .. } if text == "nope")),
        "the error is shown"
    );
}

#[test]
fn a_button_takes_its_fill_and_ink_from_opts() {
    let ui = Ui::new(
        "import bloom: button\n\
         button(rect(20, 20, 120, 30), \"Go\", {fill: #c80a1e, ink: #0aff14})\n",
    );
    assert!(ui.has_fill((0xc8, 0x0a, 0x1e)), "the face is the fill");
    assert!(ui.has_text_color((0x0a, 0xff, 0x14)), "the label is the ink");
}

#[test]
fn badges_and_pills_return_their_rects_for_flowing() {
    let ui = Ui::new(
        "import bloom: badge, pill, hint, hint_bar, card, section_label, spinner_glyph, spinner\n\
         let b = badge(10, 10, \"new\")\n\
         let p = pill(b.x + b.w + 4, 10, \"ok\", #3fb950)\n\
         let px = p.x\n\
         let inner = card(rect(10, 60, 200, 100))\n\
         let ix = inner.x\n\
         hint_bar(rect(0, 270, 400, 30), [hint(\"j/k\", \"move\")])\n\
         let w = section_label(\"Files\", 10, 200, true)\n\
         let g = spinner_glyph()\n\
         spinner(300, 100, 10)\n\
         spinner(300, 100, 10, {color: #ffffff})\n\
         spinner(300, 100, 10, #ffffff)\n",
    );
    let i = |n: &str| ui.value(n).and_then(|v| v.as_f64());
    assert!(i("px").unwrap() > 10.0 + 14.0, "the pill flows after the badge");
    assert!(i("ix").unwrap() > 10.0, "card returns its padded content rect");
    assert!(i("w").unwrap() > 0.0);
    assert!(ui.last.iter().any(|c| matches!(c, PanelCmd::Text { text, .. } if text == "move")));
}
