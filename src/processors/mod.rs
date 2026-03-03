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

/// Trait for audio processing stages that transform the mixed signal.
/// Phase 1: defined but not used — no processors active.
/// Future: EarlyReflections, FdnReverb, RayTracedReflections, ZoneBlender.
pub trait AudioProcessor: Send {
    /// Process a buffer of interleaved samples in place.
    fn process(&mut self, buffer: &mut [f32], channels: usize, sample_rate: f32);

    /// Human-readable name for debugging.
    fn name(&self) -> &str;
}
