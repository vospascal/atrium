use super::*;

/// Values below the knee (±14.0) pass through unchanged.
#[test]
fn feedback_soft_clamp_transparent_below_knee() {
    for &v in &[0.0, 1.0, -1.0, 5.0, -10.0, 13.99, -13.99] {
        let result = feedback_soft_clamp(v);
        assert_eq!(result, v, "value {v} should pass through unchanged");
    }
}

/// Extreme values are smoothly limited below ±16.0.
#[test]
fn feedback_soft_clamp_limits_extreme() {
    let result_pos = feedback_soft_clamp(100.0);
    assert!(
        result_pos > 14.0 && result_pos < 16.0,
        "100.0 should clamp below 16.0, got {result_pos}"
    );
    let result_neg = feedback_soft_clamp(-100.0);
    assert!(
        result_neg < -14.0 && result_neg > -16.0,
        "-100.0 should clamp above -16.0, got {result_neg}"
    );
    // Exact knee value passes through
    assert_eq!(feedback_soft_clamp(14.0), 14.0);
    assert_eq!(feedback_soft_clamp(-14.0), -14.0);
}

/// FDN output is linear — no soft clipping applied. MasterGainStage is
/// the single point of output limiting. Verify that a buffer with values
/// above the soft-clip knee (0.9) passes through without compression.
#[test]
fn fdn_output_is_linear() {
    let (layout, listener) = make_ctx(2, 2);
    let ctx = mix_ctx(&layout, &listener, 2, 2);
    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    // Set every sample to 1.5 (well above the 0.9 soft-clip knee).
    // With no reverb input, the FDN fallback mono-sums the buffer,
    // but output_normalization is small — the key test is that the
    // dry signal at 1.5 is NOT compressed by soft_clip.
    let frames = 64;
    let channels = 2;
    let mut buffer = vec![1.5f32; frames * channels];

    fdn.process(&mut buffer, &ctx);

    // If soft_clip were active, no sample could exceed 1.0.
    // With linear output, dry (1.5) + wet should stay above 1.0.
    let max_sample = buffer.iter().copied().fold(0.0f32, f32::max);
    assert!(
        max_sample > 1.0,
        "FDN output should be linear (no soft-clip at 0.9 knee). \
         Max sample = {max_sample}, expected > 1.0"
    );
    // Stability ceiling still enforced
    assert!(
        max_sample <= 100.0,
        "FDN output should respect ±100.0 stability ceiling. Max = {max_sample}"
    );
}

/// At 96 kHz, all 16 delay lines must remain distinct (not clamped to the
/// same value). The buffer should grow to accommodate scaled delays.
#[test]
fn delays_preserved_at_96khz() {
    let (layout, listener) = make_ctx(2, 2);
    let ctx = MixContext {
        listener: &listener,
        layout: &layout,
        sample_rate: 96000.0,
        channels: 2,
        environment: &TEST_ENVIRONMENT,
        master_gain: 1.0,
        render_channels: 2,
        reverb_input: None,
        measurement_mode: false,
    };
    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    // All 16 delays should be distinct (coprime ratios preserved)
    let mut unique_delays: Vec<usize> = fdn.delays.to_vec();
    unique_delays.sort();
    unique_delays.dedup();
    assert_eq!(
        unique_delays.len(),
        NUM_LINES,
        "all 16 delays should be distinct at 96 kHz, got {:?}",
        fdn.delays
    );

    // Longest delay should be 3347 × 2 = 6694 at 96 kHz.
    let max_delay = *fdn.delays.iter().max().unwrap();
    assert!(
        max_delay > 5000,
        "longest delay at 96 kHz should be ~6694, got {max_delay}"
    );

    // Buffer should have grown beyond the 48 kHz minimum of 512
    assert!(
        fdn.buffer_size >= 1024,
        "buffer should be >= 1024 at 96 kHz, got {}",
        fdn.buffer_size
    );
}

