use super::*;
use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::propagation::GroundProperties;
use crate::pipeline::environment::Environment;
use crate::pipeline::wall_material::WallMaterial;
use atrium_core::listener::Listener;
use atrium_core::speaker::{Speaker, SpeakerLayout};
use atrium_core::types::Vec3;

/// A square of speakers on channels 0–3 (a B-format bus) for 4 channels,
/// a stereo pair (a bilateral bus) for 2.
fn make_ctx(channels: usize) -> (SpeakerLayout, Listener) {
    let corners = [
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
        Vec3::new(-1.0, 1.0, 0.0),
        Vec3::new(-1.0, -1.0, 0.0),
    ];
    let speakers: Vec<Speaker> = corners
        .iter()
        .take(channels)
        .enumerate()
        .map(|(channel, &position)| Speaker { position, channel })
        .collect();
    let layout = SpeakerLayout::new(&speakers, None, channels);
    let listener = Listener::new(Vec3::ZERO, 0.0);
    (layout, listener)
}

const TEST_ATMOSPHERE: AtmosphericParams = AtmosphericParams {
    temperature_c: 20.0,
    humidity_pct: 50.0,
    pressure_kpa: 101.325,
};

fn test_environment() -> Environment {
    Environment {
        min: Vec3::new(-5.0, -5.0, -5.0),
        max: Vec3::new(5.0, 5.0, 5.0),
        wall_materials: [WallMaterial::HARD_WALL; 6],
        ground: GroundProperties::hard(),
        atmosphere: TEST_ATMOSPHERE,
        air: crate::audio::turbulence::AirMotion::STILL,
    }
}

fn test_mix_context<'a>(
    layout: &'a SpeakerLayout,
    listener: &'a Listener,
    environment: &'a Environment,
    channels: usize,
) -> MixContext<'a> {
    MixContext {
        listener,
        layout,
        sample_rate: 48000.0,
        channels,
        environment,
        master_gain: 1.0,
        render_channels: channels,
        reverb_input: None,
        measurement_mode: false,
    }
}

#[test]
fn silent_input_silent_output() {
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment, 4);
    stage.init(&ctx);

    let mut buffer = vec![0.0f32; 4 * 512];
    stage.process(&mut buffer, &ctx);

    let max = buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        max < 1e-10,
        "silent input should produce silent output, got {max}"
    );
}

#[test]
fn impulse_produces_delayed_taps() {
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment, 4);
    stage.init(&ctx);

    // Write an impulse in W channel at frame 0.
    let total_frames = 2048;
    let mut buffer = vec![0.0f32; 4 * total_frames];
    buffer[0] = 1.0; // W channel impulse

    stage.process(&mut buffer, &ctx);

    // Each line should produce output at its delay time.
    // At 48kHz: 336, 528, 816, 1104 samples.
    let tap_frames: Vec<usize> = DELAY_MS
        .iter()
        .map(|&ms| ((ms / 1000.0) * 48000.0) as usize)
        .collect();

    for &tap_frame in &tap_frames {
        if tap_frame < total_frames {
            // Check any FOA channel — rotation distributes energy across W/Y/X.
            let energy_at_tap: f32 = (0..4).map(|ch| buffer[tap_frame * 4 + ch].powi(2)).sum();
            assert!(
                energy_at_tap > 1e-6,
                "should have energy at tap frame {tap_frame}, got {energy_at_tap}"
            );
        }
    }
}

#[test]
fn rotation_redistributes_energy() {
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment, 4);
    stage.init(&ctx);

    // Impulse in X channel only (front direction).
    let total_frames = 2048;
    let mut buffer = vec![0.0f32; 4 * total_frames];
    buffer[3] = 1.0; // X channel impulse

    stage.process(&mut buffer, &ctx);

    // After the first tap (7ms = 336 samples), Y channel should have
    // energy from the rotated X input.
    let y_energy: f32 = buffer[336 * 4..]
        .iter()
        .skip(1)
        .step_by(4)
        .map(|s| s * s)
        .sum();
    assert!(
        y_energy > 1e-8,
        "rotation should redistribute X energy into Y channel, got Y energy {y_energy}"
    );
}

