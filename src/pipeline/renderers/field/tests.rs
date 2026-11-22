use super::*;
use atrium_core::types::Vec3;

/// A field from all round, heard at the origin facing +x.
const ALL_ROUND: FieldPlacement = FieldPlacement {
    arc: None,
    receiver: Vec3::ZERO,
    listener_yaw: 0.0,
};

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
        ALL_ROUND,
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

/// On headphones the field is not decorrelated here: each source is handed
/// over as it is, faded by the probe's glide from one callback's share to
/// the next (1 to 0 over four frames: 1, ¾, ½, ¼).
#[test]
fn ears_encoding_hands_the_source_over_as_it_is() {
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
        ALL_ROUND,
        &mut bus,
    );
    assert_eq!(bus, [1.0, 0.75, 0.5, 0.25]);
}

// ── Arcs ─────────────────────────────────────────────────────────────

/// The 5.1 layout of `scenes/nature.yaml` around its listening point.
fn nature_5_1() -> SpeakerLayout {
    SpeakerLayout::surround_5_1(
        Vec3::new(-3.0, 2.0, 0.0),
        Vec3::new(3.0, 2.0, 0.0),
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::new(-3.0, -2.0, 0.0),
        Vec3::new(3.0, -2.0, 0.0),
    )
}

/// An arc `width` degrees wide whose middle lies `centre` degrees from +x.
fn arc_towards(width: f32, centre: f32) -> FieldPlacement {
    FieldPlacement {
        arc: Some(AmbientArc {
            width: width.to_radians(),
            centre: centre.to_radians(),
        }),
        receiver: Vec3::ZERO,
        listener_yaw: 0.0,
    }
}

fn render_impulse_from(
    placement: FieldPlacement,
    encoding: FieldEncoding,
    layout: &SpeakerLayout,
    channels: usize,
) -> Vec<f32> {
    let mut renderer = FieldRenderer::default();
    renderer.ensure_topology(1, 48000.0);
    let mut source = ImpulseSource { fired: false };
    let frames = 40_000;
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
        placement,
        &mut output,
    );
    output
}

/// An arc's speakers carry all of the source's power and no more, within
/// 0.1 dB, however wide and wherever it lies: each speaker's copy has its
/// share, and the shares add up to one. The LFE stays silent.
#[test]
fn an_arc_on_speakers_keeps_the_sources_power() {
    let layout = nature_5_1();
    for width in [10.0f32, 60.0, 120.0, 240.0, 350.0] {
        for centre in [0.0f32, 90.0, 147.0, 270.0] {
            let output = render_impulse_from(
                arc_towards(width, centre),
                FieldEncoding::Speaker,
                &layout,
                6,
            );
            let total: f32 = [0, 1, 2, 4, 5]
                .iter()
                .map(|&channel| channel_energy(&output, 6, channel))
                .sum();
            let decibels = 10.0 * total.log10();
            assert!(
                decibels.abs() < 0.1,
                "{width}° at {centre}°: {decibels:+.3} dB"
            );
            assert_eq!(channel_energy(&output, 6, 3), 0.0, "LFE stays silent");
        }
    }
}

/// A narrow arc lies on its speakers: 10° about the front left speaker of a
/// square plays from that speaker alone.
#[test]
fn a_narrow_arc_at_a_speaker_plays_from_it_alone() {
    let layout = SpeakerLayout::quad(
        Vec3::new(-1.0, 1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
    );
    let channels = layout.total_channels();
    let output = render_impulse_from(
        arc_towards(10.0, 135.0),
        FieldEncoding::Speaker,
        &layout,
        channels,
    );
    let front_left = channel_energy(&output, channels, 0);
    let total: f32 = (0..channels)
        .map(|channel| channel_energy(&output, channels, channel))
        .sum();
    assert!(
        front_left / total > 0.99,
        "front left has {:.3}",
        front_left / total
    );
}

/// On the ambisonic bus an arc decodes, through the layout's own decoder,
/// to exactly the power of a point of the same level, within 0.1 dB,
/// whatever its width: the arc's covariance has a plane wave's trace and
/// the decoder preserves energy. All round, as it always has been, it
/// decodes 1.25 dB lower: a quarter of it is in Z, which a horizontal
/// layout never plays.
#[test]
fn an_arc_on_the_ambisonic_bus_decodes_as_loud_as_a_point() {
    use atrium_core::ambisonics::{foa_encode, AllRadDecoder, BFormat};
    use atrium_core::listener::Listener;

    let layout = nature_5_1();
    let listener = Listener::new(Vec3::ZERO, std::f32::consts::FRAC_PI_2);
    let decoder = AllRadDecoder::from_listener_epad(layout.speakers(), 5, &listener);
    let decoded_power = |bformat: &BFormat| -> f64 {
        decoder
            .decode(bformat)
            .gains
            .iter()
            .map(|gain| (*gain as f64).powi(2))
            .sum()
    };
    let point = decoded_power(&foa_encode(0.3, 0.0, 1.0));
    let mix_power = |mix: &[[f32; 4]; 4]| -> f64 {
        (0..4)
            .map(|voice| {
                decoded_power(&BFormat {
                    w: mix[0][voice],
                    y: mix[1][voice],
                    z: mix[2][voice],
                    x: mix[3][voice],
                })
            })
            .sum()
    };
    for width in [10.0f32, 60.0, 120.0, 240.0, 350.0] {
        for centre in [0.0f32, 90.0, 200.0] {
            let mix = super::bformat_arc::bformat_mix(width.to_radians(), centre.to_radians());
            let decibels = 10.0 * (mix_power(&mix) / point).log10();
            assert!(
                decibels.abs() < 0.1,
                "{width}° at {centre}°: {decibels:+.3} dB"
            );
        }
    }
    let all_round = [
        [0.5, 0.0, 0.0, 0.0],
        [0.0, 0.5, 0.0, 0.0],
        [0.0, 0.0, 0.5, 0.0],
        [0.0, 0.0, 0.0, 0.5],
    ];
    let decibels = 10.0 * (mix_power(&all_round) / point).log10();
    assert!(
        (decibels + 1.25).abs() < 0.05,
        "all round {decibels:+.3} dB"
    );
}
