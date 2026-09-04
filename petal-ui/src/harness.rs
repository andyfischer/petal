//! A headless driver for testing widget logic with no renderer attached.
//!
//! Mirrors the standard host frame contract exactly (bind input → reset →
//! run → drain), so behavior verified here matches what a real embedder
//! sees. Time advances only through [`Headless::frame`]'s fixed `dt`, making
//! multi-click and animation tests deterministic.
//!
//! ```no_run
//! use petal_ui::harness::Headless;
//!
//! let mut ui = Headless::new(
//!     "state hits = 0\n\
//!      if clicked({x: 10, y: 10, w: 80, h: 24}) then hits = hits + 1 end",
//! )
//! .unwrap();
//! ui.click(20, 20);
//! assert_eq!(ui.state()["hits"], 1);
//! ```

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value::Value;

use crate::draw::{self, DrawCommand};
use crate::host_data::{self, DataProvider};
use crate::input::{self, InputEvent, InputState};

/// Fixed per-frame dt (60 fps) so tests are deterministic.
pub const FRAME_DT: f64 = 1.0 / 60.0;

pub struct Headless {
    pub env: Env,
    pub input: InputState,
    program_id: ProgramId,
    stack_id: StackKey,
    frame_count: i64,
    /// Absolute clock (seconds) published to the script as `time()` each frame.
    ///
    /// It starts at `t0 = 0.0` and [`frame`](Self::frame) advances it by
    /// [`FRAME_DT`] after every frame, so the clock is a pure function of the
    /// frame count (`time == frames_run * FRAME_DT`) and never reads the
    /// system clock — a script that sums `dt()` sees exactly `time()`.
    /// Assigning to it still works: the value assigned is what the *next*
    /// frame publishes, and the automatic advance resumes from there.
    pub time: f64,
    /// Draw commands produced by the most recent [`frame`](Self::frame).
    pub commands: Vec<DrawCommand>,
    /// Value returned by the most recent run.
    pub result: Value,
    /// Host data source for the `host_data` native, swapped into the
    /// thread-local channel around each run (see [`set_data_provider`](Self::set_data_provider)).
    provider: Option<DataProvider>,
    /// Font source for the `font` / `fonts` / `text_width` natives, swapped in
    /// around each run the same way (see
    /// [`set_font_source`](Self::set_font_source)).
    fonts: Option<draw::FontProvider>,
}

impl Headless {
    /// Compile `source` in a fresh `Env` with the standard input + draw
    /// natives and the `ui` prelude module (implicit import), sized 800×600.
    pub fn new(source: &str) -> Result<Self, String> {
        Self::with_size(source, 800, 600)
    }

    pub fn with_size(source: &str, width: i32, height: i32) -> Result<Self, String> {
        Self::build(source, None, width, height, &[])
    }

    /// Load a script from a file at an explicit drawable size. Imports resolve
    /// relative to the file's own directory (the app-beside-its-modules layout
    /// every UI example uses), which [`new`](Self::new) cannot do — it has
    /// only text.
    pub fn from_file_with_size(
        path: &std::path::Path,
        width: i32,
        height: i32,
    ) -> Result<Self, String> {
        Self::from_file_with_paths(path, width, height, &[])
    }

    /// The same, plus extra module search directories — the `-I` of the CLI.
    /// A shared Petal library (a component set, a math module) lives outside
    /// the app's own directory, and without this the only way to run such an
    /// app headlessly is to copy the library next to it.
    pub fn from_file_with_paths(
        path: &std::path::Path,
        width: i32,
        height: i32,
        module_paths: &[std::path::PathBuf],
    ) -> Result<Self, String> {
        let source =
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::build(&source, Some(path), width, height, module_paths)
    }

    fn build(
        source: &str,
        origin: Option<&std::path::Path>,
        width: i32,
        height: i32,
        module_paths: &[std::path::PathBuf],
    ) -> Result<Self, String> {
        let mut env = Env::new();
        crate::register_all(&mut env);
        input::bind_dimensions(&mut env, width, height);
        input::bind_frame_info(&mut env, 0.0, 0);
        if let Some(dir) = origin.and_then(|p| p.parent())
            && !dir.as_os_str().is_empty()
        {
            env.add_module_path(dir.to_path_buf());
        }
        for dir in module_paths {
            env.add_module_path(dir.clone());
        }
        let program_id = match origin {
            Some(path) => env.load_program_at(source, path)?,
            None => env.load_program(source)?,
        };
        let stack_id = env.create_stack(program_id)?;
        Ok(Self {
            env,
            input: InputState::new(),
            program_id,
            stack_id,
            frame_count: 0,
            time: 0.0,
            commands: Vec::new(),
            result: Value::Nil,
            provider: None,
            fonts: None,
        })
    }

    /// Attach a host data source for the `host_data(kind, arg)` native. It is
    /// swapped into the thread-local channel for the duration of each
    /// [`frame`](Self::frame), mirroring how a real embedder wires its
    /// provider around `env.run`.
    pub fn set_data_provider(&mut self, provider: DataProvider) {
        self.provider = Some(provider);
    }

