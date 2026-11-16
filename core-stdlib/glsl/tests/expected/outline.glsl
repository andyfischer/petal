// void effect(inout Fx fx)
vec2 px = params.width / fx.resolution;
float d = scene_depth(fx.uv);
float dx = abs(scene_depth(fx.uv + vec2(px.x, 0.0)) - scene_depth(fx.uv - vec2(px.x, 0.0)));
float dy = abs(scene_depth(fx.uv + vec2(0.0, px.y)) - scene_depth(fx.uv - vec2(0.0, px.y)));
float edge = smoothstep(params.threshold, params.threshold * 2.5, (dx + dy) / max(d, 0.1)) * (1.0 - smoothstep(150.0, 400.0, d));
fx.color = mix(fx.color, linear_to_srgb(params.color), edge * params.amount);
