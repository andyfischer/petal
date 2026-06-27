use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

use crate::input::InputState;

// ── Env channel names (host↔script) ──────────────────────────────────────
// Host→script input/timing state is bound as uniforms; script→host draw
// commands and mouse-grab requests use output buffers. Replaces thread-locals.

/// Buffered-output channel carrying draw commands to the renderer.
pub const DRAW_COMMANDS_SYMBOL: &str = "draw_commands";
const SYM_DT: &str = "dt";
const SYM_FRAME_COUNT: &str = "frame_count";
const SYM_SCREEN_WIDTH: &str = "screen_width";
const SYM_SCREEN_HEIGHT: &str = "screen_height";
const SYM_MOUSE_X: &str = "mouse_x";
const SYM_MOUSE_Y: &str = "mouse_y";
const SYM_MOUSE_DX: &str = "mouse_dx";
const SYM_MOUSE_DY: &str = "mouse_dy";
const SYM_KEYS_DOWN: &str = "keys_down";
const SYM_KEYS_PRESSED: &str = "keys_pressed";
const SYM_BUTTONS_DOWN: &str = "mouse_buttons_down";
const SYM_BUTTONS_PRESSED: &str = "mouse_buttons_pressed";
/// Output channel: mouse grab/release requests from the sketch.
const MOUSE_GRAB_SIGNAL: &str = "mouse_grab";

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

// ── Host-side helpers: bind state into the Env before a run ───────────────

/// Bind the per-frame dt + frame_count uniforms.
pub fn bind_frame_info(env: &mut Env, dt: f64, frame_count: i64) {
    let s = env.intern_symbol(SYM_DT);
    env.set_binding(s, Value::Float(dt));
    let s = env.intern_symbol(SYM_FRAME_COUNT);
    env.set_binding(s, Value::Int(frame_count));
}

/// Bind the screen dimensions (set once; persists across runs).
pub fn bind_dimensions(env: &mut Env, width: i32, height: i32) {
    let s = env.intern_symbol(SYM_SCREEN_WIDTH);
    env.set_binding(s, Value::Int(width as i64));
    let s = env.intern_symbol(SYM_SCREEN_HEIGHT);
    env.set_binding(s, Value::Int(height as i64));
}

/// Bind the current input snapshot (down/pressed sets, mouse position + delta).
pub fn bind_input(env: &mut Env, input: &InputState) {
    bind_str_list(env, SYM_KEYS_DOWN, input.keys_down.iter());
    bind_str_list(env, SYM_KEYS_PRESSED, input.keys_pressed.iter());
    bind_int_list(env, SYM_BUTTONS_DOWN, input.mouse_buttons.iter().map(|b| *b as i64));
    bind_int_list(env, SYM_BUTTONS_PRESSED, input.mouse_buttons_pressed.iter().map(|b| *b as i64));
    let s = env.intern_symbol(SYM_MOUSE_X);
    env.set_binding(s, Value::Int(input.mouse_x as i64));
    let s = env.intern_symbol(SYM_MOUSE_Y);
    env.set_binding(s, Value::Int(input.mouse_y as i64));
    let s = env.intern_symbol(SYM_MOUSE_DX);
    env.set_binding(s, Value::Int(input.mouse_dx as i64));
    let s = env.intern_symbol(SYM_MOUSE_DY);
    env.set_binding(s, Value::Int(input.mouse_dy as i64));
}

/// Drain the `mouse_grab` output buffer, returning the last requested state.
pub fn take_mouse_grab(env: &mut Env) -> Option<bool> {
    let s = env.intern_symbol(MOUSE_GRAB_SIGNAL);
    let vals = env.take_output_buffer(s);
    vals.into_iter().rev().find_map(|v| match v {
        Value::Bool(b) => Some(b),
        _ => None,
    })
}

fn bind_str_list<'a>(env: &mut Env, name: &str, items: impl Iterator<Item = &'a String>) {
    let vals: Vec<Value> = items
        .map(|k| Value::String(env.heap_mut().alloc_string(k.clone())))
        .collect();
    let list = Value::List(env.heap_mut().alloc_list(vals));
    let s = env.intern_symbol(name);
    env.set_binding(s, list);
}

