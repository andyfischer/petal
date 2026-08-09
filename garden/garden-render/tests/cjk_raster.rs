//! End-to-end raster check for the CJK/emoji font fallback: render a
//! mixed-script string through the real GPU text pipeline and confirm the
//! non-Latin cells produce ink rather than being blank.
//!
//! This needs a GPU adapter and an installed fallback font. Both are optional:
//! `HeadlessRenderer::new` returns `Err` when no adapter exists, and the test
//! skips when neither happens — so `cargo test -p garden-render` stays green in
//! headless CI. Set `GARDEN_RASTER_OUT=/path/to.png` to also dump the frame for
//! visual inspection.

use garden_render::{Color, HeadlessRenderer, Primitive, Rect, Scene, TextStyle, FONT_SIZE};

/// Fallback fonts must exist for the non-Latin cells to render as glyphs; this
/// mirrors the (private) candidate list in `text.rs` for the macOS/Linux paths
/// the test cares about.
fn fallback_font_present() -> bool {
    [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Apple Color Emoji.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ]
    .iter()
    .any(|p| std::path::Path::new(p).exists())
}

/// Count pixels in `cap` (within the given logical-pixel band, scaled by
/// `scale`) that differ noticeably from the background — i.e. glyph ink.
fn ink_in_band(cap: &garden_render::Capture, bg: [u8; 3], x0: f32, x1: f32, scale: f32) -> u32 {
    let px0 = (x0 * scale) as u32;
    let px1 = (x1 * scale) as u32;
    let mut ink = 0;
    for y in 0..cap.height {
        for x in px0..px1.min(cap.width) {
            let i = ((y * cap.width + x) * 4) as usize;
            let (r, g, b) = (cap.rgba[i], cap.rgba[i + 1], cap.rgba[i + 2]);
            let d = (r as i32 - bg[0] as i32).abs()
                + (g as i32 - bg[1] as i32).abs()
                + (b as i32 - bg[2] as i32).abs();
            if d > 40 {
                ink += 1;
            }
        }
    }
    ink
}

#[test]
fn mixed_script_renders_ink_in_non_latin_cells() {
    if !fallback_font_present() {
        eprintln!("skipping: no CJK/emoji fallback font installed");
        return;
    }

    let scale = 2.0_f32;
    let (lw, lh) = (240.0_f32, 40.0_f32);
    let mut renderer = match HeadlessRenderer::new((lw, lh), scale as f64) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            return;
        }
    };

    let bg = Color::rgb(0.08, 0.09, 0.11);
    let fg = Color::rgb(0.90, 0.92, 0.95);
    // Latin, Han, hiragana, hangul, emoji — one glyph each.
    let text = "A中あ한🙂".to_string();
    let scene = Scene {
        bg,
        primitives: vec![Primitive::Text {
            pos: (4.0, 6.0),
            text,
            color: fg,
            clip: Rect::new(0.0, 0.0, lw, lh),
            size: FONT_SIZE,
            style: TextStyle::default(),
        }],
    };

    let cap = renderer.capture(&scene);
    let bg8 = [
        (bg.r * 255.0) as u8,
        (bg.g * 255.0) as u8,
        (bg.b * 255.0) as u8,
    ];

    let cell = renderer.cell_size().0; // logical advance of one mono cell
                                       // The Latin 'A' sits in cell 0; the CJK/kana glyphs follow it. Look for ink
                                       // in the band just past the first cell — if fallback failed these would be
                                       // blank (JetBrains Mono has no coverage there and would emit nothing usable).
    let latin_ink = ink_in_band(&cap, bg8, 4.0, 4.0 + cell, scale);
    let non_latin_ink = ink_in_band(&cap, bg8, 4.0 + cell, 4.0 + cell * 6.0, scale);

    if let Ok(path) = std::env::var("GARDEN_RASTER_OUT") {
        write_png(&cap, &path);
        eprintln!("wrote {path}");
    }

    assert!(latin_ink > 0, "Latin 'A' should render ink");
    assert!(
        non_latin_ink > latin_ink,
        "non-Latin cells should render substantial glyph ink via fallback \
         (latin={latin_ink}, non_latin={non_latin_ink})"
    );
}

fn write_png(cap: &garden_render::Capture, path: &str) {
    let file = std::fs::File::create(path).expect("create png");
    let w = std::io::BufWriter::new(file);
    let mut enc = png::Encoder::new(w, cap.width, cap.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(&cap.rgba)
        .expect("png data");
}
