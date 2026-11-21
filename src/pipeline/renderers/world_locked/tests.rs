use super::*;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::listener::Listener;
use atrium_core::panner::{distance_gain_at_model, DistanceModelType};
use atrium_core::source::SoundSource;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::pipeline::environment::Environment;
use crate::pipeline::wall_material::WallMaterial;

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
    render_over_floor(
        source,
        blocks,
        &living_room_layout(),
        0.0,
        &GroundProperties::hard(),
    )
}

/// Render `blocks` 10 ms buffers of one source over a floor at z = 0.
/// The walls and ceiling are 500 m away, beyond the one-second delay
/// lines, so the floor is the only surface that can produce a tap.
fn render_over_floor(
    source: &mut ScriptedSource,
    blocks: usize,
    layout: &SpeakerLayout,
    wall_reflectivity: f32,
    ground: &GroundProperties,
) -> Vec<f32> {
    let mut renderer = WorldLockedRenderer::new(WorldLockedParams {
        longest_delay_seconds: 1.0,
    });
    renderer.ensure_topology(1, layout, SAMPLE_RATE);

    let listener = Listener::new(Vec3::ZERO, 0.0);
    let environment_min = Vec3::new(-500.0, -500.0, 0.0);
    let environment_max = Vec3::new(500.0, 500.0, 500.0);
    let mut environment = if wall_reflectivity == 0.0 {
        Environment::anechoic(environment_min, environment_max)
    } else {
        Environment::uniform(
            environment_min,
            environment_max,
            WallMaterial::with_reflectivity(wall_reflectivity),
        )
    };
    environment.ground = *ground;
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
            path_target: listener.position,
            environment: &environment,
            sample_rate: SAMPLE_RATE,
            distance_model: &distance_model,
            layout,
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
    assert!(outward_pickup_gain(centre, radius, speaker, Vec3::new(-50.0, 0.0, 0.0)).abs() < 1e-5);
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
    let tone = |index: usize| (std::f32::consts::TAU * 1000.0 * index as f32 / SAMPLE_RATE).sin();
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
    let tone = |index: usize| (std::f32::consts::TAU * 200.0 * index as f32 / SAMPLE_RATE).sin();
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

/// The 6 × 4 m living room: the speakers' bounding box is centred on the
/// origin, so the ring centre is (0, 0, 0) and the radius is the corner
/// distance √(3² + 2²) = √13 = 3.6056 m. (The mean of the five speakers
/// would be (0, 0.4, 0): three in front, two behind.)
#[test]
fn speaker_ring_is_the_bounding_box_centre_and_farthest_speaker() {
    let (centre, radius) = speaker_ring(&living_room_layout());
    assert!(centre.distance_to(Vec3::ZERO) < 1e-6, "centre {centre:?}");
    assert!((radius - 13f32.sqrt()).abs() < 1e-5, "radius {radius}");
}

/// Hand-derived points of the pickup blend. Ring radius R = 4, speaker on
/// the +X axis at (4, 0, 0).
#[test]
fn pickup_gain_at_hand_derived_points() {
    let centre = Vec3::ZERO;
    let radius = 4.0;
    let speaker = Vec3::new(4.0, 0.0, 0.0);
    // Source at 1.5 R = 6 m from the centre, 90° off the speaker's axis:
    // (4, √20, 0) is √(16 + 20) = 6 m out. Blend x = (6 − 4)/4 = 0.5,
    // smoothstep 0.5²·(3 − 1) = 0.5; cardioid at 90° = 0.5;
    // gain = 1 + 0.5·(0.5 − 1) = 0.75.
    let halfway = outward_pickup_gain(centre, radius, speaker, Vec3::new(4.0, 20f32.sqrt(), 0.0));
    assert!((halfway - 0.75).abs() < 1e-5, "halfway {halfway}");
    // Blend x = 0.25 (5 m out), smoothstep 0.0625·2.5 = 0.15625. The
    // source at (4, 3, 0) is 90° off axis: gain = 1 − 0.15625·0.5 = 0.921875.
    let quarter = outward_pickup_gain(centre, radius, speaker, Vec3::new(4.0, 3.0, 0.0));
    assert!((quarter - 0.921875).abs() < 1e-5, "quarter {quarter}");
    // Full cardioid (≥ 2 R out), 60° off axis: 0.5·(1 + cos 60°) = 0.75.
    let sixty = 60f32.to_radians();
    let far = speaker + Vec3::new(sixty.cos(), sixty.sin(), 0.0) * 50.0;
    let gain = outward_pickup_gain(centre, radius, speaker, far);
    assert!((gain - 0.75).abs() < 1e-5, "60° cardioid {gain}");
}

/// The source delay line reads `delay` samples behind the newest sample
/// with linear interpolation. On a ramp signal x[n] = n interpolation is
/// exact, so after writing 0..=9 a delay of 2.25 reads 9 − 2.25 = 6.75.
#[test]
fn source_delay_line_reads_fractional_delays_exactly_on_a_ramp() {
    let mut delay_line = SourceDelayLine::new(16);
    for value in 0..10 {
        delay_line.write(value as f32);
    }
    assert_eq!(delay_line.read(0.0), 9.0);
    assert!((delay_line.read(2.25) - 6.75).abs() < 1e-6);
    assert!((delay_line.read(7.5) - 1.5).abs() < 1e-6);
    assert_eq!(delay_line.max_delay(), 14.0);
}

/// Speakers equidistant from the source, and mirror images about the
/// room's YZ plane, pick up identical signals: a source on the centre
/// line (x = 0) must give FL ≡ FR and RL ≡ RR sample for sample.
#[test]
fn source_on_the_centre_line_gives_mirror_speakers_identical_output() {
    let mut source = ScriptedSource {
        sample_index: 0,
        position_at: |_| Vec3::new(0.0, 7.0, 1.5),
        signal_at: noise,
    };
    let rendered = render_free_field(&mut source, 20);
    let pairs = [(FRONT_LEFT, FRONT_RIGHT), (REAR_LEFT, REAR_RIGHT)];
    for (left, right) in pairs {
        let left_samples = channel(&rendered, left);
        let right_samples = channel(&rendered, right);
        assert!(left_samples.iter().any(|sample| sample.abs() > 1e-3));
        for (index, (a, b)) in left_samples.iter().zip(&right_samples).enumerate() {
            assert!(
                (a - b).abs() < 1e-7,
                "channels {left}/{right} differ at sample {index}: {a} vs {b}"
            );
        }
    }
}

/// End-to-end level, per speaker, for a steady (DC) source at the room
/// centre: inside the ring the pickup is omni, so each speaker plays
/// ground gain × inverse-law gain. Corners are √13 = 3.606 m away →
/// 1/√13 = 0.27735; the centre speaker is 2 m away → 0.5. Air absorption
/// passes DC unchanged; the walls are 500 m away and reflect nothing.
/// Nothing normalises the sum across speakers: each one is a microphone.
#[test]
fn steady_source_at_the_centre_plays_the_distance_law_in_every_speaker() {
    let mut source = ScriptedSource {
        sample_index: 0,
        position_at: |_| Vec3::ZERO,
        signal_at: |_| 1.0,
    };
    let rendered = render_free_field(&mut source, 12);
    let ground = GroundProperties::hard();
    let settled = |ch: usize| *channel(&rendered, ch).last().unwrap();
    let corner_distance = 13f32.sqrt();
    let corner = ground_effect_gain(corner_distance, 0.0, 0.0, &ground) / corner_distance;
    let centre = ground_effect_gain(2.0, 0.0, 0.0, &ground) * 0.5;
    for ch in [FRONT_LEFT, FRONT_RIGHT, REAR_LEFT, REAR_RIGHT] {
        assert!(
            (settled(ch) - corner).abs() < 1e-4,
            "channel {ch}: {} vs {corner}",
            settled(ch)
        );
    }
    assert!(
        (settled(CENTRE) - centre).abs() < 1e-4,
        "centre {}",
        settled(CENTRE)
    );
    assert!(
        channel(&rendered, 3).iter().all(|sample| *sample == 0.0),
        "LFE stays silent"
    );
}

/// The living-room rectangle with every speaker raised to `height` metres.
fn raised_layout(height: f32) -> SpeakerLayout {
    SpeakerLayout::surround_5_1(
        Vec3::new(-3.0, 2.0, height),
        Vec3::new(3.0, 2.0, height),
        Vec3::new(0.0, 2.0, height),
        Vec3::new(-3.0, -2.0, height),
        Vec3::new(3.0, -2.0, height),
    )
}

fn steady_source_at_height_1_2() -> ScriptedSource {
    ScriptedSource {
        sample_index: 0,
        position_at: |_| Vec3::new(0.0, 0.0, 1.2),
        signal_at: |_| 1.0,
    }
}

/// The floor is counted once. Source and speakers 1.2 m above a floor
/// reflecting 81 % of the energy (amplitude 0.9), hard ground:
///   direct  √13 = 3.6056 m          → 1/√13       = 0.27735
///   bounce  √(13 + 2.4²) = 4.3313 m → 0.9/4.3313  = 0.20779
/// A steady source settles at their sum, 0.48514. The ISO ground term
/// (×1.41 here) describes that same bounce and must not be applied on top.
#[test]
fn reflecting_floor_is_rendered_as_its_tap_without_the_ground_gain() {
    let mut source = steady_source_at_height_1_2();
    let rendered = render_over_floor(
        &mut source,
        12,
        &raised_layout(1.2),
        0.81,
        &GroundProperties::hard(),
    );
    let settled = *channel(&rendered, FRONT_LEFT).last().unwrap();
    let expected = 1.0 / 13f32.sqrt() + 0.9 / 18.76f32.sqrt();
    assert!((expected - 0.48514).abs() < 1e-4, "hand value {expected}");
    assert!((settled - expected).abs() < 1e-3, "{settled} vs {expected}");
}

/// A speaker standing on the floor is equally far from the source and from
/// its floor image, so there is no delayed tap and the bounce arrives with
/// the direct sound. The ground term is then the only floor model and
/// stays: hard ground at this range is +3 dB, a gain of 10^(3/20) = 1.4125.
#[test]
fn speaker_on_a_reflecting_floor_keeps_the_ground_gain() {
    let mut source = steady_source_at_height_1_2();
    let ground = GroundProperties::hard();
    let rendered = render_over_floor(&mut source, 12, &living_room_layout(), 0.81, &ground);
    let settled = *channel(&rendered, FRONT_LEFT).last().unwrap();
    let ground_gain = ground_effect_gain(13f32.sqrt(), 1.2, 0.0, &ground);
    assert!(
        (ground_gain - 1.4125).abs() < 1e-3,
        "ground gain {ground_gain}"
    );
    let expected = ground_gain / 14.44f32.sqrt();
    assert!((settled - expected).abs() < 1e-3, "{settled} vs {expected}");
}

/// Outdoors nothing reflects, so there is no floor tap even for raised
/// speakers, and the ground term carries the ground: porous ground never
/// boosts, and the level is ground gain × 1/√13.
#[test]
fn open_field_keeps_the_ground_gain_for_raised_speakers() {
    let mut source = steady_source_at_height_1_2();
    let ground = GroundProperties::soft();
    let rendered = render_over_floor(&mut source, 12, &raised_layout(1.2), 0.0, &ground);
    let settled = *channel(&rendered, FRONT_LEFT).last().unwrap();
    let ground_gain = ground_effect_gain(13f32.sqrt(), 1.2, 1.2, &ground);
    assert!(ground_gain <= 1.0, "porous ground gain {ground_gain}");
    let expected = ground_gain / 13f32.sqrt();
    assert!((settled - expected).abs() < 1e-3, "{settled} vs {expected}");
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
