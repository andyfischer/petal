// void effect(inout Fx fx)
vec2 cell = floor(fx.uv * fx.resolution / max(params.size, 1.0));
vec3 n = hash33(vec3(cell.x, cell.y, floor(fx.time * params.speed))) - 0.5;
float weight = clamp(1.0 - abs(fx_luma(fx.color) - 0.45) * 1.2, 0.25, 1.0);
fx.color = fx.color + mix(vec3(n.x, n.x, n.x), n, params.color) * params.amount * 0.22 * weight;
