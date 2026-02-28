use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use notify::{RecursiveMode, Watcher};
use sdl2::event::Event;
use sdl2::keyboard::Scancode;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

use crate::input::scancode_to_name;
use crate::native_fns::{self, ExampleEntry, BROWSER_STATE, DRAW_COMMANDS, FRAME_INFO, INPUT_STATE};
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
}

const BROWSER_SCRIPT: &str = include_str!("../examples/browser.ptl");

/// Normal interactive game loop (no agent protocol).
pub fn run_game(source_path: &str, config: GameConfig) -> Result<(), String> {
    run_game_inner(Some(source_path), None, config)
}

/// Browser mode: no source file, show file browser.
pub fn run_browser(examples_dir: &Path, config: GameConfig) -> Result<(), String> {
    run_game_inner(None, Some(examples_dir), config)
}

fn populate_browser_state(examples_dir: &Path) {
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
    BROWSER_STATE.with(|b| {
        let mut bs = b.borrow_mut();
        bs.examples = entries;
        bs.pending_launch = None;
    });
}

fn run_game_inner(
    source_path: Option<&str>,
    examples_dir: Option<&Path>,
    config: GameConfig,
) -> Result<(), String> {
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

    // Determine if we start in browser mode
    let mut in_browser = source_path.is_none();

    // If browser mode, populate examples and load browser script
    if let Some(dir) = examples_dir {
        populate_browser_state(dir);
    }

    let (initial_source, initial_path) = if let Some(sp) = source_path {
        let src = std::fs::read_to_string(sp)
            .map_err(|e| format!("Failed to read {}: {}", sp, e))?;
        (src, Some(sp.to_string()))
    } else {
        (BROWSER_SCRIPT.to_string(), None)
    };

    let (mut env, mut program_id, mut stack_id) = init_petal_from_source(&initial_source, &config)?;

    let mut current_source_path: Option<String> = initial_path;

    let (mut _reload_tx, mut reload_rx) = mpsc::channel();
    let mut _watcher: Option<notify::RecommendedWatcher> = None;
    if config.hot_reload {
        if let Some(ref sp) = current_source_path {
            let (tx, rx) = mpsc::channel();
            _reload_tx = tx.clone();
            reload_rx = rx;
            _watcher = setup_watcher(sp, tx)?;
        }
    }

    let mut last_frame = Instant::now();
    let mut frame_count: i64 = 0;

    // Keep track of examples_dir for returning to browser
    let examples_dir_buf = examples_dir.map(|d| d.to_path_buf());

    'game: loop {
        INPUT_STATE.with(|s| s.borrow_mut().begin_frame());

        let mut escape_pressed = false;
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'game,
                Event::KeyDown { scancode: Some(sc), .. } if sc == Scancode::Escape => {
                    escape_pressed = true;
                }
                Event::KeyDown { scancode: Some(sc), .. } => {
                    if let Some(name) = scancode_to_name(sc) {
                        INPUT_STATE.with(|s| {
                            s.borrow_mut().keys_down.insert(name.to_string());
                        });
                    }
                }
                Event::KeyUp { scancode: Some(sc), .. } => {
                    if let Some(name) = scancode_to_name(sc) {
                        INPUT_STATE.with(|s| {
                            s.borrow_mut().keys_down.remove(name);
                        });
                    }
                }
                Event::MouseMotion { x, y, .. } => {
                    INPUT_STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        state.mouse_x = x;
                        state.mouse_y = y;
                    });
                }
                Event::MouseButtonDown { mouse_btn, .. } => {
                    INPUT_STATE.with(|s| {
                        s.borrow_mut().mouse_buttons.insert(mouse_btn as u8);
                    });
                }
                Event::MouseButtonUp { mouse_btn, .. } => {
                    INPUT_STATE.with(|s| {
                        s.borrow_mut().mouse_buttons.remove(&(mouse_btn as u8));
                    });
                }
                _ => {}
            }
        }

        // Handle Escape
        if escape_pressed {
            if in_browser {
                // In browser mode, Escape quits
                break 'game;
            } else if examples_dir_buf.is_some() {
                // In game mode with browser available, return to browser
                let source = BROWSER_SCRIPT.to_string();
                match switch_script(&mut env, &source, &config) {
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
            } else {
                // Direct file mode with no browser, Escape quits
                break 'game;
            }
        }

        let now = Instant::now();
        let dt = now.duration_since(last_frame).as_secs_f64();
        last_frame = now;
        frame_count += 1;

        FRAME_INFO.with(|f| {
            let mut info = f.borrow_mut();
            info.dt = dt;
            info.frame_count = frame_count;
        });

        if let Some(ref sp) = current_source_path {
            check_hot_reload(&reload_rx, sp, &mut env, program_id, stack_id);
        }

        DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().clear());

        env.reset_stack(stack_id)?;
        if let Err(e) = env.run(stack_id) {
            eprintln!("[petal error] {}", e);
        }

        drain_output(&mut env);

        // Check if browser wants to launch a script
        let pending = BROWSER_STATE.with(|b| b.borrow_mut().pending_launch.take());
        if let Some(script_path) = pending {
            match std::fs::read_to_string(&script_path) {
                Ok(source) => {
                    match switch_script(&mut env, &source, &config) {
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
                                _watcher = setup_watcher(&script_path, tx).unwrap_or(None);
                            }
                        }
                        Err(e) => eprintln!("[browser] failed to launch {}: {}", script_path, e),
                    }
                }
                Err(e) => eprintln!("[browser] failed to read {}: {}", script_path, e),
            }
        }

        let commands = DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().drain(..).collect::<Vec<_>>());
        renderer::render(&mut canvas, commands, &font);
        canvas.present();
    }

    Ok(())
}

