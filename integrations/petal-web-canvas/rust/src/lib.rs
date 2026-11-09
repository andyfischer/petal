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
    DrawOptions, FrameCommand, PixelSnap, clear_draw_commands, reset_canvas_ids,
    take_draw_commands, take_draw_commands_for, take_frame_commands_for,
    take_frame_commands_snapped,
};
use petal_ui::draw_wire::DrawWire;
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
    /// The strings of the frame last drained by `take_draw_buffer` or
    /// `run_speculative`, served by `draw_strings`.
    draw_strings: Vec<String>,
    /// Built by `with_subpixel(true)`: the draw natives keep a script's
    /// fractional coordinates (`petal_ui::draw::DrawOptions::subpixel`).
    subpixel: bool,
    /// Which commands of the frame last drained by a `_subpixel` drain carry
    /// fractional geometry (a `SubCommand`), by index; served by
    /// `draw_sub_indices`. Empty after a whole-pixel drain.
    sub_indices: Vec<u32>,
    /// Which commands of the frame last drained by `take_draw_buffer_subpixel`
    /// were drawn under `pixel_snap(true)`, by index; served by
    /// `draw_pixel_snap_indices`. Empty after any other drain.
    snap_all_indices: Vec<u32>,
}

#[wasm_bindgen]
impl PetalRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new() -> PetalRuntime {
        Self::build(false)
    }

    /// A runtime whose draw natives keep fractional coordinates when
    /// `subpixel` is true (`with_subpixel(false)` is `new()`). Drain its frames
    /// with `take_draw_buffer_subpixel` / `run_speculative_subpixel`: curves,
    /// strokes and polygons arrive with the script's fractions, the rect
    /// family stays whole unless the script calls `pixel_snap(false)`, and
    /// `pixel_snap(true)` snaps everything. The whole-pixel drains still work
    /// on it and return exactly what they return on a `new()` runtime.
    pub fn with_subpixel(subpixel: bool) -> PetalRuntime {
        Self::build(subpixel)
    }

    /// Whether this runtime was built with sub-pixel draw natives.
    pub fn subpixel(&self) -> bool {
        self.subpixel
    }

    fn build(subpixel: bool) -> PetalRuntime {
        // A Rust panic reaches the page as a bare `RuntimeError: unreachable`;
        // this logs its message and location to the console first.
        console_error_panic_hook::set_once();
        let mut env = Env::new();
        // The standard petal-ui set: input natives, the 2D draw vocabulary,
        // offscreen canvas ops, the `host_data` pull channel, and the `ui`
        // package, which a script reaches with `import ui`.
        petal_ui::input::register_input(&mut env);
        if subpixel {
            // Registers the canvas natives too (with the sub-pixel
            // `draw_canvas`); a `register_canvas` after it would put the
            // whole-pixel one back.
            petal_ui::draw::register_draw_with(
                &mut env,
                DrawOptions {
                    subpixel: true,
                    ..Default::default()
                },
            );
        } else {
            petal_ui::draw::register_draw(&mut env);
            petal_ui::draw::register_canvas(&mut env);
        }
        petal_ui::host_data::register_host_data(&mut env);
        petal_ui::register_prelude(&mut env);
        petal_ui::provide_interface(&mut env);
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
            draw_strings: Vec::new(),
            subpixel,
            sub_indices: Vec::new(),
            snap_all_indices: Vec::new(),
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

    /// Register an in-memory module: later programs can `import name`.
    pub fn register_module(&mut self, name: &str, source: &str) {
        self.env.register_module(name, source);
    }

    /// Register a whole package from memory. `modules_json` is a JSON object
    /// of package-relative module name (`menu`, `ui/menu`) to source text —
    /// the form `petal bundle` emits. The page has no filesystem, so this is
    /// how a library (or a host's own interface package) reaches a script.
    pub fn register_package(&mut self, name: &str, modules_json: &str) -> Result<(), JsValue> {
        let modules: std::collections::BTreeMap<String, String> = serde_json::from_str(modules_json)
            .map_err(|e| {
                JsValue::from_str(&format!(
                    "register_package: expected a JSON object of module name to source: {e}"
                ))
            })?;
        self.env
            .register_package(name, modules.iter())
            .map(|_| ())
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Announce that this runtime provides the host interface `name` at
    /// `version` (`"0.4.0"`). A package whose `petal.toml` says
    /// `[requires] name = ">=…"` loads only in a runtime that provides it.
    /// Call it where the natives of that interface are registered.
    pub fn provides(&mut self, name: &str, version: &str) -> Result<(), JsValue> {
        self.env
            .provides(name, version)
            .map_err(|e| JsValue::from_str(&e))
    }

    /// The interfaces this runtime provides, as a JSON object of name to
    /// version. Always includes `core`.
    pub fn provided_interfaces(&self) -> String {
        let map: std::collections::BTreeMap<String, String> =
            self.env.provided_interfaces().into_iter().collect();
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
    }

    /// Check this runtime's registered natives against its interface
    /// packages (comma-separated; `"ui"` for the stock runtime). Returns an
    /// empty string when they agree, else what differs.
    pub fn verify_interface(&self, packages: &str) -> String {
        let list: Vec<&str> = packages
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        match self.env.verify_interface(&list) {
            Ok(()) => String::new(),
            Err(mismatch) => mismatch.to_string(),
        }
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

    /// The frame gate (`Env::run_needed`): whether running now could differ
    /// from the last run. Call after `set_frame_info` + `begin_frame` and any
    /// prop flush, before `reset_and_run`; `false` means the last frame's
    /// commands are this frame's, and the caller may skip both the run and
    /// the repaint. Binds this frame's inputs as a side effect, exactly as
    /// `reset_and_run` would.
    pub fn frame_needed(&mut self, stack_id: u32) -> bool {
        bind_frame_info(&mut self.env, self.dt, self.frame_count);
        bind_time(&mut self.env, self.time);
        bind_dimensions(&mut self.env, self.width, self.height);
        bind_input(&mut self.env, &self.input);
        self.env.run_needed(StackKey(stack_id))
    }

    /// Force the next `frame_needed` to answer true (something outside the
    /// runtime's view changed: a font table, host-owned data).
    pub fn invalidate_frame(&mut self, stack_id: u32) {
        self.env.invalidate_run(StackKey(stack_id));
    }

    pub fn take_output(&mut self) -> String {
        let output = self.env.take_output();
        serde_json::to_string(&output).unwrap_or_else(|_| "[]".to_string())
    }

    // --- Graphics ---

    /// Drain the frame's draw commands in petal-ui's binary wire form
    /// (`petal_ui::draw_wire`): the numbers come back as a `Float64Array`,
    /// and the strings they reference are then served by `draw_strings`.
    /// `decodeDrawCommands` in `src/draw-wire.ts` turns the pair into the
    /// `{ op, ...fields }` objects the renderer draws.
    pub fn take_draw_buffer(&mut self) -> Vec<f64> {
        let cmds = take_draw_commands(&mut self.env);
        self.encode(&cmds)
    }

    /// `take_draw_buffer` for a sub-pixel host: the same frame, the same
    /// opcodes and layout (`DrawWire::encode_frame`), with fractional numbers
    /// where the frame has them. On a `with_subpixel(true)` runtime those are
    /// the script's own; on any other they are already whole, and the buffer
    /// equals `take_draw_buffer`'s. Strings are served by `draw_strings`, and
    /// which commands are sub-pixel by `draw_sub_indices`, and which were
    /// drawn under `pixel_snap(true)` by `draw_pixel_snap_indices`.
    pub fn take_draw_buffer_subpixel(&mut self) -> Vec<f64> {
        let (cmds, modes): (Vec<FrameCommand>, Vec<PixelSnap>) =
            take_frame_commands_snapped(&mut self.env).into_iter().unzip();
        let numbers = self.encode_frame(&cmds);
        self.snap_all_indices = modes
            .iter()
            .enumerate()
            .filter(|(_, m)| **m == PixelSnap::All)
            .map(|(i, _)| i as u32)
            .collect();
        numbers
    }

    /// The strings of the frame last drained by `take_draw_buffer`,
    /// `take_draw_buffer_subpixel` or a `run_speculative`, indexed by that
    /// buffer.
    pub fn draw_strings(&self) -> Vec<String> {
        self.draw_strings.clone()
    }

    /// The indices (in decode order) of the commands of the frame last
    /// drained by `take_draw_buffer_subpixel` or `run_speculative_subpixel`
    /// that carry fractional geometry: drawn under a snap mode that leaves
    /// them unsnapped. Empty after a whole-pixel drain.
    pub fn draw_sub_indices(&self) -> Vec<u32> {
        self.sub_indices.clone()
    }

    /// The indices (in decode order) of the commands of the frame last
    /// drained by `take_draw_buffer_subpixel` that were drawn under
    /// `pixel_snap(true)`: pixel art, which a renderer draws without image
    /// smoothing. Whole pixels either way, so `draw_sub_indices` cannot tell
    /// these from the default mode's. Empty after any other drain.
    pub fn draw_pixel_snap_indices(&self) -> Vec<u32> {
        self.snap_all_indices.clone()
    }

    fn encode(&mut self, cmds: &[petal_ui::draw::DrawCommand]) -> Vec<f64> {
        let wire = DrawWire::encode(cmds);
        self.draw_strings = wire.strings;
        self.sub_indices.clear();
        self.snap_all_indices.clear();
        wire.numbers
    }

    fn encode_frame(&mut self, cmds: &[FrameCommand]) -> Vec<f64> {
        let wire = DrawWire::encode_frame(cmds);
        self.draw_strings = wire.strings;
        self.sub_indices = cmds
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, FrameCommand::Sub(_)))
            .map(|(i, _)| i as u32)
            .collect();
        self.snap_all_indices.clear();
        wire.numbers
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

    /// Bind the *vertical* metrics of the default font — where a run's ink
    /// sits relative to the `y` a script draws it at, so `text_metrics()` can
    /// answer and a centred label can land on the line it claims to.
    ///
    /// Every value is a ratio of the font size. `baseline` is measured against
    /// this renderer's own convention: it draws with `textBaseline = "top"`,
    /// so `y` → baseline is the font bounding box's ascent.
    pub fn set_default_font_vertical(
        &mut self,
        baseline: f64,
        descent: f64,
        line_height: f64,
        cap_height: f64,
        x_height: f64,
    ) {
        let vertical = petal_ui::draw::VerticalMetrics {
            baseline,
            descent,
            line_height,
            cap_height,
            x_height,
        };
        petal_ui::draw::bind_text_vertical_metrics(&mut self.env, &vertical);
    }

    /// Bind advance ratios for a *named* face (a role like `ui` / `mono` /
    /// `serif`, or a family name) — what `text_width(s, size, font)` uses.
    /// The name may be a variant key (`ui@700`, `ui@i`, `ui@700i`), which is
    /// how a style's weight/italic finds the right table.
    #[allow(clippy::too_many_arguments)]
    pub fn set_font_metrics(
        &mut self,
        name: &str,
        advances: &[f64],
        fallback: f64,
        baseline: f64,
        descent: f64,
        line_height: f64,
        cap_height: f64,
        x_height: f64,
    ) {
        let metrics = petal_ui::draw::FontMetrics::proportional(advances.to_vec(), fallback)
            .with_vertical(petal_ui::draw::VerticalMetrics {
                baseline,
                descent,
                line_height,
                cap_height,
                x_height,
            });
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

    /// Run one frame on a fork of the active stack and return its draw
    /// commands, leaving the live state untouched. The result is a draw
    /// buffer like `take_draw_buffer`'s, with its strings in `draw_strings`.
    pub fn run_speculative(&mut self) -> Result<Vec<f64>, JsValue> {
        let fork = self.begin_speculative()?;
        let result = self
            .env
            .run(fork)
            .map(|_| take_draw_commands_for(&mut self.env, fork));
        let result = result
            .map(|cmds| self.encode(&cmds))
            .map_err(|e| JsValue::from_str(&e));
        self.env.drop_fork(fork);
        result
    }

    /// `run_speculative`, drained as `take_draw_buffer_subpixel` drains.
    pub fn run_speculative_subpixel(&mut self) -> Result<Vec<f64>, JsValue> {
        let fork = self.begin_speculative()?;
        let result = self
            .env
            .run(fork)
            .map(|_| take_frame_commands_for(&mut self.env, fork));
        let result = result
            .map(|cmds| self.encode_frame(&cmds))
            .map_err(|e| JsValue::from_str(&e));
        self.env.drop_fork(fork);
        result
    }

    /// Prepare a speculative frame: a reset fork of the active stack, to be
    /// run, drained and dropped by the caller.
    fn begin_speculative(&mut self) -> Result<StackKey, JsValue> {
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
        Ok(fork)
    }
}

impl Default for PetalRuntime {
    fn default() -> Self {
        Self::new()
    }
}
