//! Sub-pixel draw commands (`DrawOptions::subpixel`, `take_frame_commands`,
//! `pixel_snap`): a host that opts in gets a script's fractions, and a host
//! that does not — or that drains whole pixels — sees exactly what it saw
//! before.
//!
//! Drives raw `Env`s and `FrameCore` only, so it also runs with
//! `--no-default-features`.

use petal::env::Env;
use petal::execution_context::EmitSite;
use petal::heap::Heap;
use petal::value::Value;
use petal_ui::draw::{
    DRAW_COMMANDS_SYMBOL, DrawCommand, DrawOptions, FrameCommand, SubCommand, register_draw,
    register_draw_with, take_draw_commands, take_draw_commands_for, take_draw_commands_traced,
    take_frame_commands, take_frame_commands_for, take_frame_commands_snapped,
    take_frame_commands_traced,
};
use petal_ui::subpixel::PixelSnap;
use petal_ui::frame_core::{FrameCore, FrameRun};

const SUB: DrawOptions = DrawOptions { subpixel: true };

/// An env with the input and draw natives (`options`) and the `ui` package.
fn env_with(options: DrawOptions, ui: bool) -> Env {
    let mut env = Env::new();
    petal_ui::input::register_input(&mut env);
    register_draw_with(&mut env, options);
    if ui {
        petal_ui::register_prelude(&mut env);
    }
    env
}

/// Run `body` once in a fresh env: under `import ui` when `ui`, against the
/// raw natives otherwise. The draw buffer is left for the caller to drain.
fn run(options: DrawOptions, ui: bool, body: &str) -> Env {
    let mut env = env_with(options, ui);
    let script = petal_ui::script_for(&env, body);
    env.run_source(&script).unwrap_or_else(|e| panic!("{e}\n{body}"));
    env
}

fn raw_buffer(env: &mut Env) -> Vec<Value> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    env.output_buffer(sym).to_vec()
}

