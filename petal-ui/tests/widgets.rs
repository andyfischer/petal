//! Behavioral tests for the prelude-v2 component library (layout, motion,
//! theme resolution, and the widget set), driven headlessly through the
//! standard host frame contract like `tests/prelude.rs`.
//!
//! Harness metrics: `text_width(s, size)` is `round(chars × size × 0.6)`, so
//! geometry in these tests is computed with that ruler.

use petal_ui::draw::DrawCommand;
use petal_ui::harness::Headless;
use petal_ui::input::{InputEvent, Modifiers};

fn run_headless(source: &str, check: impl Fn(&mut Headless)) {
    let mut ui = Headless::new(source).unwrap_or_else(|e| panic!("compile failed: {e}"));
    check(&mut ui);
}

fn rects(cmds: &[DrawCommand]) -> Vec<(i32, i32, u32, u32)> {
    cmds.iter()
        .filter_map(|c| match c {
            DrawCommand::Rect { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
            _ => None,
        })
        .collect()
}

fn texts(cmds: &[DrawCommand]) -> Vec<String> {
    cmds.iter()
        .filter_map(|c| match c {
            DrawCommand::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ── Layout: RectCut ────────────────────────────────────────────────────────

#[test]
fn rectcut_slices_and_stacks() {
    let src = "state s = \"\"\n\
               let cl = cut_left(rect(10, 20, 100, 50), 30)\n\
               let cr = cut_right(rect(0, 0, 100, 50), 30)\n\
               let ct = cut_top(rect(0, 0, 100, 50), 12)\n\
               let cb = cut_bottom(rect(0, 0, 100, 50), 12)\n\
               s = str([cl.cut.x, cl.cut.w, cl.rest.x, cl.rest.w,\n\
                        cr.cut.x, cr.rest.w, ct.cut.h, ct.rest.y,\n\
                        cb.cut.y, cb.rest.h])";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state_string("s").as_deref(),
            Some("[10, 30, 40, 70, 70, 70, 12, 12, 38, 38]"),
            "cut_* return the strip and the rest"
        );
    });
}

#[test]
fn split_pad_and_stacks() {
    let src = "state s = \"\"\n\
               let sh = split_h(rect(0, 0, 110, 50), 0.5, 10)\n\
               let sv = split_v(rect(0, 0, 50, 110), 0.5, 10)\n\
               let p = pad(rect(0, 0, 100, 100), 10)\n\
               let p2 = pad(rect(0, 0, 100, 100), 10, 5)\n\
               let p4 = pad(rect(0, 0, 100, 100), 1, 2, 3, 4)\n\
               let hs = hstack(rect(0, 0, 100, 20), 3, 5)\n\
               let vs = vstack(rect(0, 0, 20, 100), 3, 5)\n\
               s = str([sh.a.w, sh.b.x, sv.a.h, sv.b.y,\n\
                        p.x, p.w, p2.y, p2.w, p4.x, p4.w, p4.h,\n\
                        len(hs), hs[1].x, hs[2].w, len(vs), vs[1].y])";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state_string("s").as_deref(),
            Some("[50, 60, 50, 60, 10, 80, 5, 80, 1, 96, 94, 3, 35, 30, 3, 35]")
        );
    });
}

#[test]
fn row_and_col_distribute_fixed_and_flex() {
    // row: 200 wide, gaps 2×10, fixed 50 → leftover 130 split 0.5/0.5 = 65 each.
    let src = "state s = \"\"\n\
               let r = row(rect(0, 0, 200, 20), [50, 0.5, 0.5], 10)\n\
               let c = col(rect(0, 0, 20, 200), [40, 1.0], 10)\n\
               s = str([len(r), r[0].w, r[1].x, r[1].w, r[2].x, r[2].w,\n\
                        len(c), c[0].h, c[1].y, c[1].h])";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state_string("s").as_deref(),
            Some("[3, 50, 60, 65, 135, 65, 2, 40, 50, 150]"),
            "px entries are fixed, fractional entries share the leftover"
        );
    });
}

// ── Motion ─────────────────────────────────────────────────────────────────

#[test]
fn motion_helpers_ease_and_approach() {
    let src = "state a = 0.0\n\
               state eo = 0.0\n\
               state eio = 0.0\n\
               state snap = 0.0\n\
               a = approach(0.0, 1.0, 100.0)\n\
               eo = ease_out(1.0)\n\
               eio = ease_in_out(0.5)\n\
               snap = approach(0.9995, 1.0, 1.0)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let a = ui.state_float("a").unwrap();
        assert!(a > 0.5 && a < 1.0, "one step of rate-100 smoothing: {a}");
        assert_eq!(ui.state_float("eo"), Some(1.0));
        assert_eq!(ui.state_float("eio"), Some(0.5));
        assert_eq!(
            ui.state_float("snap"),
            Some(1.0),
            "approach snaps onto the target within a thousandth"
        );
    });
}

// ── Theme resolution ───────────────────────────────────────────────────────

