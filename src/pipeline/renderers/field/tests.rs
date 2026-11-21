use super::*;
use atrium_core::types::Vec3;

// ── Audit 2026-11: hand-derived values ────────────────────────────────

/// Unit impulse at frame 0, silence afterwards.
struct ImpulseSource {
    fired: bool,
}
impl SoundSource for ImpulseSource {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        if self.fired {
            0.0
        } else {
            self.fired = true;
            1.0
        }
    }
    fn position(&self) -> Vec3 {
        Vec3::ZERO
    }
    fn tick(&mut self, _dt: f32) {}
}

fn render_impulse(
    encoding: FieldEncoding,
    layout: &SpeakerLayout,
    channels: usize,
    frames: usize,
) -> Vec<f32> {
    let mut renderer = FieldRenderer::default();
    renderer.ensure_topology(1, 48000.0);
    let mut source = ImpulseSource { fired: false };
    let mut output = vec![0.0f32; frames * channels];
    renderer.render_source(
        0,
        &mut source,
        encoding,
        layout,
        48000.0,
        channels,
        channels,
        frames,
        (1.0, 1.0),
        &mut output,
    );
    output
}

fn channel_energy(output: &[f32], channels: usize, channel: usize) -> f32 {
    output
        .chunks_exact(channels)
        .map(|frame| frame[channel] * frame[channel])
        .sum()
}

#[test]
fn ambisonics_field_puts_one_quarter_power_in_each_of_w_y_z_x() {
    // Four voices at gain 1/√4 = ½: each channel's impulse response is
    // ½·h_voice, so its energy is ¼ and the four sum to 1. A voice is four
    // all-passes of gain g = ½ in series; each passes −g at once, so every
    // channel starts at ½·(−½)⁴ = 1/32 = 0.03125.
    // Note: a diffuse field encoded with this engine's foa_encode would have
    // E[W²] = ½ and E[Y²] = E[X²] = a²/3 = ⅙ per unit power; and every
    // production decoder drops Z, so ¼ of the field power never reaches a
    // speaker (−1.25 dB).
    let layout = SpeakerLayout::new(&[], None, 6);
    let channels = 6;
    let output = render_impulse(FieldEncoding::Ambisonics, &layout, channels, 40_000);
    for channel in 0..4 {
        assert!(
            (output[channel] - 0.03125).abs() < 1e-7,
            "ch{channel} first sample {}",
            output[channel]
        );
        let energy = channel_energy(&output, channels, channel);
        assert!((energy - 0.25).abs() < 1e-4, "ch{channel} energy {energy}");
    }
    for channel in 4..channels {
        assert_eq!(channel_energy(&output, channels, channel), 0.0);
    }
}

#[test]
fn speaker_field_skips_lfe_and_splits_power_equally_over_5_speakers() {
    // 5.1: five spatial speakers, LFE (3) excluded; gain 1/√5 → each speaker
    // carries ⅕ of the power, and the total is the source power.
    let layout = SpeakerLayout::surround_5_1(
        Vec3::new(-1.0, 1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
    );
    let channels = 6;
    let output = render_impulse(FieldEncoding::Speaker, &layout, channels, 40_000);
    let mut total = 0.0;
    for channel in [0, 1, 2, 4, 5] {
        let energy = channel_energy(&output, channels, channel);
        assert!((energy - 0.2).abs() < 1e-4, "ch{channel} energy {energy}");
        total += energy;
    }
    assert_eq!(
        channel_energy(&output, channels, 3),
        0.0,
        "LFE stays silent"
    );
    assert!((total - 1.0).abs() < 5e-4, "total field power {total}");
}

/// On headphones the field is not decorrelated here: each source is added
/// to the mono bus as it is, faded by the probe's glide from one callback's
/// share to the next (1 to 0 over four frames: 1, ¾, ½, ¼).
#[test]
fn ears_encoding_adds_the_source_to_the_bus_as_it_is() {
    struct Constant;
    impl SoundSource for Constant {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            1.0
        }
        fn position(&self) -> Vec3 {
            Vec3::ZERO
        }
        fn tick(&mut self, _dt: f32) {}
    }
    let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
    let mut renderer = FieldRenderer::default();
    renderer.ensure_topology(1, 48000.0);
    let mut bus = vec![0.5f32; 4];
    renderer.render_source(
        0,
        &mut Constant,
        FieldEncoding::Ears,
        &layout,
        48000.0,
        1,
        2,
        4,
        (1.0, 0.0),
        &mut bus,
    );
    assert_eq!(bus, [1.5, 1.25, 1.0, 0.75]);
}
