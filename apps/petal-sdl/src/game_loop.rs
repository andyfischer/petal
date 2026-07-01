use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use notify::{RecursiveMode, Watcher};
use sdl2::event::Event;
use sdl2::keyboard::Scancode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::render::Canvas;
use sdl2::surface::Surface;
use sdl2::ttf::Font;
use sdl2::video::Window;

use crate::commands::DrawCommand;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

use crate::commands::{clear_draw_commands, take_draw_commands};
use crate::input::{mods_from_sdl, scancode_to_name, sdl_button_to_std, InputEvent, InputState};
use crate::native_fns::{
    self, bind_dimensions, bind_examples, bind_frame_info, bind_input, reset_canvas_ids,
    take_pending_launch, ExampleEntry,
};
use crate::protocol::{self, Command, Response};
use crate::renderer;
use crate::screenshot;

pub struct GameConfig {
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub hot_reload: bool,
    #[allow(dead_code)]
    pub agent: bool,
    #[allow(dead_code)]
    pub headless: bool,
    /// When set, enables browser mode: populate BROWSER_STATE from this directory.
    pub examples_dir: Option<PathBuf>,
}

const BROWSER_SCRIPT: &str = include_str!("../examples/browser.ptl");

enum PollResult {
    None,
    Quit,
    Escape,
}

/// Poll SDL events, translating them into standard `petal_ui` input events.
/// Returns Quit/Escape signals. The caller starts the script frame afterwards
/// with `input.begin_frame(dt)`.
fn poll_sdl_events(event_pump: &mut sdl2::EventPump, input: &mut InputState) -> PollResult {
    let mut result = PollResult::None;
    for event in event_pump.poll_iter() {
        match event {
            Event::Quit { .. } => return PollResult::Quit,
            Event::KeyDown { scancode: Some(sc), .. } if sc == Scancode::Escape => {
                result = PollResult::Escape;
            }
            // OS auto-repeats are dropped: `key_pressed` fires once per
            // physical press, matching the pre-petal-ui behavior.
            Event::KeyDown { scancode: Some(sc), keymod, repeat: false, .. } => {
                input.event(InputEvent::Modifiers(mods_from_sdl(keymod)));
                if let Some(name) = scancode_to_name(sc) {
                    input.event(InputEvent::KeyDown { key: name.to_string() });
                }
            }
            Event::KeyUp { scancode: Some(sc), keymod, .. } => {
                input.event(InputEvent::Modifiers(mods_from_sdl(keymod)));
                if let Some(name) = scancode_to_name(sc) {
                    input.event(InputEvent::KeyUp { key: name.to_string() });
                }
            }
            Event::TextInput { text, .. } => {
                input.event(InputEvent::Text { text });
            }
            Event::MouseMotion { x, y, .. } => {
                input.event(InputEvent::MouseMove { x, y });
            }
            Event::MouseButtonDown { mouse_btn, .. } => {
                if let Some(button) = sdl_button_to_std(mouse_btn) {
                    input.event(InputEvent::MouseDown { button });
                }
            }
            Event::MouseButtonUp { mouse_btn, .. } => {
                if let Some(button) = sdl_button_to_std(mouse_btn) {
                    input.event(InputEvent::MouseUp { button });
                }
            }
            Event::MouseWheel { precise_x, precise_y, .. } => {
                // SDL y > 0 means "scrolled up"; the standard scroll_y() is
                // positive scrolling down.
                input.event(InputEvent::Scroll {
                    dx: precise_x as f64,
                    dy: -precise_y as f64,
                });
            }
            _ => {}
        }
    }
    result
}

/// Scan a directory for example `.ptl` scripts (excluding the browser itself),
/// returning display-name/path entries sorted by path.
fn load_examples(examples_dir: &Path) -> Vec<ExampleEntry> {
    let mut entries: Vec<ExampleEntry> = Vec::new();
    if let Ok(mut dir_entries) = std::fs::read_dir(examples_dir) {
        let mut paths: Vec<PathBuf> = Vec::new();
        while let Some(Ok(entry)) = dir_entries.next() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "ptl") {
                let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                if name != "browser" {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        for path in paths {
            let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            // Capitalize first letter
            let display_name = {
                let mut c = name.chars();
                match c.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                }
            };
            entries.push(ExampleEntry {
                name: display_name,
                path: path.to_string_lossy().to_string(),
            });
        }
    }
    entries
}

