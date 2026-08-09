// Flat-shaded triangle-list shader.
//
// Each vertex carries a position (logical pixels) and a pre-linearized RGBA
// color. Unlike the quad shader there is no instancing — callers tessellate
// geometry (lines, circles, polygons) into a triangle list on the CPU and the
// whole list is one vertex buffer.

struct Globals {
    // Logical viewport size in pixels (physical size / scale factor).
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Vertex {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(v: Vertex) -> VsOut {
    // Logical pixels -> NDC (y flipped: pixel y grows downward).
    let ndc = vec2<f32>(
        v.pos.x / globals.screen_size.x * 2.0 - 1.0,
        1.0 - v.pos.y / globals.screen_size.y * 2.0,
    );

    var out: VsOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.color = v.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
