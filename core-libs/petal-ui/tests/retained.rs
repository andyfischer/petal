//! Retained drawing and the wider draw vocabulary: `cached_layer`,
//! `draw_rects`, the single-native `draw_rect` / `draw_polyline` shapes, the
//! extended `draw_image`, and text decorations. Each is pinned at the command
//! list — what a host is handed — in both the lowered form every host gets
//! and the form a host that asked for it gets.

use petal_ui::compose::{HostCaps, invalidate_caches, set_host_caps};
use petal_ui::draw::{BatchRect, DrawCommand, bind_image_sizes, take_image_requests};
use petal_ui::harness::Headless;

fn ui(src: &str) -> Headless {
    Headless::with_ui(src).unwrap_or_else(|e| panic!("compile failed: {e}"))
}

fn cmds(src: &str) -> Vec<DrawCommand> {
    ui(src).frame().unwrap().to_vec()
}

fn rect(x: i32, y: i32, w: u32, h: u32, rgb: (u8, u8, u8), a: u8) -> DrawCommand {
    DrawCommand::Rect { x, y, w, h, r: rgb.0, g: rgb.1, b: rgb.2, a, radius: 0 }
}

// ── draw_rect / draw_polyline as single natives ───────────────────────────

#[test]
fn draw_rect_takes_every_shape_it_ever_did_and_a_packed_colour() {
    let out = cmds(
        "let c = #102030\n\
         draw_rect(1, 2, 3, 4, 16, 32, 48)\n\
         draw_rect(1, 2, 3, 4, 16, 32, 48, 128)\n\
         draw_rect(1, 2, 3, 4, c)\n\
         draw_rect(1, 2, 3, 4, c, 128)\n\
         draw_rect(rect(1, 2, 3, 4), c)\n\
         draw_rect({x: 1, y: 2, w: 3, h: 4}, c, 128)\n\
         draw_rect(1, 2, 3, 4, 1056816)\n\
         draw_rect(rect(1, 2, 3, 4), 1056816, 128)\n\
         draw_rect(x: 1, y: 2, w: 3, h: 4, c: c, a: 128)\n\
         draw_rect(rect: rect(1, 2, 3, 4), c: c)\n\
         draw_rect(1.9, 2.9, 3.9, 4.9, c)",
    );
    let opaque = rect(1, 2, 3, 4, (16, 32, 48), 255);
    let half = rect(1, 2, 3, 4, (16, 32, 48), 128);
    assert_eq!(
        out,
        [
            opaque.clone(),
            half.clone(),
            opaque.clone(),
            half.clone(),
            opaque.clone(),
            half.clone(),
            opaque.clone(),
            half.clone(),
            half,
            opaque.clone(),
            opaque
        ]
    );
}

#[test]
fn draw_rect_says_what_is_wrong_with_a_bad_colour() {
    let err = ui("draw_rect(1, 2, 3, 4, \"red\")").frame().unwrap_err();
    assert!(err.contains("expects a colour"), "{err}");
    let err = ui("draw_rect(1, 2, 3, 4, {r: 1})").frame().unwrap_err();
    assert!(err.contains("colour record with field 'g'"), "{err}");
}

#[test]
fn draw_polyline_takes_a_colour_record_a_packed_colour_or_flat_channels() {
    let out = cmds(
        "let pts = [[0, 0], [4, 0], [4, 4]]\n\
         let c = #102030\n\
         draw_polyline(pts, c)\n\
         draw_polyline(pts, c, 128)\n\
         draw_polyline(pts, c, 128, 3)\n\
         draw_polyline(pts, 16, 32, 48)\n\
         draw_polyline(pts, 16, 32, 48, 128, 3)\n\
         draw_polyline(pts, 1056816)\n\
         draw_polyline(pts, c, width: 3)",
    );
    let line = |a: u8, width: u32| DrawCommand::Polyline {
        points: vec![(0, 0), (4, 0), (4, 4)],
        r: 16,
        g: 32,
        b: 48,
        a,
        width,
    };
    assert_eq!(
        out,
        [
            line(255, 1),
            line(128, 1),
            line(128, 3),
            line(255, 1),
            line(128, 3),
            line(255, 1),
            line(255, 3)
        ]
    );
}

// ── draw_rects ────────────────────────────────────────────────────────────

