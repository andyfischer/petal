// void effect(inout Fx fx)
vec2 px = 1.0 / fx.resolution;
vec3 around = (src(fx.uv + vec2(px.x, 0.0)) + src(fx.uv - vec2(px.x, 0.0)) + src(fx.uv + vec2(0.0, px.y)) + src(fx.uv - vec2(0.0, px.y))) * 0.25;
fx.color = fx.color + (fx.color - around) * params.amount * 2.0;
