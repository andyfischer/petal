use base64::Engine;
use image::{Rgb, RgbImage};

use crate::commands::DrawCommand;
use crate::framebuffer::Framebuffer;

fn render(commands: &[DrawCommand], width: u32, height: u32) -> RgbImage {
    let mut fb = Framebuffer::new(width, height);
    fb.execute(commands);
    let mut img = RgbImage::from_pixel(width, height, Rgb([0, 0, 0]));
    for (i, px) in fb.color.chunks_exact(3).enumerate() {
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        img.put_pixel(x, y, Rgb([px[0], px[1], px[2]]));
    }
    img
}

pub fn save_png(commands: &[DrawCommand], width: u32, height: u32, path: &str) -> Result<(), String> {
    let img = render(commands, width, height);
    img.save(path).map_err(|e| e.to_string())
}

pub fn render_to_png_base64(commands: &[DrawCommand], width: u32, height: u32) -> String {
    let img = render(commands, width, height);
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encode failed");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}
