//! PQ helpers and the ITU-R BT.2390 display mapping.

// ---- PQ (SMPTE ST.2084) --------------------------------------------------------
// The constants are exact binary rationals — 2610/2^14 and so on — so every digit is
// representable rather than spurious. Clippy counts decimal digits and cannot see that;
// truncating them would introduce the very error the lint exists to prevent.

#[allow(clippy::excessive_precision)]
const PQ_M1: f32 = 0.1593017578125; // 2610/16384
const PQ_M2: f32 = 78.84375; // 2523/4096 * 128
const PQ_C1: f32 = 0.8359375; // 3424/4096
#[allow(clippy::excessive_precision)]
const PQ_C2: f32 = 18.8515625; // 2413/4096 * 32
const PQ_C3: f32 = 18.6875; // 2392/4096 * 32

/// PQ is ABSOLUTE: signal 1.0 is 10 000 cd/m². Our framebuffer convention is
/// `physical / 100`, so relative 1.0 is SDR reference white and relative 100 is the PQ
/// ceiling. Both conversions below bridge exactly that.
pub(crate) const PQ_RELATIVE_CEILING: f32 = 100.0;

/// Relative luminance (1.0 = SDR reference white) to a PQ signal in `[0, 1]`.
pub fn pq_from_relative(value: f32) -> f32 {
    let luminance = (value / PQ_RELATIVE_CEILING).clamp(0.0, 1.0);
    let shaped = luminance.powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * shaped) / (1.0 + PQ_C3 * shaped)).powf(PQ_M2)
}

/// The inverse of [`pq_from_relative`].
pub fn relative_from_pq(signal: f32) -> f32 {
    let shaped = signal.max(0.0).powf(1.0 / PQ_M2);
    let numerator = (shaped - PQ_C1).max(0.0);
    let denominator = (PQ_C2 - PQ_C3 * shaped).max(1.0e-6);
    (numerator / denominator).powf(1.0 / PQ_M1) * PQ_RELATIVE_CEILING
}

// ---- BT.2390 -------------------------------------------------------------------

/// ITU-R BT.2390-4's EETF, per channel, computed in PQ.
///
/// The knee is **computed from the display**: `KS = 1.5·maxLum − 0.5`, where `maxLum` is
/// the display peak as a fraction of the content peak. A brighter panel pushes it later
/// and compresses less — the property a hand-placed knee cannot have.
///
/// Two documented simplifications against the spec, both because their inputs do not
/// exist here: black level is taken as zero at both ends (no platform we probe reports
/// one), and it is applied per channel rather than on luminance.
pub fn bt2390(value: f32, display_peak: f32, content_peak: f32) -> f32 {
    let content_pq = pq_from_relative(content_peak);
    // Nothing to map: the display can already show everything the content holds. Returned
    // EXACTLY, not approximately — selecting this curve on a capable display must not
    // silently alter a picture that needed no alteration.
    if content_pq <= 1.0e-6 || content_peak <= display_peak {
        return value;
    }
    let max_lum = pq_from_relative(display_peak) / content_pq;
    let e1 = pq_from_relative(value) / content_pq;
    let knee_start = 1.5 * max_lum - 0.5;
    let mut e2 = e1;
    if e1 >= knee_start && knee_start < 1.0 {
        let t = (e1 - knee_start) / (1.0 - knee_start);
        let (t2, t3) = (t * t, t * t * t);
        e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * knee_start
            + (t3 - 2.0 * t2 + t) * (1.0 - knee_start)
            + (-2.0 * t3 + 3.0 * t2) * max_lum;
    }
    relative_from_pq(e2 * content_pq)
}
