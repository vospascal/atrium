//! WorldLockedRenderer — per-speaker PathStages + gain ramp.
//!
//! Each speaker is a virtual microphone. Propagation (air absorption, ground
//! effect, reflections, distance+directivity) runs per source × speaker.
//! Listener position is irrelevant.
//!
//! State: path_stages[source_idx][speaker_idx] = Vec<Box<dyn PathStage>>

use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::path_stage::{PathContext, PathStage};
use crate::pipeline::renderer::Renderer;
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Factory function type for creating PathStage instances.
type PathFactory = Box<dyn Fn(f32) -> Box<dyn PathStage> + Send>;

/// WorldLocked renderer with per-speaker propagation paths.
pub struct WorldLockedRenderer {
    /// path_stages[source_idx][speaker_idx] = Vec<Box<dyn PathStage>>
    path_stages: Vec<Vec<Vec<Box<dyn PathStage>>>>,
    /// Factories for creating PathStage instances per path.
    factories: Vec<PathFactory>,
    /// Previous per-channel gains per source (for gain ramping).
    prev_gains: Vec<[f32; MAX_CHANNELS]>,
    /// Cached speaker count for topology changes.
    speaker_count: usize,
    sample_rate: f32,
}

impl WorldLockedRenderer {
    pub fn new(factories: Vec<PathFactory>) -> Self {
        Self {
            path_stages: Vec::new(),
            factories,
            prev_gains: Vec::new(),
            speaker_count: 0,
            sample_rate: 48000.0,
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
        buffer: &mut [f32],
        channels: usize,
        num_frames: usize,
        sample_rate: f32,
    ) {
        let layout = ctx.layout;

        // 1. Update all path stages and cache broadband gains (once per buffer)
        let mut path_gains = [0.0f32; MAX_CHANNELS];
        for spk_idx in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(spk_idx) {
                if !layout.is_channel_active(speaker.channel) {
                    continue;
                }
                let path_ctx = PathContext {
                    source_pos: ctx.source_pos,
                    target_pos: speaker.position,
                    source_orientation: ctx.source_orientation,
                    source_directivity: ctx.source_directivity,
                    atmosphere: ctx.atmosphere,
                    ground: ctx.ground,
                    room_min: ctx.room_min,
                    room_max: ctx.room_max,
                    sample_rate,
                };

                let mut combined_gain = 1.0f32;
                if let Some(stages) = self
                    .path_stages
                    .get_mut(source_idx)
                    .and_then(|s| s.get_mut(spk_idx))
                {
                    for path_stage in stages.iter_mut() {
                        path_stage.update(&path_ctx);
                        combined_gain *= path_stage.gain_modifier();
                    }
                }
                path_gains[speaker.channel] = combined_gain;
            }
        }

        // 2. Per-sample rendering
        let inv_frames = 1.0 / num_frames as f32;
        let prev = &self.prev_gains[source_idx];

        for frame in 0..num_frames {
            let t = frame as f32 * inv_frames;
            let raw = source.next_sample(sample_rate);

            // Source-level DSP (envelopes, source EQ)
            let mut sample = raw;
            for stage in source_stages.iter_mut() {
                sample = stage.process_sample(sample);
            }

            // Per-speaker: path DSP (filters/delays) + cached broadband gain + ramp
            for spk_idx in 0..layout.speaker_count() {
                if let Some(speaker) = layout.speaker_by_index(spk_idx) {
                    let ch = speaker.channel;
                    if !layout.is_channel_active(ch) {
                        continue;
                    }
                    let mut spk_sample = sample;

                    // Per-sample path DSP (air absorption filter, reflection delays)
                    if let Some(stages) = self
                        .path_stages
                        .get_mut(source_idx)
                        .and_then(|s| s.get_mut(spk_idx))
                    {
                        for path_stage in stages.iter_mut() {
                            spk_sample = path_stage.process_sample(spk_sample);
                        }
                    }

                    // Broadband gain (cached) × gain ramp
                    let gain = prev[ch] + (path_gains[ch] - prev[ch]) * t;
                    buffer[frame * channels + ch] += spk_sample * gain;
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

        // Grow source dimension
        while self.path_stages.len() < source_count {
            let mut speakers = Vec::with_capacity(self.speaker_count);
            for _ in 0..self.speaker_count {
                let stages: Vec<Box<dyn PathStage>> =
                    self.factories.iter().map(|f| f(sample_rate)).collect();
                speakers.push(stages);
            }
            self.path_stages.push(speakers);
        }

        // Grow speaker dimension if layout changed
        for source_stages in &mut self.path_stages {
            while source_stages.len() < self.speaker_count {
                let stages: Vec<Box<dyn PathStage>> =
                    self.factories.iter().map(|f| f(sample_rate)).collect();
                source_stages.push(stages);
            }
        }

        while self.prev_gains.len() < source_count {
            self.prev_gains.push([0.0; MAX_CHANNELS]);
        }
    }

    fn reset(&mut self) {
        for source in &mut self.path_stages {
            for speaker in source {
                for stage in speaker {
                    stage.reset();
                }
            }
        }
        for gains in &mut self.prev_gains {
            gains.fill(0.0);
        }
    }
}
