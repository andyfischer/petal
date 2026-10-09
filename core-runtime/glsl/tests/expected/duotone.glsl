// void effect(inout Fx fx)
vec3 two = mix(linear_to_srgb(params.dark), linear_to_srgb(params.light), fx_luma(fx.color));
fx.color = mix(fx.color, two, params.amount);