fn switch_script(
    env: &mut Env,
    source: &str,
    config: &GameConfig,
) -> Result<(ProgramId, StackKey), String> {
    let program_id = env.load_program(source)?;
    let stack_id = env.create_stack(program_id)?;
    FRAME_INFO.with(|f| {
        let mut info = f.borrow_mut();
        info.screen_width = config.width as i32;
        info.screen_height = config.height as i32;
        info.frame_count = 0;
        info.dt = 0.0;
    });
    Ok((program_id, stack_id))
}

fn init_petal_from_source(
    source: &str,
    config: &GameConfig,
) -> Result<(Env, ProgramId, StackKey), String> {
    let mut env = Env::new();
    native_fns::register_all(&mut env);

    let program_id = env.load_program(source)?;
    let stack_id = env.create_stack(program_id)?;

    FRAME_INFO.with(|f| {
        let mut info = f.borrow_mut();
        info.screen_width = config.width as i32;
        info.screen_height = config.height as i32;
    });

    Ok((env, program_id, stack_id))
}

/// Agent mode with SDL window (hybrid): game runs interactively,
/// LLM can pause/resume/step/inspect via stdin protocol.
pub fn run_agent(source_path: &str, config: GameConfig) -> Result<(), String> {
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

    let (mut env, program_id, stack_id) = init_petal(source_path, &config)?;

    let (reload_tx, reload_rx) = mpsc::channel();
    let _watcher = if config.hot_reload {
        setup_watcher(source_path, reload_tx)?
    } else {
        None
    };

    let cmd_rx = protocol::spawn_stdin_reader();
    let mut paused = false;
    let mut last_frame = Instant::now();
    let mut frame_count: i64 = 0;

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
            );
        }

        // Poll SDL events (always, even when paused, to keep window responsive)
        INPUT_STATE.with(|s| s.borrow_mut().begin_frame());
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'game,
                Event::KeyDown { scancode: Some(sc), .. } if sc == Scancode::Escape => {
                    break 'game
                }
                Event::KeyDown { scancode: Some(sc), .. } => {
                    if let Some(name) = scancode_to_name(sc) {
                        INPUT_STATE.with(|s| {
                            s.borrow_mut().keys_down.insert(name.to_string());
                        });
                    }
                }
                Event::KeyUp { scancode: Some(sc), .. } => {
                    if let Some(name) = scancode_to_name(sc) {
                        INPUT_STATE.with(|s| {
                            s.borrow_mut().keys_down.remove(name);
                        });
                    }
                }
                Event::MouseMotion { x, y, .. } => {
                    INPUT_STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        state.mouse_x = x;
                        state.mouse_y = y;
                    });
                }
                Event::MouseButtonDown { mouse_btn, .. } => {
                    INPUT_STATE.with(|s| {
                        s.borrow_mut().mouse_buttons.insert(mouse_btn as u8);
                    });
                }
                Event::MouseButtonUp { mouse_btn, .. } => {
                    INPUT_STATE.with(|s| {
                        s.borrow_mut().mouse_buttons.remove(&(mouse_btn as u8));
                    });
                }
                _ => {}
            }
        }

        if !paused {
            let now = Instant::now();
            let dt = now.duration_since(last_frame).as_secs_f64();
            last_frame = now;
            frame_count += 1;

            FRAME_INFO.with(|f| {
                let mut info = f.borrow_mut();
                info.dt = dt;
                info.frame_count = frame_count;
            });

            check_hot_reload(&reload_rx, source_path, &mut env, program_id, stack_id);

            DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().clear());

            env.reset_stack(stack_id)?;
            if let Err(e) = env.run(stack_id) {
                eprintln!("[petal error] {}", e);
            }

            drain_output(&mut env);
        }

        // Always render (shows last frame when paused)
        let commands = DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().drain(..).collect::<Vec<_>>());
        if !commands.is_empty() {
            renderer::render(&mut canvas, commands, &font);
        }
        canvas.present();
    }

    Ok(())
}

