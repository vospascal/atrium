//! The feedback path: the lossless mixing matrix and the soft clamp that
//! keeps the recirculating signal bounded.

use super::delay_lines::NUM_LINES;

/// In-place fast Walsh–Hadamard transform, normalized to be orthonormal.
/// This is the FDN's lossless mixing matrix; it works for any power-of-two
/// `NUM_LINES` (the old code hardcoded the 8-point butterfly).
#[inline(always)]
pub(super) fn hadamard(v: &mut [f32; NUM_LINES]) {
    let mut h = 1;
    while h < NUM_LINES {
        let mut i = 0;
        while i < NUM_LINES {
            for j in i..i + h {
                let a = v[j];
                let b = v[j + h];
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += h * 2;
        }
        h *= 2;
    }
    let scale = 1.0 / (NUM_LINES as f32).sqrt();
    for x in v.iter_mut() {
        *x *= scale;
    }
}

/// Soft clamp for FDN feedback loop stability. Generous ±16.0 ceiling
/// (~24 dB headroom) with smooth knee starting at ±14.0. Uses the same
/// rational saturation curve as `soft_clip()` but at a much wider range.
///
/// Hard clamping at ±4.0 introduced discontinuities that compound through
/// the feedback loop, producing audible distortion. This soft version
/// asymptotes smoothly toward ±16.0, so fed-back signal remains smooth
/// even when the clamp activates.
#[inline(always)]
pub(super) fn feedback_soft_clamp(x: f32) -> f32 {
    const CEILING: f32 = 16.0;
    const KNEE: f32 = 14.0;
    const WIDTH: f32 = CEILING - KNEE; // 2.0
    if x > KNEE {
        let excess = (x - KNEE) / WIDTH;
        KNEE + WIDTH * excess / (1.0 + excess)
    } else if x < -KNEE {
        let excess = (-x - KNEE) / WIDTH;
        -(KNEE + WIDTH * excess / (1.0 + excess))
    } else {
        x
    }
}
