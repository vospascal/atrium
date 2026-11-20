package atrium.app

import org.jetbrains.skia.RuntimeEffect

/** Ink blooms with feathered edges. All the grain is masked by the live dye. */
private const val BloomSksl = """
uniform shader dye;
uniform float cell;
uniform float scale;
uniform float time;
uniform float grain;

float hash(float2 p) {
    p = fract(p * float2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

float noise(float2 p) {
    float2 i = floor(p);
    float2 f = fract(p);
    float2 s = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash(i), hash(i + float2(1.0, 0.0)), s.x),
        mix(hash(i + float2(0.0, 1.0)), hash(i + 1.0), s.x),
        s.y
    );
}

half4 main(float2 fragCoord) {
    float2 p = fragCoord / scale;
    float2 q = fragCoord / cell;
    float reach = clamp(scale * 0.017 / cell, 1.4, 4.5);
    float2 drift = float2(0.11, -0.075) * time;
    float wash = noise(p * 12.0 + drift * 0.5);
    float crossWash = noise(p * 15.0 - drift * 0.4 + 9.6);
    float2 warped = p + (float2(wash, crossWash) - 0.5) * 0.115;
    float fibers = noise(warped * float2(66.0, 53.0) + drift);
    float sediment = noise(warped * 182.0 - drift * 0.65);
    q += reach * 1.3 * (float2(fibers, noise(warped.yx * 43.0 + 13.2)) - 0.5);

    float amount = dye.eval(q).a;
    float2 h = float2(reach, 0.0);
    float2 v = float2(0.0, reach);
    float left = dye.eval(q - h).a;
    float right = dye.eval(q + h).a;
    float above = dye.eval(q - v).a;
    float below = dye.eval(q + v).a;
    float support = max(amount, max(max(left, right), max(above, below)));
    if (support < 0.006) { return half4(0.0); }

    float pooled = mix(amount, (left + right + above + below) * 0.25, 0.30);
    float edge = length(float2(right - left, below - above));
    float fleck = hash(floor(fragCoord * 0.83));
    float granules = sediment * 0.52 + fleck * 0.48;
    float paper = mix(wash, granules, grain) * 0.52 + fibers * 0.48;
    float density = pooled * (0.72 + 0.48 * wash + 0.23 * fibers);
    float body = smoothstep(0.012, 0.38, density);
    float bleeding = smoothstep(0.02, 0.27, edge) * smoothstep(0.022, 0.19, support);
    float pigment = body * (0.37 + 0.57 * paper) + bleeding * (0.14 + 0.28 * fibers);
    float tone = clamp(0.54 + 0.40 * paper + 0.19 * body, 0.0, 1.0);
    float alpha = clamp(pigment, 0.0, 0.95);
    return half4(half3(tone * alpha), half(alpha));
}
"""

internal val BloomEffect: RuntimeEffect by lazy { RuntimeEffect.makeForShader(BloomSksl) }
