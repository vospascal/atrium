// Audio processing pipeline.
//
// References for future processors:
//   - Early reflections: image-source method for BoxRoom
//   - FDN reverb: see fundsp (https://github.com/SamiPerttu/fundsp) for composable DSP graphs
//   - Occlusion: audionimbus/Steam Audio (https://github.com/MaxenceMaire/audionimbus)
//   - Zone blending: crossfade processor params by zone weights (see idea.md §zones)
//
// See REFERENCES.md for full list.

pub mod early_reflections;
pub mod fdn_reverb;

use crate::spatial::listener::Listener;
use crate::world::types::Vec3;

/// Trait for audio processing stages that transform the mixed signal.
pub trait AudioProcessor: Send {
    /// Called once when sample_rate and room geometry are known (before audio callback starts).
    /// Default no-op for processors that don't need spatial info.
    fn init(
        &mut self,
        _room_min: Vec3,
        _room_max: Vec3,
        _listener: &Listener,
        _sample_rate: f32,
    ) {
    }

    /// Process a buffer of interleaved samples in place.
    fn process(&mut self, buffer: &mut [f32], channels: usize, sample_rate: f32);

    /// Clear all internal delay lines / state to silence.
    fn reset(&mut self) {}

    /// Human-readable name for debugging.
    fn name(&self) -> &str;
}
