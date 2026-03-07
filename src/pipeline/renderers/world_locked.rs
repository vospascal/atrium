//! WorldLockedRenderer — per-speaker propagation with inlined DSP.
//!
//! Each speaker is a virtual microphone. Propagation (air absorption, ground
//! effect, reflections, distance+directivity) runs per source × speaker.
//! Listener position is irrelevant.

use atrium_core::directivity::directivity_gain;
use atrium_core::panner::{distance_gain_at_model, DistanceModelType};
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::audio::propagation::ground_effect_gain;
use crate::pipeline::path::PathSet;
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};
use crate::pipeline::stages::air_absorption::AirAbsorptionFilter;
use crate::pipeline::stages::reflections::ReflectionCore;

/// Per source × speaker propagation state.
struct SpeakerPropagation {
    air_absorption: AirAbsorptionFilter,
    reflections: ReflectionCore,
    ground_gain: f32,
    dist_dir_gain: f32,
}

/// Configuration for WorldLocked per-speaker propagation.
pub struct WorldLockedParams {
    pub ref_distance: f32,
    pub max_distance: f32,
    pub rolloff: f32,
    pub model: DistanceModelType,
    pub wet_gain: f32,
    pub wall_reflectivity: f32,
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
        wet_gain: f32,
        wall_reflectivity: f32,
    ) -> SpeakerPropagation {
        SpeakerPropagation {
            air_absorption: AirAbsorptionFilter::new(sample_rate),
            reflections: ReflectionCore::new(wet_gain, wall_reflectivity),
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
        source_stages: &mut [&mut dyn SourceStage],
        ctx: &SourceContext,
        _src_out: &SourceOutput,
        _paths: &PathSet,
        out: &mut OutputBuffer,
    ) {
        let layout = ctx.layout;

        // 1. Update per-speaker propagation and cache broadband gains (once per buffer)
        let mut path_gains = [0.0f32; MAX_CHANNELS];
        for spk_idx in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(spk_idx) {
                if !layout.is_channel_active(speaker.channel) {
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
                );

                // Reflections tap update
                prop.reflections.update(
                    ctx.room_min,
                    ctx.room_max,
                    ctx.source_pos,
                    speaker.position,
                    out.sample_rate,
                );

                // Distance + directivity
                let dist_gain = distance_gain_at_model(
                    ctx.source_pos,
                    speaker.position,
                    self.params.ref_distance,
                    self.params.max_distance,
                    self.params.rolloff,
                    self.params.model,
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
            let mut sample = raw;
            for stage in source_stages.iter_mut() {
                sample = stage.process_sample(sample);
            }

            // Per-speaker: air absorption filter + reflections + broadband gain ramp
            for spk_idx in 0..layout.speaker_count() {
                if let Some(speaker) = layout.speaker_by_index(spk_idx) {
                    let ch = speaker.channel;
                    if !layout.is_channel_active(ch) {
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
        let wet = self.params.wet_gain;
        let refl = self.params.wall_reflectivity;

        // Grow source dimension
        while self.propagation.len() < source_count {
            let speakers: Vec<SpeakerPropagation> = (0..self.speaker_count)
                .map(|_| Self::new_speaker_propagation(sample_rate, wet, refl))
                .collect();
            self.propagation.push(speakers);
        }

        // Grow speaker dimension if layout changed
        for source_props in &mut self.propagation {
            while source_props.len() < self.speaker_count {
                source_props.push(Self::new_speaker_propagation(sample_rate, wet, refl));
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
