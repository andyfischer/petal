use std::cell::RefCell;

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalState};

use crate::commands::DrawCommand;
use crate::input::InputState;

#[derive(Default)]
pub struct FrameInfo {
    pub dt: f64,
    pub frame_count: i64,
    pub screen_width: i32,
    pub screen_height: i32,
}

thread_local! {
    pub static DRAW_COMMANDS: RefCell<Vec<DrawCommand>> = RefCell::new(Vec::new());
    pub static INPUT_STATE: RefCell<InputState> = RefCell::new(InputState::default());
    pub static FRAME_INFO: RefCell<FrameInfo> = RefCell::new(FrameInfo::default());
}

pub fn register_all(env: &mut Env) {
    env.register_native("clear", native_clear);
    env.register_native("draw_rect", native_draw_rect);
    env.register_native("draw_rect_outline", native_draw_rect_outline);
    env.register_native("draw_line", native_draw_line);
    env.register_native("draw_circle", native_draw_circle);
    env.register_native("draw_text", native_draw_text);
    env.register_native("key_down", native_key_down);
    env.register_native("key_pressed", native_key_pressed);
    env.register_native("mouse_x", native_mouse_x);
    env.register_native("mouse_y", native_mouse_y);
    env.register_native("mouse_down", native_mouse_down);
    env.register_native("dt", native_dt);
    env.register_native("frame_count", native_frame_count);
    env.register_native("screen_width", native_screen_width);
    env.register_native("screen_height", native_screen_height);
}

// --- Drawing ---

fn native_clear(state: &mut PetalState) -> NativeResult {
    let r = state.get_int(1)? as u8;
    let g = state.get_int(2)? as u8;
    let b = state.get_int(3)? as u8;
    DRAW_COMMANDS.with(|cmds| {
        cmds.borrow_mut().push(DrawCommand::Clear { r, g, b });
    });
    state.push_nil();
    Ok(1)
}

fn native_draw_rect(state: &mut PetalState) -> NativeResult {
    let x = state.get_int(1)? as i32;
    let y = state.get_int(2)? as i32;
    let w = state.get_int(3)? as u32;
    let h = state.get_int(4)? as u32;
    let r = state.get_int(5)? as u8;
    let g = state.get_int(6)? as u8;
    let b = state.get_int(7)? as u8;
    DRAW_COMMANDS.with(|cmds| {
        cmds.borrow_mut().push(DrawCommand::Rect { x, y, w, h, r, g, b });
    });
    state.push_nil();
    Ok(1)
}

fn native_draw_rect_outline(state: &mut PetalState) -> NativeResult {
    let x = state.get_int(1)? as i32;
    let y = state.get_int(2)? as i32;
    let w = state.get_int(3)? as u32;
    let h = state.get_int(4)? as u32;
    let r = state.get_int(5)? as u8;
    let g = state.get_int(6)? as u8;
    let b = state.get_int(7)? as u8;
    DRAW_COMMANDS.with(|cmds| {
        cmds.borrow_mut().push(DrawCommand::RectOutline { x, y, w, h, r, g, b });
    });
    state.push_nil();
    Ok(1)
}

fn native_draw_line(state: &mut PetalState) -> NativeResult {
    let x1 = state.get_int(1)? as i32;
    let y1 = state.get_int(2)? as i32;
    let x2 = state.get_int(3)? as i32;
    let y2 = state.get_int(4)? as i32;
    let r = state.get_int(5)? as u8;
    let g = state.get_int(6)? as u8;
    let b = state.get_int(7)? as u8;
    DRAW_COMMANDS.with(|cmds| {
        cmds.borrow_mut().push(DrawCommand::Line { x1, y1, x2, y2, r, g, b });
    });
    state.push_nil();
    Ok(1)
}

fn native_draw_circle(state: &mut PetalState) -> NativeResult {
    let cx = state.get_int(1)? as i32;
    let cy = state.get_int(2)? as i32;
    let radius = state.get_int(3)? as i32;
    let r = state.get_int(4)? as u8;
    let g = state.get_int(5)? as u8;
    let b = state.get_int(6)? as u8;
    DRAW_COMMANDS.with(|cmds| {
        cmds.borrow_mut().push(DrawCommand::Circle { cx, cy, radius, r, g, b });
    });
    state.push_nil();
    Ok(1)
}

fn native_draw_text(state: &mut PetalState) -> NativeResult {
    let text = state.get_string(1)?;
    let x = state.get_int(2)? as i32;
    let y = state.get_int(3)? as i32;
    let size = state.get_int(4)? as u16;
    let r = state.get_int(5)? as u8;
    let g = state.get_int(6)? as u8;
    let b = state.get_int(7)? as u8;
    DRAW_COMMANDS.with(|cmds| {
        cmds.borrow_mut().push(DrawCommand::Text { text, x, y, size, r, g, b });
    });
    state.push_nil();
    Ok(1)
}

// --- Input ---

fn native_key_down(state: &mut PetalState) -> NativeResult {
    let name = state.get_string(1)?;
    let down = INPUT_STATE.with(|s| s.borrow().key_down(&name));
    state.push_bool(down);
    Ok(1)
}

fn native_key_pressed(state: &mut PetalState) -> NativeResult {
    let name = state.get_string(1)?;
    let pressed = INPUT_STATE.with(|s| s.borrow().key_pressed(&name));
    state.push_bool(pressed);
    Ok(1)
}

fn native_mouse_x(state: &mut PetalState) -> NativeResult {
    let x = INPUT_STATE.with(|s| s.borrow().mouse_x);
    state.push_int(x as i64);
    Ok(1)
}

fn native_mouse_y(state: &mut PetalState) -> NativeResult {
    let y = INPUT_STATE.with(|s| s.borrow().mouse_y);
    state.push_int(y as i64);
    Ok(1)
}

fn native_mouse_down(state: &mut PetalState) -> NativeResult {
    let button = state.get_int(1)? as u8;
    let down = INPUT_STATE.with(|s| s.borrow().mouse_down(button));
    state.push_bool(down);
    Ok(1)
}

// --- Timing ---

fn native_dt(state: &mut PetalState) -> NativeResult {
    let dt = FRAME_INFO.with(|f| f.borrow().dt);
    state.push_float(dt);
    Ok(1)
}

fn native_frame_count(state: &mut PetalState) -> NativeResult {
    let count = FRAME_INFO.with(|f| f.borrow().frame_count);
    state.push_int(count);
    Ok(1)
}

fn native_screen_width(state: &mut PetalState) -> NativeResult {
    let w = FRAME_INFO.with(|f| f.borrow().screen_width);
    state.push_int(w as i64);
    Ok(1)
}

fn native_screen_height(state: &mut PetalState) -> NativeResult {
    let h = FRAME_INFO.with(|f| f.borrow().screen_height);
    state.push_int(h as i64);
    Ok(1)
}
