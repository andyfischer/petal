//! The generic desktop game loop, shared by every SDL host.
//!
//! The loop owns *platform policy* — SDL init, the window and canvas, the event
//! pump, frame timing, the agent/headless/screenshot/record entry points, hot
//! reload, and pointer-grab handling — and drives a [`Host`] for the parts that
//! vary between apps: which natives a script can call, how a frame is painted,
//! and how a frame is captured to pixels/JSON. The default binary supplies
//! [`crate::default_host::DefaultHost`] (an SDL-canvas renderer over the
//! `petal-ui` draw vocabulary); other apps (e.g. `petal-fps`) supply their own
//! `Host` and drop all of this scaffolding.
//!
//! Every host follows the same frame contract as the web hosts, so behavior is
//! portable:
//!
//! ```text
//! poll events → input.begin_frame(dt) → bind frame_info/input → [gate]
//!   → env.run → host.end_frame → host.present
//! ```

use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

use image::RgbImage;
use sdl2::render::Canvas;
use sdl2::video::Window;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

use petal_ui::draw::{DrawCommand, clear_draw_commands};
use petal_ui::input::{InputState, bind_frame_info, bind_input, bind_time, dimensions, take_mouse_grab};

use crate::timeline::Timeline;

use crate::input::{Gamepads, poll_sdl_events_with_gamepads};
use crate::protocol::{self, ClockSource, Command, Response};
use crate::watcher::{check_hot_reload, setup_watcher};

/// Window + mode configuration shared by every run entry point.
pub struct GameConfig {
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub hot_reload: bool,
    #[allow(dead_code)]
    pub agent: bool,
    #[allow(dead_code)]
    pub headless: bool,
    /// Record a timeline of forked executions for rewind / scrub / replay
    /// (see [`crate::timeline`]). Needs a host that can present a command
    /// list ([`Host::present_commands`]).
    pub timeline: bool,
    /// How many frames the timeline keeps.
    pub history: usize,
}

impl GameConfig {
    pub fn new(width: u32, height: u32, title: impl Into<String>) -> Self {
        Self {
            width,
            height,
            title: title.into(),
            hot_reload: true,
            agent: false,
            headless: false,
            timeline: true,
            history: DEFAULT_HISTORY,
        }
    }
}

/// Ten seconds at 60 fps.
pub const DEFAULT_HISTORY: usize = 600;

/// A request to (re)load a different script into the running host — the
/// mechanism behind an example browser's "launch" and "return to browser".
pub struct ScriptSwitch {
    pub source: String,
    /// The on-disk path, when the source came from a file (so it can `import`
    /// siblings and hot-reload). `None` for an embedded source (e.g. a browser).
    pub path: Option<String>,
}

/// What the Escape key does in windowed interactive mode.
pub enum EscapeAction {
    Quit,
    Switch(ScriptSwitch),
}

/// The per-app seam. Everything a host must provide to run on this loop; every
/// method beyond the three required ones has an inert default, so a minimal
/// host is small. Hosts must not re-implement the loop, event translation,
/// protocol, or watcher — those live here.
pub trait Host {
    /// Register this host's natives, prelude, and modules into a fresh `Env`.
    /// Called once, before any program is loaded.
    fn register(&mut self, env: &mut Env);

    /// Called immediately after `sdl2::init()`, in the run modes that create an
    /// SDL context at all (windowed interactive and windowed agent). This is
    /// where a host opens subsystems the loop doesn't own — an audio device
    /// ([`crate::audio::AudioOutput`]), haptics, extra windows — without having
    /// to re-implement the loop to get at the `Sdl` handle. The headless,
    /// screenshot, and record modes never init SDL and never call this, so
    /// anything opened here must be optional to the host's operation.
    fn on_sdl_init(&mut self, _sdl: &sdl2::Sdl) {}

    /// Paint the live frame's draw output (drained from `env`'s default draw
    /// buffer) to the window and present. Windowed modes only.
    fn present(&mut self, canvas: &mut Canvas<Window>, env: &mut Env) -> Result<(), String>;

    /// Paint an already-drained command list and present. The timeline drains
    /// the frame itself (it needs the emit attribution alongside the commands)
    /// and hands the host the list plus its overlay, so a host that renders the
    /// `petal-ui` vocabulary implements this and answers `true` from
    /// [`supports_timeline`](Self::supports_timeline). The default declines.
    fn present_commands(
        &mut self,
        _canvas: &mut Canvas<Window>,
        _commands: Vec<DrawCommand>,
    ) -> Result<(), String> {
        Err("this host cannot present a command list".to_string())
    }

