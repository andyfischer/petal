use serde::Serialize;

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
