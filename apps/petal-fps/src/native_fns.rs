use std::cell::RefCell;

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

use crate::input::InputState;

/// Name of the buffered-output channel that carries draw commands from the
/// sketch to the renderer. The host interns the same name to drain it.
pub const DRAW_COMMANDS_SYMBOL: &str = "draw_commands";

/// Emit a draw command into the `draw_commands` output buffer on the Env.
fn emit_draw(state: &mut PetalCxt, tag: &str, data: Vec<Value>) {
    let sym = state.intern_symbol(DRAW_COMMANDS_SYMBOL);
    state.emit(sym, tag, data);
}

/// Collect the first `n` arguments (1-indexed) as integer `Value`s — the common
/// shape for draw commands whose arguments are all numbers.
fn int_args(state: &PetalCxt, n: usize) -> Result<Vec<Value>, String> {
    (1..=n).map(|i| state.get_int(i).map(Value::Int)).collect()
}

#[derive(Default)]
pub struct FrameInfo {
    pub dt: f64,
    pub frame_count: i64,
    pub screen_width: i32,
    pub screen_height: i32,
}

thread_local! {
    pub static INPUT_STATE: RefCell<InputState> = RefCell::new(InputState::default());
    pub static FRAME_INFO: RefCell<FrameInfo> = RefCell::new(FrameInfo::default());
}

pub fn register_all(env: &mut Env) {
    // Frame / info
    env.register_native("dt", native_dt);
    env.register_native("frame_count", native_frame_count);
    env.register_native("screen_width", native_screen_width);
    env.register_native("screen_height", native_screen_height);

    // 3D drawing
    env.register_native("clear3d", native_clear3d);
    env.register_native("sky_gradient", native_sky_gradient);
    env.register_native("triangle3d", native_triangle3d);
    env.register_native("triangle3d_shaded", native_triangle3d_shaded);
    env.register_native("line3d", native_line3d);

    // 2D (HUD) drawing
    env.register_native("rect2d", native_rect2d);
    env.register_native("line2d", native_line2d);
    env.register_native("circle2d", native_circle2d);
    env.register_native("text2d", native_text2d);

    // Input
    env.register_native("key_down", native_key_down);
    env.register_native("key_pressed", native_key_pressed);
    env.register_native("mouse_x", native_mouse_x);
    env.register_native("mouse_y", native_mouse_y);
    env.register_native("mouse_dx", native_mouse_dx);
    env.register_native("mouse_dy", native_mouse_dy);
    env.register_native("mouse_down", native_mouse_down);
    env.register_native("mouse_pressed", native_mouse_pressed);
    env.register_native("grab_mouse", native_grab_mouse);
    env.register_native("release_mouse", native_release_mouse);

    // Logging
    env.register_native("log", native_log);
}

// --- Frame / info ---

fn native_dt(state: &mut PetalCxt) -> NativeResult {
    state.push_float(FRAME_INFO.with(|f| f.borrow().dt));
    Ok(1)
}
fn native_frame_count(state: &mut PetalCxt) -> NativeResult {
    state.push_int(FRAME_INFO.with(|f| f.borrow().frame_count));
    Ok(1)
}
fn native_screen_width(state: &mut PetalCxt) -> NativeResult {
    state.push_int(FRAME_INFO.with(|f| f.borrow().screen_width) as i64);
    Ok(1)
}
fn native_screen_height(state: &mut PetalCxt) -> NativeResult {
    state.push_int(FRAME_INFO.with(|f| f.borrow().screen_height) as i64);
    Ok(1)
}

// --- 3D drawing ---

fn native_clear3d(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 3)?;
    emit_draw(state, "clear3d", args);
    state.push_nil();
    Ok(1)
}

fn native_sky_gradient(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 6)?;
    emit_draw(state, "sky_gradient", args);
    state.push_nil();
    Ok(1)
}

fn native_triangle3d(state: &mut PetalCxt) -> NativeResult {
    let args = vec![
        Value::Float(state.get_float(1)?), Value::Float(state.get_float(2)?), Value::Float(state.get_float(3)?),
        Value::Float(state.get_float(4)?), Value::Float(state.get_float(5)?), Value::Float(state.get_float(6)?),
        Value::Float(state.get_float(7)?), Value::Float(state.get_float(8)?), Value::Float(state.get_float(9)?),
        Value::Int(state.get_int(10)?), Value::Int(state.get_int(11)?), Value::Int(state.get_int(12)?),
    ];
    emit_draw(state, "triangle3d", args);
    state.push_nil();
    Ok(1)
}