    /// Whether [`present_commands`](Self::present_commands) is implemented, which
    /// is what the loop needs to run the timeline on this host.
    fn supports_timeline(&self) -> bool {
        false
    }

    /// Rasterize a command list with no window — a recorded timeline frame plus
    /// its overlay, for the agent `screenshot` command while frozen.
    fn render_commands_image(
        &mut self,
        _commands: &[DrawCommand],
        _width: u32,
        _height: u32,
    ) -> Result<RgbImage, String> {
        Err("this host cannot render a command list".to_string())
    }

    /// Rasterize `stack`'s pending draw output into an RGB image, with no
    /// window — used by `--screenshot`/`--record` and the agent `screenshot`
    /// command. `stack` is a speculative fork; drain it with the host's
    /// vocabulary (`take_draw_commands_for`).
    fn render_image(
        &mut self,
        env: &mut Env,
        stack: StackKey,
        width: u32,
        height: u32,
    ) -> Result<RgbImage, String>;

    /// The program to run when the CLI got no path (e.g. an example browser).
    /// `None` (the default) makes "no source file" a usage error.
    fn default_source(&mut self) -> Option<ScriptSwitch> {
        None
    }

    /// Bind host state after each (re)load of a program — dimensions are
    /// already bound. `path` is the loaded program's path (`None` for embedded
    /// sources). The default host binds text metrics and its example list here.
    fn on_program_loaded(&mut self, _env: &mut Env, _path: Option<&str>) {}

    /// Reset per-frame host bindings right before the script runs (both live
    /// and speculative frames) — e.g. the offscreen-canvas id counter.
    fn prepare_frame(&mut self, _env: &mut Env) {}

    /// Whether the loop may skip a frame the runtime's frame gate says would
    /// reproduce the last one (`Env::run_needed`). On a skipped frame nothing
    /// between `prepare_frame` and `end_frame` runs; `present` is still called
    /// so a vsync'd window keeps its pacing, with an empty draw buffer — the
    /// default host's persistent framebuffer re-blits the last frame. A host
    /// whose `prepare_frame`/`end_frame` pair does per-frame work the script
    /// must feed every frame (a fantasy console pumping audio from the
    /// frame's output) answers `false`.
    fn frame_gating(&self) -> bool {
        true
    }

    /// Serialize `stack`'s pending draw output as JSON for the agent
    /// `capture_draw_commands` response. Default: JSON `null`.
    fn draw_commands_json(&mut self, _env: &mut Env, _stack: StackKey) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// Optional per-frame draw statistics for the agent `draw_stats` command.
    /// Default: `None` (the command reports "unsupported").
    fn draw_stats(&mut self, _env: &mut Env, _stack: StackKey) -> Option<serde_json::Value> {
        None
    }

    /// Windowed interactive Escape behavior. Default: quit the app.
    fn on_escape(&mut self, _env: &mut Env) -> EscapeAction {
        EscapeAction::Quit
    }

    /// After each interactive frame, optionally request a script switch (an
    /// example browser drains its `launch_script` channel here). Default: none.
    fn after_frame(&mut self, _env: &mut Env) -> Option<ScriptSwitch> {
        None
    }

    /// Called once after every *committed* frame, in every run mode — windowed,
    /// windowed-agent, headless `step`, screenshot, and record — so a host can
    /// drain output it accumulated during the script run even when there is no
    /// window and nothing will ever be presented (pushing a frame of audio, for
    /// instance).
    ///
    /// Ordering within a frame:
    ///
    /// ```text
    /// prepare_frame → env.run → drain print output → end_frame → [after_frame → present]
    /// ```
    ///
    /// `present` is windowed-only and runs after this; `end_frame` is the hook
    /// that is guaranteed everywhere. It is *not* called for **speculative**
    /// frames (the forked runs behind `--screenshot`'s final capture and the
    /// agent's `screenshot` / `capture_draw_commands` commands): those exist to
    /// be read and thrown away, and their side effects are discarded with the
    /// fork, so emitting them would double up a frame of a host's output.
    fn end_frame(&mut self, _env: &mut Env) {}
}

/// A loaded program + its stack, path, and file watcher. Threaded through the
/// interactive loop so a browser "launch"/"return" can swap it wholesale.
struct Loaded {
    program_id: ProgramId,
    stack_id: StackKey,
    path: Option<String>,
    reloader: Reloader,
}

