//! A silent placeholder source for empty slots in the 16-slot source pool.
//!
//! The audio scene keeps a fixed-size pool of source slots so that live
//! add/remove never grows the parallel vectors or pipeline topology on the
//! audio thread (that would allocate). Empty slots hold a `SilenceNode`, which
//! reports `is_active() == false` so `render_pipeline` skips it entirely and it
//! contributes nothing to telemetry's active mask.

use crate::world::types::Vec3;
use atrium_core::source::SoundSource;

/// A source that produces no audio and is never active. Occupies a free slot
/// in the source pool until a real source replaces it.
pub struct SilenceNode;

impl SoundSource for SilenceNode {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        0.0
    }

    fn position(&self) -> Vec3 {
        Vec3::new(0.0, 0.0, 0.0)
    }

    /// Inactive → `render_pipeline` and the perceptual layer skip this slot.
    fn is_active(&self) -> bool {
        false
    }

    fn tick(&mut self, _dt: f32) {}
}
