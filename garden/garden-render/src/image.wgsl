struct Globals {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var bitmap: texture_2d<f32>;
@group(1) @binding(1) var bitmap_sampler: sampler;

struct ImageInstance {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) alpha: f32,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

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
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let color = textureSample(bitmap, bitmap_sampler, in.uv);
    return vec4<f32>(color.rgb, color.a * in.alpha);
}
