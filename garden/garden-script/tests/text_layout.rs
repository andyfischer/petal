//! Integration tests for the text_layout placement library as a Garden panel
//! sees it: the modules are registered (so `import text_layout` works from a
//! pushed drawer with no directory of its own), and the placement is checked
//! against the host's *real* font metrics rather than an assumed proportion.
//!
//! These are the library's tests, not Garden's. They live here for the same
//! reason bloom's do — this is a host that embeds it, and it is the host that
//! answers `text_metrics`, so a test that mocked the metrics would be testing
//! its own arithmetic.

use garden_script::PanelHost;
use petal_ui::draw::VerticalMetrics;

const W: i32 = 400;
const H: i32 = 300;

/// Run one frame of `source` and hand back the panel, for reading `let`
/// bindings out of. Nothing here needs input or animation: placement is a pure
/// function of the box, the style and the host's metrics.
struct Ui {
    host: PanelHost,
}

impl Ui {
    fn new(source: &str) -> Ui {
        Ui::build(source, None)
    }

    /// A panel whose host has published `vertical` for both roles — what
    /// garden-app does with the numbers it measures off the renderer
    /// (`adopt_font`). A bare `PanelHost` publishes nothing, which is a
    /// supported state, not a broken one: the script then measures the
    /// built-in proportions.
    fn with_metrics(source: &str, vertical: VerticalMetrics) -> Ui {
        Ui::build(source, Some(vertical))
    }

    fn build(source: &str, vertical: Option<VerticalMetrics>) -> Ui {
        let mut host = PanelHost::from_source("text-layout-test", source).expect("compiles");
        host.set_dimensions(W, H);
        if let Some(v) = vertical {
            // A plausible proportional table, so the advances move with the
            // vertical metrics rather than staying on the monospace estimate.
            let ratios = vec![0.55f64; 128];
            host.set_font_metrics_with_ui(ratios.clone(), v, ratios, v);
        }
        host.frame(1.0 / 60.0, 0).expect("frame");
        Ui { host }
    }

    fn float(&self, name: &str) -> f64 {
        self.host
            .observed_json()
            .get(name)
            .unwrap_or_else(|| panic!("no binding `{name}`"))
            .as_f64()
            .unwrap_or_else(|| panic!("`{name}` is not a number"))
    }

    fn int(&self, name: &str) -> i64 {
        self.host
            .observed_json()
            .get(name)
            .unwrap_or_else(|| panic!("no binding `{name}`"))
            .as_i64()
            .unwrap_or_else(|| panic!("`{name}` is not an int"))
    }

    fn string(&self, name: &str) -> String {
        self.host
            .observed_json()
            .get(name)
            .unwrap_or_else(|| panic!("no binding `{name}`"))
            .as_str()
            .unwrap_or_else(|| panic!("`{name}` is not a string"))
            .to_string()
    }
}

#[test]
fn text_layout_is_importable_from_a_pushed_drawer() {
    // `from_source` has no file to resolve imports against: this passes only
    // because Garden registers the library as a package in memory. It has to,
    // because bloom imports it.
    let ui = Ui::new("import text_layout\nlet v = text_layout.VERSION\n");
    assert_eq!(ui.int("v"), 1);
    let ui = Ui::new("import text_layout/fit: elide\nlet s = elide(\"abc\", 12, 999)\n");
    assert_eq!(ui.string("s"), "abc");
}

/// A face nothing else would produce, so a reading of it is proof the host's
/// numbers reached the script rather than the built-in guess (0.8 / 0.2 / 1.2 /
/// 0.7 / 0.52).
const PROBE_FACE: VerticalMetrics = VerticalMetrics {
    baseline: 1.05,
    descent: 0.35,
    line_height: 1.4,
    cap_height: 0.73,
    x_height: 0.51,
};

