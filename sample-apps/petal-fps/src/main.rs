//! petal-fps — a hybrid Rust + Petal first-person-shooter experiment.
//!
//! The Petal script (e.g. `examples/cyberpunk_city.ptl`) owns the camera,
//! projection math, per-frame scene construction, level geometry, entities,
//! physics, AI, and HUD. The Rust side is only a thin delta over `petal-sdl`:
//! a software z-buffered triangle rasterizer (`framebuffer.rs`) and the
//! `triangle3d` native family (`native_fns.rs`), wired together by
//! [`host::FpsHost`]. Everything else — the window, event loop, input,
//! agent/headless/screenshot/record modes, and hot reload — is reused from the
//! `petal-sdl` integration.
//!
//! Run:
//!   cargo run --release -- examples/cyberpunk_city.ptl
//!   cargo run --release -- --screenshot out.png examples/cyberpunk_city.ptl
//!   cargo run --release -- --record frames/ --frames 30 examples/cyberpunk_city.ptl

mod commands;
mod font;
mod framebuffer;
mod host;
mod native_fns;
mod renderer;

use host::FpsHost;
use petal_sdl::{run_agent, run_game, run_headless, run_record, run_screenshot, GameConfig};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut width: u32 = 800;
    let mut height: u32 = 600;
    let mut title = String::from("petal-fps");
    let mut hot_reload = true;
    let mut agent = false;
    let mut headless = false;
    let mut screenshot_path: Option<String> = None;
    let mut screenshot_frames: u32 = 60;
    let mut record_dir: Option<String> = None;
    let mut record_warmup: u32 = 30;
    let mut source_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--width" => { i += 1; width = args[i].parse().unwrap_or(width); }
            "--height" => { i += 1; height = args[i].parse().unwrap_or(height); }
            "--title" => { i += 1; title = args[i].clone(); }
            "--no-hot-reload" => hot_reload = false,
            "--agent" => agent = true,
            "--headless" => { headless = true; agent = true; }
            "--screenshot" => { i += 1; screenshot_path = Some(args[i].clone()); }
            "--record" => { i += 1; record_dir = Some(args[i].clone()); }
            "--warmup" => { i += 1; record_warmup = args[i].parse().unwrap_or(30); }
            "--frames" => { i += 1; screenshot_frames = args[i].parse().unwrap_or(60); }
            arg if !arg.starts_with('-') => source_path = Some(arg.to_string()),
            other => {
                eprintln!("Unknown option: {}", other);
                print_usage();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let source = match source_path {
        Some(s) => s,
        None => {
            eprintln!("Error: source .ptl path required");
            print_usage();
            std::process::exit(1);
        }
    };

    let config = GameConfig { width, height, title, hot_reload, agent, headless };
    let mut host = FpsHost::new(width, height);
    let sp = Some(source.as_str());

    let result = if let Some(ref out_path) = screenshot_path {
        run_screenshot(sp, config, out_path, screenshot_frames, &mut host)
    } else if let Some(ref dir) = record_dir {
        run_record(sp, config, dir, screenshot_frames, record_warmup, &mut host)
    } else if headless {
        run_headless(sp, config, &mut host)
    } else if agent {
        run_agent(sp, config, &mut host)
    } else {
        run_game(sp, config, &mut host)
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!("Usage: petal-fps [options] <game.ptl>");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --width <n>         Window width (default: 800)");
    eprintln!("  --height <n>        Window height (default: 600)");
    eprintln!("  --title <str>       Window title");
    eprintln!("  --no-hot-reload     Disable file watcher");
    eprintln!("  --agent             Windowed agent mode (JSON on stdin/stdout)");
    eprintln!("  --headless          Headless agent mode (no window)");
    eprintln!("  --screenshot <f>    Run N frames then write a PNG and exit");
    eprintln!("  --record <dir>      Write a PNG per frame into dir (flipbook)");
    eprintln!("  --frames <n>        Frames for --screenshot/--record (default: 60)");
    eprintln!("  --warmup <n>        Warmup frames before --record (default: 30)");
}