const BATCH: &str = "let pal = [#ff0000, {r: 0, g: 255, b: 0, a: 128}, 255]\n\
                     draw_rects([0, 0, 4, 4, 0,  4, 0, 4, 4, 1,  8, 0, 4, 4, 2], pal, 100, 50)";

fn batch_expected() -> Vec<BatchRect> {
    vec![
        BatchRect { x: 100, y: 50, w: 4, h: 4, r: 255, g: 0, b: 0, a: 255 },
        BatchRect { x: 104, y: 50, w: 4, h: 4, r: 0, g: 255, b: 0, a: 128 },
        BatchRect { x: 108, y: 50, w: 4, h: 4, r: 0, g: 0, b: 255, a: 255 },
    ]
}

/// A host that did not ask for batches gets one `Rect` each, in order.
#[test]
fn a_batch_is_lowered_to_rects_for_a_host_that_did_not_ask_for_it() {
    let out = cmds(BATCH);
    let want: Vec<DrawCommand> = batch_expected()
        .into_iter()
        .map(|r| rect(r.x, r.y, r.w, r.h, (r.r, r.g, r.b), r.a))
        .collect();
    assert_eq!(out, want);
}

#[test]
fn a_host_that_asked_gets_the_batch_as_one_command() {
    let mut ui = ui(BATCH);
    set_host_caps(&mut ui.env, HostCaps { batch_rects: true, ..HostCaps::default() });
    let out = ui.frame().unwrap().to_vec();
    assert_eq!(out, [DrawCommand::Rects { rects: batch_expected() }]);
    // And the wire shape a JSON consumer sees.
    let json = serde_json::to_value(&out[0]).unwrap();
    assert_eq!(json["op"], "rects");
    assert_eq!(json["rects"][1]["a"], 128);
}

#[test]
fn a_batch_takes_nested_rows_a_packed_array_and_colours_without_a_palette() {
    let want = vec![
        rect(1, 2, 3, 4, (16, 32, 48), 255),
        rect(5, 6, 7, 8, (255, 0, 0), 255),
    ];
    assert_eq!(cmds("draw_rects([[1, 2, 3, 4, 1056816], [5, 6, 7, 8, #ff0000]])"), want);
    assert_eq!(cmds("draw_rects([1, 2, 3, 4, 1056816, 5, 6, 7, 8, 16711680])"), want);
    assert_eq!(
        cmds(
            "let src = [1, 2, 3, 4, 1056816, 5, 6, 7, 8, 16711680]\n\
             let a = f64_array(10)\n\
             for i in range(0, 10) do a[i] = src[i] end\n\
             draw_rects(a)"
        ),
        want
    );
    assert_eq!(cmds("draw_rects([])"), []);
}

#[test]
fn a_batch_reports_a_bad_index_and_a_ragged_list() {
    let err = ui("draw_rects([0, 0, 1, 1, 3], [#ff0000])").frame().unwrap_err();
    assert!(err.contains("palette index 3 is outside the palette"), "{err}");
    let err = ui("draw_rects([0, 0, 1, 1])").frame().unwrap_err();
    assert!(err.contains("five values per rect"), "{err}");
}

// ── cached_layer ──────────────────────────────────────────────────────────

/// `runs` counts how often the body ran; `key` and the rect move on keys.
const CACHED: &str = "state runs = 0\n\
                      state key = 1\n\
                      state x = 100\n\
                      state w = 40\n\
                      if key_pressed(\"k\") then key = key + 1 end\n\
                      if key_pressed(\"m\") then x = x + 10 end\n\
                      if key_pressed(\"w\") then w = w + 10 end\n\
                      var ran = false\n\
                      cached_layer(key, rect(x, 20, w, 30), fn()\n\
                        set ran = true\n\
                        draw_rect(0, 0, 10, 10, #ff0000)\n\
                        draw_line(0, 0, 5, 5, #00ff00)\n\
                      end)\n\
                      if get ran then runs = runs + 1 end";

fn expanded(x: i32, w: u32) -> Vec<DrawCommand> {
    vec![
        DrawCommand::ClipPush { x, y: 20, w, h: 30, radius: 0 },
        rect(x, 20, 10, 10, (255, 0, 0), 255),
        DrawCommand::Line { x1: x, y1: 20, x2: x + 5, y2: 25, r: 0, g: 255, b: 0, a: 255, width: 1 },
        DrawCommand::ClipPop,
    ]
}

