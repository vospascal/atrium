//! WorldLockedRenderer — the speakers are microphones standing in the world.
//!
//! Each speaker plays what a microphone at its position would pick up, so the
//! listener's position is irrelevant: the sound field is locked to the room.
//! Per source × speaker:
//!
//! 1. **Arrival delay** `distance / c`. A far sound reaches the near side of
//!    the room first; that time difference is what carries direction. It is
//!    ramped per sample, so a moving source gets its Doppler shift for free.
//! 2. **Outward-facing pickup.** Like a spaced surround array, each microphone
//!    faces away from the room centre. For sources inside the speaker ring the
//!    pickup is omnidirectional (a fly next to a speaker is loudest there,
//!    whichever way that speaker faces); it becomes a full cardioid by twice
//!    the ring radius. Without this a distant source arrives at near-equal
//!    level in every speaker and carries almost no direction.
//! 3. Air absorption, first-order reflections, distance law and source
//!    directivity, each for the source → speaker path. The ISO ground effect
//!    applies only where the floor bounce has no reflection tap of its own.

use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};
use atrium_core::types::Vec3;

use crate::audio::filters::AirAbsorptionFilter;
use crate::audio::propagation::ground_effect_gain;
use crate::pipeline::path::{PathEffectChain, PathSet};
use crate::pipeline::renderer::SourceContext;
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::room_acoustics;
use crate::pipeline::stages::reflections::ReflectionCore;

/// Per source × speaker propagation state.
struct SpeakerPropagation {
    air_absorption: AirAbsorptionFilter,
    reflections: ReflectionCore,
    /// Arrival delay in samples at the end of the previous buffer.
    /// `None` until the first buffer, so a new source starts at its true delay
    /// instead of sweeping up from zero.
    delay_samples: Option<f32>,
}

/// The dry signal of one source, kept long enough for the farthest speaker.
struct SourceDelayLine {
    buffer: Box<[f32]>,
    mask: usize,
    write_pos: usize,
}

impl SourceDelayLine {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2).next_power_of_two();
        Self {
            buffer: vec![0.0; capacity].into_boxed_slice(),
            mask: capacity - 1,
            write_pos: 0,
        }
    }

    /// Longest delay that can be read back, in samples.
    fn max_delay(&self) -> f32 {
        (self.buffer.len() - 2) as f32
    }

    #[inline]
    fn write(&mut self, sample: f32) {
        self.write_pos = (self.write_pos + 1) & self.mask;
        self.buffer[self.write_pos] = sample;
    }

    /// Read `delay` samples behind the newest sample, linearly interpolated.
    #[inline]
    fn read(&self, delay: f32) -> f32 {
        let whole = delay as usize;
        let fraction = delay - whole as f32;
        let newer = self.buffer[(self.write_pos + self.buffer.len() - whole) & self.mask];
        let older = self.buffer[(self.write_pos + self.buffer.len() - whole - 1) & self.mask];
        newer + (older - newer) * fraction
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }
}

/// Configuration for WorldLocked per-speaker propagation.
pub struct WorldLockedParams {
    /// Longest delay a reflection or a source → speaker arrival can have in
    /// this room, in seconds. Delay lines are sized from it in samples once
    /// the device sample rate is known (`ensure_topology`).
    pub longest_delay_seconds: f32,
}

/// Smallest ring for the per-speaker reflection taps, in samples.
const MIN_REFLECTION_CAPACITY: usize = 4096;
/// Smallest ring for the source → speaker arrival delay, in samples.
const MIN_PROPAGATION_CAPACITY: usize = 8192;

/// Centre of the speaker array and the distance of its farthest speaker.
///
/// The centre is the middle of the speakers' bounding box, not their mean: a
/// 5.1 layout has three speakers in front and two behind, and the mean would
/// sit forward of the room's middle.
fn speaker_ring(layout: &SpeakerLayout) -> (Vec3, f32) {
    let speakers = layout.speakers();
    let centre = layout.bounding_box_centre();
    let radius = speakers
        .iter()
        .map(|speaker| speaker.position.distance_to(centre))
        .fold(0.0f32, f32::max);
    (centre, radius)
}