#[test]
fn host_palette_defaults_the_theme_and_theme_set_outranks_it() {
    // Away from (0,0) so the button paints its resting face.
    let src = "state accent_r = 0\n\
               state n = 0\n\
               n = n + 1\n\
               if n == 3 then theme_set({panel: #010101}) end\n\
               if n == 4 then theme_reset() end\n\
               accent_r = ui_theme().accent.r\n\
               button({x: 200, y: 200, w: 80, h: 24}, \"Go\")";
    let mut ui = Headless::new(src).unwrap();

    // Frame 1: no host palette bound → built-in default (accent #3c8cff).
    let cmds = ui.frame().unwrap().to_vec();
    assert_eq!(ui.state_int("accent_r"), Some(0x3c));
    assert!(
        rects_with_color(&cmds, (0x1d, 0x20, 0x27)),
        "resting face is the built-in panel color: {cmds:?}"
    );

    // Frame 2: host palette bound → widgets pick it up with no script change.
    petal_ui::input::bind_host_palette(
        &mut ui.env,
        &[
            ("panel", [7, 7, 7, 255]),
            ("accent", [1, 2, 3, 255]),
            ("green", [4, 5, 6, 255]),
        ],
    );
    let cmds = ui.frame().unwrap().to_vec();
    assert_eq!(
        ui.state_int("accent_r"),
        Some(1),
        "ui_theme().accent now comes from the host palette"
    );
    assert!(
        rects_with_color(&cmds, (7, 7, 7)),
        "the button face takes the host panel color: {cmds:?}"
    );

    // Frame 3: an explicit theme_set outranks the host palette…
    let cmds = ui.frame().unwrap().to_vec();
    assert!(
        rects_with_color(&cmds, (1, 1, 1)),
        "theme_set wins over the host palette: {cmds:?}"
    );
    assert_eq!(
        ui.state_int("accent_r"),
        Some(1),
        "unset keys of the explicit theme keep the resolved (host) value"
    );

    // Frame 4: …and theme_reset goes back to host-palette resolution.
    let cmds = ui.frame().unwrap().to_vec();
    assert!(
        rects_with_color(&cmds, (7, 7, 7)),
        "theme_reset restores host-palette defaulting: {cmds:?}"
    );
}

fn rects_with_color(cmds: &[DrawCommand], rgb: (u8, u8, u8)) -> bool {
    cmds.iter()
        .any(|c| matches!(c, DrawCommand::Rect { r, g, b, .. } if (*r, *g, *b) == rgb))
}

#[test]
fn theme_carries_semantic_tokens_and_scales() {
    let src = "state s = \"\"\n\
               let t = ui_theme()\n\
               s = str([t.success.r, t.danger.r, t.warn.r, t.surface.r,\n\
                        t.border.r, t.border_strong.r, t.muted.r, t.overlay.r,\n\
                        t.space_sm, t.space, t.space_lg,\n\
                        t.radius_sm, t.radius, t.radius_lg,\n\
                        t.font_sm, t.font_md, t.font_lg, t.font_xl])";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state_string("s").as_deref(),
            Some("[63, 248, 210, 29, 58, 77, 170, 38, 4, 8, 16, 3, 6, 10, 12, 14, 18, 24]"),
            "semantic tokens + spacing/radius/type scales are all present"
        );
    });
}

#[test]
fn theme_from_palette_maps_the_new_tokens_too() {
    let src = "state s = \"\"\n\
               let t = theme_from_palette({green: #010101, red: #020202,\n\
                                           orange: #030303, border_focused: #040404})\n\
               s = str([t.success.r, t.danger.r, t.warn.r, t.border_strong.r, t.font_md])";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_string("s").as_deref(), Some("[1, 2, 3, 4, 14]"));
    });
}

// ── Spinner + progress ─────────────────────────────────────────────────────

#[test]
fn spinner_emits_an_arc_and_glyph_cycles() {
    let src = "state g = \"\"\n\
               spinner(50, 50, 12)\n\
               g = spinner_glyph()";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DrawCommand::Arc { cx: 50, cy: 50, .. })),
            "spinner draws an arc: {cmds:?}"
        );
        assert_eq!(ui.state_string("g").as_deref(), Some("|"));
    });
}

#[test]
fn progress_bar_fill_is_proportional() {
    let src = "progress_bar({x: 0, y: 0, w: 100, h: 8}, 0.5)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let rs = rects(&cmds);
        assert_eq!(rs.len(), 2, "track + fill: {cmds:?}");
        assert_eq!(rs[0], (0, 0, 100, 8), "track spans the rect");
        assert_eq!(rs[1], (0, 0, 50, 8), "fill is frac of the width");
    });

    // frac < 0 → the indeterminate sweep still draws a segment inside the rect.
    let src = "progress_bar({x: 10, y: 0, w: 100, h: 8}, -1.0)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let rs = rects(&cmds);
        assert_eq!(rs.len(), 2, "track + sweeping segment: {cmds:?}");
        let (x, _, w, _) = rs[1];
        assert!(
            x >= 10 && x + w as i32 <= 110,
            "segment clipped to the track"
        );
    });
}

// ── Checkbox / toggle / radio / slider ─────────────────────────────────────

