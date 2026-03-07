//! Concrete stage implementations.
//!
//! Each file contains one or more stage structs implementing
//! `SourceStage` or `MixStage`.

// MixStages (post-mix, whole-buffer)
pub mod ambi_decode;
pub mod ambi_multi_delay;
pub mod delay_comp;
pub mod fdn_reverb;
pub mod lfe_crossover;
pub mod master_gain;

// SourceStages (per-source, before routing)
pub mod air_absorption;
pub mod ground_effect;
pub mod reflections;

/// Soft-clip to [-1, 1] with smooth knee starting at ±0.9.
/// Linear (transparent) for normal signals; smoothly compresses peaks
/// using a rational curve x/(1+x) that asymptotes toward ±1.0.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    const WIDTH: f32 = 1.0 - KNEE; // 0.1
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
