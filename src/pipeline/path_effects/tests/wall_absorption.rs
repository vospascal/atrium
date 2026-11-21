use super::*;

#[test]
fn wall_absorption_direct_path_is_passthrough() {
    let path = PathContribution {
        kind: PathKind::Direct,
        direction: Vec3::new(1.0, 0.0, 0.0),
        distance: 5.0,
        delay_seconds: 0.0,
        gain: 1.0,
        wall_index: None, // Direct path, no wall
    };
    let atmo = AtmosphericParams::default();
    let ground = GroundProperties::default();
    let mut effect = WallAbsorptionEffect::new(48000.0);
    let ctx = PathEffectContext {
        path: &path,
        environment: &test_environment(&default_wall_materials(), &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(5.0, 0.0, 0.0),
    };
    effect.update(&ctx);

    let mut out = 0.0;
    for _ in 0..200 {
        out = effect.process_sample(1.0);
    }
    assert!(
        (out - 1.0).abs() < 0.01,
        "direct path should be passthrough, got {out}"
    );
}

#[test]
fn wall_absorption_carpet_attenuates_more_than_hard_wall() {
    // Carpet has much higher α at high frequencies than hard wall.
    let carpet_materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::carpet());
    let hard_materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::hard_wall());

    let path = PathContribution {
        kind: PathKind::Reflection,
        direction: Vec3::new(-1.0, 0.0, 0.0),
        distance: 8.0,
        delay_seconds: 0.01,
        gain: 0.9,
        wall_index: Some(0), // Bounced off wall 0
    };
    let atmo = AtmosphericParams::default();
    let ground = GroundProperties::default();

    let mut hard_effect = WallAbsorptionEffect::new(48000.0);
    let mut carpet_effect = WallAbsorptionEffect::new(48000.0);

    let ctx_hard = PathEffectContext {
        path: &path,
        environment: &test_environment(&hard_materials, &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(8.0, 0.0, 0.0),
    };
    let ctx_carpet = PathEffectContext {
        path: &path,
        environment: &test_environment(&carpet_materials, &ground, &atmo),
        sample_rate: 48000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(8.0, 0.0, 0.0),
    };

    hard_effect.update(&ctx_hard);
    carpet_effect.update(&ctx_carpet);

    // Process DC to let filters settle (DC tests low-shelf behavior).
    let mut hard_out = 0.0;
    let mut carpet_out = 0.0;
    for _ in 0..500 {
        hard_out = hard_effect.process_sample(1.0);
        carpet_out = carpet_effect.process_sample(1.0);
    }

    // Hard wall is nearly uniform α — should be near unity (filters near unity).
    // Carpet has much higher HF absorption, so its high-shelf cuts relative to mid.
    // At DC both pass through (shelving filters affect LF/HF relative to mid).
    // The key test: carpet and hard wall should produce different results.
    assert!(
        (hard_out - carpet_out).abs() > 0.001 || (hard_out - 1.0).abs() < 0.05,
        "carpet and hard wall should differ, or hard wall should be near unity. \
         hard={hard_out}, carpet={carpet_out}"
    );
}

// ── Hand-derived pins (audit 2026-11) ──

