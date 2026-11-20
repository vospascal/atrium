package atrium.app

import org.jetbrains.skia.RuntimeEffect

/** Side-lit smoke. The density and its slope come from the advected dye, including the curls. */
private const val SmokeSksl = """
uniform shader dye;
uniform float cell;
uniform float scale;
uniform float time;
uniform float monochrome;

half4 main(float2 fragCoord) {
    float2 p = fragCoord / scale;
    float2 q = fragCoord / cell;
    // A small wrinkle across each fold keeps its bright rim smooth between dye cells.
    q += 0.45 * sin(float2(p.y, p.x) * 51.0 + sin(p * 23.0) * 1.6 + time * 0.48);
    float amount = dye.eval(q).a;

    // A narrow sample reveals the rim, while a wider one gives the translucent sheet its body.
    float reach = clamp(scale * 0.015 / cell, 1.3, 4.0);
    float2 h = float2(reach, 0.0);
    float2 v = float2(0.0, reach);
    float left = dye.eval(q - h).a;
    float right = dye.eval(q + h).a;
    float above = dye.eval(q - v).a;
    float below = dye.eval(q + v).a;
    float support = max(amount, max(max(left, right), max(above, below)));
    if (support < 0.005) { return half4(0.0); }

    float2 slope = float2(right - left, below - above);
    float gradient = length(slope);
    float rim = smoothstep(0.012, 0.17, gradient) * smoothstep(0.015, 0.18, support);
    float facing = dot(slope, float2(-0.44, 0.90)) / max(0.01, gradient);
    float litRim = rim * mix(0.42, 1.0, smoothstep(-0.55, 0.75, facing));

    float nearby = (left + right + above + below) * 0.25;
    float veil = pow(nearby, 0.78) * 0.50;
    float core = smoothstep(0.05, 0.65, amount);
    float glow = max(core * 0.92, litRim);
    float alpha = clamp(veil + 0.61 * litRim + 0.33 * core, 0.0, 0.98);
    float3 mist = mix(float3(0.44, 0.56, 0.64), float3(0.92, 0.97, 1.0), glow);
    float3 neutral = float3(dot(mist, float3(0.2126, 0.7152, 0.0722)));
    return half4(half3(mix(mist, neutral, monochrome) * alpha), half(alpha));
}
"""

internal val SmokeEffect: RuntimeEffect by lazy { RuntimeEffect.makeForShader(SmokeSksl) }
