use super::*;

// ── Pre-delay from room geometry (Phase 4A) ──────────────────────────

#[test]
fn pre_delay_10m_cube() {
    // 10m cube: V=1000, S=600 → MFP = 4×1000/600 = 6.667m
    // t = 6.667 / 343.42 ≈ 19.4ms → 932 samples at 48kHz
    let mut fdn = FdnReverbStage::new();
    fdn.compute_pre_delay(48000.0, 1000.0, 600.0, 343.42);
    let expected_ms = 19.4;
    let actual_ms = fdn.pre_delay_samples as f32 / 48.0;
    assert!(
        (actual_ms - expected_ms).abs() < 1.0,
        "10m cube pre-delay should be ~{expected_ms}ms, got {actual_ms:.1}ms ({} samples)",
        fdn.pre_delay_samples
    );
}

#[test]
fn pre_delay_small_room() {
    // 6×4×3m atrium: V=72, S=108 → MFP = 4×72/108 = 2.667m
    // t = 2.667 / 343.42 ≈ 7.8ms → 373 samples at 48kHz
    let mut fdn = FdnReverbStage::new();
    let (volume, surface_area) =
        room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(6.0, 4.0, 3.0));
    fdn.compute_pre_delay(48000.0, volume, surface_area, 343.42);
    let expected_ms = 7.8;
    let actual_ms = fdn.pre_delay_samples as f32 / 48.0;
    assert!(
        (actual_ms - expected_ms).abs() < 1.0,
        "6×4×3m room pre-delay should be ~{expected_ms}ms, got {actual_ms:.1}ms ({} samples)",
        fdn.pre_delay_samples
    );
}

#[test]
fn pre_delay_varies_with_room_size() {
    let mut fdn_small = FdnReverbStage::new();
    let mut fdn_large = FdnReverbStage::new();

    // Small room: 3×3×3m
    let (vol_s, sa_s) = room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(3.0, 3.0, 3.0));
    fdn_small.compute_pre_delay(48000.0, vol_s, sa_s, 343.42);

    // Large room: 20×15×10m
    let (vol_l, sa_l) = room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(20.0, 15.0, 10.0));
    fdn_large.compute_pre_delay(48000.0, vol_l, sa_l, 343.42);

    assert!(
        fdn_large.pre_delay_samples > fdn_small.pre_delay_samples,
        "larger room should have longer pre-delay: {} vs {} samples",
        fdn_large.pre_delay_samples,
        fdn_small.pre_delay_samples
    );
}

#[test]
fn pre_delay_degenerate_room_uses_fallback() {
    let mut fdn = FdnReverbStage::new();
    // Zero volume room → should use fallback (20ms)
    fdn.compute_pre_delay(48000.0, 0.0, 0.0, 343.42);
    let expected_samples = (PRE_DELAY_FALLBACK_SECONDS * 48000.0) as usize;
    assert_eq!(
        fdn.pre_delay_samples, expected_samples,
        "degenerate room should use fallback pre-delay"
    );
}

// ── HF decay from materials (Phase 4B) ────────────────────────────

#[test]
fn hard_walls_rt60_ratio_near_unity() {
    // Hard walls have nearly uniform absorption across bands (0.02–0.05),
    // so RT60 at 500 Hz and 4 kHz should be similar (ratio near 1.0).
    let environment_min = Vec3::ZERO;
    let environment_max = Vec3::new(6.0, 4.0, 3.0);
    let (volume, _) = room_acoustics::room_geometry(environment_min, environment_max);
    let wall_areas = room_acoustics::wall_surface_areas(environment_min, environment_max);
    let atmosphere = AtmosphericParams::default();

    let rt60_low = room_acoustics::sabine_rt60_at_band(
        volume,
        &wall_areas,
        &TEST_ENVIRONMENT.wall_materials,
        &atmosphere,
        room_acoustics::MID_BAND_INDEX,
    );
    let rt60_high = room_acoustics::sabine_rt60_at_band(
        volume,
        &wall_areas,
        &TEST_ENVIRONMENT.wall_materials,
        &atmosphere,
        RT60_HIGH_BAND,
    );

    let ratio = rt60_high / rt60_low;
    // Hard walls: alpha at 500 Hz = 0.03, at 4 kHz = 0.05 + air absorption.
    // Ratio should be between 0.5 and 1.0 (close to 1 but air absorption pulls HF down).
    assert!(
        ratio > 0.4 && ratio < 1.0,
        "hard walls RT60 ratio should be near 1.0, got {ratio:.3} (low={rt60_low:.3}s, high={rt60_high:.3}s)"
    );
}