/// Owns the hot-reload watcher + its receiver. `poll` is a no-op when disabled
/// or when the program has no on-disk path (an embedded browser can't reload).
struct Reloader {
    rx: mpsc::Receiver<()>,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Reloader {
    fn disabled() -> Self {
        let (_tx, rx) = mpsc::channel();
        Self { rx, _watcher: None }
    }

    fn start(env: &Env, program_id: ProgramId, path: Option<&str>, enabled: bool) -> Self {
        if !enabled {
            return Self::disabled();
        }
        match path {
            Some(p) => {
                let (tx, rx) = mpsc::channel();
                match setup_watcher(env, program_id, p, tx) {
                    Ok(w) => Self { rx, _watcher: w },
                    Err(e) => {
                        eprintln!("[hot-reload] {}", e);
                        Self::disabled()
                    }
                }
            }
            None => Self::disabled(),
        }
    }

    /// Returns `true` when a new program was installed.
    fn poll(
        &self,
        env: &mut Env,
        loaded_program: ProgramId,
        stack_id: StackKey,
        path: Option<&str>,
    ) -> bool {
        match path {
            Some(p) => check_hot_reload(&self.rx, p, env, loaded_program, stack_id),
            None => false,
        }
    }
}

// --- Windowed interactive mode ---

pub fn run_game<H: Host>(
    source_path: Option<&str>,
    config: GameConfig,
    host: &mut H,
) -> Result<(), String> {
    let sdl = sdl2::init()?;
    crate::input::suppress_untranslated_events();
    host.on_sdl_init(&sdl);
    let video = sdl.video()?;

    let window = video
        .window(&config.title, config.width, config.height)
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .build()
        .map_err(|e| e.to_string())?;

    let mut event_pump = sdl.event_pump()?;
    let mut gamepads = Gamepads::new(&sdl);

    let mut env = Env::new();
    host.register(&mut env);
    let mut current = load_initial(&mut env, source_path, &config, host)?;

    let timeline_on = config.timeline && host.supports_timeline();
    let mut timeline = Timeline::new(config.history);
    if timeline_on {
        env.enable_emit_trace(true);
    }

    let mut last_frame = Instant::now();
    // The script's `time()`: simulated seconds, so a rewind rewinds it too.
    let mut sim_time: f64 = 0.0;
    let mut frame_count: i64 = 0;
    let mut input = InputState::default();
    let mut mouse_grabbed = false;

    'game: loop {
        match poll_sdl_events_with_gamepads(&mut event_pump, &mut input, &mut gamepads) {
            crate::input::PollResult::Quit => break 'game,
            crate::input::PollResult::Escape => match host.on_escape(&mut env) {
                EscapeAction::Quit => break 'game,
                EscapeAction::Switch(sw) => {
                    timeline.clear(&mut env);
                    perform_switch(
                        &mut env,
                        sw,
                        &config,
                        host,
                        &mut current,
                        &mut frame_count,
                        &mut last_frame,
                    );
                    sim_time = 0.0;
                    continue;
                }
            },
            crate::input::PollResult::None => {}
        }

        let now = Instant::now();
        let dt = now.duration_since(last_frame).as_secs_f64();
        last_frame = now;
        input.begin_frame(dt);

        if timeline_on {
            handle_timeline_keys(
                &mut timeline,
                &mut env,
                &current,
                &input,
                &mut frame_count,
                &mut sim_time,
            );
        }

        let reloaded = current.reloader.poll(
            &mut env,
            current.program_id,
            current.stack_id,
            current.path.as_deref(),
        );
        if reloaded && timeline_on {
            after_reload(&mut timeline, &mut env, &current, host);
        }

        // Frozen: nothing runs. Present the cursor frame and its overlay.
        if timeline_on && timeline.is_frozen() {
            present_timeline(&mut timeline, &mut env, &current, &mut canvas, host)?;
            continue;
        }

        frame_count += 1;
        sim_time += dt;
        // Advance the ExecutionContext frame so pending-resource ages grow.
        env.advance_frame(current.stack_id);
        bind_frame_info(&mut env, dt, frame_count);
        bind_time(&mut env, sim_time);
        bind_input(&mut env, &input);

        // The frame gate: with every input bound, skip the run if nothing the
        // last run read has changed (the timeline records every frame's
        // commands, so it runs ungated).
        let skip = host.frame_gating() && !timeline_on && !env.run_needed(current.stack_id);
        if !skip {
            clear_draw_commands(&mut env);
            host.prepare_frame(&mut env);
            env.reset_stack(current.stack_id)?;
            if let Err(e) = env.run(current.stack_id) {
                eprintln!("[petal error] {}", e);
            }
            drain_output(&mut env);
            host.end_frame(&mut env);
        }

        // Honor the script's pointer grab/release requests (pointer lock for
        // mouselook). Set once when it changes, so we don't thrash SDL.
        if let Some(want_grab) = take_mouse_grab(&mut env) {
            if want_grab != mouse_grabbed {
                sdl.mouse().set_relative_mouse_mode(want_grab);
                mouse_grabbed = want_grab;
            }
        }

        if let Some(sw) = host.after_frame(&mut env) {
            timeline.clear(&mut env);
            perform_switch(
                &mut env,
                sw,
                &config,
                host,
                &mut current,
                &mut frame_count,
                &mut last_frame,
            );
            sim_time = 0.0;
            continue;
        }

        if timeline_on {
            if let Err(e) =
                timeline.record(&mut env, current.stack_id, frame_count, dt, sim_time, &input)
            {
                eprintln!("[timeline] {}", e);
            }
            present_timeline(&mut timeline, &mut env, &current, &mut canvas, host)?;
        } else {
            host.present(&mut canvas, &mut env)?;
        }
    }

