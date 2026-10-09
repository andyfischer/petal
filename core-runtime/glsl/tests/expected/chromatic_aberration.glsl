// void effect(inout Fx fx)
vec2 d = fx.uv - 0.5;
vec2 offset = d * dot(d, d) * params.amount * 0.03;
fx.color = vec3(src(fx.uv + offset).x, fx.color.y, src(fx.uv - offset).z);