#[test]
fn carpet_walls_have_lower_hf_ratio() {
    // Carpet has much higher absorption at 4 kHz (0.40) vs 500 Hz (0.08),
    // so RT60_high / RT60_low should be significantly lower than hard walls.
    let environment_min = Vec3::ZERO;
    let environment_max = Vec3::new(6.0, 4.0, 3.0);
    let (volume, _) = room_acoustics::room_geometry(environment_min, environment_max);
    let wall_areas = room_acoustics::wall_surface_areas(environment_min, environment_max);
    let atmosphere = AtmosphericParams::default();

    let carpet_materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::carpet());

    let rt60_low = room_acoustics::sabine_rt60_at_band(
        volume,
        &wall_areas,
        &carpet_materials,
        &atmosphere,
        room_acoustics::MID_BAND_INDEX,
    );
    let rt60_high = room_acoustics::sabine_rt60_at_band(
        volume,
        &wall_areas,
        &carpet_materials,
        &atmosphere,
        RT60_HIGH_BAND,
    );

    let ratio = rt60_high / rt60_low;
    assert!(
        ratio < 0.5,
        "carpet walls should have RT60_high/RT60_low < 0.5, got {ratio:.3} (low={rt60_low:.3}s, high={rt60_high:.3}s)"
    );
}

#[test]
fn fdn_init_uses_material_derived_damping() {
    // Verify that FDN init produces different damping for different materials.
    let environment_min = Vec3::ZERO;
    let environment_max = Vec3::new(6.0, 4.0, 3.0);

    let hard_environment =
        Environment::uniform(environment_min, environment_max, WallMaterial::HARD_WALL);
    let carpet_environment =
        Environment::uniform(environment_min, environment_max, WallMaterial::carpet());

    let layout = SpeakerLayout::new(&[], None, 2);
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

    let mut fdn_hard = FdnReverbStage::new();
    let ctx_hard = MixContext {
        listener: &listener,
        layout: &layout,
        sample_rate: 48000.0,
        channels: 2,
        environment: &hard_environment,
        master_gain: 1.0,
        render_channels: 2,
        reverb_input: None,
        measurement_mode: false,
    };
    fdn_hard.init(&ctx_hard);

    let mut fdn_carpet = FdnReverbStage::new();
    let ctx_carpet = MixContext {
        listener: &listener,
        layout: &layout,
        sample_rate: 48000.0,
        channels: 2,
        environment: &carpet_environment,
        master_gain: 1.0,
        render_channels: 2,
        reverb_input: None,
        measurement_mode: false,
    };
    fdn_carpet.init(&ctx_carpet);

    // Hard walls should have higher output normalization (less damping, more buildup)
    // than carpet walls (more damping, less buildup).
    assert!(
        fdn_hard.output_normalization < fdn_carpet.output_normalization,
        "hard walls should have lower output_norm (more energy buildup) than carpet: hard={}, carpet={}",
        fdn_hard.output_normalization, fdn_carpet.output_normalization
    );
}

#[test]
fn pre_delay_clamped_minimum() {
    let mut fdn = FdnReverbStage::new();
    // Very tiny room: 0.1×0.1×0.1m → MFP ≈ 0.067m → t ≈ 0.19ms
    // Should clamp to 5ms minimum
    let (volume, surface_area) =
        room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(0.1, 0.1, 0.1));
    fdn.compute_pre_delay(48000.0, volume, surface_area, 343.42);
    let min_samples = (0.005 * 48000.0) as usize;
    assert!(
        fdn.pre_delay_samples >= min_samples,
        "pre-delay should be at least 5ms ({min_samples} samples), got {}",
        fdn.pre_delay_samples
    );
}