    Ok(())
}

/// The windowed timeline chords, read from the frame's input edges before the
/// script runs: F5 freeze / resume, F6 trail on/off, F7 track the shape under
/// the pointer, and while frozen `,` / `.` scrub (hold for real-time, shift
/// for 5x).
fn handle_timeline_keys(
    timeline: &mut Timeline,
    env: &mut Env,
    current: &Loaded,
    input: &InputState,
    frame_count: &mut i64,
    sim_time: &mut f64,
) {
    if input.was_key_pressed("f5") {
        if timeline.is_frozen() {
            match timeline.unfreeze(env, current.stack_id) {
                Ok(Some((fc, t))) => {
                    *frame_count = fc;
                    *sim_time = t;
                    eprintln!("[timeline] resumed at frame {}", fc);
                }
                Ok(None) => {}
                Err(e) => eprintln!("[timeline] {}", e),
            }
        } else {
            timeline.freeze();
            eprintln!(
                "[timeline] frozen — {} frames recorded; , and . scrub, F5 resumes",
                timeline.len()
            );
        }
    }
    if input.was_key_pressed("f6") {
        timeline.show_trail = !timeline.show_trail;
    }
    if input.was_key_pressed("f7") {
        match timeline.track_at(env, current.program_id, input.mouse_x, input.mouse_y) {
            Some(site) => eprintln!(
                "[timeline] tracking {} at line {}",
                site.callee.as_deref().unwrap_or("<call>"),
                site.span.map_or(0, |s| s.start.line)
            ),
            None => {
                timeline.untrack();
                eprintln!("[timeline] nothing attributable under the pointer");
            }
        }
    }
    if timeline.is_frozen() {
        let step = if input.is_key_down("shift") { 5 } else { 1 };
        if input.is_key_down("comma") {
            timeline.scrub_by(-step);
        }
        if input.is_key_down("period") {
            timeline.scrub_by(step);
        }
    }
}

/// A hot reload landed. Old frames keep their attribution ids; when frozen,
/// re-simulate everything after the cursor through the edited program so the
/// future on screen is the one the new code produces.
fn after_reload<H: Host>(timeline: &mut Timeline, env: &mut Env, current: &Loaded, host: &mut H) {
    timeline.on_reload(env, current.program_id);
    if let Some(c) = timeline.cursor {
        match timeline.replay(env, current.stack_id, c, host) {
            Ok(n) => eprintln!("[timeline] replayed {} frames through the new code", n),
            Err(e) => eprintln!("[timeline] replay failed: {}", e),
        }
    }
}

/// Present the timeline's current frame (the cursor while frozen, else the
/// frame just recorded) plus its overlay.
fn present_timeline<H: Host>(
    timeline: &mut Timeline,
    env: &mut Env,
    current: &Loaded,
    canvas: &mut Canvas<Window>,
    host: &mut H,
) -> Result<(), String> {
    let (w, h) = dimensions(env);
    let mut cmds = timeline
        .current()
        .map(|r| r.commands.clone())
        .unwrap_or_default();
    cmds.extend(timeline.overlay(env, current.program_id, w as i32, h as i32));
    host.present_commands(canvas, cmds)
}

// --- Windowed agent mode (hybrid: interactive window + stdin protocol) ---

