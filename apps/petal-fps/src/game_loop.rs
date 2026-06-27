use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

use notify::{RecursiveMode, Watcher};
use sdl2::event::Event;
use sdl2::keyboard::Scancode;
use sdl2::mouse::MouseButton;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

use crate::framebuffer::Framebuffer;
use crate::input::scancode_to_name;
use crate::native_fns::{self, DRAW_COMMANDS, FRAME_INFO, INPUT_STATE};
use crate::protocol::{self, Command, Response};
use crate::renderer::Renderer;
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

enum PollResult {
    None,
    Quit,
    Escape,
}

fn poll_sdl_events(event_pump: &mut sdl2::EventPump) -> PollResult {
    INPUT_STATE.with(|s| s.borrow_mut().begin_frame());
    let mut result = PollResult::None;
    for event in event_pump.poll_iter() {
        match event {
            Event::Quit { .. } => return PollResult::Quit,
            Event::KeyDown { scancode: Some(sc), repeat: false, .. } if sc == Scancode::Escape => {
                result = PollResult::Escape;
            }
            Event::KeyDown { scancode: Some(sc), repeat: false, .. } => {
                if let Some(name) = scancode_to_name(sc) {
                    INPUT_STATE.with(|s| {
                        let mut st = s.borrow_mut();
                        st.keys_down.insert(name.to_string());
                        st.keys_pressed.insert(name.to_string());
                    });
                }
            }
            Event::KeyUp { scancode: Some(sc), .. } => {
                if let Some(name) = scancode_to_name(sc) {
                    INPUT_STATE.with(|s| { s.borrow_mut().keys_down.remove(name); });
                }
            }
            Event::MouseMotion { x, y, xrel, yrel, .. } => {
                INPUT_STATE.with(|s| {
                    let mut st = s.borrow_mut();
                    st.mouse_x = x;
                    st.mouse_y = y;
                    st.mouse_dx += xrel;
                    st.mouse_dy += yrel;
                });
            }
            Event::MouseButtonDown { mouse_btn, .. } => {
                let idx = button_index(mouse_btn);
                INPUT_STATE.with(|s| {
                    let mut st = s.borrow_mut();
                    st.mouse_buttons.insert(idx);
                    st.mouse_buttons_pressed.insert(idx);
                });
            }
            Event::MouseButtonUp { mouse_btn, .. } => {
                let idx = button_index(mouse_btn);
                INPUT_STATE.with(|s| { s.borrow_mut().mouse_buttons.remove(&idx); });
            }
            _ => {}
        }
    }
    result
}

fn button_index(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::X1 => 4,
        MouseButton::X2 => 5,
        MouseButton::Unknown => 0,
    }
}

/// Windowed interactive mode.
pub fn run_game(source_path: &str, config: GameConfig) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;

    let window = video
        .window(&config.title, config.width, config.height)
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;

    let canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .build()
        .map_err(|e| e.to_string())?;

    let mut renderer = Renderer::new(canvas, config.width, config.height)?;
    let mut fb = Framebuffer::new(config.width, config.height);
    let mut event_pump = sdl.event_pump()?;
    sdl.mouse().set_relative_mouse_mode(false);

    let (mut env, _program_id, stack_id) = init_petal(source_path, &config)?;

    let (tx, rx) = mpsc::channel();
    let _watcher = if config.hot_reload { setup_watcher(source_path, tx)? } else { None };

    let mut last_frame = Instant::now();
    let mut frame_count: i64 = 0;
    let mut mouse_grabbed = false;

    'game: loop {
        match poll_sdl_events(&mut event_pump) {
            PollResult::Quit | PollResult::Escape => break 'game,
            PollResult::None => {}
        }

        // Honour Petal's grab/release requests.
        let want_grab = INPUT_STATE.with(|s| s.borrow().want_mouse_grab);
        if want_grab != mouse_grabbed {
            sdl.mouse().set_relative_mouse_mode(want_grab);
            mouse_grabbed = want_grab;
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

        check_hot_reload(&rx, source_path, &mut env, _program_id, stack_id);

        DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().clear());
        env.reset_stack(stack_id)?;
        if let Err(e) = env.run(stack_id) {
            eprintln!("[petal error] {}", e);
        }
        drain_output(&mut env);

        let commands = DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().drain(..).collect::<Vec<_>>());
        fb.execute(&commands);
        renderer.present(&fb)?;
    }
    Ok(())
}

/// Windowed agent mode — agent can drive step-by-step while the window stays
/// open so a human can watch.
pub fn run_agent(source_path: &str, config: GameConfig) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;

    let window = video
        .window(&config.title, config.width, config.height)
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;

    let canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .build()
        .map_err(|e| e.to_string())?;

    let mut renderer = Renderer::new(canvas, config.width, config.height)?;
    let mut fb = Framebuffer::new(config.width, config.height);
    let mut event_pump = sdl.event_pump()?;

    let (mut env, program_id, stack_id) = init_petal(source_path, &config)?;

    let (tx, rx) = mpsc::channel();
    let _watcher = if config.hot_reload { setup_watcher(source_path, tx)? } else { None };

    let cmd_rx = protocol::spawn_stdin_reader();
    let mut paused = false;
    let mut last_frame = Instant::now();
    let mut frame_count: i64 = 0;

    protocol::send_response(&Response {
        frame: Some(0),
        paused: Some(false),
        ..Response::ok()
    });

    'game: loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            handle_command(cmd, &mut env, program_id, stack_id, &mut paused, config.width, config.height);
        }

        match poll_sdl_events(&mut event_pump) {
            PollResult::Quit | PollResult::Escape => break 'game,
            PollResult::None => {}
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

            check_hot_reload(&rx, source_path, &mut env, program_id, stack_id);

            DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().clear());
            env.reset_stack(stack_id)?;
            if let Err(e) = env.run(stack_id) {
                eprintln!("[petal error] {}", e);
            }
            drain_output(&mut env);
        }

        let commands = DRAW_COMMANDS.with(|cmds| cmds.borrow_mut().drain(..).collect::<Vec<_>>());
        if !commands.is_empty() {
            fb.execute(&commands);
        }
        renderer.present(&fb)?;
    }
    Ok(())
}

