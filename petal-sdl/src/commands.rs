use serde::Serialize;

#[derive(Serialize)]
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
}