#[test]
fn a_script_measures_the_metrics_its_host_published() {
    // The whole point of the binding: a panel asks about *this* host's face.
    // Garden's own numbers come off cosmic-text in garden-app; here the test
    // plays that part, so a regression in the plumbing between
    // `set_font_metrics_with_ui` and `text_metrics` fails right here.
    let src = "let m = text_metrics(20)\n\
               let baseline = m.baseline\n\
               let line_height = m.line_height\n\
               let cap = m.cap_height\n\
               let desc = m.descent\n";
    let ui = Ui::with_metrics(src, PROBE_FACE);
    assert!((ui.float("baseline") - 21.0).abs() < 0.001, "1.05 × 20");
    assert!((ui.float("desc") - 7.0).abs() < 0.001, "0.35 × 20");
    assert!((ui.float("line_height") - 28.0).abs() < 0.001, "1.4 × 20");
    assert!((ui.float("cap") - 14.6).abs() < 0.001, "0.73 × 20");

    // Metrics are px at the style's size, so one published record serves every
    // size.
    let ui = Ui::with_metrics("let b = text_metrics(10).baseline\n", PROBE_FACE);
    assert!((ui.float("b") - 10.5).abs() < 0.001);
}

#[test]
fn a_host_that_publishes_nothing_still_answers() {
    // A bare PanelHost has bound no face. That is a supported state — an
    // embedder may have no measurements to give — and a script must get usable
    // proportions rather than zeros, which would collapse every centred label
    // onto the baseline.
    let ui = Ui::new(
        "let m = text_metrics(20)\n\
         let baseline = m.baseline\n\
         let cap = m.cap_height\n\
         let desc = m.descent\n",
    );
    assert!((ui.float("baseline") - 16.0).abs() < 0.001, "0.8 × 20");
    assert!((ui.float("cap") - 14.0).abs() < 0.001, "0.7 × 20");
    assert!(
        ui.float("baseline") > ui.float("cap") && ui.float("desc") > 0.0,
        "the defaults describe a plausible face"
    );
}

#[test]
fn placement_follows_the_published_face() {
    // Not just reported — *used*. The same box and the same style place the
    // baseline differently under a different face, which is the difference
    // between a label centred on this host and a label centred on a guess.
    let src = "import text_layout: text_line_y\n\
               let y = text_line_y(Rect(0, 0, 40, 40), {size: 20})\n";
    let guessed = Ui::new(src).float("y");
    let published = Ui::with_metrics(src, PROBE_FACE).float("y");
    // Default: 20 + 14/2 - 16 = 11. Probe face: 20 + 14.6/2 - 21 = 6.3.
    assert!((guessed - 11.0).abs() < 0.001, "got {guessed}");
    assert!((published - 6.3).abs() < 0.001, "got {published}");
}

#[test]
fn a_centred_label_centres_its_cap_height() {
    // The property the library exists for: the ink of a capital, not the line
    // box, is what lands on the box's middle. A label with no descender looks
    // high when its line box is centred instead.
    let ui = Ui::new(
        "import text_layout: text_baseline\n\
         let r = Rect(0, 40, 200, 30)\n\
         let st = {size: 16}\n\
         let m = text_metrics(st)\n\
         let base = text_baseline(r, st)\n\
         let ink_top = base - m.cap_height\n\
         let ink_mid = (ink_top + base) / 2.0\n",
    );
    assert!(
        (ui.float("ink_mid") - 55.0).abs() < 0.001,
        "cap ink centred on the box's middle (55), got {}",
        ui.float("ink_mid")
    );
}

#[test]
fn the_drawn_y_is_the_baseline_less_the_faces_own_offset() {
    // `draw_text` takes a line-box top, a script reasons in baselines, and
    // `text_top` is the only conversion between them. Getting this backwards is
    // the bug in its purest form, so it is asserted directly.
    let ui = Ui::new(
        "import text_layout: text_baseline, text_top, text_line_y\n\
         let r = Rect(0, 0, 100, 40)\n\
         let st = {size: 14}\n\
         let m = text_metrics(st)\n\
         let base = text_baseline(r, st)\n\
         let top = text_top(base, st)\n\
         let same = text_line_y(r, st)\n\
         let gap = base - top\n",
    );
    let ui_m = ui.float("gap");
    let ui_baseline = Ui::new("let b = text_metrics(14).baseline\n").float("b");
    assert!((ui_m - ui_baseline).abs() < 0.001, "text_top undoes baseline");
    assert!(
        (ui.float("top") - ui.float("same")).abs() < 0.001,
        "text_line_y is text_top of text_baseline"
    );
}

