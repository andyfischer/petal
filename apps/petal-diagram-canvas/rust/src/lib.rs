//! WASM runtime for petal-diagram-canvas.
//!
//! Wraps the core Petal `Env` with graphics-specific native functions
//! (draw commands, input, timing) for canvas-based frame-loop execution.

use std::collections::HashSet;

use wasm_bindgen::prelude::*;

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value::value_to_json;
use petal::value::Value;

// ---------------------------------------------------------------------------
// Env channel names (host↔script). Host→script input/timing state is bound as
// uniforms (`set_binding`); script→host draw commands use an output buffer;
// per-frame ids use a counter. These replace the old thread-locals.
// ---------------------------------------------------------------------------

const SYM_DT: &str = "dt";
const SYM_FRAME_COUNT: &str = "frame_count";
const SYM_SCREEN_WIDTH: &str = "screen_width";
const SYM_SCREEN_HEIGHT: &str = "screen_height";
const SYM_MOUSE_X: &str = "mouse_x";
const SYM_MOUSE_Y: &str = "mouse_y";
const SYM_KEYS_DOWN: &str = "keys_down";
const SYM_KEYS_PRESSED: &str = "keys_pressed";
const SYM_BUTTONS_DOWN: &str = "mouse_buttons_down";
const SYM_BUTTONS_PRESSED: &str = "mouse_buttons_pressed";
/// Per-frame offscreen-canvas id counter (ids are 1-based; 0 is the main
/// framebuffer). Reset to 1 before each run so ids are stable per frame.
const CANVAS_ID_COUNTER: &str = "canvas_id";

// ---------------------------------------------------------------------------
// Draw commands
// ---------------------------------------------------------------------------
//
// Draw commands are emitted into the `draw_commands` buffered-output channel on
// the Env as `Value::EnumVariant { tag, data }` — a string opcode plus a flat
// argument list — and pulled out as JSON for the canvas renderer (each command
// serializes to `{ "type": "enum", "tag": "rect", "data": [...] }`).

/// Name of the buffered-output channel carrying draw commands to the renderer.
const DRAW_COMMANDS_SYMBOL: &str = "draw_commands";

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

// ---------------------------------------------------------------------------
// Input state
// ---------------------------------------------------------------------------

/// Host-owned input bookkeeping (the host's copy; the script reads a snapshot
/// of it via Env bindings, not this struct directly). `keys_prev` /
/// `mouse_buttons_prev` retain last frame's state for pressed-edge detection.
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
    /// Snapshot the current state into prev (call at frame start, before the
    /// host applies this frame's input events) so `pressed` = down && !prev.
    fn begin_frame(&mut self) {
        self.keys_prev = self.keys_down.clone();
        self.mouse_buttons_prev = self.mouse_buttons.clone();
    }

    /// Bind the current input snapshot into the Env as uniforms the script
    /// reads via the input native fns. Down/pressed sets are bound as lists.
    fn bind_into(&self, env: &mut Env) {
        let bind_str_set = |env: &mut Env, name: &str, items: &[String]| {
            let vals: Vec<Value> = items
                .iter()
                .map(|k| Value::String(env.heap_mut().alloc_string(k.clone())))
                .collect();
            let list = Value::List(env.heap_mut().alloc_list(vals));
            let sym = env.intern_symbol(name);
            env.set_binding(sym, list);
        };
        let bind_int_set = |env: &mut Env, name: &str, items: &[i64]| {
            let vals: Vec<Value> = items.iter().map(|n| Value::Int(*n)).collect();
            let list = Value::List(env.heap_mut().alloc_list(vals));
            let sym = env.intern_symbol(name);
            env.set_binding(sym, list);
        };
        let bind_int = |env: &mut Env, name: &str, n: i64| {
            let sym = env.intern_symbol(name);
            env.set_binding(sym, Value::Int(n));
        };

        let down: Vec<String> = self.keys_down.iter().cloned().collect();
        bind_str_set(env, SYM_KEYS_DOWN, &down);
        let pressed: Vec<String> = self.keys_down.difference(&self.keys_prev).cloned().collect();
        bind_str_set(env, SYM_KEYS_PRESSED, &pressed);

        let bdown: Vec<i64> = self.mouse_buttons.iter().map(|b| *b as i64).collect();
        bind_int_set(env, SYM_BUTTONS_DOWN, &bdown);
        let bpressed: Vec<i64> = self
            .mouse_buttons
            .difference(&self.mouse_buttons_prev)
            .map(|b| *b as i64)
            .collect();
        bind_int_set(env, SYM_BUTTONS_PRESSED, &bpressed);

        bind_int(env, SYM_MOUSE_X, self.mouse_x as i64);
        bind_int(env, SYM_MOUSE_Y, self.mouse_y as i64);
    }
}

