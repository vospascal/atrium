use super::*;

#[test]
fn ground_effect_hard_ground_boosts_low_freq() {
    // Hard ground (G=0): ISO Table 3 gives -1.5 dB at low freq (boost).
    let ground = GroundProperties {
        g_source: 0.0,
        g_receiver: 0.0,
        g_middle: 0.0,
    };
    let path = PathContribution {
        kind: PathKind::Direct,
        direction: Vec3::new(1.0, 0.0, 0.0),
        distance: 10.0,
        delay_seconds: 0.0,
        gain: 1.0,
        wall_index: None,
    };
    let atmo = AtmosphericParams::default();
    let mut effect = GroundEffectFilter::new(48000.0);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&open_floor_wall_materials(), &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::new(0.0, 0.0, 1.5),
        target_pos: Vec3::new(10.0, 0.0, 1.5),
    };
    effect.update(&ctx);

    // Process DC (low freq) — should pass through near unity or slightly boosted.
    let mut out = 0.0;
    for _ in 0..500 {
        out = effect.process_sample(1.0);
    }
    // Hard ground at low freq: -1.5 dB low shelf = ~0.84, but DC passes below shelf.
    // Key point: hard ground does NOT strongly attenuate.
    assert!(
        out > 0.7,
        "hard ground should not strongly attenuate DC, got {out}"
    );
}

