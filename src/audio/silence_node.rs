//! A silent placeholder source for empty slots in the 16-slot source pool.
//!
//! The audio scene keeps a fixed-size pool of source slots so that live
//! add/remove never grows the parallel vectors or pipeline topology on the
//! audio thread (that would allocate). Empty slots hold a `SilenceNode`, which
//! reports `is_active() == false` so `render_pipeline` skips it entirely and it
//! contributes nothing to telemetry's active mask.

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A placeholder slot must contribute exactly nothing and be skipped.
    #[test]
    fn silence_node_is_inactive_and_outputs_exact_zero() {
        let mut node = SilenceNode;
        assert!(!node.is_active());
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            for _ in 0..64 {
                assert_eq!(node.next_sample(sample_rate), 0.0);
            }
        }
        node.tick(0.01);
        assert_eq!(node.position(), Vec3::new(0.0, 0.0, 0.0));
        // Defaults of the trait: unmuted, no spread, unit reference distance.
        assert!(!node.is_muted());
        assert_eq!(node.spread(), 0.0);
        assert_eq!(node.ref_distance(), 1.0);
    }
}
