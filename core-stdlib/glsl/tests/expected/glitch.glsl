// void effect(inout Fx fx)
float t = floor(fx.time * params.speed);
float row = floor(fx.uv.y * params.blocks);
float on = step(1.0 - params.amount * 0.35, hash12(vec2(row, t)));
float shift = (hash12(vec2(row + 7.0, t)) - 0.5) * 0.12 * params.amount * on;
vec2 uv = vec2(clamp(fx.uv.x + shift, 0.0, 1.0), fx.uv.y);
vec3 c = src(uv);
fx.color = vec3(mix(c.x, src(uv + vec2(0.006 * params.amount, 0.0)).x, on), c.y, c.z);