pub fn run_agent<H: Host>(
    source_path: Option<&str>,
    config: GameConfig,
    host: &mut H,
) -> Result<(), String> {
    let sdl = sdl2::init()?;
    crate::input::suppress_untranslated_events();
    host.on_sdl_init(&sdl);
    let video = sdl.video()?;

    let window = video
        .window(&config.title, config.width, config.height)
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .build()
        .map_err(|e| e.to_string())?;

    let mut event_pump = sdl.event_pump()?;
    let mut gamepads = Gamepads::new(&sdl);

    let mut env = Env::new();
    host.register(&mut env);
    let current = load_initial(&mut env, source_path, &config, host)?;
    let timeline_on = config.timeline && host.supports_timeline();
    let mut timeline = Timeline::new(config.history);
    if timeline_on {
        env.enable_emit_trace(true);
    }

    let cmd_rx = protocol::spawn_stdin_reader();
    let mut paused = false;
    let mut last_frame = Instant::now();
    let start = Instant::now();
    let mut frame_count: i64 = 0;
    let mut input = InputState::default();

    protocol::send_response(&Response {
        frame: Some(0),
        paused: Some(false),
        ..Response::ok()
    });

    'game: loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            // Share the live loop's real clock so a Step interleaved with the
            // live frames below never rewinds `time()`.
            handle_command(
                cmd,
                &mut env,
                &current,
                &mut paused,
                &mut input,
                &mut frame_count,
                ClockSource::Wall(start),
                host,
                timeline_on.then_some(&mut timeline),
            );
        }

        match poll_sdl_events_with_gamepads(&mut event_pump, &mut input, &mut gamepads) {
            crate::input::PollResult::Quit | crate::input::PollResult::Escape => break 'game,
            crate::input::PollResult::None => {}
        }

        if !paused {
            let now = Instant::now();
            let dt = now.duration_since(last_frame).as_secs_f64();
            last_frame = now;
            frame_count += 1;
            // Advance the ExecutionContext frame so pending-resource ages grow.
            env.advance_frame(current.stack_id);

            input.begin_frame(dt);
            bind_frame_info(&mut env, dt, frame_count);
            bind_time(&mut env, start.elapsed().as_secs_f64());
            let reloaded = current.reloader.poll(
                &mut env,
                current.program_id,
                current.stack_id,
                current.path.as_deref(),
            );
            if reloaded && timeline_on {
                after_reload(&mut timeline, &mut env, &current, host);
            }

            clear_draw_commands(&mut env);
            host.prepare_frame(&mut env);
            bind_input(&mut env, &input);

            env.reset_stack(current.stack_id)?;
            if let Err(e) = env.run(current.stack_id) {
                eprintln!("[petal error] {}", e);
            }
            drain_output(&mut env);
            host.end_frame(&mut env);
            if timeline_on {
                let t = start.elapsed().as_secs_f64();
                if let Err(e) =
                    timeline.record(&mut env, current.stack_id, frame_count, dt, t, &input)
                {
                    eprintln!("[timeline] {}", e);
                }
            }
        }

        if timeline_on {
            // Presents the cursor frame while frozen, the latest otherwise —
            // and the retained frame while merely paused.
            present_timeline(&mut timeline, &mut env, &current, &mut canvas, host)?;
        } else {
            // Always present (shows the retained frame when paused).
            host.present(&mut canvas, &mut env)?;
        }
    }

    Ok(())
}

// --- Headless agent mode: no window, purely protocol-driven ---

pub fn run_headless<H: Host>(
    source_path: Option<&str>,
    config: GameConfig,
    host: &mut H,
) -> Result<(), String> {
    let mut env = Env::new();
    host.register(&mut env);
    let current = load_initial(&mut env, source_path, &config, host)?;
    // Headless has no presenter, so the timeline needs no host support: it
    // only records, rewinds and replays.
    let timeline_on = config.timeline;
    let mut timeline = Timeline::new(config.history);
    if timeline_on {
        env.enable_emit_trace(true);
    }

    let cmd_rx = protocol::spawn_stdin_reader();
    let mut paused = true; // Headless starts paused — the agent drives frames.
    let mut input = InputState::default();
    let mut frame_count: i64 = 0;

    protocol::send_response(&Response {
        frame: Some(0),
        paused: Some(true),
        ..Response::ok()
    });

    loop {
        let cmd = match cmd_rx.recv() {
            Ok(cmd) => cmd,
            Err(_) => break, // stdin closed
        };
        let reloaded = current.reloader.poll(
            &mut env,
            current.program_id,
            current.stack_id,
            current.path.as_deref(),
        );
        if reloaded && timeline_on {
            after_reload(&mut timeline, &mut env, &current, host);
        }
        // Headless is fully scripted (no real-clock loop), so frames step on the
        // deterministic clock for reproducibility.
        handle_command(
            cmd,
            &mut env,
            &current,
            &mut paused,
            &mut input,
            &mut frame_count,
            ClockSource::Fixed,
            host,
            timeline_on.then_some(&mut timeline),
        );
    }

    Ok(())
}

