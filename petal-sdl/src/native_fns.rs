use std::cell::RefCell;

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};

use crate::commands::DrawCommand;
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

thread_local! {
    pub static DRAW_COMMANDS: RefCell<Vec<DrawCommand>> = RefCell::new(Vec::new());
    pub static INPUT_STATE: RefCell<InputState> = RefCell::new(InputState::default());
    pub static FRAME_INFO: RefCell<FrameInfo> = RefCell::new(FrameInfo::default());
    pub static BROWSER_STATE: RefCell<BrowserState> = RefCell::new(BrowserState::default());
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
    let r = state.get_int(1)? as u8;
    let g = state.get_int(2)? as u8;
    let b = state.get_int(3)? as u8;
    DRAW_COMMANDS.with(|cmds| {
        cmds.borrow_mut().push(DrawCommand::Clear { r, g, b });
    });
    state.push_nil();
    Ok(1)
}

fn native_draw_rect(state: &mut PetalCxt) -> NativeResult {
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

fn native_draw_rect_outline(state: &mut PetalCxt) -> NativeResult {
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

fn native_draw_line(state: &mut PetalCxt) -> NativeResult {
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

fn native_draw_circle(state: &mut PetalCxt) -> NativeResult {
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

fn native_draw_text(state: &mut PetalCxt) -> NativeResult {
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
