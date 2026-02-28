mod commands;
mod game_loop;
mod input;
mod native_fns;
mod protocol;
mod renderer;
mod screenshot;

use std::path::PathBuf;

use game_loop::GameConfig;

/// Resolved at compile time to `petal-sdl/examples/`.
const EXAMPLES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut width: u32 = 800;
    let mut height: u32 = 600;
    let mut title = String::from("Petal Game");
    let mut hot_reload = true;
    let mut agent = false;
    let mut headless = false;
    let mut screenshot_path: Option<String> = None;
    let mut screenshot_frames: u32 = 120;
    let mut source_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--width" => {
                i += 1;
                width = args[i].parse().expect("Invalid width");
            }
            "--height" => {
                i += 1;
                height = args[i].parse().expect("Invalid height");
            }
            "--title" => {
                i += 1;
                title = args[i].clone();
            }
            "--no-hot-reload" => {
                hot_reload = false;
            }
            "--agent" => {
                agent = true;
            }
            "--headless" => {
                headless = true;
                agent = true; // headless implies agent
            }
            "--screenshot" => {
                i += 1;
                screenshot_path = Some(args[i].clone());
            }
            "--frames" => {
                i += 1;
                screenshot_frames = args[i].parse().expect("Invalid frame count");
            }
            arg if !arg.starts_with('-') => {
                source_path = Some(arg.to_string());
            }
            other => {
                eprintln!("Unknown option: {}", other);
                print_usage();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let config = GameConfig {
        width,
        height,
        title,
        hot_reload,
        agent,
        headless,
    };

    let result = if let Some(ref sp) = source_path {
        if let Some(ref out_path) = screenshot_path {
            game_loop::run_screenshot(sp, config, out_path, screenshot_frames)
        } else if headless {
            game_loop::run_headless(sp, config)
        } else if agent {
            game_loop::run_agent(sp, config)
        } else {
            game_loop::run_game(sp, config)
        }
    } else {
        // No source file — browser mode
        let dir = PathBuf::from(EXAMPLES_DIR);
        if !dir.is_dir() {
            eprintln!("Error: no source file provided and examples directory not found at {}", EXAMPLES_DIR);
            print_usage();
            std::process::exit(1);
        }
        game_loop::run_browser(&dir, config)
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!("Usage: petal-sdl [options] <game.ptl>");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --width <n>       Window width (default: 800)");
    eprintln!("  --height <n>      Window height (default: 600)");
    eprintln!("  --title <str>     Window title (default: \"Petal Game\")");
    eprintln!("  --no-hot-reload   Disable file watching");
    eprintln!("  --agent           Enable agent protocol (JSON over stdin/stdout)");
    eprintln!("  --headless        Headless agent mode (no window, implies --agent)");
    eprintln!("  --screenshot <f>  Run headlessly, save PNG screenshot to file, then exit");
    eprintln!("  --frames <n>      Frames to run before screenshot (default: 120)");
}
