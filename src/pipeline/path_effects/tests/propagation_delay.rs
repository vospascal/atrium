use super::*;

fn make_delay_ctx(delay_seconds: f32) -> (PathContribution, AtmosphericParams, GroundProperties) {
    let path = PathContribution {
        kind: PathKind::Reflection,
        direction: Vec3::new(-1.0, 0.0, 0.0),
        distance: 10.0,
        delay_seconds,
        gain: 0.9,
        wall_index: Some(0),
    };
    (
        path,
        AtmosphericParams::default(),
        GroundProperties::default(),
    )
}

#[test]
fn propagation_delay_zero_is_passthrough() {
    let (path, atmo, ground) = make_delay_ctx(0.0);
    let mut effect = PropagationDelayEffect::new(48000.0, 8192);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };
    effect.update(&ctx);

    // With zero delay, output should equal input immediately
    let out = effect.process_sample(0.5);
    assert!(
        (out - 0.5).abs() < 1e-6,
        "zero delay should pass through, got {out}"
    );
}

#[test]
fn propagation_delay_integer_samples() {
    let sample_rate = 48000.0;
    let delay_samples = 10.0;
    let delay_seconds = delay_samples / sample_rate;

    let (path, atmo, ground) = make_delay_ctx(delay_seconds);
    let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };
    effect.update(&ctx);

    // Feed an impulse at sample 0, then silence
    let _ = effect.process_sample(1.0);
    for _ in 1..10 {
        let out = effect.process_sample(0.0);
        assert!(out.abs() < 1e-6, "should be silent before delay, got {out}");
    }
    // At sample 10 (after 10 samples of delay), the impulse should appear
    let out = effect.process_sample(0.0);
    assert!(
        (out - 1.0).abs() < 1e-6,
        "impulse should appear at delay offset, got {out}"
    );
}

#[test]
fn propagation_delay_fractional_interpolates() {
    let sample_rate = 48000.0;
    let delay_samples = 5.5;
    let delay_seconds = delay_samples / sample_rate;

    let (path, atmo, ground) = make_delay_ctx(delay_seconds);
    let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };
    effect.update(&ctx);

    // Feed an impulse, then silence
    let _ = effect.process_sample(1.0);
    for _ in 1..5 {
        let _ = effect.process_sample(0.0);
    }
    // At sample 5 (integer part of 5.5): should get partial impulse
    let out5 = effect.process_sample(0.0);
    // At sample 6: should get the complementary part
    let out6 = effect.process_sample(0.0);

    // Linear interpolation: sample 5 gets 0.5, sample 6 gets 0.5
    assert!(
        (out5 - 0.5).abs() < 1e-6,
        "fractional delay at int part: expected 0.5, got {out5}"
    );
    assert!(
        (out6 - 0.5).abs() < 1e-6,
        "fractional delay at int+1: expected 0.5, got {out6}"
    );
}

#[test]
fn propagation_delay_reset_clears_buffer() {
    let sample_rate = 48000.0;
    let delay_seconds = 10.0 / sample_rate;

    let (path, atmo, ground) = make_delay_ctx(delay_seconds);
    let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };
    effect.update(&ctx);

    // Fill buffer with signal
    for _ in 0..20 {
        effect.process_sample(1.0);
    }

    // Reset should clear everything
    effect.reset();
    effect.update(&ctx);

    // After reset, feeding silence should produce silence (no stale data)
    for _ in 0..20 {
        let out = effect.process_sample(0.0);
        assert!(out.abs() < 1e-6, "after reset, should be silent, got {out}");
    }
}

#[test]
fn propagation_delay_preserves_signal_energy() {
    let sample_rate = 48000.0;
    let delay_seconds = 100.0 / sample_rate;

    let (path, atmo, ground) = make_delay_ctx(delay_seconds);
    let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };
    effect.update(&ctx);

    // Feed N samples of DC, then enough silence for the delay to drain
    let n = 200;
    let mut total_out = 0.0;
    for _ in 0..n {
        total_out += effect.process_sample(1.0);
    }
    for _ in 0..200 {
        total_out += effect.process_sample(0.0);
    }

    // Total energy out should equal total energy in (delay is lossless)
    assert!(
        (total_out - n as f32).abs() < 1e-3,
        "delay should preserve energy: expected {n}, got {total_out}"
    );
}

// ── Hand-derived pins (audit 2026-11) ──