#[test]
fn vertical_alignment_pins_the_edge_it_names() {
    let ui = Ui::new(
        "import text_layout: text_baseline\n\
         let r = Rect(0, 20, 100, 60)\n\
         let st = {size: 14}\n\
         let m = text_metrics(st)\n\
         let top = text_baseline(r, st, \"top\") - m.baseline\n\
         let bottom = text_baseline(r, st, \"bottom\") + m.descent\n\
         let onbase = text_baseline(r, st, \"baseline\")\n",
    );
    assert!(
        (ui.float("top") - 20.0).abs() < 0.001,
        "\"top\" puts the line box's top on r.y"
    );
    assert!(
        (ui.float("bottom") - 80.0).abs() < 0.001,
        "\"bottom\" puts the descenders on r.y + r.h"
    );
    assert!(
        (ui.float("onbase") - 80.0).abs() < 0.001,
        "\"baseline\" makes r.y + r.h the baseline itself"
    );
}

#[test]
fn horizontal_alignment_uses_the_width_it_will_draw() {
    let ui = Ui::new(
        "import text_layout: text_place\n\
         let r = Rect(10, 0, 200, 30)\n\
         let st = {size: 14}\n\
         let l = text_place(\"Save\", r, st, \"left\")\n\
         let c = text_place(\"Save\", r, st, \"center\")\n\
         let g = text_place(\"Save\", r, st, \"right\")\n\
         let lx = l.x\n\
         let cx = c.x\n\
         let gx = g.x\n\
         let w = l.w\n",
    );
    let w = ui.float("w");
    assert!(w > 0.0, "a measured run has a width");
    assert!((ui.float("lx") - 10.0).abs() < 0.001);
    assert!((ui.float("cx") - (10.0 + (200.0 - w) / 2.0)).abs() < 0.001);
    assert!((ui.float("gx") - (10.0 + 200.0 - w)).abs() < 0.001);
}

#[test]
fn wrapping_fits_the_box_and_keeps_the_words() {
    let ui = Ui::new(
        "import text_layout: wrap_text, lines_width\n\
         let st = {size: 12}\n\
         let src = \"the quick brown fox jumps over the lazy dog\"\n\
         let lines = wrap_text(src, st, 100)\n\
         let n = len(lines)\n\
         let widest = lines_width(lines, st)\n\
         let rejoined = join(lines, \" \")\n",
    );
    assert!(ui.int("n") > 1, "a long line wraps");
    assert!(
        ui.float("widest") <= 100.0,
        "no line is wider than the box: {}",
        ui.float("widest")
    );
    assert_eq!(
        ui.string("rejoined"),
        "the quick brown fox jumps over the lazy dog",
        "wrapping breaks the text, it does not edit it"
    );
}

#[test]
fn a_word_wider_than_the_box_breaks_instead_of_overflowing() {
    // The case a naive wrapper ("find the last space") loops forever on.
    let ui = Ui::new(
        "import text_layout: wrap_text, lines_width\n\
         let st = {size: 12}\n\
         let lines = wrap_text(\"supercalifragilistic\", st, 40)\n\
         let n = len(lines)\n\
         let widest = lines_width(lines, st)\n",
    );
    assert!(ui.int("n") > 1, "it broke rather than overflowing");
    assert!(ui.float("widest") <= 40.0);
}