// ---------------------------------------------------------------------------
// Reading bound uniforms (native-fn side)
// ---------------------------------------------------------------------------

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

/// Is `needle` present in the bound string list named `name`?
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

/// Is `needle` present in the bound int list named `name`?
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

// ---------------------------------------------------------------------------
// Native functions — drawing
// ---------------------------------------------------------------------------

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

    // Validate up front; the renderer re-reads the points list on decode.
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

// ---------------------------------------------------------------------------
// Native functions — offscreen canvases (PGraphics-style render targets)
// ---------------------------------------------------------------------------

fn native_create_canvas(state: &mut PetalCxt) -> NativeResult {
    let w = state.get_int(1)?;
    let h = state.get_int(2)?;
    let sym = state.intern_symbol(CANVAS_ID_COUNTER);
    let id = state.next_counter(sym) as i64;
    emit_draw(state, "create_canvas", vec![Value::Int(id), Value::Int(w), Value::Int(h)]);
    state.push_int(id);
    Ok(1)
}

fn native_draw_to(state: &mut PetalCxt) -> NativeResult {
    let id = state.get_int(1)?;
    emit_draw(state, "set_target", vec![Value::Int(id)]);
    state.push_nil();
    Ok(1)
}

fn native_draw_to_screen(state: &mut PetalCxt) -> NativeResult {
    emit_draw(state, "set_target", vec![Value::Int(0)]);
    state.push_nil();
    Ok(1)
}

fn native_draw_canvas(state: &mut PetalCxt) -> NativeResult {
    let args = int_args(state, 3)?;
    emit_draw(state, "draw_canvas", args);
    state.push_nil();
    Ok(1)
}

// ---------------------------------------------------------------------------
// Native functions — input
// ---------------------------------------------------------------------------

fn native_mouse_x(state: &mut PetalCxt) -> NativeResult {
    let x = binding_int(state, SYM_MOUSE_X);
    state.push_int(x);
    Ok(1)
}

fn native_mouse_y(state: &mut PetalCxt) -> NativeResult {
    let y = binding_int(state, SYM_MOUSE_Y);
    state.push_int(y);
    Ok(1)
}

fn native_mouse_down(state: &mut PetalCxt) -> NativeResult {
    let button = state.get_int(1)?;
    let down = binding_has_int(state, SYM_BUTTONS_DOWN, button);
    state.push_bool(down);
    Ok(1)
}

fn native_mouse_pressed(state: &mut PetalCxt) -> NativeResult {
    let button = state.get_int(1)?;
    let pressed = binding_has_int(state, SYM_BUTTONS_PRESSED, button);
    state.push_bool(pressed);
    Ok(1)
}

fn native_key_down(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let down = binding_has_str(state, SYM_KEYS_DOWN, &name);
    state.push_bool(down);
    Ok(1)
}

fn native_key_pressed(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    let pressed = binding_has_str(state, SYM_KEYS_PRESSED, &name);
    state.push_bool(pressed);
    Ok(1)
}

// ---------------------------------------------------------------------------
// Native functions — timing
// ---------------------------------------------------------------------------

fn native_dt(state: &mut PetalCxt) -> NativeResult {
    let dt = binding_float(state, SYM_DT);
    state.push_float(dt);
    Ok(1)
}