// --- Screenshot mode: run N frames headlessly, save a PNG, exit ---

pub fn run_screenshot<H: Host>(
    source_path: Option<&str>,
    config: GameConfig,
    output_path: &str,
    frames: u32,
    host: &mut H,
) -> Result<(), String> {
    let mut env = Env::new();
    host.register(&mut env);
    let current = load_initial(&mut env, source_path, &config, host)?;

    let mut input = InputState::default();
    let mut frame_count: i64 = 0;
    for _ in 0..frames {
        run_committed_frame(
            &mut env,
            current.stack_id,
            &mut input,
            &mut frame_count,
            host,
        )?;
    }

    let (img, output) = capture_image(
        &mut env,
        current.stack_id,
        &input,
        config.width,
        config.height,
        host,
    )?;
    for line in output {
        eprintln!("{}", line);
    }
    crate::screenshot::save_png(&img, output_path)?;
    eprintln!("Screenshot saved to {}", output_path);
    Ok(())
}

// --- Record mode: write a PNG per frame into a directory (flipbook) ---

pub fn run_record<H: Host>(
    source_path: Option<&str>,
    config: GameConfig,
    out_dir: &str,
    frames: u32,
    warmup: u32,
    host: &mut H,
) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;

    let mut env = Env::new();
    host.register(&mut env);
    let current = load_initial(&mut env, source_path, &config, host)?;

    let mut input = InputState::default();
    let mut frame_count: i64 = 0;
    for _ in 0..warmup {
        run_committed_frame(
            &mut env,
            current.stack_id,
            &mut input,
            &mut frame_count,
            host,
        )?;
    }
    for i in 0..frames {
        run_committed_frame(
            &mut env,
            current.stack_id,
            &mut input,
            &mut frame_count,
            host,
        )?;
        let (img, _) = capture_image(
            &mut env,
            current.stack_id,
            &input,
            config.width,
            config.height,
            host,
        )?;
        let path = format!("{}/frame_{:04}.png", out_dir, i);
        crate::screenshot::save_png(&img, &path)?;
    }
    eprintln!("[record] wrote {} frames to {}", frames, out_dir);
    Ok(())
}

// --- Shared helpers ---

/// Load the initial program: the CLI path when given, else the host's default
/// source (e.g. a browser). Errors if neither is available.
fn load_initial<H: Host>(
    env: &mut Env,
    source_path: Option<&str>,
    config: &GameConfig,
    host: &mut H,
) -> Result<Loaded, String> {
    let switch = match source_path {
        Some(sp) => {
            let source =
                std::fs::read_to_string(sp).map_err(|e| format!("Failed to read {}: {}", sp, e))?;
            ScriptSwitch {
                source,
                path: Some(sp.to_string()),
            }
        }
        None => host
            .default_source()
            .ok_or_else(|| "no source file provided".to_string())?,
    };
    load_switch(env, switch, config, host)
}

/// Load + install a program: compile, create its stack, bind dimensions, let
/// the host bind its per-program state, and start a watcher. Shared by the
/// initial load and every browser switch.
fn load_switch<H: Host>(
    env: &mut Env,
    switch: ScriptSwitch,
    config: &GameConfig,
    host: &mut H,
) -> Result<Loaded, String> {
    let program_id = match &switch.path {
        Some(sp) => env.load_program_at(&switch.source, Path::new(sp))?,
        None => env.load_program(&switch.source)?,
    };
    let stack_id = env.create_stack(program_id)?;
    petal_ui::input::bind_dimensions(env, config.width as i32, config.height as i32);
    bind_frame_info(env, 0.0, 0);
    host.on_program_loaded(env, switch.path.as_deref());

    let reloader = Reloader::start(env, program_id, switch.path.as_deref(), config.hot_reload);
    Ok(Loaded {
        program_id,
        stack_id,
        path: switch.path,
        reloader,
    })
}