pub fn run_game(source_path: Option<&str>, config: GameConfig) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let ttf = sdl2::ttf::init().map_err(|e| e.to_string())?;

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
    let font = load_font(&ttf, 24)?;

    // Persistent software framebuffer: pixels accumulate across frames unless a
    // `clear()` (DrawCommand::Clear) wipes them. Sized to the drawable area.
    let (fb_w, fb_h) = canvas.output_size()?;
    let mut framebuffer = Some(new_framebuffer(fb_w, fb_h)?);

    let mut in_browser = source_path.is_none();
    let has_browser = config.examples_dir.is_some();

    let (mut env, mut program_id, mut stack_id) = init_petal(source_path, &config)?;

    let mut current_source_path: Option<String> = source_path.map(|s| s.to_string());

    let (mut _reload_tx, mut reload_rx) = mpsc::channel();
    let mut _watcher: Option<notify::RecommendedWatcher> = None;
    if config.hot_reload {
        if let Some(ref sp) = current_source_path {
            let (tx, rx) = mpsc::channel();
            _reload_tx = tx.clone();
            reload_rx = rx;
            _watcher = setup_watcher(&env, program_id, sp, tx)?;
        }
    }

    let mut last_frame = Instant::now();
    let mut frame_count: i64 = 0;
    let mut input = InputState::default();

    'game: loop {
        match poll_sdl_events(&mut event_pump, &mut input) {
            PollResult::Quit => break 'game,
            PollResult::Escape if in_browser || !has_browser => break 'game,
            PollResult::Escape => {
                // In game mode with browser available, return to browser
                match switch_script(&mut env, BROWSER_SCRIPT, None, &config) {
                    Ok((pid, sid)) => {
                        program_id = pid;
                        stack_id = sid;
                        in_browser = true;
                        current_source_path = None;
                        _watcher = None;
                        frame_count = 0;
                        last_frame = Instant::now();
                    }
                    Err(e) => eprintln!("[browser] failed to return to browser: {}", e),
                }
                continue;
            }
            PollResult::None => {}
        }

        let now = Instant::now();
        let dt = now.duration_since(last_frame).as_secs_f64();
        last_frame = now;
        frame_count += 1;

        input.begin_frame(dt);
        bind_frame_info(&mut env, dt, frame_count);

        if let Some(ref sp) = current_source_path {
            check_hot_reload(&reload_rx, sp, &mut env, program_id, stack_id);
        }

        clear_draw_commands(&mut env);
        reset_canvas_ids(&mut env);
        bind_input(&mut env, &input);

        env.reset_stack(stack_id)?;
        if let Err(e) = env.run(stack_id) {
            eprintln!("[petal error] {}", e);
        }

        drain_output(&mut env);

        // Check if browser wants to launch a script
        let pending = take_pending_launch(&mut env);
        if let Some(script_path) = pending {
            match std::fs::read_to_string(&script_path) {
                Ok(source) => {
                    match switch_script(&mut env, &source, Some(&script_path), &config) {
                        Ok((pid, sid)) => {
                            program_id = pid;
                            stack_id = sid;
                            in_browser = false;
                            current_source_path = Some(script_path.clone());
                            frame_count = 0;
                            last_frame = Instant::now();
                            // Set up file watcher for new script
                            if config.hot_reload {
                                let (tx, rx) = mpsc::channel();
                                _reload_tx = tx.clone();
                                reload_rx = rx;
                                _watcher =
                                    setup_watcher(&env, program_id, &script_path, tx)
                                        .unwrap_or(None);
                            }
                        }
                        Err(e) => eprintln!("[browser] failed to launch {}: {}", script_path, e),
                    }
                }
                Err(e) => eprintln!("[browser] failed to read {}: {}", script_path, e),
            }
        }

        let commands = take_draw_commands(&mut env);
        let surface = framebuffer.take().expect("framebuffer present");
        framebuffer = Some(present_frame(&mut canvas, surface, commands, &font)?);
    }

    Ok(())
}

fn switch_script(
    env: &mut Env,
    source: &str,
    source_path: Option<&str>,
    config: &GameConfig,
) -> Result<(ProgramId, StackKey), String> {
    // Loading with the script's path lets it `import` sibling .ptl files.
    let program_id = match source_path {
        Some(sp) => env.load_program_at(source, Path::new(sp))?,
        None => env.load_program(source)?,
    };
    let stack_id = env.create_stack(program_id)?;
    bind_dimensions(env, config.width as i32, config.height as i32);
    bind_frame_info(env, 0.0, 0);
    Ok((program_id, stack_id))
}