/// Pickup gain of a speaker's outward-facing virtual microphone for a source.
///
/// Omnidirectional (1.0) while the source is inside the speaker ring, blending
/// to a cardioid `0.5·(1 + cos θ)` by twice the ring radius, where θ is the
/// angle between the microphone's outward axis and the direction to the source.
/// Continuous everywhere, so a source crossing the ring does not jump in level.
fn outward_pickup_gain(
    ring_centre: Vec3,
    ring_radius: f32,
    speaker_pos: Vec3,
    source_pos: Vec3,
) -> f32 {
    let axis = speaker_pos - ring_centre;
    let to_source = source_pos - speaker_pos;
    if ring_radius < 1e-3 || axis.length() < 1e-3 || to_source.length() < 1e-3 {
        return 1.0;
    }
    let cos_angle = axis.normalize().dot(to_source.normalize()).clamp(-1.0, 1.0);
    let cardioid = 0.5 * (1.0 + cos_angle);

    let source_radius = source_pos.distance_to(ring_centre);
    let outside = ((source_radius - ring_radius) / ring_radius).clamp(0.0, 1.0);
    // Smoothstep: zero slope at both ends of the blend.
    let directional = outside * outside * (3.0 - 2.0 * outside);

    1.0 + directional * (cardioid - 1.0)
}

/// WorldLocked renderer with per-speaker propagation.
pub struct WorldLockedRenderer {
    /// propagation[source_idx][speaker_idx]
    propagation: Vec<Vec<SpeakerPropagation>>,
    /// One dry-signal delay line per source, read once per speaker.
    delay_lines: Vec<SourceDelayLine>,
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
            delay_lines: Vec::new(),
            prev_gains: Vec::new(),
            speaker_count: 0,
            // Unknown until the output device is opened; set by `ensure_topology`.
            sample_rate: 0.0,
            params,
        }
    }

    fn new_speaker_propagation(sample_rate: f32, reflection_capacity: usize) -> SpeakerPropagation {
        SpeakerPropagation {
            air_absorption: AirAbsorptionFilter::new(sample_rate),
            reflections: ReflectionCore::new(reflection_capacity),
            delay_samples: None,
        }
    }
}