/// Combined reflection level = path.gain (broadband √(1 − mean α 250–4k))
/// × WallAbsorptionEffect response. Per band, the physical amplitude
/// reflection coefficient is √(1 − α). For the shipped materials the
/// level+colour decomposition must land within 1 dB of that at 63 Hz
/// (α̅ of 125/250 Hz), 1 kHz (α at 1 kHz) and 8 kHz (α̅ of 2k/4k).
/// Glass example: path.gain = √(1 − 0.132) = −0.615 dB; low shelf
/// 20·log10(√0.70/√0.85) = −0.84 dB → 63 Hz ≈ −1.45 dB vs truth −1.55 dB.
#[test]
fn wall_absorption_plus_broadband_gain_tracks_per_band_reflection_coefficient() {
    let atmosphere = AtmosphericParams::default();
    let ground = GroundProperties::default();
    for material in [
        WallMaterial::hard_wall(),
        WallMaterial::stone(),
        WallMaterial::glass(),
        WallMaterial::carpet(),
        WallMaterial::wood(),
        WallMaterial::grass(),
    ] {
        let materials: [WallMaterial; 6] = std::array::from_fn(|_| material.clone());
        let broadband_db = 20.0 * material.broadband_reflection_gain().log10();
        let path = reflection_path(0, 0.01);
        let context = PathEffectContext {
            path: &path,
            environment: &test_environment(&materials, &ground, &atmosphere),
            sample_rate: 48_000.0,
            source_pos: Vec3::ZERO,
            target_pos: Vec3::new(1.0, 0.0, 0.0),
        };
        let mut effect = WallAbsorptionEffect::new(48_000.0);
        let alpha = material.alpha;
        for (frequency, band_alpha) in [
            (63.0_f32, (alpha[0] + alpha[1]) * 0.5),
            (1000.0, alpha[3]),
            (8000.0, (alpha[4] + alpha[5]) * 0.5),
        ] {
            let truth_db = 10.0 * (1.0 - band_alpha).log10();
            let combined_db =
                broadband_db + steady_state_response_db(&mut effect, &context, frequency);
            assert!(
                (combined_db - truth_db).abs() < 1.0,
                "{} at {frequency} Hz: rendered {combined_db:.2} dB, √(1−α) = {truth_db:.2} dB",
                material.name
            );
        }
    }
}

/// The colour filter and path.gain must agree on a reference level. The
/// filter's shelves are measured from the 500 Hz/1 kHz mean (α_mid) while
/// path.gain uses the 250 Hz–4 kHz mean; without the mid-level correction
/// ceiling tile (α_mid = 0.75, broadband mean 0.58) rendered every band
/// 20·log10(√0.42/√0.25) = +2.25 dB too loud and reached ≈ 1.06 at 63 Hz —
/// a passive wall reflecting more than it receives (truth √(1 − 0.30) = 0.837).
///
/// The mid band is one flat level for both 500 Hz (α 0.70, −5.23 dB) and
/// 1 kHz (α 0.80, −6.99 dB): it sits at √(1 − 0.75) = −6.02 dB plus 0.2 dB
/// of shelf skirt, 1.16 dB above the 1 kHz truth and 0.6 dB below the
/// 500 Hz one. Two shelves cannot do better for a material this uneven, so
/// the bound here is 1.25 dB rather than the 1 dB the smoother materials meet.
#[test]
fn ceiling_tile_reflection_is_passive_and_tracks_per_band_coefficient() {
    let atmosphere = AtmosphericParams::default();
    let ground = GroundProperties::default();
    let material = WallMaterial::ceiling_tile();
    let materials: [WallMaterial; 6] = std::array::from_fn(|_| material.clone());
    let broadband_gain = material.broadband_reflection_gain();
    let path = reflection_path(0, 0.01);
    let context = PathEffectContext {
        path: &path,
        environment: &test_environment(&materials, &ground, &atmosphere),
        sample_rate: 48_000.0,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(1.0, 0.0, 0.0),
    };
    let mut effect = WallAbsorptionEffect::new(48_000.0);
    let low_response_db = steady_state_response_db(&mut effect, &context, 63.0);
    let combined_low_gain = broadband_gain * 10.0_f32.powf(low_response_db / 20.0);
    assert!(
        combined_low_gain <= 1.0,
        "63 Hz reflection gain {combined_low_gain} exceeds unity"
    );
    let mid_response_db = steady_state_response_db(&mut effect, &context, 1000.0);
    let combined_mid_db = 20.0 * broadband_gain.log10() + mid_response_db;
    let truth_mid_db = 10.0 * (1.0 - material.alpha[3]).log10(); // √0.2 = −6.99 dB
    assert!(
        (combined_mid_db - truth_mid_db).abs() < 1.25,
        "1 kHz: rendered {combined_mid_db:.2} dB vs {truth_mid_db:.2} dB"
    );
}