#[test]
fn noop_for_stereo() {
    let (layout, listener) = make_ctx(2);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment, 2);
    stage.init(&ctx);

    let mut buffer = vec![0.5f32; 2 * 256];
    let original = buffer.clone();
    stage.process(&mut buffer, &ctx);

    assert_eq!(buffer, original, "stereo buffer should be unchanged");
}

/// No feedback means no lingering tail: energy dies after the last tap.
#[test]
fn no_feedback_tail() {
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment, 4);
    stage.init(&ctx);

    // Impulse in first buffer.
    let frames = 2048;
    let mut buf1 = vec![0.0f32; 4 * frames];
    buf1[0] = 1.0;
    stage.process(&mut buf1, &ctx);

    // Second buffer (all zeros input): should have no energy because
    // without feedback, taps only reproduce what was written to the ring.
    // The impulse from buf1 was at frame 0, and all taps are < 23ms = 1104 samples.
    // By the start of buf2, all taps have already been read.
    let mut buf2 = vec![0.0f32; 4 * frames];
    stage.process(&mut buf2, &ctx);

    let max = buf2.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        max < 1e-6,
        "no feedback should mean no lingering tail, got max {max}"
    );
}

/// Decorrelation adds energy (wet > dry) but within the expected depth.
#[test]
fn decorrelation_adds_bounded_energy() {
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment, 4);
    stage.init(&ctx);

    // Constant signal in W channel for 2048 frames.
    let frames = 2048;
    let mut buffer = vec![0.0f32; 4 * frames];
    for frame in 0..frames {
        buffer[frame * 4] = 0.5; // W
    }
    let dry_energy: f32 = buffer.iter().map(|s| s * s).sum();

    stage.process(&mut buffer, &ctx);

    let wet_energy: f32 = buffer.iter().map(|s| s * s).sum();

    // Wet energy should exceed dry (decorrelation adds the taps).
    assert!(
        wet_energy > dry_energy,
        "decorrelation should add energy: wet={wet_energy}, dry={dry_energy}"
    );
    // With a W-only constant signal, Z-rotation preserves W, so all 4 taps
    // add coherently: output W = dry + 4 × tap_gain × dry. Worst case ratio
    // is (1 + 4 × 0.15)² / 1 ≈ 2.56. Allow up to 3× for safety.
    assert!(
        wet_energy < dry_energy * 3.0,
        "decorrelation energy should be bounded: wet={wet_energy}, dry={dry_energy}"
    );
}

// ── Audit 2026-11: hand-derived values ────────────────────────────────

fn context_at_rate<'a>(
    layout: &'a SpeakerLayout,
    listener: &'a Listener,
    environment: &'a Environment,
    sample_rate: f32,
) -> MixContext<'a> {
    MixContext {
        sample_rate,
        ..test_mix_context(layout, listener, environment, 4)
    }
}

#[test]
fn delay_lengths_tap_gain_and_lowpass_coefficient_at_48k() {
    // 7, 11, 17, 23 ms × 48 kHz = 336, 528, 816, 1104 samples.
    // Tap gain = DEPTH/√4 = 0.3/2 = 0.15.
    // One-pole pole p = exp(−2π·6000/48000) = exp(−π/4) = 0.455938.
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    stage.init(&context_at_rate(&layout, &listener, &environment, 48000.0));
    assert_eq!(stage.delay_samples, [336, 528, 816, 1104]);
    assert!((stage.tap_gain - 0.15).abs() < 1e-7);
    for line in &stage.lp {
        for filter in line {
            assert!(
                (filter.coeff - 0.455938).abs() < 1e-6,
                "pole {}",
                filter.coeff
            );
        }
    }
}

#[test]
fn first_tap_of_x_impulse_has_hand_derived_rotated_value() {
    // X-impulse at frame 0. At frame 336 line 0 reads it back:
    //   lowpass first sample: (1 − p)·1 = 0.544062
    //   rotate by 72°: X' = X·cos72° = 0.168124, Y' = −X·sin72° = −0.517434
    //   × tap gain 0.15: X = 0.025219, Y = −0.077615, W = Z = 0.
    // Frame 0 itself is the untouched dry impulse.
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment, 4);
    stage.init(&ctx);
    let mut buffer = vec![0.0f32; 4 * 400];
    buffer[3] = 1.0;
    stage.process(&mut buffer, &ctx);

    assert_eq!(&buffer[0..4], &[0.0, 0.0, 0.0, 1.0]);
    for frame in 1..336 {
        for channel in 0..4 {
            assert_eq!(
                buffer[frame * 4 + channel],
                0.0,
                "frame {frame} before first tap"
            );
        }
    }
    let tap = &buffer[336 * 4..336 * 4 + 4];
    assert!(tap[0].abs() < 1e-7, "W stays zero: {}", tap[0]);
    assert!((tap[1] + 0.077615).abs() < 1e-5, "Y: {}", tap[1]);
    assert!(tap[2].abs() < 1e-7, "Z stays zero: {}", tap[2]);
    assert!((tap[3] - 0.025219).abs() < 1e-5, "X: {}", tap[3]);
}

