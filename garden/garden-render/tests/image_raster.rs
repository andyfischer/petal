use garden_render::{Color, HeadlessRenderer, Primitive, Rect, Scene};

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
        }],
    };
    let capture = renderer.capture(&scene);
    let center = ((12 * capture.width + 12) * 4) as usize;
    assert!(capture.rgba[center] > 240);
    assert!(capture.rgba[center + 1] < 15);
    assert!(capture.rgba[center + 2] < 15);

    let _ = std::fs::remove_file(path);
}
