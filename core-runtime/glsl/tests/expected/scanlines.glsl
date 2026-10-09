// void effect(inout Fx fx)
float line = 0.5 + 0.5 * cos(fx.uv.y * min(params.lines, fx.resolution.y * 0.5) * 6.283185307179586);
vec3 c = fx.color * (1.0 - params.amount * (1.0 - line) * (1.0 - 0.6 * fx_luma(fx.color)));
float band = fract(fx.uv.y * 0.6 + fx.time * 0.08);
fx.color = c * (1.0 - params.roll * 0.3 * smoothstep(0.85, 1.0, 1.0 - abs(band * 2.0 - 1.0)));
