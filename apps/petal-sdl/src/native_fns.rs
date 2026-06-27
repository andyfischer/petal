use std::cell::RefCell;

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

use crate::input::InputState;

#[derive(Default)]
pub struct FrameInfo {
    pub dt: f64,
    pub frame_count: i64,
    pub screen_width: i32,
    pub screen_height: i32,
}

pub struct ExampleEntry {
    pub name: String,
    pub path: String,
}

#[derive(Default)]
pub struct BrowserState {
    pub examples: Vec<ExampleEntry>,
    pub pending_launch: Option<String>,
}

/// Name of the buffered-output channel that carries draw commands from the
/// sketch to the renderer. The host interns the same name to drain it.
pub const DRAW_COMMANDS_SYMBOL: &str = "draw_commands";

/// Emit a draw command into the `draw_commands` output buffer on the Env.
/// Replaces the old `DRAW_COMMANDS` thread-local push.
fn emit_draw(state: &mut PetalCxt, tag: &str, data: Vec<Value>) {
    let sym = state.intern_symbol(DRAW_COMMANDS_SYMBOL);
    state.emit(sym, tag, data);
}

/// Collect the first `n` arguments (1-indexed) as integer `Value`s — the common
/// shape for draw commands whose arguments are all numbers.
fn int_args(state: &PetalCxt, n: usize) -> Result<Vec<Value>, String> {
    (1..=n).map(|i| state.get_int(i).map(Value::Int)).collect()
}

thread_local! {
    pub static INPUT_STATE: RefCell<InputState> = RefCell::new(InputState::default());
    pub static FRAME_INFO: RefCell<FrameInfo> = RefCell::new(FrameInfo::default());
    pub static BROWSER_STATE: RefCell<BrowserState> = RefCell::new(BrowserState::default());
    /// Next offscreen-canvas id to hand out. Ids start at 1 (0 is reserved for
    /// the main framebuffer) and are allocated by `create_canvas`. The counter
    /// is reset at the top of every frame so ids are stable across the per-frame
    /// re-run model.
    pub static NEXT_CANVAS_ID: RefCell<u32> = RefCell::new(1);
}

/// Reset the per-frame offscreen-canvas id counter. Call this at the start of
/// each frame, before the sketch runs, so `create_canvas` hands out the same
/// ids every frame.
pub fn reset_canvas_ids() {
    NEXT_CANVAS_ID.with(|n| *n.borrow_mut() = 1);
}

pub fn register_all(env: &mut Env) {
    env.register_native("clear", native_clear);
    env.register_native("draw_rect", native_draw_rect);
    env.register_native("draw_rect_outline", native_draw_rect_outline);
    env.register_native("draw_line", native_draw_line);
    env.register_native("draw_circle", native_draw_circle);
    env.register_native("fill_triangle", native_fill_triangle);
    env.register_native("fill_poly", native_fill_poly);
    env.register_native("draw_text", native_draw_text);
    env.register_native("create_canvas", native_create_canvas);
    env.register_native("draw_to", native_draw_to);
    env.register_native("draw_to_screen", native_draw_to_screen);
    env.register_native("draw_canvas", native_draw_canvas);
    env.register_native("key_down", native_key_down);
    env.register_native("key_pressed", native_key_pressed);
    env.register_native("mouse_x", native_mouse_x);
    env.register_native("mouse_y", native_mouse_y);
    env.register_native("mouse_down", native_mouse_down);
    env.register_native("mouse_pressed", native_mouse_pressed);
    env.register_native("dt", native_dt);
    env.register_native("frame_count", native_frame_count);
    env.register_native("screen_width", native_screen_width);
    env.register_native("screen_height", native_screen_height);
    env.register_native("example_count", native_example_count);
    env.register_native("example_name", native_example_name);
    env.register_native("example_path", native_example_path);
    env.register_native("launch_script", native_launch_script);
    env.register_native("load_text_file", native_load_text_file);
    env.register_native("save_text_file", native_save_text_file);
    env.register_native("file_exists", native_file_exists);
}

// --- Drawing ---

fn native_clear(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 3)?;
    emit_draw(state, "clear", args);
    state.push_nil();
    Ok(1)
}

