use garden_render::{ClipMask, Color, HeadlessRenderer, Primitive, Rect, Scene};

#[test]
fn png_bitmap_is_loaded_scaled_and_rasterized() {
    let path = std::env::temp_dir().join(format!("garden-image-raster-{}.png", std::process::id()));
    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(file, 2, 2);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&[
            255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
        ])
        .expect("png data");

    let mut renderer = match HeadlessRenderer::new((24.0, 24.0), 1.0) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("skipping: no GPU adapter ({err})");
            return;
        }
    };
    let scene = Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![Primitive::Image {
            rect: Rect::new(4.0, 4.0, 16.0, 16.0),
            source: path.display().to_string(),
            alpha: 1.0,
            clip: Rect::new(0.0, 0.0, 24.0, 24.0),
            mask: ClipMask::NONE,
        }],
    };
    let capture = renderer.capture(&scene);
    let center = ((12 * capture.width + 12) * 4) as usize;
    assert!(capture.rgba[center] > 240);
    assert!(capture.rgba[center + 1] < 15);
    assert!(capture.rgba[center + 2] < 15);

    let _ = std::fs::remove_file(path);
}

/// The circular avatar: an image cut against a `ClipMask` whose radius is half
/// its side. The corners must be gone (the mask is the only thing that can
/// remove them — the scissor is four straight edges), the centre must survive
/// intact, and the boundary must be *antialiased* rather than a hard staircase,
/// which is what the one-device-pixel feather in `image.wgsl` buys.
#[test]
fn a_rounded_mask_cuts_an_image_into_a_circle() {
    let path = std::env::temp_dir().join(format!("garden-image-mask-{}.png", std::process::id()));
    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(file, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&[255, 0, 0, 255])
        .expect("png data");

    let mut renderer = match HeadlessRenderer::new((32.0, 32.0), 1.0) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("skipping: no GPU adapter ({err})");
            return;
        }
    };
    let rect = Rect::new(0.0, 0.0, 32.0, 32.0);
    let capture = renderer.capture(&Scene {
        bg: Color::rgb(0.0, 0.0, 0.0),
        primitives: vec![Primitive::Image {
            rect,
            source: path.display().to_string(),
            alpha: 1.0,
            clip: rect,
            mask: ClipMask::rounded(rect, 16.0),
        }],
    });
    let red = |x: u32, y: u32| capture.rgba[((y * capture.width + x) * 4) as usize];

    assert!(red(16, 16) > 240, "the centre of the disc is the image");
    assert_eq!(red(0, 0), 0, "the top-left corner is outside the circle");
    assert_eq!(red(31, 31), 0, "…and so is the bottom-right");
    // One pixel inside the rim and one outside it bracket a soft edge: a hard
    // cut would put both at 0 or 255.
    let rim: Vec<u8> = (13..19).map(|x| red(x, 0)).collect();
    assert!(
        rim.iter().any(|&v| v > 8 && v < 248),
        "the circle's edge should be feathered, got {rim:?}"
    );

    let _ = std::fs::remove_file(path);
}