#[test]
fn the_body_runs_once_and_the_picture_is_there_every_frame() {
    let mut ui = ui(CACHED);
    assert_eq!(ui.frame().unwrap(), expanded(100, 40));
    assert_eq!(ui.state_int("runs"), Some(1));
    // An input that changes nothing the layer depends on: the frame runs,
    // the body does not, and the host is handed the same picture.
    ui.key("z").unwrap();
    ui.key("z").unwrap();
    assert_eq!(ui.state_int("runs"), Some(1));
    assert_eq!(ui.commands, expanded(100, 40));
}

#[test]
fn a_new_key_or_size_runs_the_body_again_and_a_move_does_not() {
    let mut ui = ui(CACHED);
    ui.frame().unwrap();
    ui.key("m").unwrap();
    assert_eq!(ui.state_int("runs"), Some(1), "moved: re-drawn, not re-run");
    assert_eq!(ui.commands, expanded(110, 40));
    ui.key("k").unwrap();
    assert_eq!(ui.state_int("runs"), Some(2), "a new key");
    ui.key("w").unwrap();
    assert_eq!(ui.state_int("runs"), Some(3), "a new size");
    assert_eq!(ui.commands, expanded(110, 50));
    ui.key("z").unwrap();
    assert_eq!(ui.state_int("runs"), Some(3));
}

#[test]
fn invalidate_caches_runs_every_body_again() {
    let mut ui = ui(CACHED);
    ui.frame().unwrap();
    ui.key("z").unwrap();
    assert_eq!(ui.state_int("runs"), Some(1));
    invalidate_caches(&mut ui.env);
    ui.frame().unwrap();
    assert_eq!(ui.state_int("runs"), Some(2));
    ui.key("z").unwrap();
    assert_eq!(ui.state_int("runs"), Some(2));
}

/// A host that keeps the layers itself is handed the recording once, in the
/// layer's own coordinates, and after that only where to draw it.
#[test]
fn a_host_that_keeps_layers_gets_the_recording_then_only_the_draw() {
    let mut ui = ui(CACHED);
    set_host_caps(&mut ui.env, HostCaps { retained_layers: true, ..HostCaps::default() });
    let first = ui.frame().unwrap().to_vec();
    let id = match first[0] {
        DrawCommand::CacheBegin { id, w: 40, h: 30 } => id,
        ref other => panic!("expected the recording to open, got {other:?}"),
    };
    assert_eq!(
        first,
        [
            DrawCommand::CacheBegin { id, w: 40, h: 30 },
            rect(0, 0, 10, 10, (255, 0, 0), 255),
            DrawCommand::Line { x1: 0, y1: 0, x2: 5, y2: 5, r: 0, g: 255, b: 0, a: 255, width: 1 },
            DrawCommand::CacheEnd { id },
            DrawCommand::CacheDraw { id, x: 100, y: 20 },
        ]
    );
    ui.key("m").unwrap();
    assert_eq!(ui.commands, [DrawCommand::CacheDraw { id, x: 110, y: 20 }]);
    assert_eq!(ui.state_int("runs"), Some(1));
}

/// A layer that stops being drawn is let go, and recorded again when it
/// comes back — whether the host or the drain was keeping it.
#[test]
fn a_layer_that_was_not_drawn_for_a_frame_is_recorded_again() {
    let mut ui = ui("state runs = 0\n\
                     state show = true\n\
                     if key_pressed(\"s\") then show = !show end\n\
                     var ran = false\n\
                     cached_layer(0, rect(0, 0, 10, 10), fn() draw_rect(0, 0, 1, 1, #ffffff) end)\n\
                     if show then\n\
                       cached_layer(1, rect(50, 0, 10, 10), fn()\n\
                         set ran = true\n\
                         draw_rect(0, 0, 2, 2, #ffffff)\n\
                       end)\n\
                     end\n\
                     if get ran then runs = runs + 1 end");
    ui.frame().unwrap();
    assert_eq!(ui.state_int("runs"), Some(1));
    ui.key("s").unwrap(); // hidden
    ui.key("s").unwrap(); // back
    assert_eq!(ui.state_int("runs"), Some(2));
    assert!(ui.commands.contains(&rect(50, 0, 2, 2, (255, 255, 255), 255)));
}

