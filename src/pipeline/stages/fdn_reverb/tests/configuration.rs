use super::*;

// ── Configurable reverb: RT60 / pre-delay overrides, voicing, HPF ─────────

/// Total wet energy the FDN adds to a buffer (output − input), isolating the
/// tail from any dry passthrough already in the buffer.
fn added_wet_energy(fdn: &mut FdnReverbStage, ctx: &MixContext, buffer: &mut [f32]) -> f32 {
    let input = buffer.to_vec();
    fdn.process(buffer, ctx);
    buffer
        .iter()
        .zip(&input)
        .map(|(out, dry)| {
            let wet = out - dry;
            wet * wet
        })
        .sum()
}

/// An explicit RT60 override must lengthen the decay: a 3.5 s tail retains far
/// more late energy than a 1.0 s tail, independent of room geometry.
#[test]
fn rt60_override_slows_decay() {
    let channels = 6;
    let (layout, listener) = make_ctx(channels, channels);
    let ctx = mix_ctx(&layout, &listener, channels, channels);

    let tail_energy = |rt60: f32| -> f32 {
        let mut fdn = FdnReverbStage::with_config(FdnConfig {
            rt60_seconds: Some(rt60),
            ..FdnConfig::default()
        });
        fdn.init(&ctx);
        let frames = 48_000; // 1 s
        let mut buffer = vec![0.0f32; frames * channels];
        buffer[0] = 1.0; // impulse into channel 0 (mono-summed as FDN input)
        fdn.process(&mut buffer, &ctx);
        // Energy in the last 0.5 s — pure tail, dry impulse long gone.
        let start = (frames / 2) * channels;
        buffer[start..].iter().map(|s| s * s).sum()
    };

    let long = tail_energy(3.5);
    let short = tail_energy(1.0);
    assert!(
        long > short * 2.0,
        "RT60=3.5s tail should retain much more energy than RT60=1.0s: {long} vs {short}"
    );
}

/// A pre-delay override sets the delay directly (20 ms @ 48 kHz = 960 samples),
/// bypassing the mean-free-path value the test room would otherwise produce.
#[test]
fn pre_delay_override_sets_samples() {
    let channels = 6;
    let (layout, listener) = make_ctx(channels, channels);
    let ctx = mix_ctx(&layout, &listener, channels, channels);

    let mut fdn = FdnReverbStage::with_config(FdnConfig {
        pre_delay_ms: Some(20.0),
        ..FdnConfig::default()
    });
    fdn.init(&ctx);
    assert_eq!(
        fdn.diagnostics().pre_delay_samples,
        960,
        "20 ms @ 48 kHz should be exactly 960 samples of pre-delay"
    );
}

/// Speaker voicing must run the surrounds much wetter than the centre
/// (idea.md: surrounds > fronts > centre), so an impulse leaves far more
/// late energy in Ls/Rs (ch 4/5, gain 1.0) than in C (ch 2, gain 0.15).
#[test]
fn speaker_voicing_surrounds_wetter_than_center() {
    let channels = 6;
    let (layout, listener) = make_ctx(channels, channels);
    let ctx = mix_ctx(&layout, &listener, channels, channels);

    let mut fdn = FdnReverbStage::with_config(FdnConfig::speaker(Some(2.0), None));
    fdn.init(&ctx);

    let frames = 24_000;
    let mut buffer = vec![0.0f32; frames * channels];
    buffer[0] = 1.0; // impulse into channel 0
    fdn.process(&mut buffer, &ctx);

    let ch_energy = |ch: usize| -> f32 {
        (0..frames)
            .map(|f| {
                let v = buffer[f * channels + ch];
                v * v
            })
            .sum::<f32>()
    };
    let center = ch_energy(2); // gain 0.15
    let ls = ch_energy(4); // gain 1.0
    let rs = ch_energy(5); // gain 1.0
    assert!(
        ls > center * 3.0 && rs > center * 3.0,
        "surrounds should be much wetter than centre: Ls={ls}, Rs={rs}, C={center}"
    );
}

/// The reverb-send HPF must strip sub-bass out of the tail: a 40 Hz send with
/// a 160 Hz high-pass produces far less wet energy than with the HPF disabled.
#[test]
fn reverb_hpf_attenuates_low_frequency_send() {
    let channels = 6;
    let (layout, listener) = make_ctx(channels, channels);
    let ctx = mix_ctx(&layout, &listener, channels, channels);

    let wet_energy = |hpf: f32| -> f32 {
        let mut fdn = FdnReverbStage::with_config(FdnConfig {
            hpf_cutoff_hz: hpf,
            ..FdnConfig::default()
        });
        fdn.init(&ctx);
        let frames = 48_000;
        let mut buffer = vec![0.0f32; frames * channels];
        for frame in 0..frames {
            let s = (std::f32::consts::TAU * 40.0 * frame as f32 / 48_000.0).sin() * 0.5;
            for ch in 0..channels {
                buffer[frame * channels + ch] = s;
            }
        }
        added_wet_energy(&mut fdn, &ctx, &mut buffer)
    };

    let with_hpf = wet_energy(160.0);
    let without_hpf = wet_energy(0.0);
    assert!(
        with_hpf < without_hpf * 0.3,
        "160 Hz send HPF should strongly attenuate a 40 Hz tail: with={with_hpf}, without={without_hpf}"
    );
}