fn native_triangle3d_shaded(state: &mut PetalCxt) -> NativeResult {
    let args = vec![
        Value::Float(state.get_float(1)?), Value::Float(state.get_float(2)?), Value::Float(state.get_float(3)?),
        Value::Int(state.get_int(4)?), Value::Int(state.get_int(5)?), Value::Int(state.get_int(6)?),
        Value::Float(state.get_float(7)?), Value::Float(state.get_float(8)?), Value::Float(state.get_float(9)?),
        Value::Int(state.get_int(10)?), Value::Int(state.get_int(11)?), Value::Int(state.get_int(12)?),
        Value::Float(state.get_float(13)?), Value::Float(state.get_float(14)?), Value::Float(state.get_float(15)?),
        Value::Int(state.get_int(16)?), Value::Int(state.get_int(17)?), Value::Int(state.get_int(18)?),
    ];
    emit_draw(state, "triangle3d_shaded", args);
    state.push_nil();
    Ok(1)
}

fn native_line3d(state: &mut PetalCxt) -> NativeResult {
    let args = vec![
        Value::Float(state.get_float(1)?), Value::Float(state.get_float(2)?), Value::Float(state.get_float(3)?),
        Value::Float(state.get_float(4)?), Value::Float(state.get_float(5)?), Value::Float(state.get_float(6)?),
        Value::Int(state.get_int(7)?), Value::Int(state.get_int(8)?), Value::Int(state.get_int(9)?),
    ];
    emit_draw(state, "line3d", args);
    state.push_nil();
    Ok(1)
}

// --- 2D HUD drawing ---

fn native_rect2d(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 7)?;
    emit_draw(state, "rect2d", args);
    state.push_nil();
    Ok(1)
}

fn native_line2d(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 7)?;
    emit_draw(state, "line2d", args);
    state.push_nil();
    Ok(1)
}

fn native_circle2d(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 6)?;
    emit_draw(state, "circle2d", args);
    state.push_nil();
    Ok(1)
}

fn native_text2d(state: &mut PetalCxt) -> NativeResult {
    let text = state.get_string(1)?;
    let args = vec![
        Value::String(state.heap_mut().alloc_string(text)),
        Value::Int(state.get_int(2)?), Value::Int(state.get_int(3)?), Value::Int(state.get_int(4)?),
        Value::Int(state.get_int(5)?), Value::Int(state.get_int(6)?), Value::Int(state.get_int(7)?),
    ];
    emit_draw(state, "text2d", args);
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
    let p = INPUT_STATE.with(|s| s.borrow().key_pressed(&name));
    state.push_bool(p);
    Ok(1)
}

fn native_mouse_x(state: &mut PetalCxt) -> NativeResult {
    let v = INPUT_STATE.with(|s| s.borrow().mouse_x);
    state.push_int(v as i64);
    Ok(1)
}
fn native_mouse_y(state: &mut PetalCxt) -> NativeResult {
    let v = INPUT_STATE.with(|s| s.borrow().mouse_y);
    state.push_int(v as i64);
    Ok(1)
}
fn native_mouse_dx(state: &mut PetalCxt) -> NativeResult {
    let v = INPUT_STATE.with(|s| s.borrow().mouse_dx);
    state.push_int(v as i64);
    Ok(1)
}
fn native_mouse_dy(state: &mut PetalCxt) -> NativeResult {
    let v = INPUT_STATE.with(|s| s.borrow().mouse_dy);
    state.push_int(v as i64);
    Ok(1)
}
fn native_mouse_down(state: &mut PetalCxt) -> NativeResult {
    let b = state.get_int(1)? as u8;
    let down = INPUT_STATE.with(|s| s.borrow().mouse_down(b));
    state.push_bool(down);
    Ok(1)
}
fn native_mouse_pressed(state: &mut PetalCxt) -> NativeResult {
    let b = state.get_int(1)? as u8;
    let p = INPUT_STATE.with(|s| s.borrow().mouse_pressed(b));
    state.push_bool(p);
    Ok(1)
}

fn native_grab_mouse(state: &mut PetalCxt) -> NativeResult {
    INPUT_STATE.with(|s| s.borrow_mut().want_mouse_grab = true);
    state.push_nil();
    Ok(1)
}
fn native_release_mouse(state: &mut PetalCxt) -> NativeResult {
    INPUT_STATE.with(|s| s.borrow_mut().want_mouse_grab = false);
    state.push_nil();
    Ok(1)
}

// --- Logging ---

fn native_log(state: &mut PetalCxt) -> NativeResult {
    let msg = state.get_string(1)?;
    eprintln!("[petal-log] {}", msg);
    state.push_nil();
    Ok(1)
}