fn native_draw_rect(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 7)?;
    emit_draw(state, "rect", args);
    state.push_nil();
    Ok(1)
}

fn native_draw_rect_outline(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 7)?;
    emit_draw(state, "rect_outline", args);
    state.push_nil();
    Ok(1)
}

fn native_draw_line(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 7)?;
    emit_draw(state, "line", args);
    state.push_nil();
    Ok(1)
}

fn native_draw_circle(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 6)?;
    emit_draw(state, "circle", args);
    state.push_nil();
    Ok(1)
}

fn native_fill_triangle(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 9)?;
    emit_draw(state, "triangle", args);
    state.push_nil();
    Ok(1)
}

fn coord_to_i32(v: &Value) -> Result<i32, String> {
    match v {
        Value::Int(n) => Ok(*n as i32),
        Value::Float(f) => Ok(*f as i32),
        _ => Err("fill_poly() point coords must be numbers".to_string()),
    }
}

fn native_fill_poly(state: &mut PetalCxt) -> NativeResult {
    let points_value = state.get_value(1)?;
    let list_id = match points_value {
        Value::List(id) => id,
        other => {
            return Err(format!(
                "fill_poly() expects a list of points, got {}",
                other.type_name()
            ))
        }
    };

    // Validate the points up front (the renderer re-reads the list on decode).
    let elements: Vec<Value> = state.heap().get_list(list_id).to_vec();
    for el in &elements {
        match el {
            Value::Vec2(_, _) => {}
            Value::List(pid) => {
                let coords = state.heap().get_list(*pid);
                if coords.len() != 2 {
                    return Err(
                        "fill_poly() list points must have exactly 2 coords [x, y]".to_string(),
                    );
                }
                coord_to_i32(&coords[0])?;
                coord_to_i32(&coords[1])?;
            }
            other => {
                return Err(format!(
                    "fill_poly() points must be vec2 or [x, y] lists, got {}",
                    other.type_name()
                ))
            }
        }
    }

    if elements.len() < 3 {
        return Err("fill_poly() needs at least 3 points".to_string());
    }

    let r = state.get_int(2)?;
    let g = state.get_int(3)?;
    let b = state.get_int(4)?;

    emit_draw(
        state,
        "poly",
        vec![points_value, Value::Int(r), Value::Int(g), Value::Int(b)],
    );
    state.push_nil();
    Ok(1)
}

fn native_draw_text(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let args = vec![
        Value::String(state.heap_mut().alloc_string(text)),
        Value::Int(state.get_int(2)?), Value::Int(state.get_int(3)?), Value::Int(state.get_int(4)?),
        Value::Int(state.get_int(5)?), Value::Int(state.get_int(6)?), Value::Int(state.get_int(7)?),
    ];
    emit_draw(state, "text", args);
    state.push_nil();
    Ok(1)
}

// --- Offscreen canvases (PGraphics-style render targets) ---

/// Allocate an offscreen canvas of size `w`x`h` and return its integer id.
/// The id is used with `draw_to`/`draw_canvas` to direct drawing into the
/// canvas and later blit it onto the main framebuffer.
fn native_create_canvas(state: &mut PetalCxt) -> NativeResult {
    let w = state.get_int(1)? as u32;
    let h = state.get_int(2)? as u32;
    let id = NEXT_CANVAS_ID.with(|n| {
        let mut next = n.borrow_mut();
        let id = *next;
        *next += 1;
        id
    });
    emit_draw(state, "create_canvas", vec![
        Value::Int(id as i64), Value::Int(w as i64), Value::Int(h as i64),
    ]);
    state.push_int(id as i64);
    Ok(1)
}

/// Redirect subsequent draw commands into the offscreen canvas with the given
/// id. Pair with `draw_to_screen()` to return to the main framebuffer.
fn native_draw_to(state: &mut PetalCxt) -> NativeResult {
    let id = state.get_int(1)?;
    emit_draw(state, "set_target", vec![Value::Int(id)]);
    state.push_nil();
    Ok(1)
}

/// Redirect subsequent draw commands back to the main framebuffer.
fn native_draw_to_screen(state: &mut PetalCxt) -> NativeResult {
    emit_draw(state, "set_target", vec![Value::Int(0)]);
    state.push_nil();
    Ok(1)
}

