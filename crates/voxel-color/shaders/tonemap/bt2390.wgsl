// ---- ITU-R BT.2390 EETF ------------------------------------------------------
//
// The standards answer, and the only curve here with a COMPUTED knee point rather than
// one pinned by hand. Everything is done in PQ (SMPTE ST.2084), which is the point: a
// knee placed in perceptually-uniform space lands where the eye expects it, whereas the
// same knee in linear light does not.
//
// PQ is ABSOLUTE — signal 1.0 is 10 000 cd/m² — while our pipeline is relative, with 1.0
// = SDR reference white = 100 cd/m². So `PQ_RELATIVE_CEILING` = 10000/100 = 100 is the
// bridge, and every conversion goes through it.
const PQ_M1: f32 = 0.1593017578125;   // 2610/16384
const PQ_M2: f32 = 78.84375;          // 2523/4096 * 128
const PQ_C1: f32 = 0.8359375;         // 3424/4096
const PQ_C2: f32 = 18.8515625;        // 2413/4096 * 32
const PQ_C3: f32 = 18.6875;           // 2392/4096 * 32
const PQ_RELATIVE_CEILING: f32 = 100.0;

// Our relative luminance -> PQ signal (ST.2084 inverse EOTF).
fn pq_from_relative(value: f32) -> f32 {
    let luminance = clamp(value / PQ_RELATIVE_CEILING, 0.0, 1.0);
    let shaped = pow(luminance, PQ_M1);
    return pow((PQ_C1 + PQ_C2 * shaped) / (1.0 + PQ_C3 * shaped), PQ_M2);
}

// PQ signal -> our relative luminance (ST.2084 EOTF).
fn relative_from_pq(signal: f32) -> f32 {
    // Clamped to [0, 1] as ST.2084 specifies. BT.2390 never feeds it more than 1, but the
    // ICtCp inverse below can after a chroma scale, and an unclamped signal there produces
    // a negative base for `pow`.
    let shaped = pow(clamp(signal, 0.0, 1.0), 1.0 / PQ_M2);
    let numerator = max(shaped - PQ_C1, 0.0);
    let denominator = max(PQ_C2 - PQ_C3 * shaped, 1.0e-6);
    return pow(numerator / denominator, 1.0 / PQ_M1) * PQ_RELATIVE_CEILING;
}

// BT.2390-4 section 5.4.1, with black level taken as zero at both ends.
//
// That simplification is worth naming: the spec carries `minLum` for the display's black
// and a matching `E3 = E2 + minLum*(1-E2)^4` black lift. No platform we probe reports a
// black level — macOS and Android give a ratio only, and DXGI's `MinLuminance` is widely
// reported as 0 — so carrying the term would mean inventing its input. With minLum = 0 the
// lift is identity and the normalisation collapses to a single divide.
//
// KS is THE COMPUTED KNEE: `1.5 * maxLum - 0.5`, derived from how much of the content's
// range the display can actually show. Below it the signal passes through untouched;
// above it a cubic Hermite spline carries it to `maxLum`, C¹ at the join by construction.
// That is the part a hand-placed knee cannot replicate — where it sits depends on the
// display, so it moves when the display does.
fn bt2390_channel(value: f32, display_peak: f32, content_peak: f32) -> f32 {
    let content_pq = pq_from_relative(content_peak);
    // A content peak at or below the display's is nothing to compress; passing it through
    // avoids both a divide by ~0 and a pointless round trip through PQ.
    if (content_pq <= 1.0e-6 || content_peak <= display_peak) {
        return value;
    }
    let max_lum = pq_from_relative(display_peak) / content_pq;
    let e1 = pq_from_relative(value) / content_pq;
    let knee_start = 1.5 * max_lum - 0.5;

    var e2 = e1;
    // KS >= 1 means the display covers the content and the spline never engages.
    if (e1 >= knee_start && knee_start < 1.0) {
        let t = (e1 - knee_start) / (1.0 - knee_start);
        let t2 = t * t;
        let t3 = t2 * t;
        e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * knee_start
           + (t3 - 2.0 * t2 + t) * (1.0 - knee_start)
           + (-2.0 * t3 + 3.0 * t2) * max_lum;
    }
    return relative_from_pq(e2 * content_pq);
}

// Applied PER CHANNEL rather than on luminance. Per-channel is what most implementations
// do and it keeps the curve monotonic in each channel; the cost is that a saturated
// highlight desaturates as it rolls off, because the brightest channel compresses hardest.
// Luminance-preserving variants exist and would be the next refinement, not a correction.
fn tonemap_bt2390(color: vec3<f32>, display_peak: f32, content_peak: f32) -> vec3<f32> {
    let positive = max(color, vec3<f32>(0.0));
    return vec3<f32>(
        bt2390_channel(positive.x, display_peak, content_peak),
        bt2390_channel(positive.y, display_peak, content_peak),
        bt2390_channel(positive.z, display_peak, content_peak),
    );
}

