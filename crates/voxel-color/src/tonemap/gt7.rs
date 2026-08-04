//! GT7's ICtCp colour-volume mapping.

use super::bt2390::{pq_from_relative, relative_from_pq};

// ---- GT7 -----------------------------------------------------------------------
// Transcribed from the SIGGRAPH 2025 course's own supplemental `gt7_tone_mapping.cpp`,
// not reconstructed. Their framebuffer convention is `physical / 100`, identical to ours,
// so `peakIntensity` is our headroom ratio with nothing to convert.

const GT7_ALPHA: f32 = 0.25;
const GT7_MID_POINT: f32 = 0.538;
const GT7_LINEAR_SECTION: f32 = 0.444;
const GT7_TOE_STRENGTH: f32 = 1.280;
const GT7_BLEND_RATIO: f32 = 0.6;
const GT7_FADE_START: f32 = 0.98;
const GT7_FADE_END: f32 = 1.16;
/// Their `initializeAsSDR` targets 250 cd/m² and normalises back by `1/2.5`. Dropping the
/// correction would flatten the curve rather than fit it to the display.
const GT7_SDR_TARGET: f32 = 2.5;

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// GT7's scalar curve: a power toe blended into a linear section, then an exponential
/// shoulder to `peak`. This is the per-channel half of [`gt7`], exposed because comparing
/// it against the full operator is what demonstrates the chroma preservation.
pub fn gt7_curve(x: f32, peak: f32) -> f32 {
    if x < 0.0 {
        return 0.0;
    }
    let k = (GT7_LINEAR_SECTION - 1.0) / (GT7_ALPHA - 1.0);
    if x < GT7_LINEAR_SECTION * peak {
        let weight_linear = smoothstep(0.0, GT7_MID_POINT, x);
        let toe = GT7_MID_POINT * (x / GT7_MID_POINT).powf(GT7_TOE_STRENGTH);
        return (1.0 - weight_linear) * toe + weight_linear * x;
    }
    let ka = peak * GT7_LINEAR_SECTION + peak * k;
    let kb = -peak * k * (GT7_LINEAR_SECTION / k).exp();
    let kc = -1.0 / (k * peak);
    ka + kb * (x * kc).exp()
}

/// Linear Rec.709 RGB to ICtCp. The LMS rows are the ICtCp matrix over 4096; each sums to
/// exactly 4096, which is what licenses the shader's `rgbToUcs(t,t,t).x == pq(t)` shortcut.
pub fn rgb_to_ictcp(rgb: [f32; 3]) -> [f32; 3] {
    let l = (rgb[0] * 1688.0 + rgb[1] * 2146.0 + rgb[2] * 262.0) / 4096.0;
    let m = (rgb[0] * 683.0 + rgb[1] * 2951.0 + rgb[2] * 462.0) / 4096.0;
    let s = (rgb[0] * 99.0 + rgb[1] * 309.0 + rgb[2] * 3688.0) / 4096.0;
    let (lp, mp, sp) = (
        pq_from_relative(l),
        pq_from_relative(m),
        pq_from_relative(s),
    );
    [
        (2048.0 * lp + 2048.0 * mp) / 4096.0,
        (6610.0 * lp - 13613.0 * mp + 7003.0 * sp) / 4096.0,
        (17933.0 * lp - 17390.0 * mp - 543.0 * sp) / 4096.0,
    ]
}

/// The inverse of [`rgb_to_ictcp`].
pub fn ictcp_to_rgb(ictcp: [f32; 3]) -> [f32; 3] {
    let l = ictcp[0] + 0.00860904 * ictcp[1] + 0.11103 * ictcp[2];
    let m = ictcp[0] - 0.00860904 * ictcp[1] - 0.11103 * ictcp[2];
    let s = ictcp[0] + 0.560031 * ictcp[1] - 0.320627 * ictcp[2];
    let (ll, ml, sl) = (
        relative_from_pq(l.clamp(0.0, 1.0)),
        relative_from_pq(m.clamp(0.0, 1.0)),
        relative_from_pq(s.clamp(0.0, 1.0)),
    );
    [
        (3.43661 * ll - 2.50645 * ml + 0.0698454 * sl).max(0.0),
        (-0.79133 * ll + 1.9836 * ml - 0.192271 * sl).max(0.0),
        (-0.0259499 * ll - 0.0989137 * ml + 1.12486 * sl).max(0.0),
    ]
}

/// The full GT7 colour-volume operator: [`gt7_curve`] per channel, blended 60/40 toward a
/// chroma-preserving pass through ICtCp.
///
/// **The only operator here that mixes channels**, which is why it takes the triple and
/// why it keeps highlight hue where every per-channel curve desaturates. At `headroom <=
/// 1.0` it targets `GT7_SDR_TARGET` and normalises back, so an SDR display still gets
/// the curve's shape rather than a degenerate one.
pub fn gt7(rgb: [f32; 3], headroom: f32) -> [f32; 3] {
    let (peak_target, correction) = if headroom <= 1.0 {
        (GT7_SDR_TARGET, 1.0 / GT7_SDR_TARGET)
    } else {
        (headroom, 1.0)
    };
    let positive = [rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)];
    let ucs = rgb_to_ictcp(positive);
    let skewed = [
        gt7_curve(positive[0], peak_target),
        gt7_curve(positive[1], peak_target),
        gt7_curve(positive[2], peak_target),
    ];
    let skewed_ucs = rgb_to_ictcp(skewed);
    // Exact, because each ICtCp LMS row sums to 4096 — pinned by
    // `the_ictcp_lms_rows_sum_to_one_which_is_why_the_shader_shortcut_holds`.
    let target_ucs = pq_from_relative(peak_target);
    let chroma_scale = 1.0 - smoothstep(GT7_FADE_START, GT7_FADE_END, ucs[0] / target_ucs);
    let scaled = ictcp_to_rgb([skewed_ucs[0], ucs[1] * chroma_scale, ucs[2] * chroma_scale]);
    let mut out = [0.0; 3];
    for channel in 0..3 {
        let blended = (1.0 - GT7_BLEND_RATIO) * skewed[channel] + GT7_BLEND_RATIO * scaled[channel];
        out[channel] = correction * blended.min(peak_target);
    }
    out
}
