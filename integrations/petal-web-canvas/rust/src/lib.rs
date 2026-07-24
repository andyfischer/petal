//! WASM runtime for petal-web-canvas.
//!
//! Wraps the core Petal `Env` with the standard petal-ui interactivity layer
//! (input, draw commands, offscreen canvases, the `ui` prelude, `host_data`) so
//! this host shares one contract with petal-sdl instead of hand-copying it. The
//! host owns *policy* (the browser event → `InputEvent` translation lives in
//! the TypeScript side and this thin shim); petal-ui owns *semantics* (what a
//! press edge is, how draw commands serialize).

use wasm_bindgen::prelude::*;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value::value_to_json;

use petal_ui::draw::{
    clear_draw_commands, reset_canvas_ids, take_draw_commands, take_draw_commands_for,
};
use petal_ui::input::{
    InputEvent, InputState, bind_dimensions, bind_frame_info, bind_input, bind_time, buttons,
};

/// Map a DOM `MouseEvent.button` (0 = left, 1 = middle, 2 = right) onto the
/// standard petal-ui button id (0 = left, 1 = right, 2 = middle). Doing this at
/// the host boundary keeps scripts portable — `mouse_down(0)` means "left" on
/// every embedder, matching petal-sdl.
fn dom_button_to_std(button: i32) -> Option<u8> {
    Some(match button {
        0 => buttons::LEFT,
        1 => buttons::MIDDLE,
        2 => buttons::RIGHT,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// PetalRuntime — WASM-exported struct
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct PetalRuntime {
    env: Env,
    active_program: Option<ProgramId>,
    active_stack: Option<StackKey>,
    /// Host-owned input bookkeeping; events feed it as they arrive, edges are
    /// promoted by `begin_frame`, and the snapshot is bound into `env` before
    /// each run. This is petal-ui's latch model — a press that goes down and up
    /// between two frames is still delivered as a `mouse_pressed` edge.
    input: InputState,
    /// This frame's timing/dimensions, staged by `set_frame_info` and bound
    /// into `env` at run time.
    dt: f64,
    /// Absolute clock in seconds (host `performance.now()`), read fresh each
    /// frame — backs `time()`/`elapsed()` without accumulating `dt`.
    time: f64,
    frame_count: i64,
    width: i32,
    height: i32,
}

#[wasm_bindgen]
impl PetalRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new() -> PetalRuntime {
        let mut env = Env::new();
        // The standard petal-ui set: input natives, the 2D draw vocabulary,
        // offscreen canvas ops, the `host_data` pull channel, and the `ui`
        // prelude as an implicit import.
        petal_ui::input::register_input(&mut env);
        petal_ui::draw::register_draw(&mut env);
        petal_ui::draw::register_canvas(&mut env);
        petal_ui::host_data::register_host_data(&mut env);
        petal_ui::register_prelude(&mut env);
        PetalRuntime {
            env,
            active_program: None,
            active_stack: None,
            input: InputState::new(),
            dt: 0.0,
            time: 0.0,
            frame_count: 0,
            width: 0,
            height: 0,
        }
    }

    /// Bind this frame's input snapshot + timing/dimension uniforms and reset
    /// the per-frame draw buffer and canvas-id counter, so a run sees
    /// up-to-date input and stable offscreen ids.
    fn prepare_run(&mut self) {
        clear_draw_commands(&mut self.env);
        reset_canvas_ids(&mut self.env);
        bind_frame_info(&mut self.env, self.dt, self.frame_count);
        bind_time(&mut self.env, self.time);
        bind_dimensions(&mut self.env, self.width, self.height);
        bind_input(&mut self.env, &self.input);
    }

    pub fn load_program(&mut self, source: &str) -> Result<u32, JsValue> {
        let pid = self
            .env
            .load_program(source)
            .map_err(|e| JsValue::from_str(&e))?;
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

    /// Drain the frame's draw commands as a JSON array of `{ "op": ... }`
    /// objects (petal-ui's `DrawCommand` shape) for the canvas renderer.
    pub fn take_draw_commands(&mut self) -> String {
        let cmds = take_draw_commands(&mut self.env);
        serde_json::to_string(&cmds).unwrap_or_else(|_| "[]".to_string())
    }

    // --- Input (browser events → petal-ui InputState) ---
    //
    // These are called from the TS input handlers as events arrive; the
    // InputState latches edges until `begin_frame` promotes them.

    pub fn set_mouse_position(&mut self, x: i32, y: i32) {
        self.input.event(InputEvent::MouseMove { x, y });
    }

    /// `button` is a DOM `MouseEvent.button` code; it's mapped to the standard
    /// petal-ui id at the boundary.
    pub fn set_mouse_button(&mut self, button: i32, down: bool) {
        if let Some(b) = dom_button_to_std(button) {
            self.input.event(if down {
                InputEvent::MouseDown { button: b }
            } else {
                InputEvent::MouseUp { button: b }
            });
        }
    }

    /// Wheel/trackpad scroll in lines (fractional deltas carry across frames).
    pub fn scroll(&mut self, dx: f64, dy: f64) {
        self.input.event(InputEvent::Scroll { dx, dy });
    }

    /// `key` must already be a canonical petal-ui key name (the TS side maps
    /// DOM key names before calling this).
    pub fn set_key_state(&mut self, key: &str, down: bool) {
        let key = key.to_string();
        self.input.event(if down {
            InputEvent::KeyDown { key }
        } else {
            InputEvent::KeyUp { key }
        });
    }

    /// Typed text for the current frame (read by the script's `text_input()`).
    pub fn type_text(&mut self, text: &str) {
        self.input.type_text(text);
    }

    pub fn set_frame_info(
        &mut self,
        dt: f64,
        time: f64,
        frame_count: i32,
        width: i32,
        height: i32,
    ) {
        self.dt = dt;
        self.time = time;
        self.frame_count = frame_count as i64;
        self.width = width;
        self.height = height;
    }

    /// Promote pending input edges for the frame about to run. Call once per
    /// frame, after `set_frame_info` (it advances the input clock by `dt`) and
    /// before `reset_and_run`.
    pub fn begin_frame(&mut self) {
        self.input.begin_frame(self.dt);
    }

    // --- Text metrics ---
    //
    // The browser owns rasterization, so it also owns measurement: the TS side
    // measures each glyph with `ctx.measureText` at startup and pushes the
    // resulting advance ratios down here. Without this, `text_width()` assumed
    // monospace (0.6 × size) while the canvas rendered a proportional face —
    // every centered or right-aligned label was off.

    /// Bind measured advance ratios (advance ÷ font size, indexed by
    /// codepoint) for the *default* font — what `text_width(s, size)` uses.
    /// `fallback` covers codepoints past the table's end.
    pub fn set_default_font_metrics(&mut self, advances: &[f64], fallback: f64) {
        petal_ui::draw::bind_text_metrics(&mut self.env, fallback);
        petal_ui::draw::bind_text_advance_table(&mut self.env, advances);
    }

    /// Bind advance ratios for a *named* face (a role like `ui` / `mono` /
    /// `serif`, or a family name) — what `text_width(s, size, font)` uses.
    /// The name may be a variant key (`ui@700`, `ui@i`, `ui@700i`), which is
    /// how a style's weight/italic finds the right table.
    pub fn set_font_metrics(&mut self, name: &str, advances: &[f64], fallback: f64) {
        let metrics = petal_ui::draw::FontMetrics::proportional(advances.to_vec(), fallback);
        petal_ui::draw::bind_font_metrics(&mut self.env, name, &metrics);
    }

    /// Name the role the default font is, so a style with no `font` still
    /// resolves that face's bold/italic variants — matching the renderer,
    /// which draws a font-less bold command in the default stack, bold.
    pub fn set_default_font_name(&mut self, name: &str) {
        petal_ui::draw::bind_default_font_name(&mut self.env, name);
    }

    // --- Debug ---

    pub fn get_state_json(&self) -> Result<String, JsValue> {
        let pid = self
            .active_program
            .ok_or_else(|| JsValue::from_str("No active program"))?;
        let sid = self
            .active_stack
            .ok_or_else(|| JsValue::from_str("No active stack"))?;
        let map = self.env.get_state_json(pid, sid);
        Ok(serde_json::Value::Object(map).to_string())
    }

    pub fn set_state_json(&mut self, name: &str, json_value: &str) -> Result<(), JsValue> {
        let pid = self
            .active_program
            .ok_or_else(|| JsValue::from_str("No active program"))?;
        let sid = self
            .active_stack
            .ok_or_else(|| JsValue::from_str("No active stack"))?;
        let val: serde_json::Value =
            serde_json::from_str(json_value).map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.env
            .set_state_from_json(pid, sid, name, &val)
            .map_err(|e| JsValue::from_str(&e))
    }

    pub fn run_speculative(&mut self) -> Result<String, JsValue> {
        let sid = self
            .active_stack
            .ok_or_else(|| JsValue::from_str("No active stack"))?;
        // Bind input + reset the draw buffer / canvas ids so a speculative run
        // matches the live frame, then fork: the fork inherits those and runs
        // with empty output sinks, so its draw commands accumulate in the fork's
        // own context, isolated from the live state.
        //
        // We drive the fork by hand rather than calling `Env::run_speculative`,
        // which drops the fork before we could read its draw buffer — the
        // speculative frame's draw commands live in the fork's context, so we
        // must drain them (with `take_draw_commands_for`) before releasing it.
        self.prepare_run();
        let fork = self
            .env
            .fork_execution(sid)
            .map_err(|e| JsValue::from_str(&e))?;
        self.env
            .reset_stack(fork)
            .map_err(|e| JsValue::from_str(&e))?;
        let result = self
            .env
            .run(fork)
            .map(|_| {
                let cmds = take_draw_commands_for(&mut self.env, fork);
                serde_json::to_string(&cmds).unwrap_or_else(|_| "[]".to_string())
            })
            .map_err(|e| JsValue::from_str(&e));
        self.env.drop_fork(fork);
        result
    }
}

impl Default for PetalRuntime {
    fn default() -> Self {
        Self::new()
    }
}
