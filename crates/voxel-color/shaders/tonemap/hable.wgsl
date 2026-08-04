// Hable's Uncharted 2 filmic curve. A toe and a shoulder instead of Reinhard's single
// hyperbola, so blacks crush a little and highlights roll with more contrast.
//
// THE `/ f(W)` IS NOT OPTIONAL and is the half people drop: without it the curve peaks
// around 0.8 and the image reads washed. W = 11.2 is the linear white point, i.e. the
// input that maps to display 1.0.
//
// The 2.0 exposure bias is likewise part of the operator as published, not a tweak of
// ours. Omitting it is the other common error and makes Hable look far too dark, which
// then gets blamed on the curve.
//
// SDR ONLY: the normalisation pins the ceiling at 1.0, so this can no more reach HDR
// headroom than plain Reinhard can. It ignores `headroom` entirely, deliberately.
const HABLE_SHOULDER_STRENGTH: f32 = 0.15;
const HABLE_LINEAR_STRENGTH: f32 = 0.50;
const HABLE_LINEAR_ANGLE: f32 = 0.10;
const HABLE_TOE_STRENGTH: f32 = 0.20;
const HABLE_TOE_NUMERATOR: f32 = 0.02;
const HABLE_TOE_DENOMINATOR: f32 = 0.30;
const HABLE_LINEAR_WHITE: f32 = 11.2;
const HABLE_EXPOSURE_BIAS: f32 = 2.0;

fn hable_partial(value: vec3<f32>) -> vec3<f32> {
    let a = HABLE_SHOULDER_STRENGTH;
    let b = HABLE_LINEAR_STRENGTH;
    let c = HABLE_LINEAR_ANGLE;
    let d = HABLE_TOE_STRENGTH;
    let e = HABLE_TOE_NUMERATOR;
    let f = HABLE_TOE_DENOMINATOR;
    return ((value * (a * value + vec3<f32>(c * b)) + vec3<f32>(d * e))
            / (value * (a * value + vec3<f32>(b)) + vec3<f32>(d * f)))
           - vec3<f32>(e / f);
}

fn tonemap_hable(color: vec3<f32>) -> vec3<f32> {
    let positive = max(color, vec3<f32>(0.0));
    let mapped = hable_partial(positive * HABLE_EXPOSURE_BIAS);
    let white_scale = hable_partial(vec3<f32>(HABLE_LINEAR_WHITE));
    // THE CLAMP IS PART OF THE OPERATOR, not a safety net. `hable_partial` is bounded by
    // `1 - E/F` = 0.933 as its input grows, and we divide by f(W) which is smaller than
    // that — so the ratio actually rises to about 1.17 rather than stopping at 1.0. The
    // original relied on an 8-bit framebuffer clamping it away; on a float surface it
    // would not be clamped, and the result is a fixed ~17% overshoot that does not scale
    // with display headroom and is therefore not usable range, just an artifact.
    //
    // W is defined as "the linear value that maps to white", so clipping past it is the
    // design rather than a compromise. With the 2.0 bias the scene-linear white point is
    // W/2 = 5.6.
    return min(mapped / white_scale, vec3<f32>(1.0));
}