#[test]
fn checkbox_click_toggles_and_draws_its_label() {
    let src = "state v = false\n\
               v = checkbox({x: 10, y: 10, w: 150, h: 20}, \"Enable\", v)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap();
        assert!(texts(cmds).contains(&"Enable".to_string()));
        assert_eq!(ui.state()["v"], false);

        ui.click(20, 20).unwrap();
        assert_eq!(ui.state()["v"], true, "click toggles on");
        ui.frame().unwrap();
        let cmds = ui.frame().unwrap();
        assert!(
            cmds.iter().any(|c| matches!(c, DrawCommand::Line { .. })),
            "a checked box draws its checkmark strokes: {cmds:?}"
        );

        ui.click(100, 20).unwrap();
        assert_eq!(ui.state()["v"], false, "click anywhere in r toggles off");
    });
}

#[test]
fn toggle_click_flips_and_knob_travels() {
    let src = "state v = false\n\
               v = toggle({x: 0, y: 0, w: 40, h: 20}, v)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let off_knob = knob_x(&ui.commands);
        ui.click(10, 10).unwrap();
        assert_eq!(ui.state()["v"], true);
        // Let the ease settle, then the knob sits at the other end.
        ui.frames(40).unwrap();
        let on_knob = knob_x(&ui.commands);
        assert!(
            on_knob > off_knob,
            "knob moved right when toggled on ({off_knob} → {on_knob})"
        );
    });
}

fn knob_x(cmds: &[DrawCommand]) -> i32 {
    cmds.iter()
        .filter_map(|c| match c {
            DrawCommand::Circle { cx, .. } => Some(*cx),
            _ => None,
        })
        .next_back()
        .expect("toggle draws its knob circle")
}

#[test]
fn radio_group_click_selects_a_row() {
    let src = "state sel = 0\n\
               sel = radio_group({x: 0, y: 0, w: 160, h: 72}, [\"a\", \"b\", \"c\"], sel)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("sel"), Some(0));
        // Rows are 24 tall: row 2 spans y 48..72.
        ui.click(80, 60).unwrap();
        assert_eq!(ui.state_int("sel"), Some(2));
        ui.click(80, 30).unwrap();
        assert_eq!(ui.state_int("sel"), Some(1));
    });
}

#[test]
fn slider_press_jumps_and_drag_tracks() {
    // r 120×20 → knob 8, pad_x 10, usable 100: value == (mouse_x - 10) / 100.
    let src = "state v = 0.0\n\
               v = slider({x: 0, y: 0, w: 120, h: 20}, v, 0.0, 1.0)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_float("v"), Some(0.0));

        ui.mouse_move(60, 10);
        ui.mouse_down(0);
        ui.frame().unwrap();
        assert_eq!(
            ui.state_float("v"),
            Some(0.5),
            "press jumps to the position"
        );

        ui.mouse_move(200, 10); // way past the right edge: clamps to 1.0
        ui.frame().unwrap();
        assert_eq!(
            ui.state_float("v"),
            Some(1.0),
            "drag keeps tracking, clamped"
        );

        ui.mouse_up(0);
        ui.frame().unwrap();
        ui.mouse_move(20, 10);
        ui.frame().unwrap();
        assert_eq!(
            ui.state_float("v"),
            Some(1.0),
            "after release the pointer no longer drives the value"
        );
    });
}

// ── Tab bar ────────────────────────────────────────────────────────────────

#[test]
fn tab_bar_click_switches_active_tab() {
    // text_width("Aaa", 14) = round(3×8.4) = 25 → each tab is 49 wide, gap 4:
    // tab 0 spans x 0..49, tab 1 spans x 53..102.
    let src = "state act = 0\n\
               act = tab_bar({x: 0, y: 0, w: 300, h: 28}, [\"Aaa\", \"Bbb\", \"Ccc\"], act)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_int("act"), Some(0));
        ui.click(75, 14).unwrap();
        assert_eq!(ui.state_int("act"), Some(1), "click lands on tab 1");
        ui.click(130, 14).unwrap();
        assert_eq!(ui.state_int("act"), Some(2));
        // A click below the strip changes nothing.
        ui.click(75, 100).unwrap();
        assert_eq!(ui.state_int("act"), Some(2));
    });
}

// ── Splitter ───────────────────────────────────────────────────────────────

#[test]
fn splitter_drag_moves_the_divider() {
    let src = "state sp = splitter_state(0.5)\n\
               state aw = 0\n\
               let s = splitter(sp, {x: 0, y: 0, w: 400, h: 200})\n\
               sp = s.state\n\
               aw = s.a.w";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        // frac 0.5 of 400 → divider at 200, side `a` ends 4px before it.
        assert_eq!(ui.state_int("aw"), Some(196));

        // Grab the divider (band 9 around x=200) and drag right.
        ui.mouse_move(200, 100);
        ui.mouse_down(0);
        ui.frame().unwrap();
        ui.mouse_move(300, 100);
        ui.frame().unwrap();
        assert_eq!(ui.state_int("aw"), Some(296), "divider follows the drag");
        let frac = ui.state().get("sp").unwrap()["frac"].as_f64().unwrap();
        assert!((frac - 0.75).abs() < 0.01, "frac updated: {frac}");

        ui.mouse_up(0);
        ui.frame().unwrap();
        ui.mouse_move(100, 100);
        ui.frame().unwrap();
        assert_eq!(
            ui.state_int("aw"),
            Some(296),
            "after release the divider stays put"
        );
    });
}

