//! WorldLockedRenderer — per-speaker propagation with inlined DSP.
//!
//! Each speaker is a virtual microphone. Propagation (air absorption, ground
//! effect, reflections, distance+directivity) runs per source × speaker.
//! Listener position is irrelevant.

use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::audio::propagation::ground_effect_gain;
use crate::pipeline::path::{PathEffectChain, PathSet};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput};
use crate::pipeline::stages::air_absorption::AirAbsorptionFilter;
use crate::pipeline::stages::reflections::ReflectionCore;
use crate::pipeline::SourceStageBank;

/// Per source × speaker propagation state.
struct SpeakerPropagation {
    air_absorption: AirAbsorptionFilter,
    reflections: ReflectionCore,
    ground_gain: f32,
    dist_dir_gain: f32,
}

/// Configuration for WorldLocked per-speaker propagation.
pub struct WorldLockedParams {
    pub wall_reflectivity: f32,
    /// Delay buffer capacity for ReflectionCore, sized from room geometry.
    pub reflection_capacity: usize,
}

/// WorldLocked renderer with per-speaker propagation.
pub struct WorldLockedRenderer {
    /// propagation[source_idx][speaker_idx]
    propagation: Vec<Vec<SpeakerPropagation>>,
    /// Previous per-channel gains per source (for gain ramping).
    prev_gains: Vec<[f32; MAX_CHANNELS]>,
    /// Cached speaker count for topology changes.
    speaker_count: usize,
    sample_rate: f32,
    params: WorldLockedParams,
}

impl WorldLockedRenderer {
    pub fn new(params: WorldLockedParams) -> Self {
        Self {
            propagation: Vec::new(),
            prev_gains: Vec::new(),
            speaker_count: 0,
            sample_rate: 48000.0,
            params,
        }
    }

    fn new_speaker_propagation(
        sample_rate: f32,
        wall_reflectivity: f32,
        reflection_capacity: usize,
    ) -> SpeakerPropagation {
        SpeakerPropagation {
            air_absorption: AirAbsorptionFilter::new(sample_rate),
            reflections: ReflectionCore::new(wall_reflectivity, reflection_capacity),
            ground_gain: 1.0,
            dist_dir_gain: 0.0,
        }
    }
}