fn native_frame_count(state: &mut PetalCxt) -> NativeResult {
    let count = binding_int(state, SYM_FRAME_COUNT);
    state.push_int(count);
    Ok(1)
}

fn native_screen_width(state: &mut PetalCxt) -> NativeResult {
    let w = binding_int(state, SYM_SCREEN_WIDTH);
    state.push_int(w);
    Ok(1)
}

fn native_screen_height(state: &mut PetalCxt) -> NativeResult {
    let h = binding_int(state, SYM_SCREEN_HEIGHT);
    state.push_int(h);
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
    env.register_native("fill_triangle", native_fill_triangle);
    env.register_native("fill_poly", native_fill_poly);
    env.register_native("draw_text", native_draw_text);
    env.register_native("create_canvas", native_create_canvas);
    env.register_native("draw_to", native_draw_to);
    env.register_native("draw_to_screen", native_draw_to_screen);
    env.register_native("draw_canvas", native_draw_canvas);

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

/// Drain the `draw_commands` output buffer and serialize it to a JSON array.
/// Each command is a `Value::EnumVariant`, so the JSON shape is
/// `[{ "type": "enum", "tag": "rect", "data": [...] }, ...]` (see the renderer).
fn take_draw_commands(env: &mut Env) -> String {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer(sym);
    let arr: Vec<serde_json::Value> =
        values.iter().map(|v| value_to_json(v, env.heap())).collect();
    serde_json::Value::Array(arr).to_string()
}

// ---------------------------------------------------------------------------
// PetalRuntime — WASM-exported struct
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct PetalRuntime {
    env: Env,
    active_program: Option<ProgramId>,
    active_stack: Option<StackKey>,
    /// Host-owned input bookkeeping; bound into `env` before each run.
    input: InputState,
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
            input: InputState::default(),
        }
    }

    /// Bind the current input snapshot and reset the per-frame canvas-id
    /// counter, so a run sees up-to-date input and stable offscreen ids.
    fn prepare_run(&mut self) {
        self.input.bind_into(&mut self.env);
        let c = self.env.intern_symbol(CANVAS_ID_COUNTER);
        self.env.reset_counter(c, 1);
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
        self.prepare_run();
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

    pub fn take_draw_commands(&mut self) -> String {
        take_draw_commands(&mut self.env)
    }

    pub fn set_mouse_position(&mut self, x: i32, y: i32) {
        self.input.mouse_x = x;
        self.input.mouse_y = y;
    }

    pub fn set_mouse_button(&mut self, button: i32, down: bool) {
        if down {
            self.input.mouse_buttons.insert(button as u8);
        } else {
            self.input.mouse_buttons.remove(&(button as u8));
        }
    }

    pub fn set_key_state(&mut self, key: &str, down: bool) {
        if down {
            self.input.keys_down.insert(key.to_string());
        } else {
            self.input.keys_down.remove(key);
        }
    }

    pub fn set_frame_info(&mut self, dt: f64, frame_count: i32, width: i32, height: i32) {
        let dt_sym = self.env.intern_symbol(SYM_DT);
        self.env.set_binding(dt_sym, Value::Float(dt));
        let fc = self.env.intern_symbol(SYM_FRAME_COUNT);
        self.env.set_binding(fc, Value::Int(frame_count as i64));
        let w = self.env.intern_symbol(SYM_SCREEN_WIDTH);
        self.env.set_binding(w, Value::Int(width as i64));
        let h = self.env.intern_symbol(SYM_SCREEN_HEIGHT);
        self.env.set_binding(h, Value::Int(height as i64));
    }

    pub fn begin_frame(&mut self) {
        // Snapshot prev input for pressed-edge detection. The input snapshot is
        // bound (and the canvas counter reset) at run time, in `prepare_run`.
        self.input.begin_frame();
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
        // Bind input + reset canvas ids so a speculative run matches the live frame.
        self.prepare_run();
        self.env
            .run_speculative(sid)
            .map_err(|e| JsValue::from_str(&e))?;
        Ok(take_draw_commands(&mut self.env))
    }
}