/// Perform a browser switch requested by a host hook. On success, swaps in the
/// new program and resets the frame counter/clock; on failure, logs and keeps
/// the current program running (a bad launch must not kill the window).
fn perform_switch<H: Host>(
    env: &mut Env,
    switch: ScriptSwitch,
    config: &GameConfig,
    host: &mut H,
    current: &mut Loaded,
    frame_count: &mut i64,
    last_frame: &mut Instant,
) {
    match load_switch(env, switch, config, host) {
        Ok(next) => {
            *current = next;
            *frame_count = 0;
            *last_frame = Instant::now();
        }
        Err(e) => eprintln!("[browser] switch failed: {}", e),
    }
}

/// Capture a speculative frame as an RGB image (screenshot/record). Sets up the
/// frame's bindings, forks so live state is untouched, and asks the host to
/// rasterize the fork's draw output.
fn capture_image<H: Host>(
    env: &mut Env,
    stack_id: StackKey,
    input: &InputState,
    width: u32,
    height: u32,
    host: &mut H,
) -> Result<(RgbImage, Vec<String>), String> {
    host.prepare_frame(env);
    bind_input(env, input);
    let (img, output) = protocol::with_speculative_frame(env, stack_id, |env, fork| {
        host.render_image(env, fork, width, height)
    })?;
    Ok((img?, output))
}

/// Run one committed frame on the deterministic clock and fire the host's
/// end-of-frame hook. The non-interactive modes (screenshot, record, agent
/// `step`) all go through here so `Host::end_frame` has exactly the same
/// per-frame guarantee it has in the interactive loop.
fn run_committed_frame<H: Host>(
    env: &mut Env,
    stack_id: StackKey,
    input: &mut InputState,
    frame_count: &mut i64,
    host: &mut H,
) -> Result<i64, String> {
    let fc = protocol::run_one_frame(env, stack_id, input, frame_count, ClockSource::Fixed, host)?;
    host.end_frame(env);
    Ok(fc)
}

fn drain_output(env: &mut Env) {
    for line in env.take_output() {
        eprintln!("{}", line);
    }
}

/// Dispatch one agent-protocol command. Shared by windowed-agent and headless.
/// `clock` is the session's single `time()` source — the real monotonic clock
/// in windowed-agent mode (so a `Step` interleaved with the live loop stays
/// monotonic), a deterministic per-frame clock in headless mode.
///
/// `step` and the timeline commands are driven here rather than in `protocol`
/// so that each stepped frame gets the same `Host::end_frame` call a live
/// frame gets and is recorded; everything else is pure protocol and is
/// delegated unchanged.
#[allow(clippy::too_many_arguments)]
fn handle_command<H: Host>(
    cmd: Command,
    env: &mut Env,
    current: &Loaded,
    paused: &mut bool,
    input: &mut InputState,
    frame_count: &mut i64,
    clock: ClockSource,
    host: &mut H,
    timeline: Option<&mut Timeline>,
) {
    // A screenshot of a frozen (or overlay-requested) timeline is the picture
    // the window shows: the recorded frame plus trail and scrub bar.
    if let Command::Screenshot { overlay } = &cmd {
        if let Some(t) = timeline.as_deref() {
            if *overlay || t.is_frozen() {
                let (w, h) = dimensions(env);
                let mut cmds = t.current().map(|r| r.commands.clone()).unwrap_or_default();
                cmds.extend(t.overlay(env, current.program_id, w as i32, h as i32));
                match host.render_commands_image(&cmds, w, h) {
                    Ok(img) => protocol::send_response(&Response {
                        screenshot: Some(crate::screenshot::to_base64(&img)),
                        ..Response::ok()
                    }),
                    Err(e) => protocol::send_response(&Response::err(e)),
                }
                return;
            }
        }
    }
    match cmd {
        Command::Step { n } => step_frames(
            n,
            env,
            current.stack_id,
            input,
            frame_count,
            clock,
            host,
            timeline,
        ),
        Command::Timeline
        | Command::Freeze
        | Command::Unfreeze
        | Command::Scrub { .. }
        | Command::Rewind { .. }
        | Command::Replay
        | Command::Track { .. }
        | Command::Trail => match timeline {
            Some(t) => timeline_command(cmd, t, env, current, paused, frame_count, host),
            None => protocol::send_response(&Response::err(
                "the timeline is off for this session (--no-timeline, or a host without it)"
                    .to_string(),
            )),
        },
        cmd => protocol::handle_command(
            cmd,
            env,
            current.program_id,
            current.stack_id,
            paused,
            input,
            frame_count,
            clock,
            host,
        ),
    }
}

