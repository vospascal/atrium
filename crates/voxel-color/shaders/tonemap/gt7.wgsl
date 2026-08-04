// ---- GT7: Gran Turismo 7 colour-volume tone mapping ---------------------------
//
// Yasutomi, Suzuki and Uchimura, SIGGRAPH 2025. Transcribed from the course's own
// supplemental `gt7_tone_mapping.cpp`, not reconstructed from a paper.
//
// The one that fits a RENDERER rather than a mastering pipeline. BT.2390 maps content that
// was already graded to a known peak onto a smaller display, so it needs a content peak we
// cannot measure. GT7 goes scene-referred straight to the display and is parameterised by
// the display alone — so the content-peak assumption disappears entirely.
//
// Their framebuffer convention is `physical / 100`, i.e. 1.0 = 100 cd/m². IDENTICAL to
// ours, so `peakIntensity` is exactly our headroom ratio and nothing needs converting.
//
// It is a COLOUR-VOLUME operator, not a curve: brightness and chroma together. A
// per-channel pass gives a camera-like hue shift, a chroma-preserving pass through ICtCp
// keeps hue exact, and the two are blended 60/40 toward the second. That is why saturated
// highlights hold their hue here where every per-channel curve above desaturates them.
const GT7_ALPHA: f32 = 0.25;
const GT7_MID_POINT: f32 = 0.538;
const GT7_LINEAR_SECTION: f32 = 0.444;
const GT7_TOE_STRENGTH: f32 = 1.280;
const GT7_BLEND_RATIO: f32 = 0.6;
const GT7_FADE_START: f32 = 0.98;
const GT7_FADE_END: f32 = 1.16;
// Their `initializeAsSDR` targets 250 cd/m² then scales back by 1/2.5 to land in [0,1].
const GT7_SDR_TARGET: f32 = 2.5;

// ICtCp — the perceptually uniform space the chroma-preserving half works in.
fn rgb_to_ictcp(rgb: vec3<f32>) -> vec3<f32> {
    let l = (rgb.r * 1688.0 + rgb.g * 2146.0 + rgb.b * 262.0) / 4096.0;
    let m = (rgb.r * 683.0 + rgb.g * 2951.0 + rgb.b * 462.0) / 4096.0;
    let s = (rgb.r * 99.0 + rgb.g * 309.0 + rgb.b * 3688.0) / 4096.0;
    let l_pq = pq_from_relative(l);
    let m_pq = pq_from_relative(m);
    let s_pq = pq_from_relative(s);
    return vec3<f32>(
        (2048.0 * l_pq + 2048.0 * m_pq) / 4096.0,
        (6610.0 * l_pq - 13613.0 * m_pq + 7003.0 * s_pq) / 4096.0,
        (17933.0 * l_pq - 17390.0 * m_pq - 543.0 * s_pq) / 4096.0,
    );
}

fn ictcp_to_rgb(ictcp: vec3<f32>) -> vec3<f32> {
    let l = ictcp.x + 0.00860904 * ictcp.y + 0.11103 * ictcp.z;
    let m = ictcp.x - 0.00860904 * ictcp.y - 0.11103 * ictcp.z;
    let s = ictcp.x + 0.560031 * ictcp.y - 0.320627 * ictcp.z;
    let l_linear = relative_from_pq(l);
    let m_linear = relative_from_pq(m);
    let s_linear = relative_from_pq(s);
    return max(
        vec3<f32>(
            3.43661 * l_linear - 2.50645 * m_linear + 0.0698454 * s_linear,
            -0.79133 * l_linear + 1.9836 * m_linear - 0.192271 * s_linear,
            -0.0259499 * l_linear - 0.0989137 * m_linear + 1.12486 * s_linear,
        ),
        vec3<f32>(0.0),
    );
}

// `GTToneMappingCurveV2::evaluateCurve`. Toe blended into a linear section by a smoothstep,
// then an exponential shoulder asymptotic to the peak.
//
// NOTE the argument order: WGSL's `smoothstep(edge0, edge1, x)` is not theirs
// (`smoothStep(x, edge0, edge1)`). Transcribing it positionally would silently invert the
// toe.
fn gt7_curve(x: f32, peak: f32) -> f32 {
    if (x < 0.0) {
        return 0.0;
    }
    let k = (GT7_LINEAR_SECTION - 1.0) / (GT7_ALPHA - 1.0);
    if (x < GT7_LINEAR_SECTION * peak) {
        let weight_linear = smoothstep(0.0, GT7_MID_POINT, x);
        let toe_mapped = GT7_MID_POINT * pow(x / GT7_MID_POINT, GT7_TOE_STRENGTH);
        return (1.0 - weight_linear) * toe_mapped + weight_linear * x;
    }
    let ka = peak * GT7_LINEAR_SECTION + peak * k;
    let kb = -peak * k * exp(GT7_LINEAR_SECTION / k);
    let kc = -1.0 / (k * peak);
    return ka + kb * exp(x * kc);
}

fn tonemap_gt7(color: vec3<f32>, headroom: f32) -> vec3<f32> {
    // `initializeAsSDR` vs `initializeAsHDR`. With no headroom their SDR path targets
    // 250 cd/m² and normalises back, which is what keeps the curve's shape on an SDR
    // display instead of degenerating.
    // `target` is a RESERVED KEYWORD in WGSL — naga rejects it as an identifier. Not
    // obvious from the C++ this was transcribed from, where it is the natural name.
    var peak_target = headroom;
    var correction = 1.0;
    if (headroom <= 1.0) {
        peak_target = GT7_SDR_TARGET;
        correction = 1.0 / GT7_SDR_TARGET;
    }

    let positive = max(color, vec3<f32>(0.0));
    let ucs = rgb_to_ictcp(positive);
    let skewed_rgb = vec3<f32>(
        gt7_curve(positive.r, peak_target),
        gt7_curve(positive.g, peak_target),
        gt7_curve(positive.b, peak_target),
    );
    let skewed_ucs = rgb_to_ictcp(skewed_rgb);

    // `framebufferLuminanceTargetUcs_` is `rgbToUcs(target,target,target).x`, and for a
    // NEUTRAL input that collapses to a single PQ encode: each ICtCp LMS row sums to
    // exactly 4096, so l = m = s = target, and I = (2048·pq + 2048·pq)/4096 = pq(target).
    // One `pow` instead of a full colour-space round trip, and exact rather than close.
    let target_ucs = pq_from_relative(peak_target);

    let chroma_scale = 1.0 - smoothstep(GT7_FADE_START, GT7_FADE_END, ucs.x / target_ucs);
    let scaled_rgb = ictcp_to_rgb(vec3<f32>(skewed_ucs.x, ucs.y * chroma_scale, ucs.z * chroma_scale));

    let blended = mix(skewed_rgb, scaled_rgb, GT7_BLEND_RATIO);
    return correction * min(blended, vec3<f32>(peak_target));
}