#[test]
fn splitter_respects_min_sizes() {
    let src = "state sp = splitter_state(0.01)\n\
               state aw = 0\n\
               let s = splitter(sp, {x: 0, y: 0, w: 400, h: 200}, {min_a: 100, min_b: 100})\n\
               sp = s.state\n\
               aw = s.a.w";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state_int("aw"),
            Some(96),
            "a tiny frac still leaves min_a px (minus the gap)"
        );
    });
}

// ── Modal ──────────────────────────────────────────────────────────────────

const MODAL_SRC: &str = "state md = modal_state()\n\
                         state blocked = false\n\
                         state opens = 0\n\
                         if !modal_blocking(md) && button({x: 0, y: 0, w: 60, h: 20}, \"open\") then\n\
                           md = modal_open(md)\n\
                           opens = opens + 1\n\
                         end\n\
                         let m = modal(md, 200, 100, \"T\")\n\
                         md = m.modal\n\
                         blocked = modal_blocking(md)";

#[test]
fn modal_opens_draws_backdrop_and_escape_closes() {
    run_headless(MODAL_SRC, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state()["blocked"], false);

        // The opening click does not immediately dismiss the modal, even
        // though the press is outside the (centered) dialog rect.
        ui.click(30, 10).unwrap();
        assert_eq!(ui.state()["blocked"], true, "open survives its own click");
        let cmds = ui.frame().unwrap().to_vec();
        assert!(
            rects(&cmds).contains(&(0, 0, 800, 600)),
            "a full-screen backdrop dims the panel: {cmds:?}"
        );
        assert!(texts(&cmds).contains(&"T".to_string()), "title drawn");

        ui.key("escape").unwrap();
        assert_eq!(ui.state()["blocked"], false, "Escape closes");
    });
}

#[test]
fn modal_click_outside_closes_and_inside_does_not() {
    run_headless(MODAL_SRC, |ui| {
        ui.frame().unwrap();
        ui.click(30, 10).unwrap();
        assert_eq!(ui.state()["blocked"], true);

        // Screen 800×600, dialog 200×100 → centered at 300..500 × 250..350.
        ui.click(400, 300).unwrap();
        assert_eq!(ui.state()["blocked"], true, "a click inside stays open");

        ui.click(700, 500).unwrap();
        assert_eq!(ui.state()["blocked"], false, "a click outside closes");
        assert_eq!(
            ui.state_int("opens"),
            Some(1),
            "the guarded button did not re-fire"
        );
    });
}

// ── Badge / pill / card / empty state / hint bar ───────────────────────────

#[test]
fn badge_and_pill_return_their_rects() {
    let src = "state s = \"\"\n\
               let b = badge(10, 10, \"v2\")\n\
               let p = pill(b.x + b.w + 8, 10, \"ok\", ui_theme().success)\n\
               s = str([b.x, b.h, p.x])";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        // size 12 → "v2" = round(2×7.2) = 14px + 14 pad = 28 wide, h = 20.
        assert_eq!(ui.state_string("s").as_deref(), Some("[10, 20, 46]"));
        let ts = texts(&cmds);
        assert!(ts.contains(&"v2".to_string()) && ts.contains(&"ok".to_string()));
    });
}

#[test]
fn card_returns_the_padded_content_rect() {
    let src = "state s = \"\"\n\
               let c = card({x: 10, y: 10, w: 200, h: 100})\n\
               s = str([c.x, c.y, c.w, c.h])";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert_eq!(
            ui.state_string("s").as_deref(),
            Some("[26, 26, 168, 68]"),
            "content is the card inset by the theme's space_lg"
        );
        // A real drop shadow, then the border rect and the surface inside it.
        // The shadow is a `Shadow` command, not a fourth offset rectangle —
        // the hard-edged duplicate this widget used to draw read as a
        // misaligned copy of the card rather than as elevation.
        assert!(
            matches!(
                cmds[0],
                DrawCommand::Shadow {
                    x: 10,
                    y: 10,
                    w: 200,
                    h: 100,
                    dy: 1,
                    ..
                }
            ),
            "elevation-1 shadow behind the card: {cmds:?}"
        );
        assert_eq!(rects(&cmds).len(), 2, "border + surface: {cmds:?}");
    });
}

#[test]
fn empty_state_centers_its_copy() {
    let src = "empty_state({x: 0, y: 0, w: 400, h: 200}, \"No results\", \"Try again\")";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap();
        let ts = texts(cmds);
        assert_eq!(ts, vec!["No results".to_string(), "Try again".to_string()]);
    });
}

#[test]
fn hint_bar_draws_caps_and_labels() {
    let src = "hint_bar({x: 0, y: 100, w: 400, h: 26}, [hint(\"j/k\", \"move\"), hint(\"enter\", \"open\")])";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap();
        let ts = texts(cmds);
        assert_eq!(ts, vec!["j/k", "move", "enter", "open"]);
        assert!(
            rects(cmds).len() >= 3,
            "bar background + one cap per hint: {cmds:?}"
        );
    });
}

// ── Table ──────────────────────────────────────────────────────────────────

