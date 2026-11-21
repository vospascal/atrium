use super::*;

/// FDN with render_channels < channels must not touch channels beyond render_channels.
#[test]
fn render_channels_limits_wet_output() {
    let channels = 6;
    let render_channels = 2; // HRTF on 5.1 device
    let (layout, listener) = make_ctx(channels, render_channels);
    let ctx = mix_ctx(&layout, &listener, channels, render_channels);

    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    let frames = 2048;
    let mut buffer = vec![0.0f32; frames * channels];

    // Put signal only in channels 0 and 1 (stereo HRTF output)
    for frame in 0..frames {
        buffer[frame * channels] = 0.5;
        buffer[frame * channels + 1] = 0.5;
    }

    fdn.process(&mut buffer, &ctx);

    // Channels 2-5 must remain exactly zero
    for frame in 0..frames {
        for ch in 2..channels {
            assert_eq!(
                buffer[frame * channels + ch], 0.0,
                "channel {ch} at frame {frame} should be zero when render_channels={render_channels}"
            );
        }
    }

    // Channels 0-1 should have been modified (dry + wet)
    let energy_ch0: f32 = buffer.iter().step_by(channels).map(|s| s * s).sum();
    let energy_ch1: f32 = buffer.iter().skip(1).step_by(channels).map(|s| s * s).sum();
    assert!(energy_ch0 > 0.0, "channel 0 should have signal");
    assert!(energy_ch1 > 0.0, "channel 1 should have signal");
}

/// When render_channels == channels, all channels get wet signal (normal behavior).
#[test]
fn full_channels_all_get_wet() {
    let channels = 6;
    let render_channels = 6;
    let (layout, listener) = make_ctx(channels, render_channels);
    let ctx = mix_ctx(&layout, &listener, channels, render_channels);

    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    let frames = 2048;
    let mut buffer = vec![0.0f32; frames * channels];

    // Put signal in all channels
    for sample in buffer.iter_mut() {
        *sample = 0.5;
    }

    fdn.process(&mut buffer, &ctx);

    // All channels should have been processed (not just passthrough)
    for ch in 0..channels {
        let max_abs = (0..frames)
            .map(|f| buffer[f * channels + ch].abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_abs > 0.0,
            "channel {ch} should have signal with render_channels={render_channels}"
        );
    }
}

/// FDN only sums mono from render_channels, ignoring higher channels.
#[test]
fn mono_sum_only_from_render_channels() {
    let channels = 6;
    let render_channels = 2;
    let (layout, listener) = make_ctx(channels, render_channels);
    let ctx = mix_ctx(&layout, &listener, channels, render_channels);

    let mut fdn_a = FdnReverbStage::new();
    let mut fdn_b = FdnReverbStage::new();
    fdn_a.init(&ctx);
    fdn_b.init(&ctx);

    let frames = 1024;

    // Buffer A: signal in channels 0-1, noise in channels 2-5
    let mut buffer_a = vec![0.0f32; frames * channels];
    for frame in 0..frames {
        buffer_a[frame * channels] = 0.5;
        buffer_a[frame * channels + 1] = 0.5;
        for ch in 2..channels {
            buffer_a[frame * channels + ch] = 99.0; // should be ignored
        }
    }

    // Buffer B: same signal in channels 0-1, zeros in 2-5
    let mut buffer_b = vec![0.0f32; frames * channels];
    for frame in 0..frames {
        buffer_b[frame * channels] = 0.5;
        buffer_b[frame * channels + 1] = 0.5;
    }

    fdn_a.process(&mut buffer_a, &ctx);
    fdn_b.process(&mut buffer_b, &ctx);

    // Channels 0-1 should be identical regardless of what's in channels 2-5
    for frame in 0..frames {
        for ch in 0..2 {
            let a = buffer_a[frame * channels + ch];
            let b = buffer_b[frame * channels + ch];
            assert!(
                (a - b).abs() < 1e-6,
                "ch{ch} frame {frame}: content beyond render_channels should not affect output ({a} vs {b})"
            );
        }
    }
}

