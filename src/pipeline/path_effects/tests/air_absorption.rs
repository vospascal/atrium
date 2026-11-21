use super::*;

#[test]
fn air_absorption_transparent_at_zero_distance() {
    let (path, atmo, ground) = make_ctx(0.0);
    let mut effect = AirAbsorptionEffect::new(48000.0);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };
    effect.update(&ctx);
    // Feed DC (1.0) until the biquad settles — a 20kHz lowpass passes DC
    let mut out = 0.0;
    for _ in 0..200 {
        out = effect.process_sample(1.0);
    }
    assert!(
        (out - 1.0).abs() < 0.01,
        "expected near-transparent at 0m after settling, got {out}"
    );
}

#[test]
fn air_absorption_attenuates_at_distance() {
    let (path_near, atmo, ground) = make_ctx(2.0);
    let (path_far, _, _) = make_ctx(50.0);
    let mut effect_near = AirAbsorptionEffect::new(48000.0);
    let mut effect_far = AirAbsorptionEffect::new(48000.0);

    let ctx_near = PathEffectContext {
        path: &path_near,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };
    let ctx_far = PathEffectContext {
        path: &path_far,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };

    effect_near.update(&ctx_near);
    effect_far.update(&ctx_far);

    // Process a burst of samples to let the filter settle
    let mut out_near = 0.0;
    let mut out_far = 0.0;
    for _ in 0..1000 {
        out_near = effect_near.process_sample(1.0);
        out_far = effect_far.process_sample(1.0);
    }
    // Far distance should attenuate more (lower cutoff = more HF loss)
    assert!(
        out_near >= out_far,
        "near ({out_near}) should >= far ({out_far})"
    );
}

/// Verify that the 2-shelf air absorption filter matches ISO 9613
/// at octave-band frequencies within 3 dB at 50m distance.
#[test]
fn air_absorption_multiband_matches_iso9613() {
    use crate::audio::atmosphere::iso9613_alpha;

    let sample_rate = 48000.0;
    let distance = 50.0;
    let atmo = AtmosphericParams::default();

    // Set up the effect at 50m distance.
    let (path, _, ground) = make_ctx(distance);
    let mut effect = AirAbsorptionEffect::new(sample_rate);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(distance, 0.0, 0.0),
    };
    effect.update(&ctx);

    // Measure filter gain at octave-band frequencies by feeding sine waves.
    let test_freqs = [250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
    let num_frames = 8192;

    for &freq in &test_freqs {
        effect.reset();
        effect.update(&ctx);

        // Feed sine, measure steady-state output amplitude.
        let mut max_out = 0.0f32;
        for i in 0..num_frames {
            let t = i as f32 / sample_rate;
            let input = (2.0 * std::f32::consts::PI * freq * t).sin();
            let out = effect.process_sample(input);
            // Only measure in the last quarter (after settling).
            if i > num_frames * 3 / 4 {
                max_out = max_out.max(out.abs());
            }
        }

        // Expected attenuation from ISO 9613.
        let expected_db = -iso9613_alpha(freq, &atmo) * distance;
        let expected_linear = 10.0_f32.powf(expected_db / 20.0);

        // Filter gain in dB.
        let filter_db = if max_out > 1e-10 {
            20.0 * max_out.log10()
        } else {
            -100.0
        };

        let error_db = (filter_db - expected_db).abs();

        eprintln!(
            "  {freq:>5.0} Hz: filter={filter_db:>6.2} dB, \
             ISO={expected_db:>6.2} dB (linear={expected_linear:.4}), \
             error={error_db:.2} dB"
        );

        // Within 3 dB at each octave band (250 Hz–8 kHz).
        assert!(
            error_db < 3.0,
            "Air absorption at {freq} Hz, {distance}m: filter={filter_db:.2} dB, \
             expected={expected_db:.2} dB, error={error_db:.2} dB > 3 dB"
        );
    }
}
