//! The C¹ HDR knee mapping.

// ---- Our knee ----------------------------------------------------------------

/// Identity to white, then a C¹ hyperbolic shoulder asymptotic to `headroom`. Ours, not a
/// standard.
pub fn hdr_knee(luminance: f32, headroom: f32) -> f32 {
    let room = (headroom - 1.0).max(0.0);
    let positive = luminance.max(0.0);
    let mids = positive.min(1.0);
    let highs = (positive - 1.0).max(0.0);
    // `max` on the denominator, not for precision but for CORRECTNESS at zero headroom:
    // `room` and `highs` are then both 0 and the shoulder term is 0/0. A float surface
    // stores the resulting NaN; unorm would have hidden it.
    mids + highs * room / (room + highs).max(1.0e-6)
}
