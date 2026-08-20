//! Smoke test for every cart in `carts/`.
//!
//! The unit tests under `src/` pin the PPU and APU against hand-built inputs;
//! nothing there proves a *cart* still runs. A cart breaks in ways Rust tests
//! cannot see — a renamed prelude export, a native whose arity drifted, a
//! Petal-level error that only shows up on frame 90 — and the console's whole
//! value proposition is that its content is Petal, so a rotted cart is a
//! rotted product.
//!
//! Each cart is run through the real binary in `--screenshot` mode, which is
//! the one run mode that drives committed frames with **no window and no
//! audio device** (`on_sdl_init` is windowed-only) and still rasterizes a real
//! PPU frame at the end. That mode propagates a Petal error out of
//! `run_screenshot` as a process failure, so "the cart ran clean" and "the
//! cart drew something" are both observable from outside.
//!
//! Two things are asserted per cart, and they catch different rot:
//!
//!   - **no error** — the script survived [`FRAMES`] frames of its own logic.
//!   - **not blank** — the frame is not one flat color. A stubbed-out or
//!     mis-wired rasterizer still produces a perfectly valid-looking PNG full
//!     of the backdrop; only a look at the color distribution notices.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Frames each cart runs before its frame is captured. Matches the cart smoke
/// test in `docs/design.md`, and is long enough for the animated carts to have
/// cycled through a state or two rather than only their first-frame setup.
const FRAMES: &str = "120";

/// "Not blank" is checked on two axes at once, because either one alone has an
/// awkward false-positive story: a deliberately sparse cart really is mostly
/// backdrop, and a rich cart really can be drawn from few colors. Both bounds
/// are set loose enough that no cart in the tree is near them, and a frame of
/// flat backdrop — the shape a stubbed or mis-wired rasterizer produces —
/// fails both by a mile.
///
/// Measured worst cases today: `music_demo` is 94% one color, and the sparsest
/// carts use 5 distinct colors.
const MAX_UNIFORM_SHARE: f64 = 0.98;
const MIN_COLORS: usize = 3;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every cart the console can boot, in the two shapes it supports: a loose
/// `carts/*.ptl` file, and a `carts/<name>/game.ptl` entry script beside the
/// modules it imports. Deliberately rediscovered from the filesystem rather
/// than listed, so a cart added tomorrow is covered without touching this file.
fn carts() -> Vec<(String, PathBuf)> {
    let dir = crate_dir().join("carts");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("carts/ is missing").flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.is_dir() {
            let game = path.join("game.ptl");
            if game.is_file() {
                found.push((stem.to_string(), game));
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("ptl") {
            found.push((stem.to_string(), path));
        }
    }
    found.sort();
    found
}

/// Run one cart headlessly and return its final frame as `(width, height,
/// RGB8)`. Panics with the cart's own stderr on failure — a Petal error is far
/// more useful to read than "exit code 1".
fn run_cart(name: &str, cart: &Path) -> (u32, u32, Vec<u8>) {
    let out_png = std::env::temp_dir().join(format!("fantasy-nes-smoke-{}.png", name));
    let _ = std::fs::remove_file(&out_png);

    let output = Command::new(env!("CARGO_BIN_EXE_fantasy-nes"))
        .current_dir(crate_dir())
        .args(["--screenshot"])
        .arg(&out_png)
        .args(["--frames", FRAMES, "--scale", "1"])
        .arg(cart)
        .output()
        .expect("failed to spawn fantasy-nes");

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "{name}: cart exited with {}\n{stderr}",
        output.status
    );
    // The stepped and interactive loops report a mid-run script error here and
    // keep going, so a clean exit status is not on its own proof of a clean run.
    assert!(
        !stderr.contains("[petal error]"),
        "{name}: Petal error during the run\n{stderr}"
    );

    let img = image::open(&out_png)
        .unwrap_or_else(|e| panic!("{name}: screenshot did not decode: {e}"))
        .to_rgb8();
    let _ = std::fs::remove_file(&out_png);
    (img.width(), img.height(), img.into_raw())
}

/// The frame's most common color, the share it covers, and how many distinct
/// colors the frame holds.
fn color_stats(pixels: &[u8]) -> (f64, [u8; 3], usize) {
    let mut counts = std::collections::HashMap::new();
    for px in pixels.chunks_exact(3) {
        *counts.entry([px[0], px[1], px[2]]).or_insert(0usize) += 1;
    }
    let total = pixels.len() / 3;
    let distinct = counts.len();
    let (color, n) = counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .expect("empty frame");
    (n as f64 / total as f64, color, distinct)
}

#[test]
fn every_cart_runs_clean_and_draws_something() {
    let carts = carts();
    assert!(!carts.is_empty(), "no carts found");

    for (name, path) in &carts {
        let (w, h, pixels) = run_cart(name, path);
        assert_eq!(
            (w, h),
            (256, 240),
            "{name}: screenshot is not a console frame"
        );
        let (share, color, distinct) = color_stats(&pixels);
        assert!(
            share <= MAX_UNIFORM_SHARE,
            "{name}: frame is {:.1}% {color:?} — nothing was drawn",
            share * 100.0
        );
        assert!(
            distinct >= MIN_COLORS,
            "{name}: frame holds only {distinct} color(s) — nothing was drawn"
        );
    }
}

/// The launcher and the showcase game are the two carts the console is judged
/// on, and they are the two the discovery above could silently stop finding —
/// the launcher because it is excluded from the browsable list, `petal_quest`
/// because it is a directory rather than a file. Name them so a discovery bug
/// fails loudly instead of shrinking the corpus.
#[test]
fn the_launcher_and_the_showcase_game_are_in_the_corpus() {
    let names: Vec<String> = carts().into_iter().map(|(n, _)| n).collect();
    for required in ["launcher", "petal_quest"] {
        assert!(
            names.iter().any(|n| n == required),
            "{required} is missing from the cart corpus: {names:?}"
        );
    }
}
