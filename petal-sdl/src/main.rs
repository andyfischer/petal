mod commands;
mod game_loop;
mod input;
mod native_fns;
mod protocol;
mod renderer;

use game_loop::GameConfig;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut width: u32 = 800;
    let mut height: u32 = 600;
    let mut title = String::from("Petal Game");
    let mut hot_reload = true;
    let mut agent = false;
    let mut headless = false;
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

    let source_path = match source_path {
        Some(p) => p,
        None => {
            eprintln!("Error: no source file provided");
            print_usage();
            std::process::exit(1);
        }
    };

    let config = GameConfig {
        width,
        height,
        title,
        hot_reload,
        agent,
        headless,
    };

    let result = if headless {
        game_loop::run_headless(&source_path, config)
    } else if agent {
        game_loop::run_agent(&source_path, config)
    } else {
        game_loop::run_game(&source_path, config)
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
}