/// Agent mode with SDL window (hybrid): game runs interactively,
/// LLM can pause/resume/step/inspect via stdin protocol.
pub fn run_agent(source_path: Option<&str>, config: GameConfig) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let ttf = sdl2::ttf::init().map_err(|e| e.to_string())?;

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
    let font = load_font(&ttf, 24)?;

    // Persistent software framebuffer (see run_game): accumulates across frames
    // unless `clear()` wipes it.
    let (fb_w, fb_h) = canvas.output_size()?;
    let mut framebuffer = Some(new_framebuffer(fb_w, fb_h)?);

    let (mut env, program_id, stack_id) = init_petal(source_path, &config)?;

    let (reload_tx, reload_rx) = mpsc::channel();
    let _watcher = if config.hot_reload {
        match source_path {
            Some(sp) => setup_watcher(&env, program_id, sp, reload_tx)?,
            None => None,
        }
    } else {
        None
    };

    let cmd_rx = protocol::spawn_stdin_reader();
    let mut paused = false;
    let mut last_frame = Instant::now();
    let mut frame_count: i64 = 0;
    let mut input = InputState::default();

    // Signal ready
    protocol::send_response(&Response {
        frame: Some(0),
        paused: Some(false),
        ..Response::ok()
    });

    'game: loop {
        // Process all pending protocol commands
        while let Ok(cmd) = cmd_rx.try_recv() {
            handle_command(
                cmd,
                &mut env,
                program_id,
                stack_id,
                &mut paused,
                &mut input,
                &mut frame_count,
            );
        }

        // Poll SDL events (always, even when paused, to keep window responsive)
        match poll_sdl_events(&mut event_pump, &mut input) {
            PollResult::Quit | PollResult::Escape => break 'game,
            PollResult::None => {}
        }

        if !paused {
            let now = Instant::now();
            let dt = now.duration_since(last_frame).as_secs_f64();
            last_frame = now;
            frame_count += 1;

            input.begin_frame(dt);
            bind_frame_info(&mut env, dt, frame_count);

            if let Some(sp) = source_path {
                check_hot_reload(&reload_rx, sp, &mut env, program_id, stack_id);
            }

            clear_draw_commands(&mut env);
            reset_canvas_ids(&mut env);
            bind_input(&mut env, &input);

            env.reset_stack(stack_id)?;
            if let Err(e) = env.run(stack_id) {
                eprintln!("[petal error] {}", e);
            }

            drain_output(&mut env);
        }

        // Always render (shows last frame when paused). When paused, no new
        // commands are produced, so present_frame re-blits the retained surface.
        let commands = take_draw_commands(&mut env);
        let surface = framebuffer.take().expect("framebuffer present");
        framebuffer = Some(present_frame(&mut canvas, surface, commands, &font)?);
    }

    Ok(())
}

/// Headless agent mode: no SDL window, purely protocol-driven.
pub fn run_headless(source_path: Option<&str>, config: GameConfig) -> Result<(), String> {
    let (mut env, program_id, stack_id) = init_petal(source_path, &config)?;

    let (reload_tx, reload_rx) = mpsc::channel();
    let _watcher = if config.hot_reload {
        match source_path {
            Some(sp) => setup_watcher(&env, program_id, sp, reload_tx)?,
            None => None,
        }
    } else {
        None
    };

    let cmd_rx = protocol::spawn_stdin_reader();
    let mut paused = true; // Headless starts paused — LLM drives frames
    let mut input = InputState::default();
    let mut frame_count: i64 = 0;

    // Signal ready
    protocol::send_response(&Response {
        frame: Some(0),
        paused: Some(true),
        ..Response::ok()
    });

    loop {
        // Block waiting for commands (no render loop to drive)
        let cmd = match cmd_rx.recv() {
            Ok(cmd) => cmd,
            Err(_) => break, // stdin closed
        };

        if let Some(sp) = source_path {
            check_hot_reload(&reload_rx, sp, &mut env, program_id, stack_id);
        }

        handle_command(
            cmd,
            &mut env,
            program_id,
            stack_id,
            &mut paused,
            &mut input,
            &mut frame_count,
        );
    }

    Ok(())
}