/// required_buffer_size scales correctly with sample rate.
#[test]
fn buffer_size_scales_with_sample_rate() {
    // Longest delay 3347 + 10 modulation + 2 interpolation taps @ 48 kHz
    // → next_pow2(3360) = 4096.
    assert_eq!(FdnReverbStage::required_buffer_size(48000.0), 4096);
    assert!(FdnReverbStage::required_buffer_size(96000.0) >= 8192);
    assert!(FdnReverbStage::required_buffer_size(44100.0) >= 4096);
    // Very high sample rate
    assert!(FdnReverbStage::required_buffer_size(192000.0) >= 16384);
}

/// Modal-density guard — the fix for tinny/metallic reverb. Mode spacing is
/// Δf = fs / Σ(delays); sparse modes (large Δf) ring metallically on a long
/// tail. The old box-scale delays gave ~15.5 Hz; hall-scale delays must give
/// well under 4 Hz so modes pack densely enough for a smooth wash.
#[test]
fn delay_lines_dense_enough_for_smooth_tail() {
    let fs = 48_000.0;
    let mut fdn = FdnReverbStage::new();
    fdn.compute_delays(fs);

    let sum: usize = fdn.delays.iter().sum();
    let mode_spacing = fs / sum as f32;
    // 16 hall-scale lines target ~1.3 Hz — below Schroeder's overlap threshold
    // (~1.4 Hz for a 1.6 s tail), so modes overlap and the tail is smooth.
    assert!(
        mode_spacing < 2.0,
        "FDN mode spacing should be < 2 Hz for a smooth (non-metallic) tail, \
         got {mode_spacing:.1} Hz (Σdelays={sum})"
    );

    // Delays must stay mutually distinct (coprime spread preserved).
    let mut uniq = fdn.delays.to_vec();
    uniq.sort();
    uniq.dedup();
    assert_eq!(
        uniq.len(),
        NUM_LINES,
        "all delay lengths should be distinct: {:?}",
        fdn.delays
    );
}

/// Benchmark: measure FDN processing cost per second of audio.
/// Run with `cargo test fdn_benchmark -- --nocapture` to see timing.
#[test]
fn fdn_benchmark() {
    let channels = 6;
    let render_channels = 6;
    let (layout, listener) = make_ctx(channels, render_channels);
    let ctx = mix_ctx(&layout, &listener, channels, render_channels);

    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    let sample_rate = 48000.0;
    let seconds = 10.0;
    let total_frames = (sample_rate * seconds) as usize;
    let frames_per_buffer = 1024;
    let num_buffers = total_frames / frames_per_buffer;

    // Prepare a buffer with signal and a reverb input
    let buf_len = frames_per_buffer * channels;
    let mut buffer = vec![0.3f32; buf_len];
    let reverb_input = vec![0.2f32; buf_len];

    let ctx_with_reverb = MixContext {
        reverb_input: Some(&reverb_input),
        ..mix_ctx(&layout, &listener, channels, render_channels)
    };

    // Warm up
    for _ in 0..10 {
        buffer.fill(0.3);
        fdn.process(&mut buffer, &ctx_with_reverb);
    }

    let start = std::time::Instant::now();
    for _ in 0..num_buffers {
        buffer.fill(0.3);
        fdn.process(&mut buffer, &ctx_with_reverb);
    }
    let elapsed = start.elapsed();

    let ms_per_second_of_audio = elapsed.as_secs_f64() / seconds * 1000.0;
    let cpu_percent = elapsed.as_secs_f64() / seconds * 100.0;

    eprintln!();
    eprintln!("═══ FDN Benchmark (6ch, 48kHz) ═══");
    eprintln!("  Audio processed: {seconds}s ({total_frames} frames)");
    eprintln!("  Wall time: {:.2}ms", elapsed.as_secs_f64() * 1000.0);
    eprintln!("  Per second of audio: {ms_per_second_of_audio:.2}ms");
    eprintln!("  CPU load: {cpu_percent:.2}%");
    eprintln!(
        "  Per sample: {:.0}ns",
        elapsed.as_nanos() as f64 / total_frames as f64
    );
    eprintln!();

    // Sanity: FDN should use a small fraction of a core for 6ch at 48kHz.
    // 16 delay lines roughly double the 8-line cost (~10% of a core, measured
    // in isolation); the bound is loose to stay reliable under parallel test
    // load while still catching gross regressions.
    assert!(
        cpu_percent < 20.0,
        "FDN should use < 20% CPU, got {cpu_percent:.1}%"
    );
}
