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
//! 3. Air absorption, ground effect, first-order reflections, distance law and
//!    source directivity, each for the source → speaker path.

use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};
use atrium_core::types::Vec3;

use crate::audio::filters::AirAbsorptionFilter;
use crate::audio::propagation::ground_effect_gain;
use crate::pipeline::path::{PathEffectChain, PathSet};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::stages::reflections::ReflectionCore;
use crate::pipeline::SourceContext;

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
    pub wall_reflectivity: f32,
    /// Delay buffer capacity for ReflectionCore, sized from room geometry.
    pub reflection_capacity: usize,
    /// Delay buffer capacity for the source → speaker arrival delay (samples).
    pub propagation_capacity: usize,
}

/// Centre of the speaker array and the distance of its farthest speaker.
///
/// The centre is the middle of the speakers' bounding box, not their mean: a
/// 5.1 layout has three speakers in front and two behind, and the mean would
/// sit forward of the room's middle.
fn speaker_ring(layout: &SpeakerLayout) -> (Vec3, f32) {
    let speakers = layout.speakers();
    let Some(first) = speakers.first() else {
        return (Vec3::ZERO, 0.0);
    };
    let mut min = first.position;
    let mut max = first.position;
    for speaker in speakers {
        min = Vec3::new(
            min.x.min(speaker.position.x),
            min.y.min(speaker.position.y),
            min.z.min(speaker.position.z),
        );
        max = Vec3::new(
            max.x.max(speaker.position.x),
            max.y.max(speaker.position.y),
            max.z.max(speaker.position.z),
        );
    }
    let centre = (min + max) * 0.5;
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
        let speed_of_sound = ctx.atmosphere.speed_of_sound();
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
                prop.air_absorption.update(dist, ctx.atmosphere);

                // Ground effect
                let dx = ctx.source_pos.x - speaker.position.x;
                let dy = ctx.source_pos.y - speaker.position.y;
                let horizontal_dist = (dx * dx + dy * dy).sqrt();
                let ground_gain = ground_effect_gain(
                    horizontal_dist,
                    ctx.source_pos.z.max(0.0),
                    speaker.position.z.max(0.0),
                    ctx.ground,
                );

                // Reflections tap update
                prop.reflections.update(
                    ctx.environment_min,
                    ctx.environment_max,
                    ctx.source_pos,
                    speaker.position,
                    out.sample_rate,
                    speed_of_sound,
                    ctx.source_ref_distance,
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
        while self.delay_lines.len() < source_count {
            self.delay_lines
                .push(SourceDelayLine::new(self.params.propagation_capacity));
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
mod tests {
    use super::*;
    use atrium_core::directivity::DirectivityPattern;
    use atrium_core::listener::Listener;
    use atrium_core::panner::{distance_gain_at_model, DistanceModelType};
    use atrium_core::source::SoundSource;

    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::distance::DistanceModel;
    use crate::audio::propagation::GroundProperties;

    const SAMPLE_RATE: f32 = 48000.0;
    const CHANNELS: usize = 6;
    /// Channel order of the 5.1 layout: FL, FR, C, LFE, RL, RR.
    const FRONT_LEFT: usize = 0;
    const FRONT_RIGHT: usize = 1;
    const CENTRE: usize = 2;
    const REAR_LEFT: usize = 4;
    const REAR_RIGHT: usize = 5;

    /// The living-room layout used by every scene: a 6 × 4 m rectangle.
    fn living_room_layout() -> SpeakerLayout {
        SpeakerLayout::surround_5_1(
            Vec3::new(-3.0, 2.0, 0.0),
            Vec3::new(3.0, 2.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(-3.0, -2.0, 0.0),
            Vec3::new(3.0, -2.0, 0.0),
        )
    }

    /// A scripted test source: position and signal are functions of the
    /// sample index, so tests can move it and choose its waveform.
    struct ScriptedSource {
        sample_index: usize,
        position_at: fn(f32) -> Vec3,
        signal_at: fn(usize) -> f32,
    }

    impl SoundSource for ScriptedSource {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            let sample = (self.signal_at)(self.sample_index);
            self.sample_index += 1;
            sample
        }
        fn position(&self) -> Vec3 {
            (self.position_at)(self.sample_index as f32 / SAMPLE_RATE)
        }
        fn tick(&mut self, _dt: f32) {}
    }

    /// Render `blocks` 10 ms buffers of one source in an anechoic free field
    /// (no walls, no ground effect at these heights is NOT assumed: ground is
    /// hard, which is a flat gain and so cancels out of every ratio below).
    fn render_free_field(source: &mut ScriptedSource, blocks: usize) -> Vec<f32> {
        let layout = living_room_layout();
        let mut renderer = WorldLockedRenderer::new(WorldLockedParams {
            wall_reflectivity: 0.0,
            reflection_capacity: 4096,
            propagation_capacity: 65536,
        });
        renderer.ensure_topology(1, &layout, SAMPLE_RATE);

        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let ground = GroundProperties::hard();
        let distance_model = DistanceModel {
            model: DistanceModelType::Inverse,
            ref_distance: 1.0,
            max_distance: 1000.0,
            rolloff: 1.0,
        };
        let directivity = DirectivityPattern::Omni;
        let frames = 480;
        let mut rendered = Vec::with_capacity(blocks * frames * CHANNELS);
        for _ in 0..blocks {
            let position = source.position();
            let ctx = SourceContext {
                listener: &listener,
                source_pos: position,
                source_orientation: Vec3::new(1.0, 0.0, 0.0),
                source_directivity: &directivity,
                source_spread: 0.0,
                source_ref_distance: 1.0,
                dist_to_listener: position.distance_to(listener.position),
                atmosphere: &atmosphere,
                environment_min: Vec3::new(-500.0, -500.0, 0.0),
                environment_max: Vec3::new(500.0, 500.0, 500.0),
                ground: &ground,
                sample_rate: SAMPLE_RATE,
                distance_model: &distance_model,
                layout: &layout,
                reverb_send: 0.0,
            };
            let mut block = vec![0.0f32; frames * CHANNELS];
            let mut out = OutputBuffer {
                buffer: &mut block,
                reverb_send: None,
                channels: CHANNELS,
                num_frames: frames,
                sample_rate: SAMPLE_RATE,
            };
            renderer.render_source(0, source, &ctx, &PathSet::new(), &mut [], &mut out);
            rendered.extend_from_slice(&block);
        }
        rendered
    }

    fn channel(rendered: &[f32], channel: usize) -> Vec<f32> {
        rendered
            .iter()
            .skip(channel)
            .step_by(CHANNELS)
            .copied()
            .collect()
    }

    fn level_db(samples: &[f32]) -> f32 {
        let power = samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32;
        10.0 * power.max(1e-30).log10()
    }

    fn noise(sample_index: usize) -> f32 {
        // Small deterministic hash noise in [-1, 1].
        let mut state = (sample_index as u32).wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
        state ^= state >> 15;
        state = state.wrapping_mul(0x2C1B_3C6D);
        state ^= state >> 12;
        (state as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Physics: sound reaches each speaker after distance / c.
    /// A click 20 m north of the room centre, 1.2 m up, must appear on every
    /// speaker at exactly that speaker's distance ÷ speed of sound.
    #[test]
    fn click_arrives_at_each_speaker_after_distance_over_speed_of_sound() {
        let mut source = ScriptedSource {
            sample_index: 0,
            position_at: |_| Vec3::new(0.0, 20.0, 1.2),
            signal_at: |index| if index == 0 { 1.0 } else { 0.0 },
        };
        let rendered = render_free_field(&mut source, 12);
        let speed_of_sound = AtmosphericParams::default().speed_of_sound();
        let layout = living_room_layout();

        for speaker in layout.speakers() {
            let samples = channel(&rendered, speaker.channel);
            let arrival = samples
                .iter()
                .position(|x| x.abs() > 1e-6)
                .expect("click should arrive within 120 ms");
            let distance = Vec3::new(0.0, 20.0, 1.2).distance_to(speaker.position);
            let expected = distance / speed_of_sound * SAMPLE_RATE;
            assert!(
                (arrival as f32 - expected).abs() <= 1.0,
                "channel {}: click arrived at sample {arrival}, expected {expected:.1} \
                 ({distance:.2} m)",
                speaker.channel
            );
        }

        // The rear pair is 3.95 m farther from the source than the front pair
        // (22.24 m vs 18.29 m), so it must fire 11.5 ms later. That time
        // difference is the direction cue the old renderer threw away.
        let first = |ch: usize| {
            channel(&rendered, ch)
                .iter()
                .position(|x| x.abs() > 1e-6)
                .unwrap()
        };
        let lag_ms = (first(REAR_LEFT) as f32 - first(FRONT_LEFT) as f32) / SAMPLE_RATE * 1000.0;
        let expected_lag_ms = (22.24 - 18.29) / speed_of_sound * 1000.0;
        assert!(
            (lag_ms - expected_lag_ms).abs() < 0.1,
            "rear lag {lag_ms:.2} ms, expected {expected_lag_ms:.2} ms"
        );
    }

    /// A far source must be clearly louder on the side of the room that faces
    /// it. Before: a source 20 m north differed by 1.6 dB front to back.
    #[test]
    fn far_source_is_much_louder_in_the_speakers_facing_it() {
        let mut source = ScriptedSource {
            sample_index: 0,
            position_at: |_| Vec3::new(0.0, 20.0, 0.0),
            signal_at: noise,
        };
        let rendered = render_free_field(&mut source, 60);
        let settled = |ch: usize| level_db(&channel(&rendered, ch)[9600..]);

        let front = settled(CENTRE);
        let rear = settled(REAR_LEFT).max(settled(REAR_RIGHT));
        assert!(
            front - rear > 10.0,
            "front {front:.1} dB vs rear {rear:.1} dB: expected > 10 dB apart"
        );
        // Left/right symmetric for a source straight ahead.
        assert!((settled(FRONT_LEFT) - settled(FRONT_RIGHT)).abs() < 0.1);
        assert!((settled(REAR_LEFT) - settled(REAR_RIGHT)).abs() < 0.1);
    }

    /// Inside the room the pickup is omnidirectional: a fly 0.5 m from the
    /// front-left speaker, on the room side of it, is loudest in that speaker
    /// even though the speaker's microphone faces the other way.
    #[test]
    fn source_inside_the_room_is_loudest_in_the_nearest_speaker() {
        let mut source = ScriptedSource {
            sample_index: 0,
            position_at: |_| Vec3::new(-2.6, 1.7, 0.0),
            signal_at: noise,
        };
        let rendered = render_free_field(&mut source, 30);
        let settled = |ch: usize| level_db(&channel(&rendered, ch)[4800..]);
        let nearest = settled(FRONT_LEFT);
        for other in [FRONT_RIGHT, CENTRE, REAR_LEFT, REAR_RIGHT] {
            assert!(
                nearest - settled(other) > 6.0,
                "front-left {nearest:.1} dB should dominate channel {other} ({:.1} dB)",
                settled(other)
            );
        }
        // And it is exactly the free-field distance law: 0.5 m → clamped to the
        // 1 m reference, pickup gain 1.0, so no directional loss.
        let pickup = outward_pickup_gain(
            Vec3::ZERO,
            Vec3::new(3.0, 2.0, 0.0).length(),
            Vec3::new(-3.0, 2.0, 0.0),
            Vec3::new(-2.6, 1.7, 0.0),
        );
        assert_eq!(pickup, 1.0);
    }

    /// The pickup pattern is continuous across the ring and reaches a full
    /// cardioid at twice the ring radius.
    #[test]
    fn pickup_pattern_blends_from_omni_to_cardioid() {
        let centre = Vec3::ZERO;
        let radius = 4.0;
        let speaker = Vec3::new(4.0, 0.0, 0.0);
        // Far source on the speaker's own side: on axis → 1.0.
        assert!(
            (outward_pickup_gain(centre, radius, speaker, Vec3::new(50.0, 0.0, 0.0)) - 1.0).abs()
                < 1e-5
        );
        // Far source on the opposite side: cardioid null → 0.0.
        assert!(
            outward_pickup_gain(centre, radius, speaker, Vec3::new(-50.0, 0.0, 0.0)).abs() < 1e-5
        );
        // Far source at 90°: cardioid → 0.5.
        let side = outward_pickup_gain(centre, radius, speaker, Vec3::new(4.0, 50.0, 0.0));
        assert!((side - 0.5).abs() < 1e-3, "side {side}");
        // Anywhere inside the ring: omni.
        for position in [
            Vec3::ZERO,
            Vec3::new(-3.9, 0.0, 0.0),
            Vec3::new(1.0, 2.0, 0.0),
        ] {
            assert_eq!(outward_pickup_gain(centre, radius, speaker, position), 1.0);
        }
        // No jump across the ring or at the end of the blend.
        let mut previous = outward_pickup_gain(centre, radius, speaker, Vec3::new(-3.0, 0.0, 0.0));
        let mut x = -3.0f32;
        while x > -12.0 {
            x -= 0.01;
            let gain = outward_pickup_gain(centre, radius, speaker, Vec3::new(x, 0.0, 0.0));
            assert!(
                (gain - previous).abs() < 0.01,
                "jump at x={x}: {previous} → {gain}"
            );
            previous = gain;
        }
    }

    /// Doppler falls out of the ramped arrival delay. A 1 kHz tone approaching
    /// at 20 m/s is heard at f·(1 + v/c); receding, at f·(1 − v/c).
    ///
    /// This is the first-order form. The delay is computed from where the
    /// source is NOW, not where it was when the sound left it, so it differs
    /// from the exact moving-source formula f·c/(c ∓ v) by (v/c)²: 0.34 %
    /// (6 cents) at 20 m/s, 0.02 % at a walking bird's 5 m/s.
    #[test]
    fn moving_source_is_doppler_shifted() {
        let speed_of_sound = AtmosphericParams::default().speed_of_sound();
        let tone =
            |index: usize| (std::f32::consts::TAU * 1000.0 * index as f32 / SAMPLE_RATE).sin();
        // Zero crossings per second ÷ 2 = frequency. Measured on the centre
        // speaker at (0, 2), with the source moving along the Y axis through it.
        let measured_frequency = |position_at: fn(f32) -> Vec3| -> f32 {
            let mut source = ScriptedSource {
                sample_index: 0,
                position_at,
                signal_at: tone,
            };
            let rendered = render_free_field(&mut source, 150);
            let samples = channel(&rendered, CENTRE);
            let window = &samples[24000..72000];
            let crossings = window
                .windows(2)
                .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
                .count();
            crossings as f32 / (window.len() as f32 / SAMPLE_RATE)
        };

        let approaching = measured_frequency(|time| Vec3::new(0.0, 80.0 - 20.0 * time, 0.0));
        let receding = measured_frequency(|time| Vec3::new(0.0, 20.0 + 20.0 * time, 0.0));
        let expected_approaching = 1000.0 * (1.0 + 20.0 / speed_of_sound);
        let expected_receding = 1000.0 * (1.0 - 20.0 / speed_of_sound);
        assert!(
            (approaching - expected_approaching).abs() < 1.0,
            "approaching: {approaching:.1} Hz, expected {expected_approaching:.1} Hz"
        );
        assert!(
            (receding - expected_receding).abs() < 1.0,
            "receding: {receding:.1} Hz, expected {expected_receding:.1} Hz"
        );
    }

    /// A moving source must not click: the arrival delay is ramped per sample.
    /// A low tone crossing the room at 5 m/s has a bounded sample-to-sample
    /// step; a per-block delay jump would show up as a spike.
    #[test]
    fn moving_source_output_has_no_discontinuities() {
        let tone =
            |index: usize| (std::f32::consts::TAU * 200.0 * index as f32 / SAMPLE_RATE).sin();
        let mut source = ScriptedSource {
            sample_index: 0,
            position_at: |time| Vec3::new(-10.0 + 5.0 * time, 6.0, 1.0),
            signal_at: tone,
        };
        let rendered = render_free_field(&mut source, 400);
        for speaker in living_room_layout().speakers() {
            let samples = channel(&rendered, speaker.channel);
            let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let largest_step = samples[4800..]
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .fold(0.0f32, f32::max);
            // A 200 Hz sine of amplitude `peak` steps at most 2π·200/48000·peak
            // = 0.026·peak per sample. Allow 2× for the gain ramp and Doppler.
            assert!(
                largest_step < 0.052 * peak,
                "channel {}: step {largest_step:.5} vs peak {peak:.5}",
                speaker.channel
            );
        }
    }

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
