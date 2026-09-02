// Offscreen-canvas filters: one full-screen triangle, sampling one texture.
//
// `fs_blur` is one direction of a separable Gaussian; the renderer runs it
// twice (horizontal into a scratch, vertical back) at a resolution where the
// sigma is small, having halved the source down to it first. `fs_copy` is the
// plain resample used for that halving, for the upsample back into the
// canvas, and for presenting an intermediate frame to the window.

struct Params {
    // The blur axis in uv units per texel: (texel.x, 0) or (0, texel.y).
    step: vec2<f32>,
    // Standard deviation, in texels of the texture being sampled.
    sigma: f32,
    // Taps each side of the centre (the kernel is 2*taps + 1 wide).
    taps: i32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(1) @binding(0) var src: texture_2d<f32>;
@group(1) @binding(1) var src_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // One triangle that covers clip space: (-1,1), (3,1), (-1,-3), so uv
    // (0,0) is the top-left texel and the target is filled with no seam.
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: VsOut;
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_blur(in: VsOut) -> @location(0) vec4<f32> {
    let s2 = 2.0 * params.sigma * params.sigma;
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    // Level 0 explicitly: a loop is not uniform control flow as far as the
    // implicit-derivative rule is concerned, and the source has no mips.
    for (var i = -params.taps; i <= params.taps; i = i + 1) {
        let w = exp(-f32(i * i) / s2);
        sum += textureSampleLevel(src, src_sampler, in.uv + params.step * f32(i), 0.0) * w;
        weight += w;
    }
    // Premultiplied in, premultiplied out: blurring straight-alpha pixels
    // would drag the (meaningless) colour of transparent texels into the
    // edges as dark fringes.
    return sum / weight;
}

@fragment
fn fs_copy(in: VsOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src, src_sampler, in.uv, 0.0);
}