    /// Attach a font source for the `font(name)` / `fonts()` natives and the
    /// on-demand half of `text_width`, so widget logic that names a system
    /// face is testable with no renderer attached.
    ///
    /// Answers from a source are memoized *per process*, not per `Headless`,
    /// so attaching a second source in the same test binary would otherwise
    /// see the first one's answers; this clears that cache.
    pub fn set_font_source(&mut self, fonts: draw::FontProvider) {
        draw::clear_font_cache();
        self.fonts = Some(fonts);
    }

    /// Feed one input event (applied to the *next* frame's snapshot).
    pub fn event(&mut self, ev: InputEvent) {
        self.input.event(ev);
    }

    pub fn mouse_move(&mut self, x: i32, y: i32) {
        self.event(InputEvent::MouseMove { x, y });
    }

    pub fn mouse_down(&mut self, button: u8) {
        self.event(InputEvent::MouseDown { button });
    }

    pub fn mouse_up(&mut self, button: u8) {
        self.event(InputEvent::MouseUp { button });
    }

    pub fn scroll(&mut self, dy: f64) {
        self.event(InputEvent::Scroll { dx: 0.0, dy });
    }

    /// Feed a run of typed text, then run one frame — the frame that sees it
    /// through `text_input()`. Mirrors a host delivering post-layout text.
    pub fn text(&mut self, s: &str) -> Result<&[DrawCommand], String> {
        self.event(InputEvent::Text {
            text: s.to_string(),
        });
        self.frame()
    }

    /// Press (and release) a key, then run one frame — the frame that sees
    /// the `key_pressed` edge.
    pub fn key(&mut self, name: &str) -> Result<&[DrawCommand], String> {
        self.event(InputEvent::KeyDown {
            key: name.to_string(),
        });
        self.event(InputEvent::KeyUp {
            key: name.to_string(),
        });
        self.frame()
    }

    /// Move to (`x`, `y`) and left-click, then run one frame — the frame
    /// that sees the `mouse_pressed` edge. The release edge reaches the
    /// following frame.
    pub fn click(&mut self, x: i32, y: i32) -> Result<&[DrawCommand], String> {
        self.mouse_move(x, y);
        self.mouse_down(input::buttons::LEFT);
        let _ = self.frame()?;
        self.mouse_up(input::buttons::LEFT);
        Ok(&self.commands)
    }

    /// Run one script frame under the standard contract and return its draw
    /// commands (also kept in [`commands`](Self::commands)).
    pub fn frame(&mut self) -> Result<&[DrawCommand], String> {
        self.frame_count += 1;
        self.input.begin_frame(FRAME_DT);
        input::bind_frame_info(&mut self.env, FRAME_DT, self.frame_count);
        input::bind_time(&mut self.env, self.time);
        input::bind_input(&mut self.env, &self.input);
        draw::clear_draw_commands(&mut self.env);
        draw::reset_canvas_ids(&mut self.env);
        self.env.reset_stack(self.stack_id)?;
        // Make the data provider reachable from the `host_data` native for this
        // run, then take it back (with any cache it updated) afterwards.
        let saved = host_data::swap_data_provider(self.provider.take());
        let saved_fonts = draw::swap_font_provider(self.fonts.take());
        let run = self.env.run(self.stack_id);
        self.fonts = draw::swap_font_provider(saved_fonts);
        self.provider = host_data::swap_data_provider(saved);
        // The harness clock moves in lockstep with the fixed `dt` it just
        // published, so animation written against `time()` (the prelude's
        // `spinner`, `elapsed`) actually runs in a headless trace. It advances
        // even if the frame failed: a run's clock stays a function of how many
        // frames were attempted, never of the wall clock.
        self.time += FRAME_DT;
        self.result = run?;
        self.commands = draw::take_draw_commands(&mut self.env);
        Ok(&self.commands)
    }

    /// Run `n` frames with no new input (animation settling, edge decay).
    pub fn frames(&mut self, n: usize) -> Result<(), String> {
        for _ in 0..n {
            self.frame()?;
        }
        Ok(())
    }

    /// All `state` variables as a JSON map keyed by (module-qualified) name.
    pub fn state(&self) -> serde_json::Map<String, serde_json::Value> {
        self.env.get_state_json(self.program_id, self.stack_id)
    }

    /// Convenience: an integer `state` variable by name.
    pub fn state_int(&self, name: &str) -> Option<i64> {
        self.state().get(name)?.as_i64()
    }

    /// Convenience: a float `state` variable by name.
    pub fn state_float(&self, name: &str) -> Option<f64> {
        self.state().get(name)?.as_f64()
    }

    /// Convenience: a string `state` variable by name.
    pub fn state_string(&self, name: &str) -> Option<String> {
        Some(self.state().get(name)?.as_str()?.to_string())
    }
}
