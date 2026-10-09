// void effect(inout Fx fx)
vec2 px = max(params.size, 1.0) / fx.resolution;
fx.color = src((floor(fx.uv / px) + 0.5) * px);
