//! A headless driver for testing widget logic with no renderer attached.
//!
//! A thin wrapper over [`crate::frame_core::FrameCore`], the same frame core
//! Garden's panels run on (bind input → gate → reset → run → drain), so
//! behavior verified here matches what a real embedder sees. Time advances only through [`Headless::frame`]'s fixed `dt`,
//! making multi-click and animation tests deterministic.
//!
//! The *gate* is the runtime's frame gate ([`petal::env::Env::run_needed`]):
//! a frame whose inputs are exactly what the last run read, and whose last run
//! reached a fixed point, is skipped and its output retained. Skipping is
//! invisible to a correct script — the retained commands are what a run would
//! have produced — and the `replay` or `baseline` run policy
//! ([`petal::policy::RunPolicy`], set with `ui.set_policy`) turns it off for
//! tests that want every frame to execute regardless.
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

use std::ops::{Deref, DerefMut};

use petal::env::Env;

use crate::draw::{self, DrawCommand};
use crate::frame_core::FrameCore;
use crate::input::{self, InputEvent};

/// Fixed per-frame dt (60 fps) so tests are deterministic.
pub const FRAME_DT: f64 = 1.0 / 60.0;

/// A test driver over the shared [`FrameCore`]: the core owns the env, the
/// stack, input, the gate and memo counters, providers, and the seed; this
/// wrapper adds a deterministic fixed-`dt` clock and keeps each frame's draw
/// commands. It dereferences to the core, so `ui.env`, `ui.set_policy(…)`,
/// `ui.frames_run`, `ui.set_seed(…)` and the rest read as fields of the
/// harness.
pub struct Headless {
    core: FrameCore,
    frame_count: i64,
    /// Absolute clock (seconds) published to the script as `time()` each frame.
    ///
    /// It starts at `t0 = 0.0` and [`frame`](Self::frame) recomputes it from
    /// the frame count after every frame, so the clock is a pure function of
    /// that count (`time == frames_run * FRAME_DT`) and never reads the system
    /// clock — a script that sums `dt()` sees exactly `time()`. It is
    /// *computed*, not accumulated: repeatedly adding `FRAME_DT` drifts (it
    /// reaches 1.0000000000000013 after 60 frames), which would make the
    /// identity above false and a long trace depend on how it was reached.
    /// Assigning to it still works: the value assigned is what the *next*
    /// frame publishes, and the automatic advance resumes from there — the
    /// assignment simply becomes the new origin the multiplication counts
    /// from.
    pub time: f64,
    /// The clock's origin: `time` was last set to `time_origin` when
    /// `frame_count` was `origin_frame`. Both move only when the embedder
    /// assigns [`time`](Self::time) (see [`frame`](Self::frame)).
    time_origin: f64,
    origin_frame: i64,
    /// Draw commands produced by the most recent [`frame`](Self::frame).
    pub commands: Vec<DrawCommand>,
}

impl Deref for Headless {
    type Target = FrameCore;
    fn deref(&self) -> &FrameCore {
        &self.core
    }
}

impl DerefMut for Headless {
    fn deref_mut(&mut self) -> &mut FrameCore {
        &mut self.core
    }
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
        let mut core = FrameCore::new(env, program_id, stack_id);
        core.set_clock(0.0);
        Ok(Self {
            core,
            frame_count: 0,
            time: 0.0,
            time_origin: 0.0,
            origin_frame: 0,
            commands: Vec::new(),
        })
    }

    /// The shared frame core this harness wraps.
    pub fn core(&self) -> &FrameCore {
        &self.core
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
        self.core.set_font_source(fonts);
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

    /// The clock reading after `frames` frames have run, counted from the
    /// current origin. One multiplication rather than `frames` additions, so
    /// the reading carries no accumulated rounding error.
    fn clock_at(&self, frames: i64) -> f64 {
        self.time_origin + (frames - self.origin_frame) as f64 * FRAME_DT
    }

    /// Run one script frame under the standard contract and return its draw
    /// commands (also kept in [`commands`](Self::commands)).
    pub fn frame(&mut self) -> Result<&[DrawCommand], String> {
        // An embedder that assigned `time` since the last frame (tests that
        // jump the clock past a tooltip delay or a fade) re-anchors it: the
        // value it wrote is published now and becomes the origin the frames
        // after it count from.
        if self.time != self.clock_at(self.frame_count) {
            self.time_origin = self.time;
            self.origin_frame = self.frame_count;
        }
        self.frame_count += 1;
        self.core.set_clock(self.time);
        let run = self.core.frame(FRAME_DT, self.frame_count, &mut ());
        // The harness clock moves in lockstep with the fixed `dt` it just
        // published, so animation written against `time()` (the prelude's
        // `spinner`, `elapsed`) actually runs in a headless trace. It advances
        // on a skipped frame and even if the frame failed: a run's clock stays
        // a function of how many frames were attempted, never of the wall
        // clock.
        self.time = self.clock_at(self.frame_count);
        if run? == crate::frame_core::FrameRun::Ran {
            self.commands = draw::take_draw_commands(&mut self.core.env);
        }
        Ok(&self.commands)
    }

    /// Run `n` frames with no new input (animation settling, edge decay).
    pub fn frames(&mut self, n: usize) -> Result<(), String> {
        for _ in 0..n {
            self.frame()?;
        }
        Ok(())
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
