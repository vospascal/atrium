//! Per-mode pipeline definitions.
//!
//! One file per render mode, each a single `build` function that reads as the
//! definition of that mode: which resolver, which per-path effects, which
//! renderer, which post-mix stages, in order. `build_all_pipelines` is the only
//! place that lists the modes; the rest of the crate reaches this module only
//! through `build_all_pipelines` and `PipelineParams`.

pub(super) mod ambisonics;
pub(super) mod dbap;
pub(super) mod hrtf;
pub(super) mod vbap;
pub(super) mod world_locked;

use atrium_core::types::Vec3;

use super::path::{PathEffect, PathEffectFactory};
use super::render::RenderPipeline;
use super::{path_effects, room_acoustics};
use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;

/// Parameters for building pipelines (extracted from scene config).
pub struct PipelineParams {
    pub hrtf_path: String,
    pub distance_model: DistanceModel,
    /// DBAP rolloff in dB per doubling of distance.
    /// 6.0 = free-field inverse distance law, 3–5 dB for reverberant spaces.
    pub dbap_rolloff_db: f32,
    /// Room bounds for sizing delay buffers from max image-source distance.
    pub environment_min: Vec3,
    pub environment_max: Vec3,
    /// Explicit late-reverb mid-band RT60 (seconds). `None` = geometry-derived.
    pub reverb_rt60_seconds: Option<f32>,
    /// Explicit late-reverb pre-delay (milliseconds). `None` = mean-free-path.
    pub reverb_pre_delay_ms: Option<f32>,
}

impl Default for PipelineParams {
    fn default() -> Self {
        Self {
            hrtf_path: "assets/hrtf/default.sofa".into(),
            distance_model: DistanceModel::default(),
            dbap_rolloff_db: 6.0,
            environment_min: Vec3::new(-5.0, -5.0, -5.0),
            environment_max: Vec3::new(5.0, 5.0, 5.0),
            reverb_rt60_seconds: None,
            reverb_pre_delay_ms: None,
        }
    }
}

// Rendering approach. Determines which pipeline runs.
// RenderMode (atrium_core::speaker): WorldLocked, Vbap, Hrtf, Dbap, Ambisonics.
// index() and ALL defined on RenderMode. Used as pipeline array index.

/// Build all 5 pipelines. Pre-allocated at startup, zero allocation on mode switch.
pub fn build_all_pipelines(params: &PipelineParams) -> [RenderPipeline; 5] {
    [
        world_locked::build(params),
        vbap::build(params),
        hrtf::build(params),
        dbap::build(params),
        ambisonics::build(params),
    ]
}

/// Longest delay any first-order reflection in this room can have, in seconds.
///
/// Pipelines are built before the output device is opened, so delay buffers
/// are described in seconds here and sized in samples when the device's
/// sample rate arrives (`ensure_topology`). Sizing them in samples at build
/// time meant guessing a rate: at 96 kHz the buffers were half as long as the
/// room needed and long reflections arrived early.
fn longest_reflection_delay_seconds(p: &PipelineParams) -> f32 {
    let max_dist = room_acoustics::max_image_source_distance(p.environment_min, p.environment_max);
    max_dist / AtmosphericParams::default().speed_of_sound()
}

/// Where the seeds of the paths' wavering start; any number, kept apart from
/// the generators' own seeds.
const PATH_SEED: u64 = 7_000;

/// Smallest ring for a per-path propagation delay line, in samples.
const MIN_PROPAGATION_DELAY_CAPACITY: usize = 8192;

/// The per-path effects every image-source mode applies, in order: propagation
/// delay, air absorption, the air's wavering, ground effect, wall absorption.
/// VBAP, HRTF, DBAP and Ambisonics all share this exact list; WorldLocked has
/// none because it resolves a single direct path and models propagation per
/// speaker instead.
fn image_source_path_effect_factories(p: &PipelineParams) -> Vec<PathEffectFactory> {
    let longest_delay_seconds = longest_reflection_delay_seconds(p);

    vec![
        Box::new(move |sample_rate| {
            Box::new(path_effects::PropagationDelayEffect::new(
                sample_rate,
                room_acoustics::delay_capacity_for_seconds(
                    longest_delay_seconds,
                    sample_rate,
                    MIN_PROPAGATION_DELAY_CAPACITY,
                ),
            )) as Box<dyn PathEffect>
        }),
        Box::new(|sample_rate| {
            Box::new(path_effects::AirAbsorptionEffect::new(sample_rate)) as Box<dyn PathEffect>
        }),
        Box::new({
            // Every path gets a wander of its own, the same ones in the same
            // order each time the scene is built.
            let paths_built = std::sync::atomic::AtomicU64::new(0);
            move |_sample_rate| {
                let seed = paths_built.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Box::new(path_effects::TurbulenceEffect::new(PATH_SEED + seed))
                    as Box<dyn PathEffect>
            }
        }),
        Box::new(|sample_rate| {
            Box::new(path_effects::GroundEffectFilter::new(sample_rate)) as Box<dyn PathEffect>
        }),
        Box::new(|sample_rate| {
            Box::new(path_effects::WallAbsorptionEffect::new(sample_rate)) as Box<dyn PathEffect>
        }),
    ]
}
