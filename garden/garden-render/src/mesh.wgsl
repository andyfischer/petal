// Flat-shaded triangle-list shader.
//
// Each vertex carries a position (logical pixels) and a pre-linearized RGBA
// color. Unlike the quad shader there is no instancing — callers tessellate
// geometry (lines, circles, polygons) into a triangle list on the CPU and the
// whole list is one vertex buffer.

struct Globals {
    // Logical viewport size in pixels (physical size / scale factor).
    screen_size: vec2<f32>,
    // Physical pixels per logical pixel, for the clip mask's one-device-pixel
    // antialiasing feather.
    scale: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Vertex {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Rounded-clip mask: (x, y, w, h) in logical pixels…
    @location(2) mask: vec4<f32>,
    // …and its corner radius, 0 for "no mask".
    @location(3) mask_radius: f32,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    // The vertex's own logical-pixel position, interpolated: the fragment
    // stage needs it to evaluate the mask, and @builtin(position) is in
    // physical pixels (and is a sample position under MSAA).
    @location(1) local: vec2<f32>,
    @location(2) @interpolate(flat) mask: vec4<f32>,
    @location(3) @interpolate(flat) mask_radius: f32,
};

// Signed distance from `p` to a rounded rect, negative inside. The standard
// two-term form: distance to the inset box, plus the corner radius.
fn rounded_box_sdf(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let half = rect.zw * 0.5;
    let center = rect.xy + half;
    let r = min(radius, min(half.x, half.y));
    let q = abs(p - center) - (half - vec2<f32>(r, r));
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Coverage of `p` by the mask: 1 inside, 0 outside, feathered across one
// physical pixel at the boundary so a circular crop has a clean edge. A zero
// radius means "no mask" and costs one compare.
fn mask_coverage(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 1.0;
    }
    let d = rounded_box_sdf(p, rect, radius) * globals.scale;
    return clamp(0.5 - d, 0.0, 1.0);
}

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
    out.local = v.pos;
    out.mask = v.mask;
    out.mask_radius = v.mask_radius;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let cover = mask_coverage(in.local, in.mask, in.mask_radius);
    return vec4<f32>(in.color.rgb, in.color.a * cover);
}
