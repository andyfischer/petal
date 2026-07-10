//! Generic PNG helpers. A host rasterizes a frame into an [`RgbImage`] (through
//! whatever renderer it owns); these encode that image to a file or a base64
//! string for the agent `screenshot` command. Keeping the encode here means a
//! screenshot is byte-identical regardless of which host produced the pixels.

use base64::Engine;
use image::RgbImage;

pub fn save_png(img: &RgbImage, path: &str) -> Result<(), String> {
    img.save(path).map_err(|e| e.to_string())
}

pub fn to_base64(img: &RgbImage) -> String {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encode failed");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}
