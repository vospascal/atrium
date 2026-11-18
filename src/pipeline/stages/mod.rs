//! Concrete stage implementations.
//!
//! Each file contains stage structs implementing `MixStage`,
//! shared filters, or per-speaker processing primitives.

// MixStages (post-mix, whole-buffer)
pub mod ambi_decode;
pub mod ambi_multi_delay;
pub mod delay_comp;
pub mod fdn_reverb;
pub mod lfe_crossover;
pub mod master_gain;

// Pipeline analysis (test-only diagnostic tooling)
mod pipeline_analysis;

// Per-speaker reflections (used by WorldLockedRenderer)
pub mod reflections;

/// Sanitize to finite values, clamped to ±100.0 stability ceiling.
/// Used in measurement mode where soft clipping is bypassed but we still
/// need to prevent NaN/Inf from reaching the DAC or exploding the FDN.
#[inline]
pub fn sanitize_finite(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-100.0, 100.0)
    } else {
        0.0
    }
}

/// Soft-clip to [-1, 1] with smooth knee starting at ±0.9.
/// Linear (transparent) for normal signals; smoothly compresses peaks
/// using a rational curve x/(1+x) that asymptotes toward ±1.0.
///
/// Never returns a non-finite sample: NaN → 0, and ±Inf (or a finite input
/// so large that the knee excess overflows, above ~3.4e37) → ±1.0, the value
/// the curve has already reached in f32 long before that point.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    const WIDTH: f32 = 1.0 - KNEE; // 0.1
    if x > KNEE {
        let excess = (x - KNEE) / WIDTH;
        if excess.is_finite() {
            KNEE + WIDTH * excess / (1.0 + excess)
        } else {
            1.0
        }
    } else if x < -KNEE {
        let excess = (-x - KNEE) / WIDTH;
        if excess.is_finite() {
            -(KNEE + WIDTH * excess / (1.0 + excess))
        } else {
            -1.0
        }
    } else if x.is_nan() {
        0.0
    } else {
        x
    }
}
