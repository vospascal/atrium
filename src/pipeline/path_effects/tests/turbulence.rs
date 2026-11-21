use super::*;
use crate::audio::turbulence::{AirMotion, DECIBELS_PER_NEPER};

/// A meadow: nothing reflects, the sky is open.
fn open_sky() -> [WallMaterial; 6] {
    std::array::from_fn(|_| WallMaterial::open())
}

/// The deviation of a steady signal's level through the effect over five
/// minutes (dB), for a path from `source` to ears at the origin in `air`.
fn level_deviation_db(walls: &[WallMaterial; 6], air: AirMotion, source: Vec3) -> f32 {
    let ears = Vec3::new(0.0, 0.0, 1.2);
    let (path, atmosphere, ground) = make_ctx(source.distance_to(ears));
    let mut environment = test_environment(walls, &ground, &atmosphere);
    environment.air = air;
    let mut effect = TurbulenceEffect::new(1);
    let mut levels = Vec::new();
    // A buffer of 480 samples at a time, as the engine updates it.
    for _ in 0..30_000 {
        effect.update(&PathEffectContext {
            path: &path,
            environment: &environment,
            sample_rate: 48_000.0,
            source_pos: source,
            target_pos: ears,
        });
        let mut last = 0.0;
        for _ in 0..480 {
            last = effect.process_sample(1.0);
        }
        levels.push(last.ln());
    }
    let mean = levels.iter().sum::<f32>() / levels.len() as f32;
    let variance = levels
        .iter()
        .map(|level| (level - mean).powi(2))
        .sum::<f32>()
        / levels.len() as f32;
    DECIBELS_PER_NEPER * variance.sqrt()
}

/// A steady signal is the bass of the filter's two parts. For a source 40 m
/// off and 1.8 m up in a 5 m/s breeze across the path, the theory gives
/// 0.62 dB at 500 Hz and the low part a little less; a robin at 3 m is not
/// touched audibly; and under a roof nothing happens at all.
#[test]
fn a_far_sound_wavers_outdoors_and_a_near_or_indoor_one_does_not() {
    let breeze = AirMotion::from_weather(5.0 / 13.0, 0.0, 270.0);
    let far = Vec3::new(0.0, 40.0, 1.8);
    let near = Vec3::new(0.0, 3.0, 1.8);

    let far_outdoors = level_deviation_db(&open_sky(), breeze, far);
    assert!(
        (0.3..0.7).contains(&far_outdoors),
        "40 m in a breeze: {far_outdoors} dB"
    );
    let near_outdoors = level_deviation_db(&open_sky(), breeze, near);
    assert!(near_outdoors < 0.08, "3 m in a breeze: {near_outdoors} dB");
    assert_eq!(
        level_deviation_db(&default_wall_materials(), breeze, far),
        0.0
    );
    assert_eq!(level_deviation_db(&open_sky(), AirMotion::STILL, far), 0.0);
}

/// Indoors and in still air the samples are not touched: bit for bit.
#[test]
fn it_is_not_in_the_signal_where_nothing_stirs_the_air() {
    let (path, atmosphere, ground) = make_ctx(40.0);
    for (walls, air) in [
        (
            default_wall_materials(),
            AirMotion::from_weather(1.0, 1.0, 0.0),
        ),
        (open_sky(), AirMotion::STILL),
    ] {
        let mut environment = test_environment(&walls, &ground, &atmosphere);
        environment.air = air;
        let mut effect = TurbulenceEffect::new(3);
        effect.update(&PathEffectContext {
            path: &path,
            environment: &environment,
            sample_rate: 48_000.0,
            source_pos: Vec3::new(0.0, 40.0, 1.8),
            target_pos: Vec3::new(0.0, 0.0, 1.2),
        });
        for index in 0..4800 {
            let sample = (index as f32 * 0.21).sin();
            assert_eq!(effect.process_sample(sample).to_bits(), sample.to_bits());
        }
    }
}
