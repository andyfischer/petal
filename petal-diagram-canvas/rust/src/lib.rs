//! WASM runtime for petal-diagram-canvas.
//!
//! Wraps the core Petal `Env` with graphics-specific native functions
//! (draw commands, input, timing) for canvas-based frame-loop execution.

use std::cell::RefCell;
use std::collections::HashSet;

use serde::Serialize;
use wasm_bindgen::prelude::*;

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value::value_to_json;

// ---------------------------------------------------------------------------
// Thread-local state
// ---------------------------------------------------------------------------

thread_local! {
    static DRAW_COMMANDS: RefCell<Vec<DrawCommand>> = RefCell::new(Vec::new());
    static INPUT_STATE: RefCell<InputState> = RefCell::new(InputState::default());
    static FRAME_INFO: RefCell<FrameInfo> = RefCell::new(FrameInfo::default());
}

// ---------------------------------------------------------------------------
// Draw commands
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum DrawCommand {
    Clear { r: u8, g: u8, b: u8 },
    Rect { x: i32, y: i32, w: u32, h: u32, r: u8, g: u8, b: u8 },
    RectOutline { x: i32, y: i32, w: u32, h: u32, r: u8, g: u8, b: u8 },
    Line { x1: i32, y1: i32, x2: i32, y2: i32, r: u8, g: u8, b: u8 },
    Circle { cx: i32, cy: i32, radius: i32, r: u8, g: u8, b: u8 },
    Text { text: String, x: i32, y: i32, size: u16, r: u8, g: u8, b: u8 },
}

// ---------------------------------------------------------------------------
// Input state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct InputState {
    keys_down: HashSet<String>,
    keys_prev: HashSet<String>,
    mouse_x: i32,
    mouse_y: i32,
    mouse_buttons: HashSet<u8>,
    mouse_buttons_prev: HashSet<u8>,
}

impl InputState {
    fn key_down(&self, name: &str) -> bool {
        self.keys_down.contains(name)
    }
    fn key_pressed(&self, name: &str) -> bool {
        self.keys_down.contains(name) && !self.keys_prev.contains(name)
    }
    fn mouse_down(&self, button: u8) -> bool {
        self.mouse_buttons.contains(&button)
    }
    fn mouse_pressed(&self, button: u8) -> bool {
        self.mouse_buttons.contains(&button) && !self.mouse_buttons_prev.contains(&button)
    }
    fn begin_frame(&mut self) {
        self.keys_prev = self.keys_down.clone();
        self.mouse_buttons_prev = self.mouse_buttons.clone();
    }
}

// ---------------------------------------------------------------------------
// Frame info
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FrameInfo {
    dt: f64,
    frame_count: i64,
    screen_width: i32,
    screen_height: i32,
}

// ---------------------------------------------------------------------------
// Native functions — drawing
// ---------------------------------------------------------------------------

fn native_clear(state: &mut PetalCxt) -> NativeResult {
    let r = state.get_int(1)? as u8;
    let g = state.get_int(2)? as u8;
    let b = state.get_int(3)? as u8;
    DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().push(DrawCommand::Clear { r, g, b }));
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
    DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().push(DrawCommand::Rect { x, y, w, h, r, g, b }));
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
    DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().push(DrawCommand::RectOutline { x, y, w, h, r, g, b }));
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
    DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().push(DrawCommand::Line { x1, y1, x2, y2, r, g, b }));
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
    DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().push(DrawCommand::Circle { cx, cy, radius, r, g, b }));
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
    DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().push(DrawCommand::Text { text, x, y, size, r, g, b }));
    state.push_nil();
    Ok(1)
}

// ---------------------------------------------------------------------------
// Native functions — input
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Native functions — timing
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

fn register_graphics(env: &mut Env) {
    env.register_native("clear", native_clear);
    env.register_native("draw_rect", native_draw_rect);
    env.register_native("draw_rect_outline", native_draw_rect_outline);
    env.register_native("draw_line", native_draw_line);
    env.register_native("draw_circle", native_draw_circle);
    env.register_native("draw_text", native_draw_text);

    env.register_native("mouse_x", native_mouse_x);
    env.register_native("mouse_y", native_mouse_y);
    env.register_native("mouse_down", native_mouse_down);
    env.register_native("mouse_pressed", native_mouse_pressed);
    env.register_native("key_down", native_key_down);
    env.register_native("key_pressed", native_key_pressed);

    env.register_native("dt", native_dt);
    env.register_native("frame_count", native_frame_count);
    env.register_native("screen_width", native_screen_width);
    env.register_native("screen_height", native_screen_height);
}