impl Renderer for WorldLockedRenderer {
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        ctx: &SourceContext,
        _paths: &PathSet,
        _path_effects: &mut [PathEffectChain],
        out: &mut OutputBuffer,
    ) {
        let layout = ctx.layout;
        let speaker_count = layout.speaker_count().min(MAX_CHANNELS);
        let (ring_centre, ring_radius) = speaker_ring(layout);
        let speed_of_sound = ctx.environment.atmosphere.speed_of_sound();
        let delay_line = &mut self.delay_lines[source_idx];
        let max_delay = delay_line.max_delay();

        // 1. Per-speaker targets for this buffer: gain, arrival delay, filters.
        let mut path_gains = [0.0f32; MAX_CHANNELS];
        let mut delay_start = [0.0f32; MAX_CHANNELS];
        let mut delay_end = [0.0f32; MAX_CHANNELS];
        let mut channel_of = [usize::MAX; MAX_CHANNELS];
        for spk_idx in 0..speaker_count {
            if let Some(speaker) = layout.speaker_by_index(spk_idx) {
                if speaker.channel >= out.channels || !layout.is_channel_active(speaker.channel) {
                    continue;
                }
                channel_of[spk_idx] = speaker.channel;

                let prop = &mut self.propagation[source_idx][spk_idx];
                let dist = ctx.source_pos.distance_to(speaker.position);

                // Arrival delay, ramped from where the previous buffer ended.
                let target_delay = (dist / speed_of_sound * out.sample_rate).min(max_delay);
                delay_start[spk_idx] = prop.delay_samples.unwrap_or(target_delay);
                delay_end[spk_idx] = target_delay;
                prop.delay_samples = Some(target_delay);

                // Air absorption filter update
                prop.air_absorption
                    .update(dist, &ctx.environment.atmosphere);

                // Reflections tap update
                prop.reflections.update(
                    ctx.environment,
                    ctx.source_pos,
                    speaker.position,
                    out.sample_rate,
                    speed_of_sound,
                    ctx.source_ref_distance,
                );

                // Ground effect. The ISO 9613-2 ground term is the direct ray
                // plus the ray bounced off the ground. When that bounce is
                // already rendered as the floor reflection tap, applying the
                // ground term too counts the floor twice, so the tap wins.
                // The term still acts where there is no floor tap: outdoors
                // (nothing reflects) and for a speaker standing on the floor
                // (the bounce arrives with the direct sound).
                let ground_gain = if prop.reflections.has_floor_tap() {
                    1.0
                } else {
                    let dx = ctx.source_pos.x - speaker.position.x;
                    let dy = ctx.source_pos.y - speaker.position.y;
                    let horizontal_dist = (dx * dx + dy * dy).sqrt();
                    ground_effect_gain(
                        horizontal_dist,
                        ctx.source_pos.z.max(0.0),
                        speaker.position.z.max(0.0),
                        &ctx.environment.ground,
                    )
                };

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
                let pickup_gain =
                    outward_pickup_gain(ring_centre, ring_radius, speaker.position, ctx.source_pos);

                path_gains[speaker.channel] = ground_gain * dist_gain * dir_gain * pickup_gain;
            }
        }

        // 2. Per-sample rendering
        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];

        for frame in 0..out.num_frames {
            let t = frame as f32 * inv_frames;
            delay_line.write(source.next_sample(out.sample_rate));

            for spk_idx in 0..speaker_count {
                let ch = channel_of[spk_idx];
                if ch == usize::MAX {
                    continue;
                }
                let prop = &mut self.propagation[source_idx][spk_idx];

                // Arrival delay → air absorption → reflection delay taps.
                let delay = delay_start[spk_idx] + (delay_end[spk_idx] - delay_start[spk_idx]) * t;
                let arrived = delay_line.read(delay);
                let filtered = prop.air_absorption.process(arrived);
                let with_reflections = filtered + prop.reflections.process_sample(filtered);

                // Broadband gain (cached) × gain ramp
                let gain = prev[ch] + (path_gains[ch] - prev[ch]) * t;
                out.buffer[frame * out.channels + ch] += with_reflections * gain;
            }
        }

        self.prev_gains[source_idx] = path_gains;
    }

    fn name(&self) -> &str {
        "world_locked"
    }

    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, sample_rate: f32) {
        if sample_rate != self.sample_rate {
            // Delay lines and filters are sized and tuned for one sample rate.
            // This happens when the device is opened, before any audio runs.
            self.propagation.clear();
            self.delay_lines.clear();
            self.sample_rate = sample_rate;
        }
        self.speaker_count = layout.speaker_count();
        let reflection_capacity = room_acoustics::delay_capacity_for_seconds(
            self.params.longest_delay_seconds,
            sample_rate,
            MIN_REFLECTION_CAPACITY,
        );
        let propagation_capacity = room_acoustics::delay_capacity_for_seconds(
            self.params.longest_delay_seconds,
            sample_rate,
            MIN_PROPAGATION_CAPACITY,
        );

        // Grow source dimension
        while self.propagation.len() < source_count {
            let speakers: Vec<SpeakerPropagation> = (0..self.speaker_count)
                .map(|_| Self::new_speaker_propagation(sample_rate, reflection_capacity))
                .collect();
            self.propagation.push(speakers);
        }
        while self.delay_lines.len() < source_count {
            self.delay_lines
                .push(SourceDelayLine::new(propagation_capacity));
        }

        // Grow speaker dimension if layout changed
        for source_props in &mut self.propagation {
            while source_props.len() < self.speaker_count {
                source_props.push(Self::new_speaker_propagation(
                    sample_rate,
                    reflection_capacity,
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
                prop.delay_samples = None;
            }
        }
        for delay_line in &mut self.delay_lines {
            delay_line.reset();
        }
        for gains in &mut self.prev_gains {
            gains.fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests;
