// Audio processing pipeline.
//
// References for future processors:
//   - Early reflections: image-source method for BoxRoom, or ray-based via Room::cast_ray()
//   - FDN reverb: see fundsp (https://github.com/SamiPerttu/fundsp) for composable DSP graphs
//   - Ray-traced reflections: study raytraced-audio (https://github.com/whoStoleMyCoffee/raytraced-audio)
//     — persistent incremental rays, one bounce per tick, emergent room sensing
//   - Occlusion: audionimbus/Steam Audio (https://github.com/MaxenceMaire/audionimbus)
//   - Zone blending: crossfade processor params by zone weights (see idea.md §zones)
//
// See REFERENCES.md for full list.

pub mod early_reflections;
pub mod fdn_reverb;

use crate::spatial::listener::Listener;
use crate::world::ray::RayMetrics;
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

    /// Update processor parameters from ray-traced room metrics.
    /// Called once per audio buffer, before process().
    fn update_metrics(&mut self, _metrics: &RayMetrics) {}

    /// Process a buffer of interleaved samples in place.
    fn process(&mut self, buffer: &mut [f32], channels: usize, sample_rate: f32);

    /// Human-readable name for debugging.
    fn name(&self) -> &str;
}
