//! petal-fps's host-specific natives: the 3D/2D draw vocabulary and `log`.
//!
//! Everything else a sketch calls — input (`key_down`, `mouse_dx`,
//! `grab_mouse`, …), timing (`dt`, `frame_count`), and screen dimensions
//! (`screen_width`/`screen_height`) — comes from `petal_ui::input`, registered
//! alongside these in [`crate::host::FpsHost::register`]. This module owns only
//! what is particular to a software 3D rasterizer: the `triangle3d` family that
//! feeds `framebuffer.rs`.

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

/// Buffered-output channel carrying draw commands to the renderer. Shares the
/// standard `petal_ui` draw-buffer name so the loop's generic
/// `clear_draw_commands` drains it, but the *vocabulary* (see `commands.rs`) is
/// this app's own.
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

/// Register the 3D/2D draw natives and `log`. Input/timing/dimension natives
/// are registered separately from `petal_ui::input`.
pub fn register_draw(env: &mut Env) {
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

    // Logging
    env.register_native("log", native_log);
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

// --- Logging ---

fn native_log(state: &mut PetalCxt) -> NativeResult {
    let msg = state.get_string(1)?;
    eprintln!("[petal-log] {}", msg);
    state.push_nil();
    Ok(1)
}
