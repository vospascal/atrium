//! Reinhard and bounded Reinhard+HDR mappings.

// ---- Reinhard ----------------------------------------------------------------

/// `L/(1+L)` — Reinhard et al. 2002 eq. 3. Ceiling 1.0 by construction.
pub fn reinhard(luminance: f32) -> f32 {
    luminance / (1.0 + luminance)
}

/// Plain [`reinhard`] through scene white, followed by a C¹ continuation asymptotic to
/// `headroom`. At headroom 1.0 this is exactly plain Reinhard for every nonnegative input.
pub fn reinhard_headroom(luminance: f32, headroom: f32) -> f32 {
    let positive = luminance.max(0.0);
    let base = reinhard(positive);
    let room = (headroom - 1.0).max(0.0);
    let continuation = (2.0 * base - 1.0).max(0.0);
    base + room * continuation * continuation
}