const TABLE_SRC: &str = "state tb = table_state()\n\
                         let rows = [[\"a\", 1], [\"b\", 2], [\"c\", 3], [\"d\", 4], [\"e\", 5],\n\
                                     [\"f\", 6], [\"g\", 7], [\"h\", 8], [\"i\", 9], [\"j\", 10]]\n\
                         tb = table(tb, {x: 0, y: 0, w: 300, h: 158},\n\
                                    [{label: \"A\", w: 0.5}, {label: \"B\", w: 0.5}], rows)";

fn tb_field(ui: &Headless, field: &str) -> serde_json::Value {
    ui.state().get("tb").unwrap()[field].clone()
}

#[test]
fn table_header_click_cycles_sort_request() {
    run_headless(TABLE_SRC, |ui| {
        ui.frame().unwrap();
        assert_eq!(tb_field(ui, "sort_col"), -1);

        // Columns split 150/150; header is the top 28px.
        ui.click(200, 14).unwrap();
        assert_eq!(tb_field(ui, "sort_col"), 1);
        assert_eq!(tb_field(ui, "sort_asc"), true);

        ui.click(200, 14).unwrap();
        assert_eq!(
            tb_field(ui, "sort_asc"),
            false,
            "second click flips direction"
        );

        ui.click(50, 14).unwrap();
        assert_eq!(
            tb_field(ui, "sort_col"),
            0,
            "another header restarts ascending"
        );
        assert_eq!(tb_field(ui, "sort_asc"), true);
    });
}

#[test]
fn table_rows_select_by_click_and_keyboard() {
    run_headless(TABLE_SRC, |ui| {
        ui.frame().unwrap();
        assert_eq!(tb_field(ui, "selected"), 0);

        // Body starts at y=28, rows 26 tall → y 85 is row 2.
        ui.click(50, 85).unwrap();
        assert_eq!(tb_field(ui, "selected"), 2);

        ui.key("j").unwrap();
        assert_eq!(tb_field(ui, "selected"), 3, "list keys navigate the table");
        ui.key("end").unwrap();
        assert_eq!(tb_field(ui, "selected"), 9);
        assert!(
            tb_field(ui, "scroll").as_i64().unwrap() > 0,
            "selection scrolled into view"
        );
    });
}

#[test]
fn table_draws_headers_cells_and_sort_arrow() {
    run_headless(TABLE_SRC, |ui| {
        ui.frame().unwrap();
        let ts = texts(&ui.commands);
        assert!(ts.contains(&"A".to_string()) && ts.contains(&"B".to_string()));
        assert!(ts.contains(&"a".to_string()) && ts.contains(&"1".to_string()));
        assert!(
            !ts.iter().any(|t| t == "▲" || t == "▼"),
            "no arrow before a sort"
        );

        ui.click(50, 14).unwrap();
        ui.frame().unwrap();
        assert!(
            texts(&ui.commands).contains(&"▲".to_string()),
            "ascending arrow after a header click"
        );
    });
}

// ── wrap_px ────────────────────────────────────────────────────────────────

#[test]
fn wrap_px_wraps_on_the_pixel_budget() {
    // size 10 → 6px per char. "aaa bbb" = 42px.
    let src = "state a = \"\"\n\
               state b = \"\"\n\
               state hard = \"\"\n\
               a = join(wrap_px(\"aaa bbb ccc\", 40, 10), \"|\")\n\
               b = join(wrap_px(\"aaa bbb ccc\", 45, 10), \"|\")\n\
               hard = join(wrap_px(\"abcdefgh\", 20, 10), \"|\")";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(
            st["a"], "aaa|bbb|ccc",
            "42px pair does not fit a 40px budget"
        );
        assert_eq!(st["b"], "aaa bbb|ccc", "but fits 45px");
        assert_eq!(st["hard"], "abc|def|gh", "an oversized token hard-breaks");
    });
}

#[test]
fn wrap_px_takes_a_style_record_and_survives_edge_cases() {
    let src = "state styled = \"\"\n\
               state zero = \"\"\n\
               state tiny = \"\"\n\
               styled = join(wrap_px(\"aaa bbb\", 45, {size: 10}), \"|\")\n\
               zero = join(wrap_px(\"hello there\", 0, 10), \"|\")\n\
               tiny = join(wrap_px(\"ab\", 1, 10), \"|\")";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        let st = ui.state();
        assert_eq!(st["styled"], "aaa bbb");
        assert_eq!(st["zero"], "hello there", "budget <= 0 returns [s]");
        // A budget narrower than one char emits whole chars rather than looping.
        assert_eq!(st["tiny"], "a|b");
    });
}

// ── Load state ─────────────────────────────────────────────────────────────

#[test]
fn load_state_machine_polls_to_ready() {
    let src = "state ls = load_state()\n\
               state shown = false\n\
               state phase = \"\"\n\
               ls = load_poll(ls, \"k1\", 42)\n\
               phase = ls.status\n\
               shown = draw_load({x: 0, y: 0, w: 100, h: 40}, ls)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        // A plain value is ready immediately: begin → ok in one poll.
        assert_eq!(ui.state_string("phase").as_deref(), Some("ready"));
        assert_eq!(ui.state()["shown"], true);
        assert_eq!(ui.state().get("ls").unwrap()["value"], 42);
        assert!(ui.commands.is_empty(), "ready draws nothing itself");
    });
}