/// Screenshot mode: run N frames headlessly, save a PNG, exit.
pub fn run_screenshot(
    source_path: Option<&str>,
    config: GameConfig,
    output_path: &str,
    frames: u32,
) -> Result<(), String> {
    let (mut env, _program_id, stack_id) = init_petal(source_path, &config)?;

    let mut input = InputState::default();
    let mut frame_count: i64 = 0;
    for _ in 0..frames {
        protocol::run_one_frame(&mut env, stack_id, &mut input, &mut frame_count)?;
    }

    // Capture draw commands from one more speculative frame
    let commands = match protocol::capture_draw_commands(&mut env, stack_id, &input) {
        Ok((cmds, _)) => cmds,
        Err(e) => return Err(e),
    };

    screenshot::save_png(&commands, config.width, config.height, output_path)?;
    eprintln!("Screenshot saved to {}", output_path);
    Ok(())
}

// --- Shared helpers ---

/// Initialize the Petal env from a source file path, or from the embedded
/// browser script when `source_path` is None (browser mode).
/// When `config.examples_dir` is set, populates BROWSER_STATE.
fn init_petal(
    source_path: Option<&str>,
    config: &GameConfig,
) -> Result<(Env, ProgramId, StackKey), String> {
    let mut env = Env::new();
    native_fns::register_all(&mut env);

    if let Some(ref dir) = config.examples_dir {
        let examples = load_examples(dir);
        bind_examples(&mut env, &examples);
    }

    let source = match source_path {
        Some(sp) => std::fs::read_to_string(sp)
            .map_err(|e| format!("Failed to read {}: {}", sp, e))?,
        None => BROWSER_SCRIPT.to_string(),
    };

    // Loading with the script's path lets it `import` sibling .ptl files.
    let program_id = match source_path {
        Some(sp) => env.load_program_at(&source, Path::new(sp))?,
        None => env.load_program(&source)?,
    };
    let stack_id = env.create_stack(program_id)?;

    bind_dimensions(&mut env, config.width as i32, config.height as i32);

    Ok((env, program_id, stack_id))
}

fn handle_command(
    cmd: Command,
    env: &mut Env,
    program_id: ProgramId,
    stack_id: StackKey,
    paused: &mut bool,
    input: &mut InputState,
    frame_count: &mut i64,
) {
    match cmd {
        Command::Pause => {
            *paused = true;
            protocol::send_response(&Response {
                paused: Some(true),
                ..Response::ok()
            });
        }
        Command::Resume => {
            *paused = false;
            protocol::send_response(&Response {
                paused: Some(false),
                ..Response::ok()
            });
        }
        Command::Step { n } => {
            let mut last_frame = 0i64;
            for _ in 0..n {
                match protocol::run_one_frame(env, stack_id, input, frame_count) {
                    Ok(fc) => last_frame = fc,
                    Err(e) => {
                        protocol::send_response(&Response::err(e));
                        return;
                    }
                }
            }
            let output = env.take_output();
            protocol::send_response(&Response {
                frame: Some(last_frame),
                output: if output.is_empty() { None } else { Some(output) },
                ..Response::ok()
            });
        }
        Command::State => {
            let state = protocol::get_state_json(env, program_id, stack_id);
            protocol::send_response(&Response {
                state: Some(state),
                ..Response::ok()
            });
        }
        Command::CaptureDrawCommands => {
            match protocol::capture_draw_commands(env, stack_id, input) {
                Ok((commands, output)) => {
                    protocol::send_response(&Response {
                        draw_commands: Some(commands),
                        output: if output.is_empty() { None } else { Some(output) },
                        ..Response::ok()
                    });
                }
                Err(e) => {
                    protocol::send_response(&Response::err(e));
                }
            }
        }
        Command::Input { keys_down, mouse } => {
            protocol::apply_input(input, &keys_down, mouse.as_ref());
            protocol::send_response(&Response::ok());
        }
        Command::SetState { name, value } => {
            match protocol::set_state_from_json(env, program_id, stack_id, &name, &value) {
                Ok(()) => protocol::send_response(&Response::ok()),
                Err(e) => protocol::send_response(&Response::err(e)),
            }
        }
        Command::Screenshot => {
            match protocol::capture_draw_commands(env, stack_id, input) {
                Ok((commands, _output)) => {
                    let (w, h) = native_fns::dimensions(env);
                    let b64 = screenshot::render_to_png_base64(&commands, w, h);
                    protocol::send_response(&Response {
                        screenshot: Some(b64),
                        ..Response::ok()
                    });
                }
                Err(e) => {
                    protocol::send_response(&Response::err(e));
                }
            }
        }
    }
}