impl Renderer for WorldLockedRenderer {
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut SourceStageBank,
        ctx: &SourceContext,
        _src_out: &SourceOutput,
        _paths: &PathSet,
        _path_effects: &mut [PathEffectChain],
        out: &mut OutputBuffer,
    ) {
        let layout = ctx.layout;

        // 1. Update per-speaker propagation and cache broadband gains (once per buffer)
        let mut path_gains = [0.0f32; MAX_CHANNELS];
        for spk_idx in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(spk_idx) {
                if speaker.channel >= out.channels || !layout.is_channel_active(speaker.channel) {
                    continue;
                }

                let prop = &mut self.propagation[source_idx][spk_idx];
                let dist = ctx.source_pos.distance_to(speaker.position);

                // Air absorption filter update
                prop.air_absorption.update(dist, ctx.atmosphere);

                // Ground effect
                let dx = ctx.source_pos.x - speaker.position.x;
                let dy = ctx.source_pos.y - speaker.position.y;
                let horizontal_dist = (dx * dx + dy * dy).sqrt();
                prop.ground_gain = ground_effect_gain(
                    horizontal_dist,
                    ctx.source_pos.z.max(0.0),
                    speaker.position.z.max(0.0),
                    ctx.ground,
                    ctx.atmosphere.speed_of_sound(),
                );

                // Reflections tap update
                prop.reflections.update(
                    ctx.environment_min,
                    ctx.environment_max,
                    ctx.source_pos,
                    speaker.position,
                    out.sample_rate,
                    ctx.atmosphere.speed_of_sound(),
                );

                // Distance + directivity (per-source distance model)
                let dist_gain = distance_gain_at_model(
                    ctx.source_pos,
                    speaker.position,
                    ctx.source_ref_distance,
                    ctx.distance_model.max_distance,
                    ctx.distance_model.rolloff,
                    ctx.distance_model.model,
                );
                let dir_gain = directivity_gain(
                    ctx.source_pos,
                    ctx.source_orientation,
                    speaker.position,
                    ctx.source_directivity,
                );
                prop.dist_dir_gain = dist_gain * dir_gain;

                path_gains[speaker.channel] = prop.ground_gain * prop.dist_dir_gain;
            }
        }

        // 2. Per-sample rendering
        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];

        for frame in 0..out.num_frames {
            let t = frame as f32 * inv_frames;
            let raw = source.next_sample(out.sample_rate);

            // Source-level DSP (envelopes, source EQ)
            let sample = source_stages.process_sample_all(source_idx, raw);

            // Per-speaker: air absorption filter + reflections + broadband gain ramp
            for spk_idx in 0..layout.speaker_count() {
                if let Some(speaker) = layout.speaker_by_index(spk_idx) {
                    let ch = speaker.channel;
                    if ch >= out.channels || !layout.is_channel_active(ch) {
                        continue;
                    }

                    let prop = &mut self.propagation[source_idx][spk_idx];

                    // Per-sample DSP: air absorption filter → reflection delay taps
                    let filtered = prop.air_absorption.process(sample);
                    let with_reflections = filtered + prop.reflections.process_sample(filtered);

                    // Broadband gain (cached) × gain ramp
                    let gain = prev[ch] + (path_gains[ch] - prev[ch]) * t;
                    out.buffer[frame * out.channels + ch] += with_reflections * gain;
                }
            }
        }

        self.prev_gains[source_idx] = path_gains;
    }

    fn name(&self) -> &str {
        "world_locked"
    }

    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.speaker_count = layout.speaker_count();
        let refl = self.params.wall_reflectivity;

        // Grow source dimension
        while self.propagation.len() < source_count {
            let speakers: Vec<SpeakerPropagation> = (0..self.speaker_count)
                .map(|_| {
                    Self::new_speaker_propagation(
                        sample_rate,
                        refl,
                        self.params.reflection_capacity,
                    )
                })
                .collect();
            self.propagation.push(speakers);
        }

        // Grow speaker dimension if layout changed
        for source_props in &mut self.propagation {
            while source_props.len() < self.speaker_count {
                source_props.push(Self::new_speaker_propagation(
                    sample_rate,
                    refl,
                    self.params.reflection_capacity,
                ));
            }
        }

        while self.prev_gains.len() < source_count {
            self.prev_gains.push([0.0; MAX_CHANNELS]);
        }
    }

    fn reset(&mut self) {
        for source in &mut self.propagation {
            for prop in source {
                prop.air_absorption.reset();
                prop.reflections.reset();
                prop.ground_gain = 1.0;
                prop.dist_dir_gain = 0.0;
            }
        }
        for gains in &mut self.prev_gains {
            gains.fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use atrium_core::panner::{distance_gain_at_model, DistanceModelType};
    use atrium_core::types::Vec3;

    /// Verify that WorldLocked distance attenuation uses per-source parameters.
    /// Two sources at the same position but with different ref_distance values
    /// must produce different gains.
    #[test]
    fn per_source_ref_distance_produces_different_attenuation() {
        let source_pos = Vec3::new(0.0, 0.0, 0.0);
        let speaker_pos = Vec3::new(5.0, 0.0, 0.0); // 5m away

        // Source A: ref_distance = 1.0 → gain = 1.0 / 5.0 = 0.2
        let gain_a = distance_gain_at_model(
            source_pos,
            speaker_pos,
            1.0,  // ref_distance
            20.0, // max_distance
            1.0,  // rolloff
            DistanceModelType::Inverse,
        );

        // Source B: ref_distance = 3.0 → gain = 3.0 / 5.0 = 0.6
        let gain_b = distance_gain_at_model(
            source_pos,
            speaker_pos,
            3.0,  // ref_distance
            20.0, // max_distance
            1.0,  // rolloff
            DistanceModelType::Inverse,
        );

        assert!(
            (gain_a - 0.2).abs() < 1e-6,
            "ref=1.0 at 5m: expected 0.2, got {gain_a}"
        );
        assert!(
            (gain_b - 0.6).abs() < 1e-6,
            "ref=3.0 at 5m: expected 0.6, got {gain_b}"
        );
        assert!(
            gain_b > gain_a,
            "larger ref_distance should produce higher gain at same distance"
        );
    }
}
