use serde::Serialize;

/// Draw commands emitted by Petal during a frame.
///
/// Triangles are submitted in *screen space* with a per-vertex depth value.
/// Petal is responsible for the world→camera→screen projection; the Rust host
/// is responsible for rasterizing with a z-buffer.
///
/// This split is intentional: it keeps the 3D math visible and live-editable
/// in Petal, while the hot inner loop (per-pixel rasterization) stays in Rust.
#[derive(Serialize, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum DrawCommand {
    /// Clear the color buffer to a solid color and reset the depth buffer.
    Clear3d { r: u8, g: u8, b: u8 },
    /// Draw a vertical gradient skybox (top color → bottom color).
    /// Writes to the depth buffer at "infinity" so all geometry draws over it.
    SkyGradient {
        r_top: u8, g_top: u8, b_top: u8,
        r_bot: u8, g_bot: u8, b_bot: u8,
    },
    /// Screen-space triangle with per-vertex depth. Rasterized with z-buffer.
    /// Depth is a float; smaller = nearer. Rejects triangles where any vertex
    /// has depth <= 0.0 (behind near plane).
    Triangle3d {
        x1: f32, y1: f32, z1: f32,
        x2: f32, y2: f32, z2: f32,
        x3: f32, y3: f32, z3: f32,
        r: u8, g: u8, b: u8,
    },
    /// Triangle with per-vertex color (gouraud shading, for fake lighting).
    Triangle3dShaded {
        x1: f32, y1: f32, z1: f32, r1: u8, g1: u8, b1: u8,
        x2: f32, y2: f32, z2: f32, r2: u8, g2: u8, b2: u8,
        x3: f32, y3: f32, z3: f32, r3: u8, g3: u8, b3: u8,
    },
    /// Wireframe edge (z-tested). Useful for debug and neon outlines.
    Line3d {
        x1: f32, y1: f32, z1: f32,
        x2: f32, y2: f32, z2: f32,
        r: u8, g: u8, b: u8,
    },
    /// 2D filled rectangle (HUD overlay, ignores depth).
    Rect2d { x: i32, y: i32, w: u32, h: u32, r: u8, g: u8, b: u8 },
    /// 2D line (HUD).
    Line2d { x1: i32, y1: i32, x2: i32, y2: i32, r: u8, g: u8, b: u8 },
    /// 2D filled circle (HUD).
    Circle2d { cx: i32, cy: i32, radius: i32, r: u8, g: u8, b: u8 },
    /// 2D text (HUD). Uses an embedded 5x7 bitmap font.
    Text2d { text: String, x: i32, y: i32, size: u16, r: u8, g: u8, b: u8 },
}
