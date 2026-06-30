use serde::Serialize;

use petal::env::Env;
use petal::heap::Heap;
use petal::stack::StackKey;
use petal::value::Value;

use crate::native_fns::DRAW_COMMANDS_SYMBOL;

#[derive(Serialize, PartialEq, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum DrawCommand {
    Clear {
        r: u8,
        g: u8,
        b: u8,
    },
    Rect {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        r: u8,
        g: u8,
        b: u8,
    },
    RectOutline {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        r: u8,
        g: u8,
        b: u8,
    },
    Line {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        r: u8,
        g: u8,
        b: u8,
    },
    Circle {
        cx: i32,
        cy: i32,
        radius: i32,
        r: u8,
        g: u8,
        b: u8,
    },
    Text {
        text: String,
        x: i32,
        y: i32,
        size: u16,
        r: u8,
        g: u8,
        b: u8,
    },
    Triangle {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        x3: i32,
        y3: i32,
        r: u8,
        g: u8,
        b: u8,
    },
    Poly {
        points: Vec<(i32, i32)>,
        r: u8,
        g: u8,
        b: u8,
    },
    /// Allocate an offscreen canvas (PGraphics-style render target) of size
    /// `w`x`h`, identified by `id`. Offscreen canvases are transparent until
    /// drawn into and are recreated fresh each frame from the command stream,
    /// which keeps them compatible with the per-frame re-run model.
    CreateCanvas {
        id: u32,
        w: u32,
        h: u32,
    },
    /// Redirect subsequent draw commands to a render target. `id == 0` targets
    /// the main framebuffer; any other `id` targets the offscreen canvas with
    /// that id. This is the explicit, ordered form of `draw_to(canvas)`.
    SetTarget {
        id: u32,
    },
    /// Blit an offscreen canvas onto the current render target at (`x`, `y`).
    DrawCanvas {
        id: u32,
        x: i32,
        y: i32,
    },
}

/// Read a numeric `Value` (int or float) as i64.
fn as_i64(v: &Value) -> Result<i64, String> {
    match v {
        Value::Int(n) => Ok(*n),
        Value::Float(f) => Ok(*f as i64),
        other => Err(format!("expected number in draw command, got {}", other.type_name())),
    }
}

impl DrawCommand {
    /// Decode a draw command from a buffered-output `Value`. Native draw
    /// functions push each command as `Value::EnumVariant { tag, data }` where
    /// `data` is a flat list of arguments (see `native_fns.rs`); this is the
    /// inverse mapping the renderer uses to drain `Env::take_output_buffer`.
    pub fn from_value(val: &Value, heap: &Heap) -> Result<DrawCommand, String> {
        let (tag, data) = match val {
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

        // Positional helpers over the flat `data` list.
        let i32_at = |i: usize| -> Result<i32, String> { Ok(as_i64(&data[i])? as i32) };
        let u32_at = |i: usize| -> Result<u32, String> { Ok(as_i64(&data[i])? as u32) };
        let u8_at = |i: usize| -> Result<u8, String> { Ok(as_i64(&data[i])? as u8) };

        let cmd = match tag.as_str() {
            "clear" => DrawCommand::Clear { r: u8_at(0)?, g: u8_at(1)?, b: u8_at(2)? },
            "rect" => DrawCommand::Rect {
                x: i32_at(0)?, y: i32_at(1)?, w: u32_at(2)?, h: u32_at(3)?,
                r: u8_at(4)?, g: u8_at(5)?, b: u8_at(6)?,
            },
            "rect_outline" => DrawCommand::RectOutline {
                x: i32_at(0)?, y: i32_at(1)?, w: u32_at(2)?, h: u32_at(3)?,
                r: u8_at(4)?, g: u8_at(5)?, b: u8_at(6)?,
            },
            "line" => DrawCommand::Line {
                x1: i32_at(0)?, y1: i32_at(1)?, x2: i32_at(2)?, y2: i32_at(3)?,
                r: u8_at(4)?, g: u8_at(5)?, b: u8_at(6)?,
            },
            "circle" => DrawCommand::Circle {
                cx: i32_at(0)?, cy: i32_at(1)?, radius: i32_at(2)?,
                r: u8_at(3)?, g: u8_at(4)?, b: u8_at(5)?,
            },
            "triangle" => DrawCommand::Triangle {
                x1: i32_at(0)?, y1: i32_at(1)?, x2: i32_at(2)?, y2: i32_at(3)?,
                x3: i32_at(4)?, y3: i32_at(5)?,
                r: u8_at(6)?, g: u8_at(7)?, b: u8_at(8)?,
            },
            "poly" => {
                let points_id = match data[0] {
                    Value::List(id) => id,
                    ref other => {
                        return Err(format!("poly points must be a list, got {}", other.type_name()))
                    }
                };
                let mut points = Vec::new();
                for p in heap.get_list(points_id) {
                    match p {
                        Value::Vec2(x, y) => points.push((*x as i32, *y as i32)),
                        Value::List(pid) => {
                            let coords = heap.get_list(*pid);
                            points.push((as_i64(&coords[0])? as i32, as_i64(&coords[1])? as i32));
                        }
                        other => {
                            return Err(format!(
                                "poly point must be vec2 or [x, y], got {}",
                                other.type_name()
                            ))
                        }
                    }
                }
                DrawCommand::Poly { points, r: u8_at(1)?, g: u8_at(2)?, b: u8_at(3)? }
            }
            "text" => {
                let text = match data[0] {
                    Value::String(id) => heap.get_string(id).to_string(),
                    ref other => {
                        return Err(format!("text command needs a string, got {}", other.type_name()))
                    }
                };
                DrawCommand::Text {
                    text, x: i32_at(1)?, y: i32_at(2)?, size: as_i64(&data[3])? as u16,
                    r: u8_at(4)?, g: u8_at(5)?, b: u8_at(6)?,
                }
            }
            "create_canvas" => DrawCommand::CreateCanvas { id: u32_at(0)?, w: u32_at(1)?, h: u32_at(2)? },
            "set_target" => DrawCommand::SetTarget { id: u32_at(0)? },
            "draw_canvas" => DrawCommand::DrawCanvas { id: u32_at(0)?, x: i32_at(1)?, y: i32_at(2)? },
            other => return Err(format!("unknown draw command '{}'", other)),
        };
        Ok(cmd)
    }
}

/// Drain the `draw_commands` output buffer from the Env and decode it into a
/// renderable command list. Malformed commands are skipped (logged to stderr).
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

/// [`take_draw_commands`] for a *forked* stack: drain and decode the draw buffer
/// of `stack_id`'s own context. A fork's draw commands — and the heap objects
/// (string tags, list args) they reference — live in the fork's context, not the
/// default one, so both the drain and the decode must target `stack_id`'s heap.
pub fn take_draw_commands_for(env: &mut Env, stack_id: StackKey) -> Vec<DrawCommand> {
    let sym = env.intern_symbol(DRAW_COMMANDS_SYMBOL);
    let values = env.take_output_buffer_for(stack_id, sym);
    let heap = match env.heap_for(stack_id) {
        Some(h) => h,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(values.len());
    for v in &values {
        match DrawCommand::from_value(v, heap) {
            Ok(cmd) => out.push(cmd),
            Err(e) => eprintln!("[petal draw] {}", e),
        }
    }
    out
}
