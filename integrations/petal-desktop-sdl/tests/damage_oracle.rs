//! Painting by damage against painting whole, on real programs: the example
//! apps run headless under scripted input (pointer sweeps for hover, and
//! `Scenario::monkey`'s clicks, keys and typing), and every frame is painted
//! through the real renderer twice, once whole into one framebuffer and once
//! only where `petal_ui::damage` says it changed into another. The two must
//! be the same pixels on every frame, at 1x and on a 2x (Retina) drawable.

use std::path::{Path, PathBuf};

use petal_sdl::font::{self, FontBook};
use petal_sdl::screen_texture::sdl_rect;
use petal_sdl::{hidpi, renderer};
use petal_ui::damage::{Damage, DamageTracker};
use petal_ui::draw::FrameCommand;
use petal_ui_headless::harness::Headless;
use petal_ui_headless::scenario::Scenario;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::surface::Surface;

const W: i32 = 900;
const H: i32 = 640;
const FRAMES: usize = 48;

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn black(w: u32, h: u32) -> Surface<'static> {
    let mut s = Surface::new(w, h, PixelFormatEnum::RGB888).unwrap();
    s.fill_rect(None, Color::RGB(0, 0, 0)).unwrap();
    s
}

fn first_difference(a: &Surface, b: &Surface) -> Option<(usize, usize)> {
    let (w, pitch) = (a.width() as usize, a.pitch() as usize);
    let at = a.with_lock(|pa| {
        b.with_lock(|pb| (0..pa.len()).step_by(4).find(|&i| (i % pitch) / 4 < w && pa[i..i + 3] != pb[i..i + 3]))
    })?;
    Some(((at % pitch) / 4, at / pitch))
}

#[derive(Default)]
struct Tally {
    frames: usize,
    none: usize,
    rects: usize,
    full: usize,
}

/// Run `app` for [`FRAMES`] frames and paint each both ways at `factor`.
/// `Err` names the first frame whose pixels differ.
fn check(app: &Path, factor: f32, fonts: &mut FontBook, tally: &mut Tally) -> Result<(), String> {
    let mut ui = Headless::from_file_with_size(app, W, H)?;
    let monkey = Scenario::monkey(app.to_string_lossy().len() as u64, FRAMES, (W, H));
    let (pw, ph) = ((W as f32 * factor) as u32, (H as f32 * factor) as u32);
    let (mut whole, mut damaged) = (Some(black(pw, ph)), Some(black(pw, ph)));
    let mut tracker = DamageTracker::<FrameCommand>::new();
    for f in 0..FRAMES {
        // Sweep the pointer across the window between the monkey's actions.
        ui.mouse_move((f as i32 * 37) % W, (f as i32 * 53) % H);
        monkey.apply(&mut ui, f);
        let cmds = ui.frame()?.to_vec();
        let cmds = hidpi::scale_frame(cmds.into_iter().map(FrameCommand::Px).collect(), factor);
        let damage = tracker.update(&cmds, &[], pw, ph);
        tally.frames += 1;

        let mut wc = whole.take().unwrap().into_canvas()?;
        renderer::render_frame(&mut wc, cmds.iter().cloned(), fonts);
        let w = wc.into_surface();

        let mut dc = damaged.take().unwrap().into_canvas()?;
        match &damage {
            Damage::None => tally.none += 1,
            Damage::Full => {
                tally.full += 1;
                renderer::render_frame(&mut dc, cmds.iter().cloned(), fonts);
            }
            Damage::Rects(rs) => {
                tally.rects += 1;
                for r in rs {
                    renderer::render_frame_within(&mut dc, tracker.reaching(&cmds, *r), fonts, Some(sdl_rect(r)));
                }
            }
        }
        let d = dc.into_surface();
        if let Some((x, y)) = first_difference(&d, &w) {
            return Err(format!("frame {f} at {factor}x: differs at {x},{y} under {damage:?}"));
        }
        whole = Some(w);
        damaged = Some(d);
    }
    Ok(())
}

/// Every example app (`DAMAGE_ORACLE_APP=spreadsheet` for the ones whose
/// path has that in it), every third also at 2x, and the same check as
/// [`painting_by_damage_matches_painting_whole`]. About two minutes in a
/// debug build: `cargo test --test damage_oracle -- --ignored`.
#[test]
#[ignore]
fn painting_by_damage_matches_painting_whole_on_every_example() {
    let only = std::env::var("DAMAGE_ORACLE_APP").ok();
    run(|i, app| {
        let wanted = only.as_ref().is_none_or(|o| app.to_string_lossy().contains(o.as_str()));
        match (wanted, i % 3) {
            (false, _) => &[],
            (true, 0) => &[1.0, 2.0],
            (true, _) => &[1.0],
        }
    });
}

/// Every fourth example app, half of those also at 2x.
#[test]
fn painting_by_damage_matches_painting_whole() {
    run(|i, _| match i % 8 {
        0 => &[1.0, 2.0],
        4 => &[1.0],
        _ => &[],
    });
}

/// Check the example apps at the scale factors `factors(index, app)` names.
fn run(factors: impl Fn(usize, &Path) -> &'static [f32]) {
    let Ok(mut fonts) = FontBook::load_system(font::DEFAULT_LADDER) else {
        eprintln!("no system font: skipped");
        return;
    };
    let mut apps: Vec<PathBuf> = ["ui", "productivity", "dashboards"]
        .iter()
        .flat_map(|group| std::fs::read_dir(examples().join(group)).unwrap())
        .map(|e| e.unwrap().path().join("app.ptl"))
        .filter(|p| p.exists())
        .collect();
    apps.sort();
    let (mut ran, mut failures, mut tally) = (0, Vec::new(), Tally::default());
    for (i, app) in apps.iter().enumerate() {
        let name = app.parent().unwrap().strip_prefix(examples()).unwrap().display().to_string();
        for &factor in factors(i, app) {
            match check(app, factor, &mut fonts, &mut tally) {
                Ok(()) => ran += 1,
                Err(e) if e.contains("differs") => failures.push(format!("{name}: {e}")),
                // An app this harness can't run: Garden's panels need its
                // host natives.
                Err(e) => eprintln!("{name}: not run: {}", e.lines().next().unwrap_or("")),
            }
        }
    }
    eprintln!(
        "{ran} runs, {} frames: {} unchanged, {} by damage, {} whole",
        tally.frames, tally.none, tally.rects, tally.full
    );
    assert!(failures.is_empty(), "painting by damage differs:\n{}", failures.join("\n"));
    assert!(ran >= 3, "only {ran} runs: the examples should load headless");
    assert!(tally.rects > tally.frames / 5, "damage should paint part of a frame often");
}