#[test]
fn load_states_draw_spinner_and_error() {
    let src = "state ls = load_state()\n\
               ls = load_begin(ls, \"k\")\n\
               draw_load({x: 0, y: 0, w: 100, h: 40}, ls)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap();
        assert!(
            cmds.iter().any(|c| matches!(c, DrawCommand::Arc { .. })),
            "loading draws the spinner: {cmds:?}"
        );
    });

    let src = "state ls = load_state()\n\
               ls = load_fail(load_begin(ls, \"k\"), \"boom\")\n\
               state failed = false\n\
               failed = load_failed(ls)\n\
               draw_load({x: 0, y: 0, w: 200, h: 40}, ls)";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert_eq!(ui.state()["failed"], true);
        assert!(
            texts(&cmds).contains(&"boom".to_string()),
            "the error message is shown: {cmds:?}"
        );
    });
}

#[test]
fn load_poll_restarts_when_the_key_changes() {
    let src = "state ls = load_state()\n\
               state n = 0\n\
               n = n + 1\n\
               let key = if n < 3 then \"a\" else \"b\" end\n\
               ls = load_poll(ls, key, \"v-\" ++ key)\n\
               state status = \"\"\n\
               state val = \"\"\n\
               status = ls.status\n\
               val = str(ls.value)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(ui.state_string("val").as_deref(), Some("v-a"));
        ui.frames(2).unwrap();
        assert_eq!(
            ui.state_string("val").as_deref(),
            Some("v-b"),
            "a new key re-polls and lands the new value"
        );
    });
}

// ── Text field caret editing ───────────────────────────────────────────────

const CARET_SRC: &str = "state fc = focus_state()\n\
                         state buf = \"hello world\"\n\
                         state caret = -1\n\
                         let r = {x: 10, y: 10, w: 200, h: 24}\n\
                         let res = text_field(fc, \"name\", r, buf)\n\
                         fc = res.focus\n\
                         buf = res.text\n\
                         caret = res.caret";

#[test]
fn click_places_the_caret_and_typing_inserts_there() {
    run_headless(CARET_SRC, |ui| {
        ui.frame().unwrap();
        // Text origin x = 16; size 14 → 8.4px/char; 5 chars ≈ 42px → x 58.
        ui.click(58, 20).unwrap();
        assert_eq!(ui.state_int("caret"), Some(5), "caret lands after 'hello'");

        ui.text("!!").unwrap();
        assert_eq!(ui.state_string("buf").as_deref(), Some("hello!! world"));
        assert_eq!(ui.state_int("caret"), Some(7));
    });
}

#[test]
fn arrows_home_end_move_the_caret_and_backspace_deletes_before_it() {
    run_headless(CARET_SRC, |ui| {
        ui.frame().unwrap();
        ui.click(58, 20).unwrap(); // caret 5, after "hello"
        ui.key("left").unwrap();
        ui.key("left").unwrap();
        assert_eq!(ui.state_int("caret"), Some(3));

        ui.key("backspace").unwrap();
        assert_eq!(ui.state_string("buf").as_deref(), Some("helo world"));
        assert_eq!(ui.state_int("caret"), Some(2));

        ui.key("home").unwrap();
        assert_eq!(ui.state_int("caret"), Some(0));
        ui.key("backspace").unwrap();
        assert_eq!(
            ui.state_string("buf").as_deref(),
            Some("helo world"),
            "backspace at the start is a no-op"
        );

        ui.key("end").unwrap();
        assert_eq!(ui.state_int("caret"), Some(10));
        ui.key("delete").unwrap();
        assert_eq!(
            ui.state_string("buf").as_deref(),
            Some("helo world"),
            "forward-delete at the end is a no-op"
        );
        ui.key("left").unwrap();
        ui.key("delete").unwrap();
        assert_eq!(ui.state_string("buf").as_deref(), Some("helo worl"));
    });
}

#[test]
fn word_backspace_deletes_back_to_the_word_start() {
    run_headless(CARET_SRC, |ui| {
        ui.frame().unwrap();
        ui.click(100, 20).unwrap(); // focus; caret somewhere in "world"
        ui.key("end").unwrap();

        ui.event(InputEvent::Modifiers(Modifiers {
            alt: true,
            ..Default::default()
        }));
        ui.key("backspace").unwrap();
        assert_eq!(
            ui.state_string("buf").as_deref(),
            Some("hello "),
            "alt-backspace removes the whole word"
        );
        ui.key("backspace").unwrap();
        assert_eq!(
            ui.state_string("buf").as_deref(),
            Some(""),
            "…and then the space plus the word before it"
        );
        ui.event(InputEvent::Modifiers(Modifiers::default()));
    });
}