#[test]
fn elision_fits_including_its_own_ellipsis() {
    let ui = Ui::new(
        "import text_layout: elide\n\
         let st = {size: 12}\n\
         let src = \"petal-libs/text-layout/src/place.ptl\"\n\
         let head = elide(src, st, 90)\n\
         let tail = elide(src, st, 90, \"tail\")\n\
         let both = elide(src, st, 90, \"both\")\n\
         let hw = float(text_width(head, st))\n\
         let tw = float(text_width(tail, st))\n\
         let bw = float(text_width(both, st))\n",
    );
    for (name, width) in [("head", "hw"), ("tail", "tw"), ("both", "bw")] {
        assert!(
            ui.float(width) <= 90.0,
            "{name} fits the budget it was given: {}",
            ui.float(width)
        );
        assert!(ui.string(name).contains('…'), "{name} marks the cut");
    }
    let src = "petal-libs/text-layout/src/place.ptl";
    assert!(
        src.starts_with(ui.string("head").trim_end_matches('…')),
        "the default keeps the head: {:?}",
        ui.string("head")
    );
    assert!(
        src.ends_with(ui.string("tail").trim_start_matches('…')),
        "\"tail\" keeps the end, which is what a path wants: {:?}",
        ui.string("tail")
    );
}

#[test]
fn a_block_reports_the_lines_it_placed_and_the_room_it_took() {
    let ui = Ui::new(
        "import text_layout: text_block\n\
         let r = Rect(10, 20, 120, 200)\n\
         let st = {size: 12}\n\
         let b = text_block(\"the quick brown fox jumps over the lazy dog\", r, st)\n\
         let n = len(b.lines)\n\
         let h = b.h\n\
         let first_y = b.lines[0].y\n\
         let second_y = b.lines[1].y\n\
         let step = text_metrics(st).line_height\n\
         let over = if b.overflow then 1 else 0 end\n",
    );
    assert!(ui.int("n") > 1);
    assert_eq!(ui.int("over"), 0, "it fitted, so it did not overflow");
    assert!(
        (ui.float("second_y") - ui.float("first_y") - ui.float("step")).abs() < 0.001,
        "consecutive lines are one line_height apart"
    );
    // Ink to ink, not one whole line box per line: a box fitted to
    // `count × line_height` carries a leading above the first line that
    // nothing is drawn in, and looks bottom-heavy.
    assert!(
        ui.float("h") < ui.int("n") as f64 * ui.float("step"),
        "the block's height is ink to ink"
    );
}

#[test]
fn a_line_limit_elides_the_last_line() {
    let ui = Ui::new(
        "import text_layout: text_block\n\
         let r = Rect(0, 0, 120, 200)\n\
         let st = {size: 12}\n\
         let b = text_block(\"the quick brown fox jumps over the lazy dog\", r, st, {lines: 2})\n\
         let n = len(b.lines)\n\
         let last = b.lines[1].text\n\
         let over = if b.overflow then 1 else 0 end\n",
    );
    assert_eq!(ui.int("n"), 2, "it stopped at the limit");
    assert!(
        ui.string("last").contains('…'),
        "the last line says there was more: {:?}",
        ui.string("last")
    );
    assert_eq!(ui.int("over"), 1, "and the record says so too");
}

#[test]
fn a_caret_hit_test_snaps_to_the_nearer_character() {
    let ui = Ui::new(
        "import text_layout: caret_index, caret_x\n\
         let st = {size: 14}\n\
         let s = \"hello world\"\n\
         let at_start = caret_index(s, st, 0.0)\n\
         let past_end = caret_index(s, st, 9999.0)\n\
         let x3 = caret_x(s, st, 3)\n\
         let round_trip = caret_index(s, st, x3)\n",
    );
    assert_eq!(ui.int("at_start"), 0);
    assert_eq!(
        ui.int("past_end"),
        "hello world".chars().count() as i64,
        "past the end clamps to the end, not past it"
    );
    assert_eq!(
        ui.int("round_trip"),
        3,
        "the x of a caret hit-tests back to its own index"
    );
}