#[test]
fn ground_effect_soft_ground_attenuates_more() {
    // Soft ground (G=1) should attenuate more at low freq than hard ground (G=0).
    let hard_ground = GroundProperties {
        g_source: 0.0,
        g_receiver: 0.0,
        g_middle: 0.0,
    };
    let soft_ground = GroundProperties {
        g_source: 1.0,
        g_receiver: 1.0,
        g_middle: 1.0,
    };
    let path = PathContribution {
        kind: PathKind::Direct,
        direction: Vec3::new(1.0, 0.0, 0.0),
        distance: 10.0,
        delay_seconds: 0.0,
        gain: 1.0,
        wall_index: None,
    };
    let atmo = AtmosphericParams::default();

    let mut hard_effect = GroundEffectFilter::new(48000.0);
    let mut soft_effect = GroundEffectFilter::new(48000.0);

    let ctx_hard = PathEffectContext {
        path: &path,
        environment: &test_environment(&open_floor_wall_materials(), &hard_ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::new(0.0, 0.0, 1.5),
        target_pos: Vec3::new(10.0, 0.0, 1.5),
    };
    let ctx_soft = PathEffectContext {
        path: &path,
        environment: &test_environment(&open_floor_wall_materials(), &soft_ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::new(0.0, 0.0, 1.5),
        target_pos: Vec3::new(10.0, 0.0, 1.5),
    };

    hard_effect.update(&ctx_hard);
    soft_effect.update(&ctx_soft);

    // Process some signal to let filters settle.
    let mut hard_out = 0.0;
    let mut soft_out = 0.0;
    for _ in 0..500 {
        hard_out = hard_effect.process_sample(1.0);
        soft_out = soft_effect.process_sample(1.0);
    }

    // Soft ground should produce lower output (more attenuation at low shelf).
    assert!(
        hard_out > soft_out,
        "hard ground ({hard_out}) should pass more than soft ground ({soft_out})"
    );
}

/// Steady-state gain in dB of the ground filter at one frequency.
fn ground_filter_response_db(
    ground: &GroundProperties,
    horizontal_distance: f32,
    source_height: f32,
    receiver_height: f32,
    frequency: f32,
) -> f32 {
    let path = PathContribution {
        kind: PathKind::Direct,
        direction: Vec3::new(1.0, 0.0, 0.0),
        distance: horizontal_distance,
        delay_seconds: 0.0,
        gain: 1.0,
        wall_index: None,
    };
    let atmosphere = AtmosphericParams::default();
    let mut effect = GroundEffectFilter::new(48000.0);
    effect.update(&PathEffectContext {
        path: &path,
        environment: &test_environment(&open_floor_wall_materials(), ground, &atmosphere),
        sample_rate: 48000.0,
        source_pos: Vec3::new(0.0, 0.0, source_height),
        target_pos: Vec3::new(horizontal_distance, 0.0, receiver_height),
    });
    let total = 48000;
    let settle = 24000;
    let mut input_power = 0.0f64;
    let mut output_power = 0.0f64;
    for n in 0..total {
        let input = (std::f32::consts::TAU * frequency * n as f32 / 48000.0).sin();
        let output = effect.process_sample(input);
        if n >= settle {
            input_power += (input as f64).powi(2);
            output_power += (output as f64).powi(2);
        }
    }
    (10.0 * (output_power / input_power).log10()) as f32
}

/// Hard ground is a flat +3 dB (ISO 9613-2: −1.5 dB per region, every band).
/// The old filter applied the attenuation with the wrong sign and CUT by
/// 1.5 dB, disagreeing with the WorldLocked renderer's broadband gain.
#[test]
fn ground_filter_hard_ground_is_flat_plus_three_db() {
    let hard = GroundProperties::hard();
    for frequency in [63.0, 125.0, 500.0, 2000.0, 8000.0] {
        let response = ground_filter_response_db(&hard, 10.0, 1.5, 1.5, frequency);
        assert!(
            (response - 3.0).abs() < 0.05,
            "{frequency} Hz: expected +3.0 dB, got {response}"
        );
    }
}

/// The filter is a three-stage fit to the ISO octave-band table. It must
/// track the table within 3 dB from 125 Hz to 8 kHz (measured worst case: 2.9 dB,
/// where a deep 250–500 Hz dip leaks into the 125 Hz band), for porous and mixed
/// ground, at ear height and on the ground, near and far.
#[test]
fn ground_filter_tracks_iso_band_table() {
    use crate::audio::propagation::{ground_effect_bands, GROUND_BAND_FREQUENCIES};
    let mut worst = 0.0f32;
    for ground in [GroundProperties::soft(), GroundProperties::mixed(0.5)] {
        for (distance, source_height, receiver_height) in [
            (10.0, 1.2, 1.2),
            (20.0, 6.0, 1.2),
            (10.0, 0.0, 1.2),
            (60.0, 0.3, 1.2),
            (3.0, 1.0, 1.7),
        ] {
            let bands = ground_effect_bands(distance, source_height, receiver_height, &ground);
            for band in 1..8 {
                let frequency = GROUND_BAND_FREQUENCIES[band];
                let response = ground_filter_response_db(
                    &ground,
                    distance,
                    source_height,
                    receiver_height,
                    frequency,
                );
                let error = response - (-bands[band]);
                worst = worst.max(error.abs());
                assert!(
                    error.abs() < 3.0,
                    "G={} d={distance} hs={source_height} hr={receiver_height} {frequency} Hz: \
                     table {:.2} dB, filter {response:.2} dB",
                    ground.g_source,
                    -bands[band]
                );
            }
        }
    }
    eprintln!("ground filter worst deviation from ISO table: {worst:.2} dB");
}

/// Porous ground never boosts through the filter either.
#[test]
fn ground_filter_porous_ground_never_boosts() {
    let soft = GroundProperties::soft();
    for frequency in [125.0, 250.0, 500.0, 1000.0, 4000.0] {
        let response = ground_filter_response_db(&soft, 15.0, 1.2, 1.2, frequency);
        assert!(response < 0.5, "{frequency} Hz: {response} dB");
    }
}

#[test]
fn ground_effect_short_distance_is_unity() {
    let ground = GroundProperties::default();
    let path = PathContribution {
        kind: PathKind::Direct,
        direction: Vec3::new(1.0, 0.0, 0.0),
        distance: 0.1,
        delay_seconds: 0.0,
        gain: 1.0,
        wall_index: None,
    };
    let atmo = AtmosphericParams::default();
    let mut effect = GroundEffectFilter::new(48000.0);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&open_floor_wall_materials(), &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(0.1, 0.0, 0.0),
    };
    effect.update(&ctx);

    let mut out = 0.0;
    for _ in 0..200 {
        out = effect.process_sample(1.0);
    }
    assert!(
        (out - 1.0).abs() < 0.01,
        "short distance should be near unity, got {out}"
    );
}

// ── Hand-derived pins (audit 2026-11) ──

/// Ground effect for a hard floor: ISO 9613-2 gives A_gr = −3 dB (direct +
/// ground-reflected ray, energy sum). In a room, the first-order floor
/// image source (wall index 4) IS that ground-reflected ray, rendered as
/// its own path. The chain still applies the full ground effect to it — and
/// to the direct path that already has the floor image beside it — so the
/// floor bounce is counted twice. The floor path should pass the ground
/// stage at 0 dB; it gets +3 dB.
#[test]
fn ground_effect_is_not_applied_to_the_floor_reflection_path() {
    let atmosphere = AtmosphericParams::default();
    let ground = GroundProperties::hard();
    let floor_path = PathContribution {
        kind: PathKind::Reflection,
        direction: Vec3::new(0.97, 0.0, -0.24),
        distance: 10.28,
        delay_seconds: 0.0008,
        gain: 0.98,
        wall_index: Some(4),
    };
    let mut effect = GroundEffectFilter::new(48_000.0);
    let context = PathEffectContext {
        path: &floor_path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmosphere),
        sample_rate: 48_000.0,
        source_pos: Vec3::new(0.0, 0.0, 1.2),
        target_pos: Vec3::new(10.0, 0.0, 1.2),
    };
    let response = steady_state_response_db(&mut effect, &context, 1000.0);
    assert!(
        response.abs() < 0.1,
        "floor reflection got {response:.2} dB of ground effect on top of its own bounce"
    );
}
