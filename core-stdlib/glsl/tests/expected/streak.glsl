// common
float gen_streak_streak_weight(int i, int n) {
    return 1.0 - float(abs(i)) / (float(n) + 1.0);
}

// void effect(inout Fx fx)
vec3 acc = vec3(0.0, 0.0, 0.0);
float total = 0.0;
for (int i = -3; i < 3 + 1; i++) {
    vec3 s = src(fx.uv + vec2(float(i) * params.reach / 3.0, 0.0));
    float w = fx_luma(s) > params.threshold ? gen_streak_streak_weight(i, 3) : 0.0;
    acc = acc + s * w;
    total = total + w;
}
if (total <= 0.0) {
    fx.color = fx.color;
    return;
}
fx.color = fx.color + acc / total * params.amount;
