struct Globals {
    screen_size: vec2<f32>,
    // Physical pixels per logical pixel, for the clip mask's one-device-pixel
    // antialiasing feather.
    scale: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var bitmap: texture_2d<f32>;
@group(1) @binding(1) var bitmap_sampler: sampler;

struct ImageInstance {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) alpha: f32,
    // Rounded-clip mask: (x, y, w, h) in logical pixels…
    @location(3) mask: vec4<f32>,
    // …and its corner radius, 0 for "no mask".
    @location(4) mask_radius: f32,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    // The fragment's logical-pixel position, interpolated: the mask is
    // evaluated there, and @builtin(position) is in physical pixels.
    @location(2) local: vec2<f32>,
    @location(3) @interpolate(flat) mask: vec4<f32>,
    @location(4) @interpolate(flat) mask_radius: f32,
};

// Signed distance from `p` to a rounded rect, negative inside — the same
// two-term form `mesh.wgsl` uses, so an image and a mesh cut against the same
// mask agree pixel for pixel along their shared edge.
fn rounded_box_sdf(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let half = rect.zw * 0.5;
    let center = rect.xy + half;
    let r = min(radius, min(half.x, half.y));
    let q = abs(p - center) - (half - vec2<f32>(r, r));
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Coverage of `p` by the mask: 1 inside, 0 outside, feathered across one
// physical pixel so a circular crop has a clean edge rather than a staircase.
fn mask_coverage(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 1.0;
    }
    let d = rounded_box_sdf(p, rect, radius) * globals.scale;
    return clamp(0.5 - d, 0.0, 1.0);
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, image: ImageInstance) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let pos = image.pos + corner * image.size;
    let ndc = vec2<f32>(
        pos.x / globals.screen_size.x * 2.0 - 1.0,
        1.0 - pos.y / globals.screen_size.y * 2.0,
    );

    var out: VsOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    out.alpha = image.alpha;
    out.local = pos;
    out.mask = image.mask;
    out.mask_radius = image.mask_radius;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let color = textureSample(bitmap, bitmap_sampler, in.uv);
    let cover = mask_coverage(in.local, in.mask, in.mask_radius);
    return vec4<f32>(color.rgb, color.a * in.alpha * cover);
}