#[test]
fn omni_input_stays_omni_and_white_noise_wet_energy_matches_derivation() {
    // Z-rotation leaves W untouched, so a W-only input never leaks into
    // Y/Z/X. For white noise the four taps are mutually uncorrelated and
    // uncorrelated with the dry signal, so
    //   wet/dry = 4 · 0.15² · (1 − p)/(1 + p) = 0.09 · 0.373680 = 0.033631
    // where (1 − p)/(1 + p) is the one-pole's white-noise power gain.
    // (The DEPTH doc's "9 % of dry" omits the 6 kHz lowpass, which removes
    // 63 % of white-noise power: −14.7 dB rather than −10.5 dB.)
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = test_mix_context(&layout, &listener, &environment, 4);
    stage.init(&ctx);

    let frames = 1 << 18;
    let mut state: u32 = 0x1234_5678;
    let mut dry = vec![0.0f32; frames];
    for sample in dry.iter_mut() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *sample = (state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
    }
    let mut buffer = vec![0.0f32; frames * 4];
    for (frame, sample) in dry.iter().enumerate() {
        buffer[frame * 4] = *sample;
    }
    stage.process(&mut buffer, &ctx);

    let mut dry_energy = 0.0f64;
    let mut wet_energy = 0.0f64;
    for frame in 2048..frames {
        let wet = (buffer[frame * 4] - dry[frame]) as f64;
        dry_energy += (dry[frame] as f64).powi(2);
        wet_energy += wet * wet;
        for channel in 1..4 {
            assert!(buffer[frame * 4 + channel].abs() < 1e-6);
        }
    }
    let ratio = wet_energy / dry_energy;
    assert!(
        (ratio - 0.033631).abs() < 0.0015,
        "wet/dry energy {ratio}, expected 0.033631"
    );
}

#[test]
fn delay_lengths_hold_at_96k() {
    // 23 ms × 96 kHz = 2208 samples. A fixed 2048-sample ring used to clamp
    // the last line to 2047 samples (21.32 ms), and at 192 kHz the
    // 11/17/23 ms lines all clamped to 2047 and became one coherent tap.
    // The ring is now sized from the longest tap: 4096 at 96 kHz.
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    stage.init(&context_at_rate(&layout, &listener, &environment, 96000.0));
    assert_eq!(stage.delay_samples, [672, 1056, 1632, 2208]);
    assert_eq!(stage.ring.len(), 4096);
}

/// End to end at 192 kHz: an X impulse must come back once per line at
/// 7/11/17/23 ms = 1344/2112/3264/4416 frames — four distinct taps, not
/// clamped to the ring — and nowhere in between.
#[test]
fn impulse_taps_land_at_their_delays_at_192k() {
    let (layout, listener) = make_ctx(4);
    let mut stage = AmbiMultiDelayStage::new();
    let environment = test_environment();
    let ctx = context_at_rate(&layout, &listener, &environment, 192000.0);
    stage.init(&ctx);
    assert_eq!(stage.delay_samples, [1344, 2112, 3264, 4416]);

    let frames = 5000;
    let mut buffer = vec![0.0f32; frames * 4];
    buffer[3] = 1.0;
    stage.process(&mut buffer, &ctx);

    let onsets: Vec<usize> = (1..frames)
        .filter(|&frame| {
            let current: f32 = buffer[frame * 4..frame * 4 + 4]
                .iter()
                .map(|s| s.abs())
                .sum();
            let previous: f32 = buffer[(frame - 1) * 4..frame * 4]
                .iter()
                .map(|s| s.abs())
                .sum();
            current > 1e-6 && previous < current * 0.5
        })
        .collect();
    assert_eq!(onsets, vec![1344, 2112, 3264, 4416]);
}