/// The raw buffer as text: each command's tag and its args, numbers shown
/// with their kind (`1` an int, `1.5` / `2.0f` a float).
fn show_raw(values: &[Value], heap: &Heap) -> String {
    fn arg(v: &Value, heap: &Heap) -> String {
        match v {
            Value::Int(n) => n.to_string(),
            Value::Float(f) => format!("{f:?}f"),
            Value::Bool(b) => b.to_string(),
            Value::Nil => "nil".into(),
            Value::String(id) => format!("{:?}", heap.get_string(*id)),
            Value::List(id) => {
                let items: Vec<String> = heap.get_list(*id).iter().map(|v| arg(v, heap)).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Map(_) => "{..}".into(),
            other => other.type_name().to_string(),
        }
    }
    values
        .iter()
        .map(|v| match v {
            Value::EnumVariant { tag, data } => {
                let args: Vec<String> = heap.get_list(*data).iter().map(|v| arg(v, heap)).collect();
                format!("{}({})", heap.tag_str(*tag), args.join(", "))
            }
            other => arg(other, heap),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every geometry native, with fractional and negative numbers (and whole
/// ones), through the raw natives.
const RAW_SCRIPT: &str = "\
draw_rect(1.6, 2.4, 30.9, 4.5, 10, 20, 30)
draw_rect(-1.6, -2.4, 3.9, 4.5, 10, 20, 30, 128)
draw_rect_rounded(1.5, 2.5, 30.5, 4.5, 3.5, 1, 2, 3)
draw_rect_outline(1.5, 2.5, 30.5, 4.5, 1, 2, 3, 200, 2.5)
draw_rect_rounded_outline(1.5, 2.5, 30.5, 4.5, 3.5, 1, 2, 3, 200, 1.5)
draw_rect_gradient(1.5, 2.5, 3.5, 4.5, 1, 2, 3, 4, 5, 6, 7, 8, 0.5)
draw_rect_gradient_rounded(1.5, 2.5, 3.5, 4.5, 2.5, 1, 2, 3, 4, 5, 6, 7, 8, 0.5)
draw_circle_gradient(10.5, 20.25, 5.75, 1, 2, 3, 4, 5, 6, 7, 8)
draw_shadow(1.5, 2.5, 30.5, 4.5, 3.5, -2.5, 1.5, -0.5, 0.75, 1, 2, 3)
draw_line(0.5, 1.5, 10.25, -3.75, 1, 2, 3, 255, 1.5)
draw_line(0, 1, 10, 3, 1, 2, 3)
draw_circle(10.5, -20.25, 1.6, 1, 2, 3)
draw_circle_outline(10.5, 20.25, 1.6, 1, 2, 3, 100, 0.5)
draw_ellipse(10.5, 20.25, 3.5, 2.25, 1, 2, 3)
draw_ellipse_outline(10.5, 20.25, 3.5, 2.25, 1, 2, 3, 255, 1.5)
fill_arc(10.5, 20.25, 1.5, 4.5, 0.25, 3.0, 1, 2, 3)
fill_triangle(0.5, 1.5, 2.5, 3.5, 4.5, -5.5, 1, 2, 3)
fill_poly([[0.5, 1.5], [2.5, 3.5], [4.5, 5.5]], 1, 2, 3)
fill_polygon([[0.5, 1.5], {x: 2.5, y: 3.5}, [4.5, 5.5]], 1, 2, 3)
fill_fan(1.5, 2.5, [[0.5, 1.5], [2.5, 3.5]], 1, 2, 3)
draw_polyline([[0.5, 1.5], [2.5, 3.5]], 1, 2, 3, 255, 2.5)
draw_text(\"hi\", 3.5, -0.5, 14, 1, 2, 3)
draw_image(\"a.png\", 1.5, 2.5, 30.5, 40.5, 128, 2.5)
draw_canvas(1, 2.5, 3.5, 255, 10.5, 20.5)
clip(1.5, 2.5, 3.5, 4.5)
pixel_snap(false)
draw_circle(1.5, 2.5, 3.5, 1, 2, 3)
pixel_snap(true)
pixel_snap(nil)";

/// What the default natives emit for [`RAW_SCRIPT`] — today's buffer, before
/// sub-pixel commands existed. A host that does not opt in must keep seeing
/// exactly this.
const RAW_TODAY: &str = "\
rect(1, 2, 30, 4, 10, 20, 30, 255)
rect(-1, -2, 3, 4, 10, 20, 30, 128)
rect(1, 2, 30, 4, 1, 2, 3, 255, 3)
rect_outline(1, 2, 30, 4, 1, 2, 3, 200, 2, 0)
rect_outline(1, 2, 30, 4, 1, 2, 3, 200, 1, 3)
rect_gradient(1, 2, 3, 4, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0.5f)
rect_gradient(1, 2, 3, 4, 2, 1, 2, 3, 4, 5, 6, 7, 8, 0.5f)
circle_gradient(10, 20, 5, 1, 2, 3, 4, 5, 6, 7, 8)
shadow(1, 2, 30, 4, 3, 0, 1, 0, 0, 1, 2, 3, 255)
line(0, 1, 10, -3, 1, 2, 3, 255, 1)
line(0, 1, 10, 3, 1, 2, 3, 255, 1)
circle(10, -20, 1, 1, 2, 3, 255)
ellipse_outline(10, 20, 1, 1, 1, 2, 3, 100, 0)
ellipse(10, 20, 3, 2, 1, 2, 3, 255)
ellipse_outline(10, 20, 3, 2, 1, 2, 3, 255, 1)
arc(10, 20, 1.5f, 4.5f, 0.25f, 3.0f, 1, 2, 3, 255)
triangle(0, 1, 2, 3, 4, -5, 1, 2, 3, 255)
poly([[0.5f, 1.5f], [2.5f, 3.5f], [4.5f, 5.5f]], 1, 2, 3, 255)
polygon([[0.5f, 1.5f], {..}, [4.5f, 5.5f]], 1, 2, 3, 255)
fan(1, 2, [[0.5f, 1.5f], [2.5f, 3.5f]], 1, 2, 3, 255)
polyline([[0.5f, 1.5f], [2.5f, 3.5f]], 1, 2, 3, 255, 2)
text(\"hi\", 3, 0, 14, 1, 2, 3, 255)
image(\"a.png\", 1, 2, 30, 40, 128, 2)
draw_canvas(1, 2, 3, 255, 10, 20)
clip(1, 2, 3, 4, 0)
circle(1, 2, 3, 1, 2, 3, 255)";

#[test]
fn a_host_that_does_not_opt_in_emits_todays_buffer() {
    // `register_draw` and `register_draw_with(default)` are the same thing,
    // and `pixel_snap` emits nothing on either.
    for options in [None, Some(DrawOptions::default())] {
        let mut env = Env::new();
        petal_ui::input::register_input(&mut env);
        match options {
            None => register_draw(&mut env),
            Some(o) => register_draw_with(&mut env, o),
        }
        let script = petal_ui::script_for(&env, RAW_SCRIPT);
        env.run_source(&script).unwrap();
        let raw = raw_buffer(&mut env);
        assert_eq!(show_raw(&raw, env.heap()), RAW_TODAY, "{options:?}");
    }
}

#[test]
fn a_subpixel_host_emits_the_scripts_own_numbers() {
    let mut env = run(SUB, false, RAW_SCRIPT);
    let raw = raw_buffer(&mut env);
    let shown = show_raw(&raw, env.heap());
    let line = |tag: &str, n: usize| -> String {
        shown.lines().filter(|l| l.starts_with(&format!("{tag}("))).nth(n).unwrap().to_string()
    };
    assert_eq!(line("rect", 0), "rect(1.6f, 2.4f, 30.9f, 4.5f, 10, 20, 30, 255)");
    assert_eq!(line("circle", 0), "circle(10.5f, -20.25f, 1.6f, 1, 2, 3, 255)");
    assert_eq!(line("line", 0), "line(0.5f, 1.5f, 10.25f, -3.75f, 1, 2, 3, 255, 1.5f)");
    // Whole numbers stay ints.
    assert_eq!(line("line", 1), "line(0, 1, 10, 3, 1, 2, 3, 255, 1)");
    assert_eq!(line("shadow", 0), "shadow(1.5f, 2.5f, 30.5f, 4.5f, 3.5f, 0.0f, 1.5f, -0.5f, 0.75f, 1, 2, 3, 255)");
    assert_eq!(line("text", 0), "text(\"hi\", 3.5f, -0.5f, 14, 1, 2, 3, 255)");
    // Clips are whole pixels on every host.
    assert_eq!(line("clip", 0), "clip(1, 2, 3, 4, 0)");
    // The markers: `false`, `true`, `nil`.
    let marks: Vec<&str> = shown.lines().filter(|l| l.starts_with("pixel_snap(")).collect();
    assert_eq!(marks, ["pixel_snap(2)", "pixel_snap(1)", "pixel_snap(0)"]);
}

/// The same frame through `import ui`: the record overloads, decorated
/// text, a batch, layers and a retained layer.
const UI_SCRIPT: &str = "\
draw_rect(rect(1.6, 2.4, 30.9, 4.5), :102030)
draw_rect({x: -0.5, y: 2.5, w: 3.5, h: 4.5}, :102030, 128)
draw_circle({x: 10.5, y: 20.25}, 1.6, :ff0000)
draw_line({x: 0.5, y: 1.5}, {x: -10.25, y: 3.75}, :00ff00, 255, 2.5)
pixel_snap(false)
draw_text(\"hi\", {x: -0.5, y: 2.5}, {size: 14, color: :ffffff, shadow: true, outline: {width: 1}})
draw_text(\"hi\", 0.5, -1.5, 14, 1, 2, 3)
ui_layer(2)
draw_circle({x: 1.5, y: 2.5}, 3.5, :0000ff)
ui_layer(0)
draw_rects([1.5, 2.5, 3.5, 4.5, :ff0000, 5.5, 6.5, 7.5, 8.5, :00ff00])
pixel_snap(true)
cached_layer(\"k\", rect(3, 4, 50, 50), fn() draw_circle({x: 10.5, y: 7.25}, 2.5, :ff0000) end)
draw_polyline([{x: 0.5, y: 1.5}, {x: 2.5, y: 3.5}], :102030, 255, 1.5)";

#[test]
fn whole_pixel_drains_on_a_subpixel_env_return_what_a_default_env_returns() {
    for (ui, script) in [(false, RAW_SCRIPT), (true, UI_SCRIPT)] {
        let mut default = run(DrawOptions::default(), ui, script);
        let mut sub = run(SUB, ui, script);
        let want = take_draw_commands(&mut default);
        assert!(want.len() > 10);
        assert_eq!(take_draw_commands(&mut sub), want, "{script}");

        let mut default = run(DrawOptions::default(), ui, script);
        let mut sub = run(SUB, ui, script);
        let want: Vec<DrawCommand> =
            take_draw_commands_traced(&mut default).into_iter().map(|(c, _)| c).collect();
        let got: Vec<DrawCommand> =
            take_draw_commands_traced(&mut sub).into_iter().map(|(c, _)| c).collect();
        assert_eq!(got, want);

        // …and a frame drained as sub-pixel commands is the same frame: the
        // same commands in the same order, each truncated to what the
        // whole-pixel drain hands over.
        let mut sub = run(SUB, ui, script);
        let frame = take_frame_commands(&mut sub);
        let px: Vec<DrawCommand> = frame.iter().map(FrameCommand::to_px).collect();
        assert_eq!(px, want);
    }
}

#[test]
fn the_marker_never_reaches_a_drain() {
    let is_marker = |c: &DrawCommand| matches!(c, DrawCommand::Host { tag, .. } if tag == "pixel_snap");
    let mut env = run(SUB, false, RAW_SCRIPT);
    assert!(!take_draw_commands(&mut env).iter().any(is_marker));
    let mut env = run(SUB, false, RAW_SCRIPT);
    assert!(!take_draw_commands_traced(&mut env).iter().any(|(c, _)| is_marker(c)));
    let mut env = run(SUB, false, RAW_SCRIPT);
    assert!(!take_frame_commands(&mut env).iter().any(|c| is_marker(&c.to_px())));
    let mut env = run(SUB, false, RAW_SCRIPT);
    assert!(!take_frame_commands_traced(&mut env).iter().any(|(c, _)| is_marker(&c.to_px())));
    // The wire encodes what the drain returned: no `host` opcode either.
    let mut env = run(SUB, false, RAW_SCRIPT);
    let wire = petal_ui::draw_wire::take_frame_wire(&mut env);
    assert!(!wire.strings.iter().any(|s| s == "pixel_snap"));
}

#[test]
fn the_marker_never_reaches_a_fork_drain() {
    let mut env = env_with(SUB, false);
    let script = petal_ui::script_for(&env, "pixel_snap(false)\ndraw_circle(1.5, 2.5, 3.5, 1, 2, 3)");
    let program = env.load_program(&script).unwrap();
    let stack = env.create_stack(program).unwrap();
    for frame_drain in [false, true] {
        let fork = env.fork_execution(stack).unwrap();
        env.reset_stack(fork).unwrap();
        env.run(fork).unwrap();
        if frame_drain {
            let cmds = take_frame_commands_for(&mut env, fork);
            assert_eq!(
                cmds,
                [FrameCommand::Sub(SubCommand::Circle { cx: 1.5, cy: 2.5, radius: 3.5, r: 1, g: 2, b: 3, a: 255 })]
            );
        } else {
            let cmds = take_draw_commands_for(&mut env, fork);
            assert_eq!(
                cmds,
                [DrawCommand::Circle { cx: 1, cy: 2, radius: 3, r: 1, g: 2, b: 3, a: 255 }]
            );
        }
        env.drop_fork(fork);
    }
}

#[test]
fn fractions_reach_sub_commands_unmodified() {
    let mut env = run(SUB, false, &format!("pixel_snap(false)\n{RAW_SCRIPT}"));
    let frame = take_frame_commands(&mut env);
    let sub = |n: usize| match &frame[n] {
        FrameCommand::Sub(s) => s.clone(),
        other => panic!("command {n} is not sub-pixel: {other:?}"),
    };
    assert_eq!(
        sub(0),
        SubCommand::Rect { x: 1.6, y: 2.4, w: 30.9, h: 4.5, r: 10, g: 20, b: 30, a: 255, radius: 0.0 }
    );
    assert_eq!(
        sub(3),
        SubCommand::RectOutline {
            x: 1.5, y: 2.5, w: 30.5, h: 4.5, r: 1, g: 2, b: 3, a: 200, width: 2.5, radius: 0.0
        }
    );
    assert_eq!(
        sub(8),
        SubCommand::Shadow {
            x: 1.5, y: 2.5, w: 30.5, h: 4.5, radius: 3.5, blur: 0.0, spread: 1.5, dx: -0.5,
            dy: 0.75, r: 1, g: 2, b: 3, a: 255
        }
    );
    assert_eq!(
        sub(9),
        SubCommand::Line { x1: 0.5, y1: 1.5, x2: 10.25, y2: -3.75, r: 1, g: 2, b: 3, a: 255, width: 1.5 }
    );
    assert_eq!(sub(11), SubCommand::Circle { cx: 10.5, cy: -20.25, radius: 1.6, r: 1, g: 2, b: 3, a: 255 });
    assert_eq!(
        sub(12),
        SubCommand::EllipseOutline {
            cx: 10.5, cy: 20.25, rx: 1.6, ry: 1.6, r: 1, g: 2, b: 3, a: 100, width: 0.5
        }
    );
    assert_eq!(
        sub(18),
        SubCommand::Polygon { points: vec![(0.5, 1.5), (2.5, 3.5), (4.5, 5.5)], r: 1, g: 2, b: 3, a: 255 }
    );
    assert_eq!(
        sub(19),
        SubCommand::Fan { cx: 1.5, cy: 2.5, points: vec![(0.5, 1.5), (2.5, 3.5)], r: 1, g: 2, b: 3, a: 255 }
    );
    assert_eq!(
        sub(20),
        SubCommand::Polyline { points: vec![(0.5, 1.5), (2.5, 3.5)], r: 1, g: 2, b: 3, a: 255, width: 2.5 }
    );
    match sub(21) {
        SubCommand::Text { x, y, size, .. } => assert_eq!((x, y, size), (3.5, -0.5, 14)),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        sub(22),
        SubCommand::Image {
            source: "a.png".into(), x: 1.5, y: 2.5, w: 30.5, h: 40.5, a: 128, radius: 2.5,
            tint: None, src: None, slice: None
        }
    );
    assert_eq!(sub(23), SubCommand::DrawCanvas { id: 1, x: 2.5, y: 3.5, a: 255, w: 10.5, h: 20.5 });
    // A clip has no sub-pixel twin.
    assert_eq!(frame[24], FrameCommand::Px(DrawCommand::Clip { x: 1, y: 2, w: 3, h: 4, radius: 0 }));
}

/// A tiny deterministic generator, so the property below is the same on
/// every run.
struct Lcg(u64);

impl Lcg {
    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        lo + ((self.0 >> 33) as i64).rem_euclid(hi - lo + 1)
    }
}

#[test]
fn whole_numbers_make_the_same_command_either_way() {
    let mut rng = Lcg(7);
    for _ in 0..20 {
        let mut n = |lo, hi| rng.int(lo, hi);
        let script = format!(
            "draw_rect({}, {}, {}, {}, {}, {}, {}, {})\n\
             draw_rect_outline({}, {}, {}, {}, 1, 2, 3, 200, {})\n\
             draw_line({}, {}, {}, {}, 1, 2, 3, 255, {})\n\
             draw_circle({}, {}, {}, 1, 2, 3)\n\
             draw_ellipse_outline({}, {}, {}, {}, 1, 2, 3, 255, {})\n\
             fill_triangle({}, {}, {}, {}, {}, {}, 1, 2, 3)\n\
             fill_fan({}, {}, [[{}, {}], [{}, {}]], 1, 2, 3)\n\
             draw_polyline([[{}, {}], [{}, {}]], 1, 2, 3, 255, {})\n\
             draw_shadow({}, {}, {}, {}, {}, {}, {}, {}, {}, 1, 2, 3)\n\
             draw_text(\"t\", {}, {}, 12, 1, 2, 3)\n\
             draw_canvas(1, {}, {}, 255, {}, {})\n\
             draw_image(\"i\", {}, {}, {}, {}, 255, {})",
            n(-50, 50), n(-50, 50), n(0, 99), n(0, 99), n(0, 255), n(0, 255), n(0, 255), n(0, 255),
            n(-50, 50), n(-50, 50), n(0, 99), n(0, 99), n(1, 5),
            n(-50, 50), n(-50, 50), n(-50, 50), n(-50, 50), n(1, 5),
            n(-50, 50), n(-50, 50), n(0, 20),
            n(-50, 50), n(-50, 50), n(0, 20), n(0, 20), n(1, 5),
            n(-50, 50), n(-50, 50), n(-50, 50), n(-50, 50), n(-50, 50), n(-50, 50),
            n(-50, 50), n(-50, 50), n(-50, 50), n(-50, 50), n(-50, 50), n(-50, 50),
            n(-50, 50), n(-50, 50), n(-50, 50), n(-50, 50), n(1, 5),
            n(-50, 50), n(-50, 50), n(0, 99), n(0, 99), n(0, 9), n(0, 9), n(-5, 5), n(-5, 5), n(-5, 5),
            n(-50, 50), n(-50, 50),
            n(-50, 50), n(-50, 50), n(0, 99), n(0, 99),
            n(-50, 50), n(-50, 50), n(0, 99), n(0, 99), n(0, 9),
        );
        let px = take_draw_commands(&mut run(DrawOptions::default(), false, &script));
        let frame =
            take_frame_commands(&mut run(SUB, false, &format!("pixel_snap(false)\n{script}")));
        assert_eq!(frame.len(), px.len());
        for (f, p) in frame.iter().zip(&px) {
            let want = SubCommand::from_px(p).expect("every command here has a twin");
            assert_eq!(f, &FrameCommand::Sub(want.clone()), "{script}");
            assert_eq!(&want.to_px(), p);
        }
    }
}

#[test]
fn by_default_the_rect_family_snaps_and_curves_do_not() {
    let mut env = run(
        SUB,
        false,
        "draw_rect(1.5, 2.5, 3.5, 4.5, 1, 2, 3)\n\
         draw_text(\"t\", 1.5, 2.5, 12, 1, 2, 3)\n\
         draw_circle(1.5, 2.5, 3.5, 1, 2, 3)\n\
         draw_line(0.5, 1.5, 2.5, 3.5, 1, 2, 3)",
    );
    let frame = take_frame_commands(&mut env);
    assert_eq!(
        frame[0],
        FrameCommand::Px(DrawCommand::Rect { x: 1, y: 2, w: 3, h: 4, r: 1, g: 2, b: 3, a: 255, radius: 0 })
    );
    assert_eq!(frame[1], FrameCommand::Px(DrawCommand::plain_text("t", 1, 2, 12, 1, 2, 3, 255)));
    assert_eq!(
        frame[2],
        FrameCommand::Sub(SubCommand::Circle { cx: 1.5, cy: 2.5, radius: 3.5, r: 1, g: 2, b: 3, a: 255 })
    );
    assert!(matches!(frame[3], FrameCommand::Sub(SubCommand::Line { x1: 0.5, .. })));
}

#[test]
fn pixel_snap_switches_between_three_modes_and_each_frame_starts_at_the_default() {
    let body = "draw_rect(1.5, 2.5, 3.5, 4.5, 1, 2, 3)\ndraw_circle(1.5, 2.5, 3.5, 1, 2, 3)\n";
    let script = format!("{body}pixel_snap(true)\n{body}pixel_snap(false)\n{body}pixel_snap(nil)\n{body}");
    let mut env = env_with(SUB, false);
    let script = petal_ui::script_for(&env, &script);
    let program = env.load_program(&script).unwrap();
    let stack = env.create_stack(program).unwrap();
    let mut core = FrameCore::new(env, program, stack);
    let kinds = |frame: &[FrameCommand]| -> String {
        frame.iter().map(|c| if matches!(c, FrameCommand::Sub(_)) { 's' } else { 'p' }).collect()
    };
    for _ in 0..2 {
        core.invalidate();
        assert_eq!(core.frame(0.016, 1, &mut ()).unwrap(), FrameRun::Ran);
        let frame = take_frame_commands(&mut core.env);
        // default: rect snapped, circle not; true: both; false: neither;
        // nil: the default again. The second run starts over at the default.
        assert_eq!(kinds(&frame), "psppssps");
        let all_px: Vec<DrawCommand> = frame.iter().map(FrameCommand::to_px).collect();
        assert_eq!(all_px[2], all_px[0]);
        assert_eq!(frame[2], FrameCommand::Px(all_px[0].clone()));
    }
}

/// The snapped drain reports each command's mode, so a host can tell a
/// rect drawn by default from one drawn under `pixel_snap(true)` — both `Px`.
#[test]
fn the_snapped_drain_reports_each_commands_mode() {
    let body = "draw_rect(1.5, 2.5, 3.5, 4.5, 1, 2, 3)\n";
    let script = format!("{body}pixel_snap(true)\n{body}pixel_snap(false)\n{body}");
    let mut env = run(SUB, false, &script);
    let frame = take_frame_commands_snapped(&mut env);
    let modes: Vec<PixelSnap> = frame.iter().map(|(_, m)| *m).collect();
    assert_eq!(modes, [PixelSnap::Rects, PixelSnap::All, PixelSnap::Off]);
    assert!(matches!(frame[0].0, FrameCommand::Px(_)));
    assert!(matches!(frame[1].0, FrameCommand::Px(_)));
    assert!(matches!(frame[2].0, FrameCommand::Sub(_)));
}

#[test]
fn a_retained_layer_moves_its_content_in_f32_and_keeps_its_mode() {
    let src = "pixel_snap(false)\n\
               cached_layer(\"k\", rect(3, 4, 50, 50), fn()\n\
                 draw_circle({x: 10.5, y: 7.25}, 2.5, :ff0000)\n\
                 pixel_snap(true)\n\
                 draw_circle({x: 1.5, y: 1.5}, 1.5, :ff0000)\n\
               end)\n\
               pixel_snap(nil)\n\
               cached_layer(\"j\", rect(1, 1, 9, 9), fn() draw_circle({x: 0.25, y: 0.5}, 2.5, :ff0000) end)";
    let mut env = env_with(SUB, true);
    let script = petal_ui::script_for(&env, src);
    let program = env.load_program(&script).unwrap();
    let stack = env.create_stack(program).unwrap();
    let mut core = FrameCore::new(env, program, stack);
    // Frame 1 records the layers and draws them; frame 2 only draws them,
    // from what the drain kept.
    for frame_no in 1..=2 {
        core.invalidate();
        assert_eq!(core.frame(0.016, frame_no, &mut ()).unwrap(), FrameRun::Ran);
        let frame = take_frame_commands(&mut core.env);
        let circles: Vec<&FrameCommand> = frame
            .iter()
            .filter(|c| matches!(c.to_px(), DrawCommand::Circle { .. }))
            .collect();
        assert_eq!(
            circles,
            [
                &FrameCommand::Sub(SubCommand::Circle { cx: 13.5, cy: 11.25, radius: 2.5, r: 255, g: 0, b: 0, a: 255 }),
                // Snapped inside the layer, however it is drawn.
                &FrameCommand::Px(DrawCommand::Circle { cx: 4, cy: 5, radius: 1, r: 255, g: 0, b: 0, a: 255 }),
                // Recorded under the default: a circle is sub-pixel.
                &FrameCommand::Sub(SubCommand::Circle { cx: 1.25, cy: 1.5, radius: 2.5, r: 255, g: 0, b: 0, a: 255 }),
            ],
            "frame {frame_no}"
        );
        // The clip around each layer is whole pixels.
        assert!(frame.contains(&FrameCommand::Px(DrawCommand::ClipPush { x: 3, y: 4, w: 50, h: 50, radius: 0 })));
    }
}

#[test]
fn the_traced_drains_attribute_past_a_marker() {
    // On the subpixel env `pixel_snap` puts a marker between the draws, which
    // must not shift a later draw's attribution onto the marker's call site:
    // each command resolves to the line and column of its own call, as on a
    // default env (which emits no marker).
    let src = "draw_circle(1, 2, 3, 1, 2, 3)\npixel_snap(false)\ndraw_circle(4.5, 5, 6, 1, 2, 3)\npixel_snap(true)\ndraw_rect(1, 2, 3, 4, 1, 2, 3)";
    let traced = |options: DrawOptions, frame: bool| -> Vec<(u32, u32)> {
        let mut env = env_with(options, false);
        env.enable_emit_trace(true);
        let script = petal_ui::script_for(&env, src);
        let program = env.load_program(&script).unwrap();
        let stack = env.create_stack(program).unwrap();
        env.run(stack).unwrap();
        let sites: Vec<EmitSite> = if frame {
            take_frame_commands_traced(&mut env).into_iter().map(|(_, s)| s).collect()
        } else {
            take_draw_commands_traced(&mut env).into_iter().map(|(_, s)| s).collect()
        };
        let program = env.get_program(program).unwrap();
        sites
            .iter()
            .map(|s| {
                let span = program.source_map.get(s.leaf().expect("attributed")).expect("a span");
                (span.start.line, span.start.column)
            })
            .collect()
    };
    let want = traced(DrawOptions::default(), false);
    assert_eq!(want.iter().map(|p| p.0).collect::<Vec<_>>(), [1, 3, 5], "{want:?}");
    assert_eq!(traced(SUB, false), want);
    assert_eq!(traced(SUB, true), want);
}

#[test]
fn pixel_snap_checks_its_argument_on_every_host() {
    for options in [DrawOptions::default(), SUB] {
        let mut env = env_with(options, false);
        let script = petal_ui::script_for(&env, "pixel_snap(1)");
        let err = env.run_source(&script).unwrap_err();
        assert!(err.contains("pixel_snap() expects true"), "{err}");
    }
}

#[test]
fn a_subpixel_native_rejects_a_non_number_like_its_twin() {
    let msg = |options| {
        let mut env = env_with(options, false);
        let script = petal_ui::script_for(&env, "draw_circle(\"a\", 2, 3, 1, 2, 3)");
        env.run_source(&script).unwrap_err()
    };
    assert_eq!(msg(SUB), msg(DrawOptions::default()));
}
