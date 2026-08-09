//! One [`GpuContext`] owns the wgpu instance/adapter/device/queue, and
//! multiple renderers are built on top of it instead of each creating their
//! own device.
//!
//! Like `cjk_raster.rs`, this needs a GPU adapter; `GpuContext::new_headless`
//! returns `Err` when none exists and the test skips, so
//! `cargo test -p garden-render` stays green in headless CI.

use garden_render::{Color, GpuContext, HeadlessRenderer, Primitive, Rect, Scene};

/// A trivial scene: dark background with one bright quad in the top-left.
fn quad_scene(bg: Color) -> Scene {
    Scene {
        bg,
        primitives: vec![Primitive::Quad {
            rect: Rect::new(2.0, 2.0, 10.0, 10.0),
            color: Color::rgb(0.95, 0.4, 0.2),
        }],
    }
}

#[test]
fn two_renderers_share_one_gpu_context() {
    // One context for the whole test — the shared instance/adapter/device/queue.
    let ctx = match GpuContext::new_headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            return;
        }
    };

    let scale = 2.0_f64;
    let (aw, ah) = (64.0_f32, 32.0_f32);
    let (bw, bh) = (48.0_f32, 48.0_f32);

    // Two independent renderers built from the same context; neither creates
    // its own device/queue.
    let mut a = HeadlessRenderer::with_context(&ctx, (aw, ah), scale)
        .expect("renderer A from shared context");
    let mut b = HeadlessRenderer::with_context(&ctx, (bw, bh), scale)
        .expect("renderer B from shared context");

    let cap_a = a.capture(&quad_scene(Color::rgb(0.08, 0.09, 0.11)));
    let cap_b = b.capture(&quad_scene(Color::rgb(0.11, 0.09, 0.08)));

    // Physical size = logical × scale, same as HeadlessRenderer::new.
    assert_eq!(
        (cap_a.width, cap_a.height),
        ((aw as f64 * scale) as u32, (ah as f64 * scale) as u32),
        "renderer A capture dimensions"
    );
    assert_eq!(
        (cap_b.width, cap_b.height),
        ((bw as f64 * scale) as u32, (bh as f64 * scale) as u32),
        "renderer B capture dimensions"
    );

    // Both captures actually rendered: full RGBA payload, and the quad left
    // ink distinct from the background somewhere in the frame.
    assert_eq!(cap_a.rgba.len(), (cap_a.width * cap_a.height * 4) as usize);
    assert_eq!(cap_b.rgba.len(), (cap_b.width * cap_b.height * 4) as usize);
    assert!(
        has_non_bg_pixels(&cap_a),
        "renderer A frame should contain the quad, not just background"
    );
    assert!(
        has_non_bg_pixels(&cap_b),
        "renderer B frame should contain the quad, not just background"
    );
}

/// True if the frame has at least two distinct pixel values (background plus
/// something drawn on top of it).
fn has_non_bg_pixels(cap: &garden_render::Capture) -> bool {
    let first: [u8; 4] = cap.rgba[cap.rgba.len() - 4..].try_into().unwrap();
    cap.rgba.chunks_exact(4).any(|px| px != first)
}