/// Quad channel mode on 5.1 hardware: FDN only writes to active channels [0,1,4,5].
/// Channels 2 (C) and 3 (LFE) must stay silent despite render_channels=6.
#[test]
fn active_mask_respects_quad_on_5_1() {
    let channels = 6;
    let render_channels = 6;
    let (mut layout, listener) = make_ctx(channels, render_channels);
    // Quad channel mode: only FL(0), FR(1), RL(4), RR(5) active
    layout.set_active_channels(&[0, 1, 4, 5]);
    let ctx = mix_ctx(&layout, &listener, channels, render_channels);

    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    let frames = 2048;
    let mut buffer = vec![0.0f32; frames * channels];

    // Signal in active channels only
    for frame in 0..frames {
        buffer[frame * channels] = 0.5; // FL
        buffer[frame * channels + 1] = 0.5; // FR
        buffer[frame * channels + 4] = 0.5; // RL
        buffer[frame * channels + 5] = 0.5; // RR
    }

    fdn.process(&mut buffer, &ctx);

    // Channels 2 (C) and 3 (LFE) must remain zero
    for frame in 0..frames {
        for ch in [2, 3] {
            assert_eq!(
                buffer[frame * channels + ch],
                0.0,
                "channel {ch} at frame {frame} should be zero in quad mode"
            );
        }
    }

    // Active channels should have signal
    for ch in [0, 1, 4, 5] {
        let has_signal = (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10);
        assert!(has_signal, "active channel {ch} should have signal");
    }
}

/// Silent input produces silent output (no self-oscillation).
#[test]
fn silent_input_silent_output() {
    let channels = 6;
    let render_channels = 2;
    let (layout, listener) = make_ctx(channels, render_channels);
    let ctx = mix_ctx(&layout, &listener, channels, render_channels);

    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    let frames = 512;
    let mut buffer = vec![0.0f32; frames * channels];
    fdn.process(&mut buffer, &ctx);

    let max = buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(max < 1e-10, "silent input should produce silent output");
}

/// When reverb_input is provided (DBAP/VBAP/HRTF mono bus on ch0),
/// FDN reads ch0 directly and produces wet output on active channels.
#[test]
fn reverb_input_mono_bus_produces_wet_on_active_channels() {
    let channels = 6;
    let render_channels = 6;
    let (mut layout, listener) = make_ctx(channels, render_channels);
    // Quad mask: FL(0), FR(1), RL(4), RR(5) active; C(2), LFE(3) masked.
    layout.set_active_channels(&[0, 1, 4, 5]);

    // Long enough to span the late-reverb onset: the hall-scale delay lines
    // start emitting only after pre-delay (~0.9k samples in the test room) plus
    // the shortest delay (~1.3k), so a short window would see no wet yet.
    let frames = 8192;
    let mut reverb_input = vec![0.0f32; frames * channels];
    // Write mono signal to ch0 only (mimics renderer mono bus).
    for frame in 0..frames {
        reverb_input[frame * channels] = 0.5;
    }

    let ctx = MixContext {
        listener: &listener,
        layout: &layout,
        sample_rate: 48000.0,
        channels,
        environment: &TEST_ENVIRONMENT,
        master_gain: 1.0,
        render_channels,
        reverb_input: Some(&reverb_input),
        measurement_mode: false,
    };

    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    let mut buffer = vec![0.0f32; frames * channels];
    fdn.process(&mut buffer, &ctx);

    // Active channels (0, 1, 4, 5) should have wet signal.
    let active_energy: f32 = (0..frames)
        .map(|f| {
            let base = f * channels;
            [0, 1, 4, 5]
                .iter()
                .map(|&ch| buffer[base + ch].powi(2))
                .sum::<f32>()
        })
        .sum();
    assert!(
        active_energy > 0.01,
        "active channels should have wet signal"
    );

    // Masked channels (2, 3) should remain silent.
    let masked_energy: f32 = (0..frames)
        .map(|f| {
            let base = f * channels;
            buffer[base + 2].powi(2) + buffer[base + 3].powi(2)
        })
        .sum();
    assert!(
        masked_energy < 1e-10,
        "masked channels should stay silent, got energy {masked_energy}"
    );
}