/// Create a persistent software framebuffer the size of the window, cleared to
/// black. Pixels in this surface survive across frames; only a
/// `DrawCommand::Clear` wipes it, which is what makes accumulative generative
/// art work (a frame that never calls `clear()` keeps the previous frame).
fn new_framebuffer(width: u32, height: u32) -> Result<Surface<'static>, String> {
    let mut surface =
        Surface::new(width, height, PixelFormatEnum::RGB888).map_err(|e| e.to_string())?;
    surface
        .fill_rect(None, Color::RGB(0, 0, 0))
        .map_err(|e| e.to_string())?;
    Ok(surface)
}

/// Render this frame's commands into the persistent `surface`, then blit the
/// whole surface to the window and present. The surface is passed by value and
/// returned so the caller threads the same (retained) framebuffer through the
/// loop. Because the surface is software-backed, pixels persist unless a
/// `Clear` command wipes them.
fn present_frame(
    canvas: &mut Canvas<Window>,
    surface: Surface<'static>,
    commands: Vec<DrawCommand>,
    font: &Font,
) -> Result<Surface<'static>, String> {
    // Render into the persistent surface (software canvas).
    let mut sc = surface.into_canvas().map_err(|e| e.to_string())?;
    renderer::render(&mut sc, commands, font);
    let surface = sc.into_surface();

    // Upload the surface to the window as a texture and present. The full-surface
    // blit overwrites the whole window, so no window clear is needed. `copy` with
    // `None, None` scales the surface to the window if sizes differ (resizable).
    let tc = canvas.texture_creator();
    let tex = tc
        .create_texture_from_surface(&surface)
        .map_err(|e| e.to_string())?;
    canvas.copy(&tex, None, None)?;
    canvas.present();

    Ok(surface)
}

fn drain_output(env: &mut Env) {
    let output = env.take_output();
    for line in output {
        eprintln!("{}", line);
    }
}

fn check_hot_reload(
    reload_rx: &mpsc::Receiver<()>,
    source_path: &str,
    env: &mut Env,
    program_id: ProgramId,
    stack_id: StackKey,
) {
    if let Ok(()) = reload_rx.try_recv() {
        if let Ok(new_source) = std::fs::read_to_string(source_path) {
            let new_program = match env.compile_program_at(
                program_id,
                &new_source,
                Path::new(source_path),
            ) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[hot-reload] compile error: {}", e);
                    return;
                }
            };
            match env.transfer_state(stack_id, new_program) {
                Ok(result) => {
                    eprintln!(
                        "[hot-reload] preserved: {}, dropped: {}",
                        result.state_preserved, result.state_dropped
                    );
                }
                Err(e) => {
                    eprintln!("[hot-reload] error: {}", e);
                }
            }
        }
    }
}

/// Watch every directory the program's source files live in — the entry
/// script's directory plus the directory of each imported module in the
/// program's module manifest (`Env::module_manifest`). Editing an imported
/// `palette.ptl` hot-reloads the scripts that import it, not just edits to
/// the entry file. Directories are watched (non-recursively), matching the
/// original entry-file behavior; any modify event triggers a reload check.
fn setup_watcher(
    env: &Env,
    program_id: ProgramId,
    source_path: &str,
    tx: mpsc::Sender<()>,
) -> Result<Option<notify::RecommendedWatcher>, String> {
    let path = Path::new(source_path)
        .canonicalize()
        .map_err(|e| format!("Failed to resolve path: {}", e))?;

    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
        if let Ok(event) = res {
            if event.kind.is_modify() {
                let _ = tx.send(());
            }
        }
    })
    .map_err(|e| format!("Failed to create watcher: {}", e))?;

    let mut dirs: Vec<std::path::PathBuf> =
        vec![path.parent().unwrap_or(Path::new(".")).to_path_buf()];
    for entry in env.module_manifest(program_id) {
        if let Some(origin) = entry.origin
            && let Ok(canonical) = origin.canonicalize()
            && let Some(parent) = canonical.parent()
        {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs.sort();
    dirs.dedup();
    for dir in &dirs {
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| format!("Failed to watch {}: {}", dir.display(), e))?;
    }

    Ok(Some(watcher))
}

fn load_font(ttf: &sdl2::ttf::Sdl2TtfContext, size: u16) -> Result<sdl2::ttf::Font<'_, '_>, String> {
    let font_paths = [
        // macOS
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/SFNSMono.ttf",
        "/Library/Fonts/Arial.ttf",
        // Linux
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        // Windows
        "C:\\Windows\\Fonts\\arial.ttf",
    ];

    for path in &font_paths {
        if Path::new(path).exists() {
            match ttf.load_font(path, size) {
                Ok(font) => return Ok(font),
                Err(_) => continue,
            }
        }
    }

    Err("No system font found. Install a TTF font.".to_string())
}