/// Blit the offscreen canvas `id` onto the current render target at (`x`, `y`).
fn native_draw_canvas(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 3)?;
    emit_draw(state, "draw_canvas", args);
    state.push_nil();
    Ok(1)
}

// --- Input ---

fn native_key_down(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let down = INPUT_STATE.with(|s| s.borrow().key_down(&name));
    state.push_bool(down);
    Ok(1)
}

fn native_key_pressed(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let pressed = INPUT_STATE.with(|s| s.borrow().key_pressed(&name));
    state.push_bool(pressed);
    Ok(1)
}

fn native_mouse_x(state: &mut PetalCxt) -> NativeResult {
    let x = INPUT_STATE.with(|s| s.borrow().mouse_x);
    state.push_int(x as i64);
    Ok(1)
}

fn native_mouse_y(state: &mut PetalCxt) -> NativeResult {
    let y = INPUT_STATE.with(|s| s.borrow().mouse_y);
    state.push_int(y as i64);
    Ok(1)
}

fn native_mouse_down(state: &mut PetalCxt) -> NativeResult {
    let button = state.get_int(1)? as u8;
    let down = INPUT_STATE.with(|s| s.borrow().mouse_down(button));
    state.push_bool(down);
    Ok(1)
}

fn native_mouse_pressed(state: &mut PetalCxt) -> NativeResult {
    let button = state.get_int(1)? as u8;
    let pressed = INPUT_STATE.with(|s| s.borrow().mouse_pressed(button));
    state.push_bool(pressed);
    Ok(1)
}

// --- Timing ---

fn native_dt(state: &mut PetalCxt) -> NativeResult {
    let dt = FRAME_INFO.with(|f| f.borrow().dt);
    state.push_float(dt);
    Ok(1)
}

fn native_frame_count(state: &mut PetalCxt) -> NativeResult {
    let count = FRAME_INFO.with(|f| f.borrow().frame_count);
    state.push_int(count);
    Ok(1)
}

fn native_screen_width(state: &mut PetalCxt) -> NativeResult {
    let w = FRAME_INFO.with(|f| f.borrow().screen_width);
    state.push_int(w as i64);
    Ok(1)
}

fn native_screen_height(state: &mut PetalCxt) -> NativeResult {
    let h = FRAME_INFO.with(|f| f.borrow().screen_height);
    state.push_int(h as i64);
    Ok(1)
}

// --- Browser ---

fn native_example_count(state: &mut PetalCxt) -> NativeResult {
    let count = BROWSER_STATE.with(|b| b.borrow().examples.len() as i64);
    state.push_int(count);
    Ok(1)
}

fn native_example_name(state: &mut PetalCxt) -> NativeResult {
    let i = state.get_int(1)? as usize;
    let name = BROWSER_STATE.with(|b| {
        let bs = b.borrow();
        bs.examples.get(i).map(|e| e.name.clone()).unwrap_or_default()
    });
    state.push_string(name);
    Ok(1)
}

fn native_example_path(state: &mut PetalCxt) -> NativeResult {
    let i = state.get_int(1)? as usize;
    let path = BROWSER_STATE.with(|b| {
        let bs = b.borrow();
        bs.examples.get(i).map(|e| e.path.clone()).unwrap_or_default()
    });
    state.push_string(path);
    Ok(1)
}

fn native_launch_script(state: &mut PetalCxt) -> NativeResult {
    let path = state.get_string(1)?;
    BROWSER_STATE.with(|b| {
        b.borrow_mut().pending_launch = Some(path);
    });
    state.push_nil();
    Ok(1)
}

// --- File I/O ---
//
// Reads/writes are restricted to files under the working directory so Petal
// scripts can't escape out to arbitrary paths. Returns empty string on miss.

fn safe_path(path: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return None;
    }
    // Reject traversal. A single `..` component is enough to escape.
    for comp in p.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return None;
        }
    }
    Some(std::env::current_dir().ok()?.join(p))
}

