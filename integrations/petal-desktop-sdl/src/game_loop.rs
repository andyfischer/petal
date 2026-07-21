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
//! poll events → input.begin_frame(dt) → bind frame_info/input → env.run → host.present
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

use petal_ui::draw::clear_draw_commands;
use petal_ui::input::{InputState, bind_frame_info, bind_input, bind_time, take_mouse_grab};

use crate::input::poll_sdl_events;
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
}

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

    /// Paint the live frame's draw output (drained from `env`'s default draw
    /// buffer) to the window and present. Windowed modes only.
    fn present(&mut self, canvas: &mut Canvas<Window>, env: &mut Env) -> Result<(), String>;

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

    fn poll(
        &self,
        env: &mut Env,
        loaded_program: ProgramId,
        stack_id: StackKey,
        path: Option<&str>,
    ) {
        if let Some(p) = path {
            check_hot_reload(&self.rx, p, env, loaded_program, stack_id);
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

    let mut env = Env::new();
    host.register(&mut env);
    let mut current = load_initial(&mut env, source_path, &config, host)?;

    let mut last_frame = Instant::now();
    let start = Instant::now();
    let mut frame_count: i64 = 0;
    let mut input = InputState::default();
    let mut mouse_grabbed = false;

    'game: loop {
        match poll_sdl_events(&mut event_pump, &mut input) {
            crate::input::PollResult::Quit => break 'game,
            crate::input::PollResult::Escape => match host.on_escape(&mut env) {
                EscapeAction::Quit => break 'game,
                EscapeAction::Switch(sw) => {
                    perform_switch(
                        &mut env,
                        sw,
                        &config,
                        host,
                        &mut current,
                        &mut frame_count,
                        &mut last_frame,
                    );
                    continue;
                }
            },
            crate::input::PollResult::None => {}
        }

        let now = Instant::now();
        let dt = now.duration_since(last_frame).as_secs_f64();
        last_frame = now;
        frame_count += 1;
        // Advance the ExecutionContext frame so pending-resource ages grow.
        env.advance_frame(current.stack_id);

        input.begin_frame(dt);
        bind_frame_info(&mut env, dt, frame_count);
        bind_time(&mut env, start.elapsed().as_secs_f64());

        current.reloader.poll(
            &mut env,
            current.program_id,
            current.stack_id,
            current.path.as_deref(),
        );

        clear_draw_commands(&mut env);
        host.prepare_frame(&mut env);
        bind_input(&mut env, &input);

        env.reset_stack(current.stack_id)?;
        if let Err(e) = env.run(current.stack_id) {
            eprintln!("[petal error] {}", e);
        }
        drain_output(&mut env);

        // Honor the script's pointer grab/release requests (pointer lock for
        // mouselook). Set once when it changes, so we don't thrash SDL.
        if let Some(want_grab) = take_mouse_grab(&mut env) {
            if want_grab != mouse_grabbed {
                sdl.mouse().set_relative_mouse_mode(want_grab);
                mouse_grabbed = want_grab;
            }
        }

        if let Some(sw) = host.after_frame(&mut env) {
            perform_switch(
                &mut env,
                sw,
                &config,
                host,
                &mut current,
                &mut frame_count,
                &mut last_frame,
            );
            continue;
        }

        host.present(&mut canvas, &mut env)?;
    }

    Ok(())
}

// --- Windowed agent mode (hybrid: interactive window + stdin protocol) ---

pub fn run_agent<H: Host>(
    source_path: Option<&str>,
    config: GameConfig,
    host: &mut H,
) -> Result<(), String> {
    let sdl = sdl2::init()?;
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

    let mut env = Env::new();
    host.register(&mut env);
    let current = load_initial(&mut env, source_path, &config, host)?;

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
            );
        }

        match poll_sdl_events(&mut event_pump, &mut input) {
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
            current.reloader.poll(
                &mut env,
                current.program_id,
                current.stack_id,
                current.path.as_deref(),
            );

            clear_draw_commands(&mut env);
            host.prepare_frame(&mut env);
            bind_input(&mut env, &input);

            env.reset_stack(current.stack_id)?;
            if let Err(e) = env.run(current.stack_id) {
                eprintln!("[petal error] {}", e);
            }
            drain_output(&mut env);
        }

        // Always present (shows the retained frame when paused).
        host.present(&mut canvas, &mut env)?;
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
        current.reloader.poll(
            &mut env,
            current.program_id,
            current.stack_id,
            current.path.as_deref(),
        );
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
        protocol::run_one_frame(
            &mut env,
            current.stack_id,
            &mut input,
            &mut frame_count,
            ClockSource::Fixed,
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
        protocol::run_one_frame(
            &mut env,
            current.stack_id,
            &mut input,
            &mut frame_count,
            ClockSource::Fixed,
            host,
        )?;
    }
    for i in 0..frames {
        protocol::run_one_frame(
            &mut env,
            current.stack_id,
            &mut input,
            &mut frame_count,
            ClockSource::Fixed,
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

fn drain_output(env: &mut Env) {
    for line in env.take_output() {
        eprintln!("{}", line);
    }
}

/// Dispatch one agent-protocol command. Shared by windowed-agent and headless.
/// `clock` is the session's single `time()` source — the real monotonic clock
/// in windowed-agent mode (so a `Step` interleaved with the live loop stays
/// monotonic), a deterministic per-frame clock in headless mode.
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
) {
    protocol::handle_command(
        cmd,
        env,
        current.program_id,
        current.stack_id,
        paused,
        input,
        frame_count,
        clock,
        host,
    );
}