fn take_draw_commands() -> String {
    let cmds: Vec<DrawCommand> = DRAW_COMMANDS.with(|c| c.borrow_mut().drain(..).collect());
    serde_json::to_string(&cmds).unwrap_or_else(|_| "[]".to_string())
}

// ---------------------------------------------------------------------------
// PetalRuntime — WASM-exported struct
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct PetalRuntime {
    env: Env,
    active_program: Option<ProgramId>,
    active_stack: Option<StackKey>,
}

#[wasm_bindgen]
impl PetalRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new() -> PetalRuntime {
        let mut env = Env::new();
        register_graphics(&mut env);
        PetalRuntime {
            env,
            active_program: None,
            active_stack: None,
        }
    }

    pub fn load_program(&mut self, source: &str) -> Result<u32, JsValue> {
        let pid = self.env.load_program(source).map_err(|e| JsValue::from_str(&e))?;
        self.active_program = Some(pid);
        Ok(pid.0)
    }

    pub fn create_stack(&mut self, program_id: u32) -> Result<u32, JsValue> {
        let sid = self
            .env
            .create_stack(ProgramId(program_id))
            .map_err(|e| JsValue::from_str(&e))?;
        self.active_stack = Some(sid);
        Ok(sid.0)
    }

    pub fn run(&mut self, stack_id: u32) -> Result<String, JsValue> {
        let val = self
            .env
            .run(StackKey(stack_id))
            .map_err(|e| JsValue::from_str(&e))?;
        let json = value_to_json(&val, self.env.heap());
        Ok(json.to_string())
    }

    pub fn reset_and_run(&mut self, stack_id: u32) -> Result<String, JsValue> {
        self.env
            .reset_stack(StackKey(stack_id))
            .map_err(|e| JsValue::from_str(&e))?;
        self.run(stack_id)
    }

    pub fn take_output(&mut self) -> String {
        let output = self.env.take_output();
        serde_json::to_string(&output).unwrap_or_else(|_| "[]".to_string())
    }

    // --- Graphics ---

    pub fn take_draw_commands(&self) -> String {
        take_draw_commands()
    }

    pub fn set_mouse_position(&self, x: i32, y: i32) {
        INPUT_STATE.with(|s| {
            let mut st = s.borrow_mut();
            st.mouse_x = x;
            st.mouse_y = y;
        });
    }

    pub fn set_mouse_button(&self, button: i32, down: bool) {
        INPUT_STATE.with(|s| {
            let mut st = s.borrow_mut();
            if down {
                st.mouse_buttons.insert(button as u8);
            } else {
                st.mouse_buttons.remove(&(button as u8));
            }
        });
    }

    pub fn set_key_state(&self, key: &str, down: bool) {
        INPUT_STATE.with(|s| {
            let mut st = s.borrow_mut();
            if down {
                st.keys_down.insert(key.to_string());
            } else {
                st.keys_down.remove(key);
            }
        });
    }

    pub fn set_frame_info(&self, dt: f64, frame_count: i32, width: i32, height: i32) {
        FRAME_INFO.with(|f| {
            let mut fi = f.borrow_mut();
            fi.dt = dt;
            fi.frame_count = frame_count as i64;
            fi.screen_width = width;
            fi.screen_height = height;
        });
    }

    pub fn begin_frame(&self) {
        INPUT_STATE.with(|s| s.borrow_mut().begin_frame());
    }

    // --- Debug ---

    pub fn get_state_json(&self) -> Result<String, JsValue> {
        let pid = self.active_program.ok_or_else(|| JsValue::from_str("No active program"))?;
        let sid = self.active_stack.ok_or_else(|| JsValue::from_str("No active stack"))?;
        let map = self.env.get_state_json(pid, sid);
        Ok(serde_json::Value::Object(map).to_string())
    }

    pub fn set_state_json(&mut self, name: &str, json_value: &str) -> Result<(), JsValue> {
        let pid = self.active_program.ok_or_else(|| JsValue::from_str("No active program"))?;
        let sid = self.active_stack.ok_or_else(|| JsValue::from_str("No active stack"))?;
        let val: serde_json::Value =
            serde_json::from_str(json_value).map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.env
            .set_state_from_json(pid, sid, name, &val)
            .map_err(|e| JsValue::from_str(&e))
    }

    pub fn run_speculative(&mut self) -> Result<String, JsValue> {
        let sid = self.active_stack.ok_or_else(|| JsValue::from_str("No active stack"))?;
        self.env
            .run_speculative(sid)
            .map_err(|e| JsValue::from_str(&e))?;
        Ok(take_draw_commands())
    }
}