#[test]
fn layers_nest() {
    let out = cmds(
        "cached_layer(1, rect(100, 100, 50, 50), fn()\n\
           draw_rect(0, 0, 50, 50, #000000)\n\
           cached_layer(1, rect(10, 10, 20, 20), fn() draw_rect(1, 1, 2, 2, #ffffff) end)\n\
         end)",
    );
    assert_eq!(
        out,
        [
            DrawCommand::ClipPush { x: 100, y: 100, w: 50, h: 50, radius: 0 },
            rect(100, 100, 50, 50, (0, 0, 0), 255),
            DrawCommand::ClipPush { x: 110, y: 110, w: 20, h: 20, radius: 0 },
            rect(111, 111, 2, 2, (255, 255, 255), 255),
            DrawCommand::ClipPop,
            DrawCommand::ClipPop,
        ]
    );
}

/// The Moses case: a few hundred rects, said once.
#[test]
fn a_cached_batch_is_one_recording_of_one_command() {
    let mut ui = ui("state cells = []\n\
                     if len(cells) == 0 then\n\
                       for i in range(0, 300) do\n\
                         cells = append(cells, [i % 20 * 4, i / 20 * 4, 4, 4, i % 3])\n\
                       end\n\
                     end\n\
                     cached_layer(1, rect(10, 10, 80, 60), fn()\n\
                       draw_rects(cells, [#ff0000, #00ff00, #0000ff])\n\
                     end)");
    set_host_caps(&mut ui.env, HostCaps { batch_rects: true, retained_layers: true });
    let first = ui.frame().unwrap().to_vec();
    assert_eq!(first.len(), 4, "begin, one batch, end, draw");
    match &first[1] {
        DrawCommand::Rects { rects } => assert_eq!(rects.len(), 300),
        other => panic!("expected one batch, got {other:?}"),
    }
    ui.key("z").unwrap();
    assert_eq!(ui.commands.len(), 1, "after that, only the draw");
}

// ── draw_image ────────────────────────────────────────────────────────────

#[test]
fn draw_image_keeps_its_old_shapes_and_takes_tint_atlas_and_nine_slice() {
    let out = cmds(
        "draw_image(\"a.png\", 1, 2, 30, 40)\n\
         draw_image(\"a.png\", rect(1, 2, 30, 40), 128, 6)\n\
         draw_image(\"a.png\", rect(1, 2, 30, 40), {tint: #ff8000, src: rect(16, 0, 8, 8)})\n\
         draw_image(\"a.png\", 1, 2, 30, 40, {slice: 5, a: 200})\n\
         draw_image(\"a.png\", rect(1, 2, 30, 40), {slice: {l: 1, t: 2, r: 3, b: 4}, tint: 255})",
    );
    let plain = |a: u8, radius: u32| DrawCommand::Image {
        source: "a.png".into(),
        x: 1,
        y: 2,
        w: 30,
        h: 40,
        a,
        radius,
        tint: None,
        src: None,
        slice: None,
    };
    assert_eq!(out[0], plain(255, 0));
    assert_eq!(out[1], plain(128, 6));
    assert_eq!(
        out[2],
        DrawCommand::Image {
            source: "a.png".into(),
            x: 1,
            y: 2,
            w: 30,
            h: 40,
            a: 255,
            radius: 0,
            tint: Some([255, 128, 0]),
            src: Some([16, 0, 8, 8]),
            slice: None,
        }
    );
    assert!(matches!(
        out[3],
        DrawCommand::Image { a: 200, tint: None, src: None, slice: Some([5, 5, 5, 5]), .. }
    ));
    assert!(matches!(
        out[4],
        DrawCommand::Image { tint: Some([0, 0, 255]), slice: Some([1, 2, 3, 4]), .. }
    ));
    // A plain image is the JSON it always was.
    assert_eq!(
        serde_json::to_string(&out[0]).unwrap(),
        r#"{"op":"image","source":"a.png","x":1,"y":2,"w":30,"h":40}"#
    );
}