fn native_load_text_file(state: &mut PetalCxt) -> NativeResult {
    let path = state.get_string(1)?;
    let text = match safe_path(&path) {
        Some(p) => std::fs::read_to_string(&p).unwrap_or_default(),
        None => String::new(),
    };
    state.push_string(text);
    Ok(1)
}

fn native_save_text_file(state: &mut PetalCxt) -> NativeResult {
    let path = state.get_string(1)?;
    let content = state.get_string(2)?;
    let ok = match safe_path(&path) {
        Some(p) => {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&p, content).is_ok()
        }
        None => false,
    };
    state.push_bool(ok);
    Ok(1)
}

fn native_file_exists(state: &mut PetalCxt) -> NativeResult {
    let path = state.get_string(1)?;
    let exists = match safe_path(&path) {
        Some(p) => p.exists(),
        None => false,
    };
    state.push_bool(exists);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::DrawCommand;
    use petal::env::Env;

    /// Drain the `draw_commands` output buffer and decode it into DrawCommands.
    fn drain_commands(env: &mut Env) -> Vec<DrawCommand> {
        let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
        let values = env.take_output_buffer(sym);
        values
            .iter()
            .map(|v| DrawCommand::from_value(v, env.heap()).expect("decode draw command"))
            .collect()
    }

    #[test]
    fn fill_triangle_emits_triangle_command() {
        let mut env = Env::new();
        register_all(&mut env);
        env.run_source("fill_triangle(0, 0, 10, 0, 5, 8, 255, 128, 64)")
            .expect("run_source should succeed");
        let cmds = drain_commands(&mut env);
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            DrawCommand::Triangle {
                x1: 0,
                y1: 0,
                x2: 10,
                y2: 0,
                x3: 5,
                y3: 8,
                r: 255,
                g: 128,
                b: 64
            }
        );
    }

    #[test]
    fn fill_poly_from_vec2_list() {
        let mut env = Env::new();
        register_all(&mut env);
        env.run_source(
            "fill_poly([vec2(0,0), vec2(10,0), vec2(10,10), vec2(0,10)], 10, 20, 30)",
        )
        .expect("run_source should succeed");
        let cmds = drain_commands(&mut env);
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            DrawCommand::Poly {
                points: vec![(0, 0), (10, 0), (10, 10), (0, 10)],
                r: 10,
                g: 20,
                b: 30
            }
        );
    }

    #[test]
    fn fill_poly_too_few_points_errors() {
        let mut env = Env::new();
        register_all(&mut env);
        let result = env.run_source("fill_poly([vec2(0,0), vec2(1,1)], 1,2,3)");
        assert!(result.is_err(), "expected Err for fewer than 3 points");
    }

    #[test]
    fn offscreen_canvas_emits_stream_commands() {
        reset_canvas_ids();
        let mut env = Env::new();
        register_all(&mut env);
        // create_canvas returns id 1; redirect to it, draw, then blit onto main.
        env.run_source(
            "let c = create_canvas(32, 32)\n\
             draw_to(c)\n\
             draw_rect(0, 0, 4, 4, 255, 255, 255)\n\
             draw_to_screen()\n\
             draw_canvas(c, 10, 10)",
        )
        .expect("run_source should succeed");

        let cmds = drain_commands(&mut env);
        assert_eq!(
            cmds,
            vec![
                DrawCommand::CreateCanvas { id: 1, w: 32, h: 32 },
                DrawCommand::SetTarget { id: 1 },
                DrawCommand::Rect { x: 0, y: 0, w: 4, h: 4, r: 255, g: 255, b: 255 },
                DrawCommand::SetTarget { id: 0 },
                DrawCommand::DrawCanvas { id: 1, x: 10, y: 10 },
            ]
        );
    }

    #[test]
    fn canvas_ids_are_stable_after_reset() {
        let mut env = Env::new();
        register_all(&mut env);

        reset_canvas_ids();
        let a = env.run_source("create_canvas(8, 8)").expect("run ok");
        let _ = drain_commands(&mut env);
        reset_canvas_ids();
        let b = env.run_source("create_canvas(8, 8)").expect("run ok");
        let _ = drain_commands(&mut env);

        // After a per-frame reset, the same call site yields the same id.
        assert_eq!(a, Value::Int(1));
        assert_eq!(b, Value::Int(1));
    }
}
