//! Hable's Uncharted 2 filmic mapping.

// ---- Hable ---------------------------------------------------------------------

pub(crate) fn hable_partial(value: f32) -> f32 {
    let (a, b, c, d, e, f) = (0.15, 0.50, 0.10, 0.20, 0.02, 0.30);
    ((value * (a * value + c * b) + d * e) / (value * (a * value + b) + d * f)) - e / f
}

/// Hable's Uncharted 2 filmic curve, with the 2.0 exposure bias and the `/ f(W)`
/// normalisation. An **SDR** operator: the normalisation pins its ceiling at 1.0.
///
/// The final `min` is load-bearing and not in the original — see
/// `tests::hable_overshoots_white_before_the_clamp_which_is_why_there_is_one`.
pub fn hable(luminance: f32) -> f32 {
    let positive = luminance.max(0.0);
    (hable_partial(positive * 2.0) / hable_partial(11.2)).min(1.0)
}