fn bind_int_list(env: &mut Env, name: &str, items: impl Iterator<Item = i64>) {
    let vals: Vec<Value> = items.map(Value::Int).collect();
    let list = Value::List(env.heap_mut().alloc_list(vals));
    let s = env.intern_symbol(name);
    env.set_binding(s, list);
}

// ── Native-side helpers: read bound uniforms ─────────────────────────────

fn binding_float(state: &mut PetalCxt, name: &str) -> f64 {
    match state.binding_named(name) {
        Value::Float(f) => f,
        Value::Int(n) => n as f64,
        _ => 0.0,
    }
}

fn binding_int(state: &mut PetalCxt, name: &str) -> i64 {
    match state.binding_named(name) {
        Value::Int(n) => n,
        Value::Float(f) => f as i64,
        _ => 0,
    }
}

fn binding_has_str(state: &mut PetalCxt, name: &str, needle: &str) -> bool {
    let list_id = match state.binding_named(name) {
        Value::List(id) => id,
        _ => return false,
    };
    let items = state.heap().get_list(list_id).to_vec();
    items
        .iter()
        .any(|v| matches!(v, Value::String(s) if state.heap().get_string(*s) == needle))
}

fn binding_has_int(state: &mut PetalCxt, name: &str, needle: i64) -> bool {
    let list_id = match state.binding_named(name) {
        Value::List(id) => id,
        _ => return false,
    };
    state
        .heap()
        .get_list(list_id)
        .iter()
        .any(|v| matches!(v, Value::Int(n) if *n == needle))
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
    let v = binding_float(state, SYM_DT);
    state.push_float(v);
    Ok(1)
}
fn native_frame_count(state: &mut PetalCxt) -> NativeResult {
    let v = binding_int(state, SYM_FRAME_COUNT);
    state.push_int(v);
    Ok(1)
}
fn native_screen_width(state: &mut PetalCxt) -> NativeResult {
    let v = binding_int(state, SYM_SCREEN_WIDTH);
    state.push_int(v);
    Ok(1)
}
fn native_screen_height(state: &mut PetalCxt) -> NativeResult {
    let v = binding_int(state, SYM_SCREEN_HEIGHT);
    state.push_int(v);
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
    let down = binding_has_str(state, SYM_KEYS_DOWN, &name);
    state.push_bool(down);
    Ok(1)
}

fn native_key_pressed(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let p = binding_has_str(state, SYM_KEYS_PRESSED, &name);
    state.push_bool(p);
    Ok(1)
}

fn native_mouse_x(state: &mut PetalCxt) -> NativeResult {
    let v = binding_int(state, SYM_MOUSE_X);
    state.push_int(v);
    Ok(1)
}
fn native_mouse_y(state: &mut PetalCxt) -> NativeResult {
    let v = binding_int(state, SYM_MOUSE_Y);
    state.push_int(v);
    Ok(1)
}
fn native_mouse_dx(state: &mut PetalCxt) -> NativeResult {
    let v = binding_int(state, SYM_MOUSE_DX);
    state.push_int(v);
    Ok(1)
}
fn native_mouse_dy(state: &mut PetalCxt) -> NativeResult {
    let v = binding_int(state, SYM_MOUSE_DY);
    state.push_int(v);
    Ok(1)
}
fn native_mouse_down(state: &mut PetalCxt) -> NativeResult {
    let b = state.get_int(1)?;
    let down = binding_has_int(state, SYM_BUTTONS_DOWN, b);
    state.push_bool(down);
    Ok(1)
}
fn native_mouse_pressed(state: &mut PetalCxt) -> NativeResult {
    let b = state.get_int(1)?;
    let p = binding_has_int(state, SYM_BUTTONS_PRESSED, b);
    state.push_bool(p);
    Ok(1)
}

fn native_grab_mouse(state: &mut PetalCxt) -> NativeResult {
    let sym = state.intern_symbol(MOUSE_GRAB_SIGNAL);
    state.push_output(sym, Value::Bool(true));
    state.push_nil();
    Ok(1)
}
fn native_release_mouse(state: &mut PetalCxt) -> NativeResult {
    let sym = state.intern_symbol(MOUSE_GRAB_SIGNAL);
    state.push_output(sym, Value::Bool(false));
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