#[test]
fn caret_clamps_when_the_buffer_shrinks_externally() {
    let src = "state fc = focus_state()\n\
               state buf = \"abcdef\"\n\
               state caret = -1\n\
               state n = 0\n\
               n = n + 1\n\
               if n == 3 then buf = \"xy\" end\n\
               let res = text_field(fc, \"f\", {x: 0, y: 0, w: 200, h: 24}, buf)\n\
               fc = res.focus\n\
               if n != 3 then buf = res.text end\n\
               caret = res.caret";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        ui.click(190, 12).unwrap(); // focus, caret at end (6)
        assert_eq!(ui.state_int("caret"), Some(6));
        ui.frame().unwrap(); // n == 3: buf externally replaced by "xy"
        assert_eq!(
            ui.state_int("caret"),
            Some(2),
            "caret clamps to the new end"
        );
        ui.text("z").unwrap();
        assert_eq!(ui.state_string("buf").as_deref(), Some("xyz"));
    });
}

#[test]
fn draw_text_field_places_the_caret_bar_at_the_index() {
    let src = "draw_text_field({x: 0, y: 0, w: 200, h: 24}, \"abcdef\", true, 3, {})";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap();
        let x = cmds
            .iter()
            .find_map(|c| match c {
                DrawCommand::Line { x1, .. } => Some(*x1),
                _ => None,
            })
            .expect("caret line drawn");
        // inset 6 + width of "abc" at 14 (round(3×8.4) = 25) = 31.
        assert_eq!(x, 31);
    });
}

// ── Tooltip ────────────────────────────────────────────────────────────────

#[test]
fn tooltip_waits_for_the_hover_delay_then_fades_in() {
    let src = "tooltip({x: 0, y: 0, w: 100, h: 40}, \"tip!\")";
    let mut ui = Headless::new(src).unwrap();
    ui.time = 0.0;
    ui.mouse_move(50, 20);
    ui.frame().unwrap();
    assert!(
        !texts(&ui.commands).contains(&"tip!".to_string()),
        "no tooltip before the delay"
    );

    ui.time = 1.0; // well past delay + fade
    ui.frame().unwrap();
    assert!(
        texts(&ui.commands).contains(&"tip!".to_string()),
        "tooltip appears after the pointer rests"
    );

    ui.mouse_move(300, 300);
    ui.frame().unwrap();
    ui.mouse_move(50, 20);
    ui.frame().unwrap();
    assert!(
        !texts(&ui.commands).contains(&"tip!".to_string()),
        "leaving the anchor resets the rest timer"
    );
}

// ── Gallery fixture ────────────────────────────────────────────────────────

/// The gallery showcases every component and doubles as the library's
/// runnable example: it must load and run headlessly, and every tab must
/// draw without a runtime error.
#[test]
fn gallery_runs_headlessly_across_all_tabs() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../garden/examples/panels/gallery.ptl");
    let mut ui = Headless::from_file_with_size(&path, 900, 700)
        .unwrap_or_else(|e| panic!("gallery failed to load: {e}"));
    ui.frame()
        .unwrap_or_else(|e| panic!("gallery frame 1: {e}"));

    // Tab strip: page pad 16/12 → tabs start at (16, 12), height 34, size 14.
    // Widths: text_width(label)×0.6 + 24, gap 4.
    let labels = ["Controls", "Data", "Layout", "Overlays", "Theme"];
    let mut x = 16i32;
    for (i, label) in labels.iter().enumerate() {
        let w = ((label.len() as f64) * 14.0 * 0.6).round() as i32 + 24;
        ui.click(x + w / 2, 12 + 17)
            .unwrap_or_else(|e| panic!("clicking tab {label}: {e}"));
        for f in 0..5 {
            ui.frame()
                .unwrap_or_else(|e| panic!("tab {label} frame {f}: {e}"));
        }
        assert_eq!(
            ui.state_int("tab"),
            Some(i as i64),
            "clicking the {label} tab activates it"
        );
        assert!(!ui.commands.is_empty(), "tab {label} draws something");
        x += w + 4;
    }
}

// ── Typography: the face widgets set their text in ────────────────────────
// Two defects that only make sense together. The library used to draw every
// widget label in the host's *default* face — monospace on every host in this
// ecosystem — and it used to measure those labels with the bare
// `text_width(s, size)` form, which falls back to the default *metric table*
// no matter which face the run is drawn in. Fixing the face alone would move
// the bug from "wrong font" to "everything centred is a fifth of its width
// off", so both are pinned here.

/// A face bound under the "ui" role with an advance ratio nothing else in the
/// harness uses (the default table is 0.6), so a width measured *in it* is
/// unmistakably distinguishable from the default ruler.
fn with_ui_face(source: &str) -> Headless {
    let mut ui = Headless::new(source).unwrap_or_else(|e| panic!("compile failed: {e}"));
    petal_ui::draw::bind_font_metrics(
        &mut ui.env,
        "ui",
        &petal_ui::text::FontMetrics::monospace(0.3),
    );
    ui
}