#[test]
fn image_size_and_image_load_answer_from_what_the_host_published() {
    let mut ui = ui("state w = -1\n\
                     state ready = false\n\
                     ready = image_load(\"ui/atlas.png\")\n\
                     let s = image_size(\"ui/atlas.png\")\n\
                     if s != nil then w = s.w end");
    ui.frame().unwrap();
    assert_eq!(ui.state_int("w"), Some(-1));
    assert_eq!(take_image_requests(&mut ui.env), ["ui/atlas.png"]);

    bind_image_sizes(&mut ui.env, &[("ui/atlas.png", 256, 128)]);
    ui.frame().unwrap();
    assert_eq!(ui.state_int("w"), Some(256));
    assert_eq!(ui.state().get("ready"), Some(&serde_json::Value::Bool(true)));
    assert!(take_image_requests(&mut ui.env).is_empty(), "nothing left to ask for");
}

// ── Text decorations ──────────────────────────────────────────────────────

fn texts(cmds: &[DrawCommand]) -> Vec<(i32, i32, (u8, u8, u8), u8)> {
    cmds.iter()
        .filter_map(|c| match c {
            DrawCommand::Text { x, y, r, g, b, a, .. } => Some((*x, *y, (*r, *g, *b), *a)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_shadow_is_one_copy_under_the_text() {
    let out = cmds("draw_text(\"hi\", 10, 20, {size: 14, color: #ffffff, shadow: true})");
    assert_eq!(
        texts(&out),
        [(11, 21, (0, 0, 0), 160), (10, 20, (255, 255, 255), 255)]
    );
    let out = cmds(
        "draw_text(\"hi\", {x: 10, y: 20}, {size: 14, color: #ffffff, shadow: {dx: 0, dy: 3, color: #102030, a: 255}})",
    );
    assert_eq!(texts(&out)[0], (10, 23, (16, 32, 48), 255));
}

#[test]
fn an_outline_is_eight_copies_around_the_text_and_fades_with_it() {
    let out = cmds(
        "draw_text(\"hi\", 10, 20, {size: 14, color: {r: 255, g: 255, b: 255, a: 128}, outline: #000000})",
    );
    let t = texts(&out);
    assert_eq!(t.len(), 9);
    assert_eq!(t[8], (10, 20, (255, 255, 255), 128), "the text itself is last");
    assert!(t[..8].iter().all(|c| c.2 == (0, 0, 0) && c.3 == 128));
    let mut offsets: Vec<(i32, i32)> = t[..8].iter().map(|c| (c.0 - 10, c.1 - 20)).collect();
    offsets.sort();
    assert_eq!(
        offsets,
        [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)]
    );
    let wide = cmds("draw_text(\"hi\", 10, 20, {size: 14, outline: {color: #ff0000, width: 2}})");
    assert!(texts(&wide)[..8].iter().any(|c| (c.0, c.1) == (12, 22)));
}

#[test]
fn tracking_is_letter_spacing_for_drawing_and_measuring_alike() {
    let mut ui = ui("state plain = 0\nstate wide = 0\nstate spaced = 0\n\
                     plain = text_width(\"hello\", {size: 14})\n\
                     wide = text_width(\"hello\", {size: 14, tracking: 2})\n\
                     spaced = text_width(\"hello\", {size: 14, spacing: 2})\n\
                     draw_text(\"hello\", 0, 0, {size: 14, tracking: 2})");
    let out = ui.frame().unwrap().to_vec();
    assert!(matches!(out[0], DrawCommand::Text { spacing, .. } if spacing == 2.0));
    assert_eq!(ui.state_int("wide"), ui.state_int("spaced"));
    assert!(ui.state_int("wide") > ui.state_int("plain"));
}

/// A host that never drains the requests does not accumulate them forever.
#[test]
fn undrained_image_requests_do_not_pile_up() {
    let mut ui = ui("for i in range(0, 300) do image_load(\"missing-{i}.png\") end\n\
                     let t = time()");
    for _ in 0..40 {
        ui.frame().unwrap();
    }
    let sym = ui.env.intern_symbol(petal_ui::draw::IMAGE_REQUESTS_BUFFER);
    let queued = ui.env.output_buffer(sym).len();
    assert!(queued <= 1024 + 300, "{queued} requests queued after 40 frames of 300");
    assert_eq!(take_image_requests(&mut ui.env).len(), 300, "each source once");
}
