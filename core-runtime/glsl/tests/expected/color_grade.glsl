// void effect(inout Fx fx)
float l = fx_luma(fx.color);
vec3 tint = linear_to_srgb(mix(params.shadows, params.highlights, smoothstep(params.balance - 0.35, params.balance + 0.35, l)));
vec3 c = fx.color + (tint - fx_luma(tint)) * params.amount * 0.6;
fx.color = c * (1.0 - params.fade * 0.15) + params.fade * 0.08;