/// The agent-protocol face of the timeline. Every command answers with the
/// timeline's status (`timeline`), plus the command's own field.
fn timeline_command<H: Host>(
    cmd: Command,
    timeline: &mut Timeline,
    env: &mut Env,
    current: &Loaded,
    paused: &mut bool,
    frame_count: &mut i64,
    host: &mut H,
) {
    let mut resp = Response::ok();
    match cmd {
        Command::Timeline => {}
        Command::Freeze => {
            timeline.freeze();
            *paused = true;
            resp.paused = Some(true);
        }
        Command::Unfreeze => match timeline.unfreeze(env, current.stack_id) {
            Ok(Some((fc, _))) => *frame_count = fc,
            Ok(None) => {}
            Err(e) => return protocol::send_response(&Response::err(e)),
        },
        Command::Scrub { to } => {
            if !timeline.is_frozen() {
                timeline.freeze();
                *paused = true;
                resp.paused = Some(true);
            }
            timeline.scrub_to(to);
        }
        Command::Rewind { frames } => match timeline.rewind(env, current.stack_id, frames as usize) {
            Ok(Some((fc, _))) => *frame_count = fc,
            Ok(None) => return protocol::send_response(&Response::err("nothing recorded yet".to_string())),
            Err(e) => return protocol::send_response(&Response::err(e)),
        },
        Command::Replay => {
            let from = timeline.cursor.unwrap_or(1);
            match timeline.replay(env, current.stack_id, from, host) {
                Ok(n) => {
                    resp.replayed = Some(n);
                    // Not frozen: the live execution is now the replayed end.
                    if timeline.cursor.is_none() {
                        if let Some(rec) = timeline.current() {
                            *frame_count = rec.frame_count;
                        }
                    }
                }
                Err(e) => return protocol::send_response(&Response::err(e)),
            }
        }
        Command::Track { x, y } => match timeline.track_at(env, current.program_id, x, y) {
            Some(site) => {
                resp.site = Some(serde_json::json!({
                    "callee": site.callee,
                    "line": site.span.map(|s| s.start.line),
                    "column": site.span.map(|s| s.start.column),
                    "term": site.term.0,
                }));
            }
            None => {
                timeline.untrack();
                return protocol::send_response(&Response::err(
                    "nothing attributable under that point".to_string(),
                ));
            }
        },
        Command::Trail => {
            let points = timeline.trail(env, current.program_id);
            resp.trail = Some(serde_json::to_value(points).unwrap_or(serde_json::Value::Null));
        }
        _ => unreachable!("not a timeline command"),
    }
    resp.frame = Some(*frame_count);
    resp.timeline = Some(timeline.status_json());
    protocol::send_response(&resp);
}

/// The agent `step` command: run `n` committed frames, then reply with the
/// final frame number and everything the script printed across them.
#[allow(clippy::too_many_arguments)]
fn step_frames<H: Host>(
    n: u32,
    env: &mut Env,
    stack_id: StackKey,
    input: &mut InputState,
    frame_count: &mut i64,
    clock: ClockSource,
    host: &mut H,
    mut timeline: Option<&mut Timeline>,
) {
    let mut last_frame = 0i64;
    // A step while frozen resumes from the cursor first, so stepping out of a
    // rewound moment continues from that moment.
    if let Some(t) = timeline.as_deref_mut() {
        match t.unfreeze(env, stack_id) {
            Ok(Some((fc, _))) => *frame_count = fc,
            Ok(None) => {}
            Err(e) => {
                protocol::send_response(&Response::err(e));
                return;
            }
        }
    }
    for _ in 0..n {
        match protocol::run_one_frame(env, stack_id, input, frame_count, clock, host) {
            Ok(fc) => last_frame = fc,
            Err(e) => {
                protocol::send_response(&Response::err(e));
                return;
            }
        }
        host.end_frame(env);
        if let Some(t) = timeline.as_deref_mut() {
            let time = clock.now(*frame_count);
            if let Err(e) = t.record(env, stack_id, *frame_count, 1.0 / 60.0, time, input) {
                eprintln!("[timeline] {}", e);
            }
        }
    }
    let output = env.take_output();
    protocol::send_response(&Response {
        frame: Some(last_frame),
        output: if output.is_empty() {
            None
        } else {
            Some(output)
        },
        ..Response::ok()
    });
}
