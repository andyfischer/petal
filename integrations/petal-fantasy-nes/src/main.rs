//! fantasy-nes — an NES-style fantasy console whose carts are Petal scripts.
//!
//! A cart (`carts/*.ptl`) owns the artwork, the maps, the music, the menus and
//! the gameplay; it is re-run top to bottom every frame and pushes the whole
//! console state as it goes. The Rust side supplies only the two things a
//! script cannot do fast enough or at all — a PPU-shaped rasterizer and an
//! APU-shaped sound chip — wired together by [`host::NesHost`]. Everything
//! else (window, event loop, input, hot reload, agent/headless/screenshot
//! modes) is reused from the `petal-sdl` integration; see `docs/design.md`.
//!
//! Run:
//!   cargo run                                        # the launcher
//!   cargo run -- carts/hello.ptl --scale 4 --crt
//!   cargo run -- --screenshot out.png --frames 10 carts/hello.ptl

// The console's subsystems declare their full public API up front — the PPU's
// state and rasterizer seam, the APU's channels, the audio bank — so the tasks
// filling them in code against fixed signatures. Until those bodies land, most
// of that surface has no caller. Drop this once the subsystems are implemented.
#![allow(dead_code)]

mod apu;
mod audio;
mod host;
mod natives;
mod ppu;

use host::NesHost;
use petal_sdl::{GameConfig, run_agent, run_game, run_headless, run_record, run_screenshot};
use ppu::{SCREEN_H, SCREEN_W};

/// Window scale when the CLI names none. 3x (768x720) is the largest multiple
/// that fits comfortably on a laptop display.
const DEFAULT_SCALE: u32 = 3;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut scale = DEFAULT_SCALE;
    let mut crt = false;
    let mut hot_reload = true;
    let mut agent = false;
    let mut headless = false;
    let mut screenshot_path: Option<String> = None;
    let mut frames: u32 = 60;
    let mut record_dir: Option<String> = None;
    let mut record_warmup: u32 = 30;
    let mut cart_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--scale" => {
                i += 1;
                scale = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(scale);
            }
            "--crt" => crt = true,
            "--no-hot-reload" => hot_reload = false,
            "--agent" => agent = true,
            "--headless" => {
                headless = true;
                agent = true;
            }
            "--screenshot" => {
                i += 1;
                screenshot_path = args.get(i).cloned();
            }
            "--record" => {
                i += 1;
                record_dir = args.get(i).cloned();
            }
            "--frames" => {
                i += 1;
                frames = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(frames);
            }
            "--warmup" => {
                i += 1;
                record_warmup = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(record_warmup);
            }
            arg if !arg.starts_with('-') => cart_path = Some(arg.to_string()),
            other => {
                eprintln!("Unknown option: {}", other);
                print_usage();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let scale = scale.clamp(1, 8);
    let mut host = NesHost::new(scale, crt);

    // With no cart named, the loop asks the host for a default source, which is
    // the launcher — so `fantasy-nes` with no arguments boots into a menu the
    // way a console does.
    let config = GameConfig {
        width: SCREEN_W as u32 * scale,
        height: SCREEN_H as u32 * scale,
        title: String::from("fantasy-nes"),
        hot_reload,
        agent,
        headless,
    };
    let cart = cart_path.as_deref();

    let result = if let Some(ref out_path) = screenshot_path {
        run_screenshot(cart, config, out_path, frames, &mut host)
    } else if let Some(ref dir) = record_dir {
        run_record(cart, config, dir, frames, record_warmup, &mut host)
    } else if headless {
        run_headless(cart, config, &mut host)
    } else if agent {
        run_agent(cart, config, &mut host)
    } else {
        run_game(cart, config, &mut host)
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!("Usage: fantasy-nes [options] [cart.ptl]");
    eprintln!();
    eprintln!("With no cart, boots the launcher (carts/launcher.ptl).");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --scale <n>         Integer pixel scale (default: 3)");
    eprintln!("  --crt               Scanline filter");
    eprintln!("  --no-hot-reload     Disable the file watcher");
    eprintln!("  --agent             Windowed agent mode (JSON on stdin/stdout)");
    eprintln!("  --headless          Headless agent mode (no window)");
    eprintln!("  --screenshot <f>    Run N frames then write a PNG and exit");
    eprintln!("  --record <dir>      Write a PNG per frame into dir (flipbook)");
    eprintln!("  --frames <n>        Frames for --screenshot/--record (default: 60)");
    eprintln!("  --warmup <n>        Warmup frames before --record (default: 30)");
}