/// Headless agent mode: no SDL window, purely protocol-driven.
pub fn run_headless(source_path: &str, config: GameConfig) -> Result<(), String> {
    let (mut env, program_id, stack_id) = init_petal(source_path, &config)?;

    let (reload_tx, reload_rx) = mpsc::channel();
    let _watcher = if config.hot_reload {
        setup_watcher(source_path, reload_tx)?
    } else {
        None
    };

    let cmd_rx = protocol::spawn_stdin_reader();
    let mut paused = true; // Headless starts paused — LLM drives frames

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

        check_hot_reload(&reload_rx, source_path, &mut env, program_id, stack_id);

        handle_command(
            cmd,
            &mut env,
            program_id,
            stack_id,
            &mut paused,
        );
    }

    Ok(())
}

/// Screenshot mode: run N frames headlessly, save a PNG, exit.
pub fn run_screenshot(
    source_path: &str,
    config: GameConfig,
    output_path: &str,
    frames: u32,
) -> Result<(), String> {
    let (mut env, _program_id, stack_id) = init_petal(source_path, &config)?;

    for _ in 0..frames {
        protocol::run_one_frame(&mut env, stack_id)?;
    }

    // Capture draw commands from one more speculative frame
    let commands = match protocol::capture_draw_commands(&mut env, stack_id) {
        Ok((cmds, _)) => cmds,
        Err(e) => return Err(e),
    };

    screenshot::save_png(&commands, config.width, config.height, output_path)?;
    eprintln!("Screenshot saved to {}", output_path);
    Ok(())
}

// --- Shared helpers ---

fn init_petal(
    source_path: &str,
    config: &GameConfig,
) -> Result<(Env, ProgramId, StackKey), String> {
    let mut env = Env::new();
    native_fns::register_all(&mut env);

    let source = std::fs::read_to_string(source_path)
        .map_err(|e| format!("Failed to read {}: {}", source_path, e))?;

    let program_id = env.load_program(&source)?;
    let stack_id = env.create_stack(program_id)?;

    FRAME_INFO.with(|f| {
        let mut info = f.borrow_mut();
        info.screen_width = config.width as i32;
        info.screen_height = config.height as i32;
    });

    Ok((env, program_id, stack_id))
}

fn handle_command(
    cmd: Command,
    env: &mut Env,
    program_id: ProgramId,
    stack_id: StackKey,
    paused: &mut bool,
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
                match protocol::run_one_frame(env, stack_id) {
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
            match protocol::capture_draw_commands(env, stack_id) {
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
            protocol::apply_input(&keys_down, mouse);
            protocol::send_response(&Response::ok());
        }
        Command::SetState { name, value } => {
            match protocol::set_state_from_json(env, program_id, stack_id, &name, &value) {
                Ok(()) => protocol::send_response(&Response::ok()),
                Err(e) => protocol::send_response(&Response::err(e)),
            }
        }
        Command::Screenshot => {
            match protocol::capture_draw_commands(env, stack_id) {
                Ok((commands, _output)) => {
                    let (w, h) = FRAME_INFO.with(|f| {
                        let info = f.borrow();
                        (info.screen_width as u32, info.screen_height as u32)
                    });
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
            let new_program = match env.compile_program(program_id, &new_source) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[hot-reload] compile error: {}", e);
                    return;
                }
            };
            match env.hot_reload(stack_id, new_program) {
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

fn setup_watcher(
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

    let parent = path.parent().unwrap_or(Path::new("."));
    watcher
        .watch(parent, RecursiveMode::NonRecursive)
        .map_err(|e| format!("Failed to watch: {}", e))?;

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
