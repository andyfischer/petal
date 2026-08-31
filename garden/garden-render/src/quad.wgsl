// Instanced solid-quad shader.
//
// Each instance carries a rect (origin + size, in logical pixels) and an RGBA
// color. A unit quad is expanded in the vertex shader from the vertex index
// (4-vertex triangle strip), so there is no per-vertex buffer at all — only
// the per-instance buffer.

struct Globals {
    // Logical viewport size in pixels (physical size / scale factor).
    screen_size: vec2<f32>,
    // Physical pixels per logical pixel, for the clip mask's one-device-pixel
    // antialiasing feather.
    scale: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct QuadInstance {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, quad: QuadInstance) -> VsOut {
    // Unit-quad corner for a 4-vertex triangle strip:
    // vi = 0 -> (0,0), 1 -> (1,0), 2 -> (0,1), 3 -> (1,1).
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let pos = quad.pos + corner * quad.size;

    // Logical pixels -> NDC (y flipped: pixel y grows downward).
    let ndc = vec2<f32>(
        pos.x / globals.screen_size.x * 2.0 - 1.0,
        1.0 - pos.y / globals.screen_size.y * 2.0,
    );

    var out: VsOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.color = quad.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