fn text_runs(cmds: &[DrawCommand]) -> Vec<(String, i32, Option<String>)> {
    cmds.iter()
        .filter_map(|c| match c {
            DrawCommand::Text { text, x, font, .. } => Some((text.clone(), *x, font.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn widget_text_is_set_in_the_theme_face() {
    // A default-theme button names a face, and it is the proportional "ui"
    // one — not `None`, which is what "whatever the host draws by default"
    // (monospace, everywhere) serializes as.
    run_headless("button({x: 0, y: 0, w: 100, h: 24}, \"Go\")", |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        let runs = text_runs(&cmds);
        assert_eq!(
            runs.iter()
                .find(|(t, _, _)| t == "Go")
                .map(|(_, _, f)| f.as_deref()),
            Some(Some("ui")),
            "default-theme button label is set in the ui face: {cmds:?}"
        );
    });

    // And a drawer that genuinely wants monospace chrome still gets it with
    // one line — the theme token is the whole opt-out.
    let src = "theme_set({font: \"mono\"})\n\
               button({x: 0, y: 0, w: 100, h: 24}, \"Go\")";
    run_headless(src, |ui| {
        let cmds = ui.frame().unwrap().to_vec();
        assert_eq!(
            text_runs(&cmds)
                .iter()
                .find(|(t, _, _)| t == "Go")
                .map(|(_, _, f)| f.as_deref()),
            Some(Some("mono")),
            "theme_set({{font: \"mono\"}}) puts widget text back in mono: {cmds:?}"
        );
    });
}

#[test]
fn every_alignment_helper_measures_the_face_it_draws() {
    // With "ui" bound at a 0.3 advance and the default table at 0.6, a run
    // measured with the wrong ruler is twice the width of the one drawn. Each
    // assertion below is the position the widget computed; it can only be
    // right if the measure and the draw took the same style record.
    let src = "state w14 = 0\n\
               state w18 = 0\n\
               state wdef = 0\n\
               state bw = 0\n\
               w14 = text_width(\"Handle\", {size: 14, font: \"ui\"})\n\
               w18 = text_width(\"Handle\", {size: 18, font: \"ui\"})\n\
               wdef = text_width(\"Handle\", 14)\n\
               button({x: 0, y: 0, w: 200, h: 24}, \"Handle\")\n\
               let b = badge(0, 40, \"Handle\", {size: 14})\n\
               bw = b.w\n\
               section_label(\"Handle\", 0, 80, true)\n\
               empty_state({x: 0, y: 100, w: 200, h: 60}, \"Handle\", \"\")\n\
               draw_text_field({x: 0, y: 170, w: 200, h: 24}, \"Handle\", true)";
    let mut ui = with_ui_face(src);
    let cmds = ui.frame().unwrap().to_vec();
    let w14 = ui.state_int("w14").expect("w14") as i32;
    let w18 = ui.state_int("w18").expect("w18") as i32;
    let wdef = ui.state_int("wdef").expect("wdef") as i32;
    assert_ne!(
        w14, wdef,
        "the test is only meaningful while the two rulers disagree"
    );

    let runs = text_runs(&cmds);
    assert!(
        runs.iter().all(|(_, _, f)| f.as_deref() == Some("ui")),
        "every widget run names the theme face: {runs:?}"
    );

    // button: the label is centred in the button's own width.
    assert_eq!(
        runs[0].1,
        (200 - w14) / 2,
        "button centres on the width it will draw: {runs:?}"
    );
    // badge: the chip is sized to its label plus 14px of padding.
    assert_eq!(
        ui.state_int("bw").expect("bw") as i32,
        w14 + 14,
        "badge sizes itself to the run it draws"
    );
    // section_label: the underline is exactly as wide as the run.
    let underline = cmds
        .iter()
        .find_map(|c| match c {
            DrawCommand::Line {
                x1: 0, x2, y1: 95, ..
            } => Some(*x2),
            _ => None,
        })
        .expect("section_label underline");
    assert_eq!(underline, w14, "underline spans the run it sits under");
    // empty_state: centred copy, at the large size — the size travels in the
    // same record as the face.
    assert_eq!(
        runs.iter()
            .find(|(t, x, _)| t == "Handle" && *x == 100 - w18 / 2)
            .is_some(),
        true,
        "empty_state centres its title measured at font_lg: {runs:?}"
    );
    // text_field: the caret bar sits at the end of the run it is inside.
    let caret = cmds
        .iter()
        .rev()
        .find_map(|c| match c {
            DrawCommand::Line { x1, x2, .. } if x1 == x2 => Some(*x1),
            _ => None,
        })
        .expect("caret bar");
    assert_eq!(caret, 6 + w14, "caret lands at the end of the drawn run");
}

#[test]
fn over_flattens_a_tint_and_is_idempotent() {
    // The point of `over`: the same pixel, however many times it is drawn.
    // Two 50% fills stacked read 75%; one `over` at 50% drawn twice reads 50%.
    let src = "state a = \"\"\n\
               state b = \"\"\n\
               let c = over(#000000, #ffffff, 0.5)\n\
               a = str([c.r, c.g, c.b])\n\
               let d = over(c, #ffffff, 0)\n\
               b = str([d.r, d.g, d.b])\n\
               state pct = 0\n\
               pct = alpha_pct(40)";
    run_headless(src, |ui| {
        ui.frame().unwrap();
        assert_eq!(
            ui.state_string("a").as_deref(),
            Some("[128, 128, 128]"),
            "a 50% white over black is mid grey, opaque"
        );
        assert_eq!(
            ui.state_string("b").as_deref(),
            Some("[128, 128, 128]"),
            "compositing nothing over it changes nothing"
        );
        assert_eq!(ui.state_int("pct"), Some(102), "40% of 255");
    });
}
