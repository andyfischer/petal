//! WASM runtime for petal-web-canvas.
//!
//! Wraps the core Petal `Env` with the standard petal-ui interactivity layer
//! (input, draw commands, offscreen canvases, the `ui` prelude, `host_data`) so
//! this host shares one contract with petal-sdl instead of hand-copying it. The
//! host owns *policy* (the browser event → `InputEvent` translation lives in
//! the TypeScript side and this thin shim); petal-ui owns *semantics* (what a
//! press edge is, how draw commands serialize).

use wasm_bindgen::prelude::*;

use petal::dm::NoHost;
use petal::env::Env;
use petal::execution_context::EmitSite;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value::value_to_json;

use petal_ui::draw::{
    DrawCommand, DrawOptions, FrameCommand, PixelSnap, clear_draw_commands, reset_canvas_ids,
    take_draw_commands, take_draw_commands_for, take_draw_commands_traced,
    take_frame_commands_for, take_frame_commands_snapped, take_frame_commands_snapped_traced,
};
use petal_ui::draw_wire::DrawWire;
use petal_ui::edit_mode::{
    DragAlternative, DragCandidate, DragFilter, DragOutcomeV2, DragProperty, DragSelector,
    EditSession, GoalTarget, SessionOptions, SourceRewrite, describe_call, hit_test,
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
    /// Edit mode (docs/features/direct-manipulation.md, "Hosts"): the frame
    /// last drained while emit tracing was on, whole-pixel, for hit-testing a
    /// pointer against what is on screen, and each command's call chain
    /// (index-aligned). Empty while tracing is off.
    frame_cmds: Vec<DrawCommand>,
    frame_origins: Vec<EmitSite>,
    /// Keep a pre-run fork of the stack (`set_edit_checkpoint`).
    checkpoint_on: bool,
    /// The stack as it stood before the frame on screen ran, with that
    /// frame's inputs bound: a speculative re-run of it draws that frame.
    checkpoint: Option<StackKey>,
    /// The drag in progress, between `edit_begin` and `edit_finish`.
    edit: Option<EditSession>,
    /// The last release's alternatives and the press-time source their
    /// rewrites are spans in: what `edit_choose` re-applies.
    last_alternatives: Vec<DragAlternative>,
    last_source: Option<String>,
    /// The sticky choice: the literals of the alternative the author last
    /// picked (`edit_choose`), preferred by every later `edit_begin` until
    /// another pick or `edit_clear_preference`. Lives as long as the
    /// runtime; never written into the source.
    prefer: Vec<DragSelector>,
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
        // (Only in the browser: natively, as in this crate's tests, it would
        // call into a missing JS import and abort.)
        #[cfg(target_arch = "wasm32")]
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
            frame_cmds: Vec::new(),
            frame_origins: Vec::new(),
            checkpoint_on: false,
            checkpoint: None,
            edit: None,
            last_alternatives: Vec::new(),
            last_source: None,
            prefer: Vec::new(),
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
        self.load_program_inner(source).map_err(|e| JsValue::from_str(&e))
    }

    pub fn create_stack(&mut self, program_id: u32) -> Result<u32, JsValue> {
        self.create_stack_inner(program_id).map_err(|e| JsValue::from_str(&e))
    }

    pub fn run(&mut self, stack_id: u32) -> Result<String, JsValue> {
        self.run_inner(stack_id).map_err(|e| JsValue::from_str(&e))
    }

    pub fn reset_and_run(&mut self, stack_id: u32) -> Result<String, JsValue> {
        self.reset_and_run_inner(stack_id).map_err(|e| JsValue::from_str(&e))
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
        if self.env.emit_trace_enabled() {
            let (cmds, origins): (Vec<DrawCommand>, Vec<EmitSite>) =
                take_draw_commands_traced(&mut self.env).into_iter().unzip();
            self.frame_origins = origins;
            let numbers = self.encode(&cmds);
            self.frame_cmds = cmds;
            return numbers;
        }
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
        let (cmds, modes): (Vec<FrameCommand>, Vec<PixelSnap>) = if self.env.emit_trace_enabled() {
            let mut cmds = Vec::new();
            let mut modes = Vec::new();
            self.frame_origins.clear();
            for (c, m, o) in take_frame_commands_snapped_traced(&mut self.env) {
                cmds.push(c);
                modes.push(m);
                self.frame_origins.push(o);
            }
            self.frame_cmds = cmds.iter().map(FrameCommand::to_px).collect();
            (cmds, modes)
        } else {
            take_frame_commands_snapped(&mut self.env).into_iter().unzip()
        };
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

// ---------------------------------------------------------------------------
// Edit mode: drag a drawn item, get a source edit
// ---------------------------------------------------------------------------
//
// The page's half of `petal_ui::edit_mode` (docs/features/direct-manipulation.md,
// "Hosts"; docs/tasks/differentiation-rewrite.md, Design §3). The page freezes
// its frame loop while edit mode is on, so the frame on screen, its emit trace
// and its checkpoint stay put; the page owns the source text and the line
// selection, and gets back whole new source texts to put in its editor.
//
// Every answer is a JSON string, `{ "ok": false, "error": … }` on failure.

#[wasm_bindgen]
impl PetalRuntime {
    /// Record which call drew each command (`Env::enable_emit_trace`). While
    /// on, `take_draw_buffer` / `take_draw_buffer_subpixel` also keep the
    /// frame for `edit_pick`. Off by default: it costs a little per emit.
    pub fn set_emit_trace(&mut self, on: bool) {
        self.env.enable_emit_trace(on);
        if !on {
            self.frame_cmds.clear();
            self.frame_origins.clear();
        }
    }

    /// Whether emit tracing is on.
    pub fn emit_trace(&self) -> bool {
        self.env.emit_trace_enabled()
    }

    /// Keep a pre-run fork of the stack on every `reset_and_run`: the
    /// checkpoint an edit-mode drag re-runs the frame on screen from. One heap
    /// fork per frame that runs while on, nothing while off.
    pub fn set_edit_checkpoint(&mut self, on: bool) {
        self.checkpoint_on = on;
        if !on {
            if let Some(ck) = self.checkpoint.take() {
                self.env.drop_fork(ck);
            }
        }
    }

    /// The item under (`x`, `y`) in the last traced frame, and the call in the
    /// sketch that drew it: `{ok, index, callee, line, text, label, bounds}`
    /// (`line` 1-based; `bounds` `[x, y, w, h]` or null).
    pub fn edit_pick(&self, x: i32, y: i32) -> String {
        if self.frame_cmds.is_empty() {
            return err_json("no traced frame yet (turn on set_emit_trace and run a frame)");
        }
        let Some(index) = hit_test(&self.frame_cmds, x, y) else {
            return err_json("nothing is drawn there");
        };
        let origin = self.frame_origins.get(index).cloned().unwrap_or_default();
        let call = self
            .active_program
            .and_then(|pid| describe_call(&self.env, pid, &origin));
        let bounds = self.frame_cmds.get(index).and_then(petal_ui::edit_mode::bounds);
        serde_json::json!({
            "ok": true,
            "index": index,
            "callee": call.as_ref().map(|c| c.callee.clone()),
            "line": call.as_ref().map(|c| c.line),
            "text": call.as_ref().map(|c| c.text.clone()),
            "label": call.as_ref().map(|c| c.label()).unwrap_or_else(|| "a call with no source position".to_string()),
            "bounds": bounds.map(|(x, y, w, h)| vec![x as i64, y as i64, w as i64, h as i64]),
        })
        .to_string()
    }

    /// Open a drag on command `index` of the last traced frame. `lines_json`
    /// is the author's selection, a JSON array of 1-based sketch lines: the
    /// request's allow set (`line:N`), so only literals on those lines move,
    /// and up to three alternatives come back. An empty selection runs the
    /// request best effort (`tunable` first, never `const`/`rigid`, an equal
    /// split across ties, one answer or an error).
    ///
    /// Returns `{ok, policy, allow, base: [x…, y…], candidates}`; this is the
    /// expensive step (screening and the Jacobian). Any open drag is
    /// cancelled first.
    pub fn edit_begin(&mut self, index: u32, lines_json: &str) -> String {
        self.edit_cancel();
        let lines: Vec<usize> = match serde_json::from_str::<Vec<usize>>(lines_json) {
            Ok(l) => l.into_iter().filter(|l| *l >= 1).collect(),
            Err(e) => return err_json(&format!("lines must be a JSON array of line numbers: {e}")),
        };
        let filter = DragFilter::allow_lines(lines.iter().map(|l| l - 1));
        let mut opts = if filter.is_default() {
            SessionOptions::best_effort()
        } else {
            SessionOptions::default()
        };
        opts.prefer = self.prefer.clone();
        let index = index as usize;
        if index >= self.frame_cmds.len() {
            return err_json("that item is not in the traced frame");
        }
        let target = GoalTarget::at(&self.frame_origins, index, DragProperty::Position);
        // No checkpoint (edit mode armed after the last run): the live stack
        // after the frame, right for a still sketch.
        let Some(checkpoint) = self.checkpoint.or(self.active_stack) else {
            return err_json("no program is running");
        };
        match EditSession::open(&mut self.env, &mut NoHost, checkpoint, &target, &filter, &opts) {
            Ok(session) => {
                let start = session.start();
                self.edit = Some(session);
                let base: Vec<f64> = start.base_x.iter().chain(start.base_y.iter()).copied().collect();
                serde_json::json!({
                    "ok": true,
                    "policy": policy_name(&filter),
                    "allow": filter.allow.iter().map(|s| s.label()).collect::<Vec<_>>(),
                    "base": base,
                    "candidates": start.candidates.iter().map(candidate_json).collect::<Vec<_>>(),
                    "runs": start.runs,
                    "prefer": self.prefer.iter().map(|s| s.label()).collect::<Vec<_>>(),
                })
                .to_string()
            }
            Err(outcome) => outcome_json(outcome, None, Some(&filter)),
        }
    }

    /// What the open drag would write if released `dx`, `dy` pixels from the
    /// press: the best candidate's linear answer, with no runs. A JSON array
    /// of `{line, new_text, description}`.
    pub fn edit_preview(&self, dx: f64, dy: f64) -> String {
        let Some(session) = &self.edit else {
            return "[]".to_string();
        };
        let rewrites = session.preview(&self.env, &[dx, dy]);
        serde_json::Value::Array(rewrites.iter().map(rewrite_json).collect()).to_string()
    }

    /// Live apply: the sketch text the open drag would write if released
    /// `dx`, `dy` pixels from the press (always computed from the press-time
    /// source), hot-reloaded into the running program in place, keeping its
    /// state and the drag. Returns `{ok, source, rewrites}`; draw the result
    /// with `run_checkpoint` (the frame on screen re-run under the edited
    /// program), and call this at most once per animation frame. A compile
    /// error leaves the program and the drag as they were: `{ok: false,
    /// error, source}`.
    pub fn edit_live(&mut self, dx: f64, dy: f64) -> String {
        let Some(session) = self.edit.as_mut() else {
            return err_json("no drag is open");
        };
        let prev = session.live_text().map(str::to_string);
        let live = match session.preview_live(&self.env, &[dx, dy]) {
            Ok(l) => l,
            Err(e) => return err_json(&e),
        };
        let rewrites: Vec<serde_json::Value> = live.rewrites.iter().map(rewrite_json).collect();
        match self.reload_in_place(&live.text) {
            Ok(()) => serde_json::json!({ "ok": true, "source": live.text, "rewrites": rewrites }),
            Err(e) => {
                // The program still runs the text before this move, which
                // stays the drag's own (the release is not stale).
                if let Some(session) = self.edit.as_mut() {
                    session.revert_live_text(prev);
                }
                serde_json::json!({ "ok": false, "error": e, "source": live.text })
            }
        }
        .to_string()
    }

    /// Hot-reload `source` into the running program in place, keeping its
    /// state and any open drag (whose live text it becomes): what the page
    /// calls to show the alternative a release chose, or to put the
    /// press-time source back after a cancelled live drag. Draw the result
    /// with `run_checkpoint`, or by resuming the frame loop. `{ok}` or
    /// `{ok: false, error}`.
    pub fn edit_show(&mut self, source: &str) -> String {
        match self.reload_in_place(source) {
            Ok(()) => {
                if let Some(session) = self.edit.as_mut() {
                    session.set_live_text(source);
                }
                serde_json::json!({ "ok": true }).to_string()
            }
            Err(e) => err_json(&e),
        }
    }

    /// Re-run the frame on screen (the edit checkpoint) under the program as
    /// it is now, on a fork, and return its draw buffer as
    /// `take_draw_buffer` does: the live view of a drag. The live state and
    /// the checkpoint are untouched.
    pub fn run_checkpoint(&mut self) -> Result<Vec<f64>, JsValue> {
        let cmds = self.run_checkpoint_inner().map_err(|e| JsValue::from_str(&e))?;
        Ok(self.encode(&cmds))
    }

    /// Release the open drag at `dx`, `dy`: verify, round and rank, against
    /// the press-time source even when `edit_live` rewrote it under the
    /// drag. Returns `{ok, policy, alternatives, candidates}`, each
    /// alternative with the whole new sketch text (`source`) to put in the
    /// editor and `preferred` (it is the sticky choice made again), or
    /// `{ok: false, error}` (nothing reaches the goal, or the program changed
    /// under the drag by an edit not the drag's own).
    ///
    /// After a live drag the running program is back at the press-time
    /// source: show the first alternative with `edit_show`, or another with
    /// `edit_choose`.
    pub fn edit_finish(&mut self, dx: f64, dy: f64) -> String {
        let Some(session) = self.edit.take() else {
            return err_json("no drag is open");
        };
        let Some(current) = self.active_program else {
            session.cancel(&mut self.env);
            return err_json("no program is running");
        };
        let source = session.source().to_string();
        let outcome = session.finish(&mut self.env, &mut NoHost, current, &[dx, dy]);
        match &outcome {
            DragOutcomeV2::Alternatives { alternatives, .. } => {
                self.last_alternatives = alternatives.clone();
                self.last_source = Some(source.clone());
            }
            _ => {
                self.last_alternatives.clear();
                self.last_source = None;
            }
        }
        outcome_json(outcome, Some(&source), None)
    }

    /// The author picks alternative `k` of the last release: the same
    /// gesture applied that way, from the press-time source. It is
    /// hot-reloaded in place (as `edit_show`) and remembered as the sticky
    /// choice, so later drags prefer the same literals when that reaches
    /// their goals. `{ok, source, prefer}` or `{ok: false, error}`.
    pub fn edit_choose(&mut self, k: u32) -> String {
        let (Some(alt), Some(source0)) = (self.last_alternatives.get(k as usize), self.last_source.as_ref()) else {
            return err_json("no such alternative");
        };
        let text = match alt.apply_to(source0) {
            Ok(t) => t,
            Err(e) => return err_json(&e),
        };
        let prefer = alt.preference();
        if let Err(e) = self.reload_in_place(&text) {
            return err_json(&e);
        }
        if !prefer.is_empty() {
            self.prefer = prefer;
        }
        serde_json::json!({
            "ok": true,
            "source": text,
            "prefer": self.prefer.iter().map(|s| s.label()).collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// The sticky choice, as selector labels (`gap`, `at:4:13`, `fn:place`).
    pub fn edit_preference(&self) -> String {
        serde_json::Value::Array(self.prefer.iter().map(|s| s.label().into()).collect()).to_string()
    }

    /// Set the sticky choice from a JSON array of selector labels (as
    /// `edit_preference` gives them); `[]` clears it. Unparseable labels
    /// are skipped.
    pub fn edit_set_preference(&mut self, labels_json: &str) -> String {
        match serde_json::from_str::<Vec<String>>(labels_json) {
            Ok(labels) => {
                self.prefer = labels.iter().filter_map(|l| DragSelector::parse(l)).collect();
                self.edit_preference()
            }
            Err(e) => err_json(&format!("expected a JSON array of selectors: {e}")),
        }
    }

    /// Forget the sticky choice.
    pub fn edit_clear_preference(&mut self) {
        self.prefer.clear();
    }

    /// Abandon the open drag, if any.
    pub fn edit_cancel(&mut self) {
        if let Some(session) = self.edit.take() {
            session.cancel(&mut self.env);
        }
    }

    /// `edit_begin` then `edit_finish` in one call: a drag of command
    /// `index` by (`dx`, `dy`) with the selection `lines_json`.
    pub fn edit_drag(&mut self, index: u32, dx: f64, dy: f64, lines_json: &str) -> String {
        let begun = self.edit_begin(index, lines_json);
        if self.edit.is_none() {
            return begun;
        }
        self.edit_finish(dx, dy)
    }
}

impl PetalRuntime {
    // The exported entry points with plain string errors: a `JsValue` can only
    // be made in the browser, so the crate's native tests call these.

    fn load_program_inner(&mut self, source: &str) -> Result<u32, String> {
        // A drag and a checkpoint describe the program they were made under.
        self.drop_edit_state();
        let pid = self.env.load_program(source)?;
        self.active_program = Some(pid);
        Ok(pid.0)
    }

    fn create_stack_inner(&mut self, program_id: u32) -> Result<u32, String> {
        let sid = self.env.create_stack(ProgramId(program_id))?;
        self.active_stack = Some(sid);
        Ok(sid.0)
    }

    fn run_inner(&mut self, stack_id: u32) -> Result<String, String> {
        self.prepare_run();
        let val = self.env.run(StackKey(stack_id))?;
        let json = value_to_json(&val, self.env.heap());
        Ok(json.to_string())
    }

    fn reset_and_run_inner(&mut self, stack_id: u32) -> Result<String, String> {
        if self.checkpoint_on {
            // The edit-mode checkpoint: the stack as it stands with this
            // frame's inputs bound and before it runs (a re-run of the stack
            // after the run would draw the next frame, not this one).
            self.prepare_run();
            if let Some(old) = self.checkpoint.take() {
                self.env.drop_fork(old);
            }
            self.checkpoint = self.env.fork_execution(StackKey(stack_id)).ok();
        }
        self.env.reset_stack(StackKey(stack_id))?;
        self.run_inner(stack_id)
    }

    /// `reload_program` on the active stack: the program keeps its id, its
    /// state and any open drag (a literal-only edit is patched in place).
    fn reload_in_place(&mut self, source: &str) -> Result<(), String> {
        let stack = self.active_stack.ok_or("no program is running")?;
        self.env.reload_program(stack, source, None).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// `run_checkpoint`'s run: a reset fork of the checkpoint (else of the
    /// active stack), run and drained. Inputs stay as the checkpoint bound
    /// them: the frame on screen, not the pointer of the drag.
    fn run_checkpoint_inner(&mut self) -> Result<Vec<DrawCommand>, String> {
        let from = self.checkpoint.or(self.active_stack).ok_or("no program is running")?;
        let fork = self.env.fork_execution(from)?;
        let result = self
            .env
            .reset_stack(fork)
            .and_then(|_| self.env.run(fork))
            .map(|_| take_draw_commands_for(&mut self.env, fork));
        self.env.drop_fork(fork);
        result
    }

    /// Forget the drag, the checkpoint and the traced frame: a new program
    /// replaced the one they describe. The sticky choice is kept.
    fn drop_edit_state(&mut self) {
        self.last_alternatives.clear();
        self.last_source = None;
        self.edit_cancel();
        if let Some(ck) = self.checkpoint.take() {
            self.env.drop_fork(ck);
        }
        self.frame_cmds.clear();
        self.frame_origins.clear();
    }
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "ok": false, "error": msg }).to_string()
}

fn policy_name(filter: &DragFilter) -> &'static str {
    if filter.is_default() { "best_effort" } else { "ranked" }
}

fn candidate_json(c: &DragCandidate) -> serde_json::Value {
    serde_json::json!({
        "label": c.label,
        "line": c.span.map(|s| s.start_line + 1),
        "value": c.value,
        "config": c.config,
        "rigid": c.rigid,
        "eligible": c.eligible,
        "why_not": c.why_not,
        "moves": c.moves,
        "selector": c.selector.label(),
    })
}

fn rewrite_json(r: &SourceRewrite) -> serde_json::Value {
    serde_json::json!({
        "line": r.file.is_none().then_some(r.span.start_line + 1),
        "file": r.file.as_ref().map(|f| f.display().to_string()),
        "new_text": r.new_text,
        "description": r.description,
    })
}

fn outcome_json(outcome: DragOutcomeV2, source: Option<&str>, filter: Option<&DragFilter>) -> String {
    match outcome {
        DragOutcomeV2::Alternatives { alternatives, candidates } => {
            let alts: Vec<serde_json::Value> = alternatives
                .iter()
                .map(|a| {
                    let applied = source.map(|s| a.apply_to(s));
                    serde_json::json!({
                        "description": a.description,
                        "verified": a.verified,
                        "moves_others": a.moves_others,
                        "config_only": a.config_only,
                        "preferred": a.preferred,
                        "prefer": a.preference().iter().map(|s| s.label()).collect::<Vec<_>>(),
                        "score": a.score,
                        "sketch_only": a.sketch_only(),
                        "rewrites": a.rewrites.iter().map(rewrite_json).collect::<Vec<_>>(),
                        "source": applied.as_ref().and_then(|r| r.as_ref().ok()),
                        "apply_error": applied.as_ref().and_then(|r| r.as_ref().err()),
                    })
                })
                .collect();
            serde_json::json!({
                "ok": true,
                "alternatives": alts,
                "candidates": candidates.iter().map(candidate_json).collect::<Vec<_>>(),
            })
            .to_string()
        }
        DragOutcomeV2::Refused { why, candidates } => serde_json::json!({
            "ok": false,
            "error": why,
            "policy": filter.map(policy_name),
            "candidates": candidates.iter().map(candidate_json).collect::<Vec<_>>(),
        })
        .to_string(),
        DragOutcomeV2::Stale => err_json("the program changed under the drag; try again"),
    }
}

impl Default for PetalRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKETCH: &str = "import ui\nlet x = 100\nlet y = 80\ndraw_circle(x, y, 20, 200, 100, 50)\ndraw_rect(300, 40, 60, 30, 50, 100, 200)\n";

    /// A runtime in edit mode with `source` loaded and one frame run.
    /// As the page does it: the program runs a few frames untraced, then
    /// edit mode turns tracing and the checkpoint on and runs one more.
    fn edit_runtime(source: &str) -> PetalRuntime {
        let mut rt = PetalRuntime::new();
        let pid = rt.load_program_inner(source).unwrap();
        let sid = rt.create_stack_inner(pid).unwrap();
        let mut frame = |rt: &mut PetalRuntime, n: i32| {
            rt.set_frame_info(1.0 / 60.0, n as f64 / 60.0, n, 640, 480);
            rt.begin_frame();
            rt.reset_and_run_inner(sid).unwrap();
            rt.take_draw_buffer();
        };
        frame(&mut rt, 0);
        frame(&mut rt, 1);
        rt.set_emit_trace(true);
        rt.set_edit_checkpoint(true);
        frame(&mut rt, 2);
        rt
    }

    fn json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    fn pick_index(rt: &PetalRuntime, x: i32, y: i32) -> u32 {
        let p = json(&rt.edit_pick(x, y));
        assert_eq!(p["ok"], true, "{p}");
        p["index"].as_u64().unwrap() as u32
    }

    #[test]
    fn pick_names_the_call_and_its_line() {
        let rt = edit_runtime(SKETCH);
        let p = json(&rt.edit_pick(100, 80));
        assert_eq!(p["ok"], true, "{p}");
        assert_eq!(p["callee"], "draw_circle");
        assert_eq!(p["line"], 4);
        let p = json(&rt.edit_pick(320, 50));
        assert_eq!(p["line"], 5, "{p}");
        let p = json(&rt.edit_pick(600, 400));
        assert_eq!(p["ok"], false);
    }

    #[test]
    fn a_drag_with_nothing_selected_is_best_effort() {
        let mut rt = edit_runtime(SKETCH);
        let index = pick_index(&rt, 100, 80);
        let begun = json(&rt.edit_begin(index, "[]"));
        assert_eq!(begun["policy"], "best_effort", "{begun}");
        let out = json(&rt.edit_finish(40.0, 0.0));
        assert_eq!(out["ok"], true, "{out}");
        let alts = out["alternatives"].as_array().unwrap();
        assert_eq!(alts.len(), 1, "best effort gives one answer: {out}");
        assert!(alts[0]["source"].as_str().unwrap().contains("let x = 140"), "{out}");
    }

    #[test]
    fn a_selection_is_the_allow_set() {
        // Two literals move the circle horizontally: `x` on line 1 and the
        // offset on line 3. Selecting line 3 moves only the offset.
        let src = "import ui\nlet x = 100\nlet off = 0\ndraw_circle(x + off, 80, 20, 200, 100, 50)\n";
        let mut rt = edit_runtime(src);
        let index = pick_index(&rt, 100, 80);
        let out = json(&rt.edit_drag(index, 25.0, 0.0, "[3]"));
        assert_eq!(out["ok"], true, "{out}");
        for alt in out["alternatives"].as_array().unwrap() {
            let s = alt["source"].as_str().unwrap();
            assert!(s.starts_with("import ui\nlet x = 100\n"), "line 2 is not selected: {s}");
        }
        let top = out["alternatives"][0]["source"].as_str().unwrap();
        assert!(top.contains("let off = 25"), "{top}");
        // A selection holding nothing that moves the shape is refused.
        let out = json(&rt.edit_drag(index, 25.0, 0.0, "[1]"));
        assert_eq!(out["ok"], false, "{out}");
    }

    #[test]
    fn an_unreachable_goal_is_an_error() {
        let src = "import ui\ndraw_circle(rigid(100), 80, 20, 200, 100, 50)\n";
        let mut rt = edit_runtime(src);
        let index = pick_index(&rt, 100, 80);
        let out = json(&rt.edit_drag(index, 25.0, 0.0, "[]"));
        assert_eq!(out["ok"], false, "{out}");
        assert!(out["error"].as_str().is_some_and(|e| !e.is_empty()), "{out}");
    }

    #[test]
    fn the_checkpoint_draws_the_frame_on_screen() {
        // An animation: the drag re-runs the frame on screen, not the next.
        let src = "import ui\nstate t = 0\nt = t + 10\ndraw_circle(100 + t, 80, 20, 200, 100, 50)\n";
        let mut rt = edit_runtime(src);
        let index = pick_index(&rt, 130, 80);
        let begun = json(&rt.edit_begin(index, "[]"));
        assert_eq!(begun["base"][0], 130.0, "{begun}");
        rt.edit_cancel();
    }

    #[test]
    fn a_new_program_drops_the_drag() {
        let mut rt = edit_runtime(SKETCH);
        let index = pick_index(&rt, 100, 80);
        assert_eq!(json(&rt.edit_begin(index, "[]"))["ok"], true);
        rt.load_program_inner(SKETCH).unwrap();
        assert_eq!(json(&rt.edit_finish(10.0, 0.0))["ok"], false);
        assert_eq!(json(&rt.edit_pick(100, 80))["ok"], false, "the traced frame is gone too");
    }

    #[test]
    fn a_live_drag_reloads_each_move_and_still_releases() {
        let src = "import ui\nlet x = 100\nlet off = 0\ndraw_circle(x + off, 80, 20, 200, 100, 50)\n";
        let mut rt = edit_runtime(src);
        let index = pick_index(&rt, 100, 80);
        assert_eq!(json(&rt.edit_begin(index, "[2, 3]"))["ok"], true);
        for dx in [4.0, 15.0, 30.0] {
            let live = json(&rt.edit_live(dx, 0.0));
            assert_eq!(live["ok"], true, "{live}");
            assert_ne!(live["source"], src);
            // The frame on screen, re-run under the edited program.
            let cmds = rt.run_checkpoint_inner().unwrap();
            match &cmds[0] {
                DrawCommand::Circle { cx, .. } => assert_eq!(*cx, 100 + dx as i32, "{cmds:?}"),
                c => panic!("{c:?}"),
            }
        }
        let out = json(&rt.edit_finish(30.0, 0.0));
        assert_eq!(out["ok"], true, "the drag's own live text is not stale: {out}");
        let alts = out["alternatives"].as_array().unwrap();
        assert!(alts.len() >= 2, "{out}");
        assert_eq!(json(&rt.edit_show(alts[0]["source"].as_str().unwrap()))["ok"], true);

        // Re-apply the gesture the other way, and it sticks.
        let other = alts[1]["source"].as_str().unwrap().to_string();
        let chose = json(&rt.edit_choose(1));
        assert_eq!(chose["source"], other.as_str(), "{chose}");
        let prefer = chose["prefer"].clone();
        assert_eq!(json(&rt.edit_preference()), prefer);
        let index = pick_index(&rt, 100, 80);
        let begun = json(&rt.edit_begin(index, "[2, 3]"));
        assert_eq!(begun["prefer"], prefer, "{begun}");
        let out = json(&rt.edit_finish(-10.0, 0.0));
        assert_eq!(out["alternatives"][0]["preferred"], true, "{out}");
        assert_eq!(out["alternatives"][0]["prefer"], prefer, "{out}");
        rt.edit_clear_preference();
        assert_eq!(rt.edit_preference(), "[]");
    }

    #[test]
    fn an_edit_from_elsewhere_under_a_live_drag_is_stale() {
        let mut rt = edit_runtime(SKETCH);
        let index = pick_index(&rt, 100, 80);
        assert_eq!(json(&rt.edit_begin(index, "[2]"))["ok"], true);
        assert_eq!(json(&rt.edit_live(10.0, 0.0))["ok"], true);
        rt.reload_in_place(&SKETCH.replace("let y = 80", "let y = 90")).unwrap();
        assert_eq!(json(&rt.edit_finish(10.0, 0.0))["ok"], false);
    }
}
