//! Per source × output path processing stage.
//!
//! PathStages model propagation along a specific path (source → target point).
//! They live inside the Renderer, not alongside SourceStages.
//!
//! - **WorldLocked**: N_speakers × N_sources instances (source → each speaker)
//! - **Listener-relative modes**: N_sources instances (source → listener)
//!
//! This separation exists because WorldLocked propagation is per-speaker:
//! air absorption, ground effect, and reflections all differ for each speaker
//! since each has a different distance/geometry to the source.

use atrium_core::directivity::DirectivityPattern;
use atrium_core::types::Vec3;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::propagation::GroundProperties;

/// Context for a single propagation path (source → target point).
///
/// Uses a struct rather than loose parameters so the interface stays stable
/// as the engine grows (occlusion, non-flat ground, barriers, etc.).
pub struct PathContext<'a> {
    pub source_pos: Vec3,
    pub target_pos: Vec3,
    pub source_orientation: Vec3,
    pub source_directivity: &'a DirectivityPattern,
    pub atmosphere: &'a AtmosphericParams,
    pub ground: &'a GroundProperties,
    pub room_min: Vec3,
    pub room_max: Vec3,
    pub sample_rate: f32,
}

/// Per-path processing stage.
///
/// `update()` runs once per buffer per path (buffer-rate parameter updates).
/// `process_sample()` runs per sample in the inner loop (filters, delays).
/// `gain_modifier()` returns a broadband gain cached per buffer — not per sample.
pub trait PathStage: Send {
    /// Update for a specific propagation path. Called once per buffer per path.
    fn update(&mut self, ctx: &PathContext);

    /// Per-sample DSP for this path (filters, delays).
    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        sample
    }

    /// Broadband gain modifier for this path. Cached per buffer, not per sample.
    fn gain_modifier(&self) -> f32 {
        1.0
    }

    /// Human-readable name for profiling/debugging.
    fn name(&self) -> &str;

    /// Reset internal state (delay lines, filter coefficients).
    fn reset(&mut self) {}
}