/// The +X reflection of source (1,0,0) / listener origin in a [-5,5]³ room
/// travels 8 m further than the direct path: 8 / 343.42 m/s = 23.2951 ms.
/// At 48 kHz that is 1118.164 samples, so linear interpolation splits a unit
/// impulse into 1 − 0.164 = 0.836 at sample 1118 and 0.164 at sample 1119.
/// At 44.1 kHz: 1027.313 → 0.687 / 0.313. At 96 kHz: 2236.327 → 0.673 / 0.327.
#[test]
fn propagation_delay_places_reflection_impulse_at_hand_derived_sample() {
    let atmosphere = AtmosphericParams::default();
    let ground = GroundProperties::default();
    let delay_seconds = 8.0 / atmosphere.speed_of_sound();
    let path = reflection_path(1, delay_seconds);
    for (sample_rate, integer_part, fraction) in [
        (44_100.0_f32, 1027usize, 0.313_f32),
        (48_000.0, 1118, 0.164),
        (96_000.0, 2236, 0.327),
    ] {
        let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
        effect.update(&PathEffectContext {
            path: &path,
            environment: &test_environment(&default_wall_materials(), &ground, &atmosphere),
            sample_rate,
            source_pos: Vec3::new(1.0, 0.0, 0.0),
            target_pos: Vec3::ZERO,
        });
        let mut response = vec![0.0f32; integer_part + 3];
        for (sample_index, output) in response.iter_mut().enumerate() {
            let input = if sample_index == 0 { 1.0 } else { 0.0 };
            *output = effect.process_sample(input);
        }
        assert!(response[..integer_part]
            .iter()
            .all(|value| value.abs() < 1e-6));
        assert!(
            (response[integer_part] - (1.0 - fraction)).abs() < 2e-3,
            "{sample_rate} Hz: sample {integer_part} = {}",
            response[integer_part]
        );
        assert!(
            (response[integer_part + 1] - fraction).abs() < 2e-3,
            "{sample_rate} Hz: sample {} = {}",
            integer_part + 1,
            response[integer_part + 1]
        );
        assert!(response[integer_part + 2].abs() < 1e-6);
    }
}

/// Requested delays beyond the ring are clamped to capacity − 2 instead of
/// wrapping and reading future samples. 8192-sample ring → 8190 samples.
#[test]
fn propagation_delay_clamps_to_capacity_minus_two() {
    let atmosphere = AtmosphericParams::default();
    let ground = GroundProperties::default();
    let sample_rate = 48_000.0;
    let path = reflection_path(0, 1.0); // 48000 samples requested
    let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
    effect.update(&PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmosphere),
        sample_rate,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    });
    let mut first_nonzero = None;
    for sample_index in 0..9000 {
        let input = if sample_index == 0 { 1.0 } else { 0.0 };
        if effect.process_sample(input).abs() > 0.5 && first_nonzero.is_none() {
            first_nonzero = Some(sample_index);
        }
    }
    assert_eq!(first_nonzero, Some(8190));
}

/// A moving source changes `delay_seconds` every buffer. The delay line
/// jumps straight to the new tap instead of ramping, so the output skips
/// samples. A 1 kHz sine at 48 kHz never moves more than
/// 2π·1000/48000 = 0.131 per sample; alternating the tap between 100 and
/// 105 samples every 256-sample buffer (≈ 0.036 m of path per 5.3 ms, a
/// 6.8 m/s path-length rate — a darting source) skips 4 samples forward
/// (100 → 105) or 6 back (105 → 100), steps up to 2·sin(3·ω) = 0.765 —
/// a click on every buffer boundary (measured 0.759). WorldLocked
/// already ramps its arrival delay per sample; the per-path chain does not.
#[test]
fn propagation_delay_change_is_click_free() {
    let atmosphere = AtmosphericParams::default();
    let ground = GroundProperties::default();
    let sample_rate = 48_000.0;
    let frequency = 1000.0;
    let largest_natural_step = std::f32::consts::TAU * frequency / sample_rate;
    let mut effect = PropagationDelayEffect::new(sample_rate, 8192);
    let mut previous_output = 0.0f32;
    let mut largest_step = 0.0f32;
    for sample_index in 0..48_000usize {
        if sample_index % 256 == 0 {
            let delay_samples = if (sample_index / 256) % 2 == 0 {
                100.0
            } else {
                105.0
            };
            let path = reflection_path(0, delay_samples / sample_rate);
            effect.update(&PathEffectContext {
                path: &path,
                environment: &test_environment(&default_wall_materials(), &ground, &atmosphere),
                sample_rate,
                source_pos: Vec3::ZERO,
                target_pos: Vec3::new(1.0, 0.0, 0.0),
            });
        }
        let input = (std::f32::consts::TAU * frequency * sample_index as f32 / sample_rate).sin();
        let output = effect.process_sample(input);
        if sample_index > 1024 {
            largest_step = largest_step.max((output - previous_output).abs());
        }
        previous_output = output;
    }
    assert!(
        largest_step < 1.5 * largest_natural_step,
        "largest sample-to-sample step {largest_step} vs sine slope {largest_natural_step}"
    );
}
