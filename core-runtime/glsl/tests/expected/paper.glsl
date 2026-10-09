// void effect(inout Fx fx)
vec2 p = fx.uv * fx.resolution / max(params.scale, 0.25);
float tooth = fbm(p * 0.35, 4) - 0.5;
float fiber = noise(vec2(p.x * 0.08, p.y * 1.3)) * noise(vec2(p.x * 1.1, p.y * 0.06)) - 0.25;
fx.color = fx.color * (1.0 + (tooth * 0.8 + fiber * params.fibers) * params.amount);
