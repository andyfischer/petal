use serde::Serialize;

use petal::env::Env;
use petal::heap::Heap;
use petal::value::Value;

use crate::native_fns::DRAW_COMMANDS_SYMBOL;

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

fn as_i64(v: &Value) -> Result<i64, String> {
    match v {
        Value::Int(n) => Ok(*n),
        Value::Float(f) => Ok(*f as i64),
        other => Err(format!("expected number in draw command, got {}", other.type_name())),
    }
}

fn as_f32(v: &Value) -> Result<f32, String> {
    match v {
        Value::Float(f) => Ok(*f as f32),
        Value::Int(n) => Ok(*n as f32),
        other => Err(format!("expected number in draw command, got {}", other.type_name())),
    }
}

impl DrawCommand {
    /// Decode a draw command from a buffered-output `Value`. Native draw
    /// functions push each command as `Value::EnumVariant { tag, data }` with a
    /// flat argument list (see `native_fns.rs`); this is the inverse mapping.
    pub fn from_value(val: &Value, heap: &Heap) -> Result<DrawCommand, String> {
        let (tag, d) = match val {
            Value::EnumVariant { tag, data } => {
                (heap.get_string(*tag).to_string(), heap.get_list(*data).to_vec())
            }
            other => {
                return Err(format!(
                    "draw command must be an enum value, got {}",
                    other.type_name()
                ))
            }
        };

        let i32_at = |i: usize| -> Result<i32, String> { Ok(as_i64(&d[i])? as i32) };
        let u32_at = |i: usize| -> Result<u32, String> { Ok(as_i64(&d[i])? as u32) };
        let u8_at = |i: usize| -> Result<u8, String> { Ok(as_i64(&d[i])? as u8) };
        let f32_at = |i: usize| -> Result<f32, String> { as_f32(&d[i]) };

        let cmd = match tag.as_str() {
            "clear3d" => DrawCommand::Clear3d { r: u8_at(0)?, g: u8_at(1)?, b: u8_at(2)? },
            "sky_gradient" => DrawCommand::SkyGradient {
                r_top: u8_at(0)?, g_top: u8_at(1)?, b_top: u8_at(2)?,
                r_bot: u8_at(3)?, g_bot: u8_at(4)?, b_bot: u8_at(5)?,
            },
            "triangle3d" => DrawCommand::Triangle3d {
                x1: f32_at(0)?, y1: f32_at(1)?, z1: f32_at(2)?,
                x2: f32_at(3)?, y2: f32_at(4)?, z2: f32_at(5)?,
                x3: f32_at(6)?, y3: f32_at(7)?, z3: f32_at(8)?,
                r: u8_at(9)?, g: u8_at(10)?, b: u8_at(11)?,
            },
            "triangle3d_shaded" => DrawCommand::Triangle3dShaded {
                x1: f32_at(0)?, y1: f32_at(1)?, z1: f32_at(2)?, r1: u8_at(3)?, g1: u8_at(4)?, b1: u8_at(5)?,
                x2: f32_at(6)?, y2: f32_at(7)?, z2: f32_at(8)?, r2: u8_at(9)?, g2: u8_at(10)?, b2: u8_at(11)?,
                x3: f32_at(12)?, y3: f32_at(13)?, z3: f32_at(14)?, r3: u8_at(15)?, g3: u8_at(16)?, b3: u8_at(17)?,
            },
            "line3d" => DrawCommand::Line3d {
                x1: f32_at(0)?, y1: f32_at(1)?, z1: f32_at(2)?,
                x2: f32_at(3)?, y2: f32_at(4)?, z2: f32_at(5)?,
                r: u8_at(6)?, g: u8_at(7)?, b: u8_at(8)?,
            },
            "rect2d" => DrawCommand::Rect2d {
                x: i32_at(0)?, y: i32_at(1)?, w: u32_at(2)?, h: u32_at(3)?,
                r: u8_at(4)?, g: u8_at(5)?, b: u8_at(6)?,
            },
            "line2d" => DrawCommand::Line2d {
                x1: i32_at(0)?, y1: i32_at(1)?, x2: i32_at(2)?, y2: i32_at(3)?,
                r: u8_at(4)?, g: u8_at(5)?, b: u8_at(6)?,
            },
            "circle2d" => DrawCommand::Circle2d {
                cx: i32_at(0)?, cy: i32_at(1)?, radius: i32_at(2)?,
                r: u8_at(3)?, g: u8_at(4)?, b: u8_at(5)?,
            },
            "text2d" => {
                let text = match d[0] {
                    Value::String(id) => heap.get_string(id).to_string(),
                    ref other => {
                        return Err(format!("text2d needs a string, got {}", other.type_name()))
                    }
                };
                DrawCommand::Text2d {
                    text, x: i32_at(1)?, y: i32_at(2)?, size: as_i64(&d[3])? as u16,
                    r: u8_at(4)?, g: u8_at(5)?, b: u8_at(6)?,
                }
            }
            other => return Err(format!("unknown draw command '{}'", other)),
        };
        Ok(cmd)
    }
}

/// Drain the `draw_commands` output buffer from the Env and decode it.
/// Malformed commands are skipped (logged to stderr).
pub fn take_draw_commands(env: &mut Env) -> Vec<DrawCommand> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer(sym);
    let mut out = Vec::with_capacity(values.len());
    for v in &values {
        match DrawCommand::from_value(v, env.heap()) {
            Ok(cmd) => out.push(cmd),
            Err(e) => eprintln!("[petal draw] {}", e),
        }
    }
    out
}

/// Discard any buffered draw commands (defensive clear at the top of a frame).
pub fn clear_draw_commands(env: &mut Env) {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    env.clear_output_buffer(sym);
}