pub fn run_headless(source_path: &str, config: GameConfig) -> Result<(), String> {
    let (mut env, program_id, stack_id) = init_petal(source_path, &config)?;

    let (tx, rx) = mpsc::channel();
    let _watcher = if config.hot_reload { setup_watcher(source_path, tx)? } else { None };

    let cmd_rx = protocol::spawn_stdin_reader();
    let mut paused = true;

    protocol::send_response(&Response {
        frame: Some(0),
        paused: Some(true),
        ..Response::ok()
    });

    loop {
        let cmd = match cmd_rx.recv() {
            Ok(cmd) => cmd,
            Err(_) => break,
        };
        check_hot_reload(&rx, source_path, &mut env, program_id, stack_id);
        handle_command(cmd, &mut env, program_id, stack_id, &mut paused, config.width, config.height);
    }
    Ok(())
}

pub fn run_screenshot(source_path: &str, config: GameConfig, output_path: &str, frames: u32) -> Result<(), String> {
    let (mut env, _program_id, stack_id) = init_petal(source_path, &config)?;

    // Simulate some frames of fixed-dt time so gameplay can reach a steady state.
    for _ in 0..frames {
        protocol::run_one_frame(&mut env, stack_id)?;
    }

    let (commands, output) = protocol::capture_draw_commands(&mut env, stack_id)?;
    for line in output {
        eprintln!("{}", line);
    }
    screenshot::save_png(&commands, config.width, config.height, output_path)?;
    eprintln!("[screenshot] wrote {}", output_path);
    let stats = protocol::compute_stats(&commands);
    eprintln!(
        "[screenshot] frames={} commands={} triangles={} lines={} rects={} z=[{:?},{:?}]",
        frames, stats.total, stats.triangles, stats.lines, stats.rects, stats.z_min, stats.z_max
    );
    Ok(())
}

/// Record several frames to a directory as a flipbook. Useful for agents to
/// inspect motion without opening a window.
pub fn run_record(source_path: &str, config: GameConfig, out_dir: &str, frames: u32, warmup: u32) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let (mut env, _program_id, stack_id) = init_petal(source_path, &config)?;

    for _ in 0..warmup {
        protocol::run_one_frame(&mut env, stack_id)?;
    }
    for i in 0..frames {
        protocol::run_one_frame(&mut env, stack_id)?;
        let (commands, _) = protocol::capture_draw_commands(&mut env, stack_id)?;
        let path = format!("{}/frame_{:04}.png", out_dir, i);
        screenshot::save_png(&commands, config.width, config.height, &path)?;
    }
    eprintln!("[record] wrote {} frames to {}", frames, out_dir);
    Ok(())
}

fn init_petal(source_path: &str, config: &GameConfig) -> Result<(Env, ProgramId, StackKey), String> {
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
    width: u32,
    height: u32,
) {
    match cmd {
        Command::Pause => {
            *paused = true;
            protocol::send_response(&Response { paused: Some(true), ..Response::ok() });
        }
        Command::Resume => {
            *paused = false;
            protocol::send_response(&Response { paused: Some(false), ..Response::ok() });
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
            protocol::send_response(&Response { state: Some(state), ..Response::ok() });
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
                Err(e) => protocol::send_response(&Response::err(e)),
            }
        }
        Command::Input { keys_down, mouse, mouse_delta } => {
            protocol::apply_input(&keys_down, mouse.as_ref(), mouse_delta.as_ref());
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
                    let b64 = screenshot::render_to_png_base64(&commands, width, height);
                    let stats = protocol::compute_stats(&commands);
                    protocol::send_response(&Response {
                        screenshot: Some(b64),
                        stats: Some(stats),
                        ..Response::ok()
                    });
                }
                Err(e) => protocol::send_response(&Response::err(e)),
            }
        }
        Command::DrawStats => {
            match protocol::capture_draw_commands(env, stack_id) {
                Ok((commands, _output)) => {
                    let stats = protocol::compute_stats(&commands);
                    protocol::send_response(&Response { stats: Some(stats), ..Response::ok() });
                }
                Err(e) => protocol::send_response(&Response::err(e)),
            }
        }
    }
}

fn drain_output(env: &mut Env) {
    for line in env.take_output() {
        eprintln!("{}", line);
    }
}

fn check_hot_reload(
    rx: &mpsc::Receiver<()>,
    source_path: &str,
    env: &mut Env,
    program_id: ProgramId,
    stack_id: StackKey,
) {
    if let Ok(()) = rx.try_recv() {
        if let Ok(new_source) = std::fs::read_to_string(source_path) {
            let new_program = match env.compile_program(program_id, &new_source) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[hot-reload] compile error: {}", e);
                    return;
                }
            };
            match env.transfer_state(stack_id, new_program) {
                Ok(result) => eprintln!(
                    "[hot-reload] preserved: {}, dropped: {}",
                    result.state_preserved, result.state_dropped
                ),
                Err(e) => eprintln!("[hot-reload] error: {}", e),
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
