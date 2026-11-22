use atrium_core::directivity::DirectivityPattern;
use atrium_core::types::Vec3;

use super::rain::RainSynthParams;
use super::*;
use crate::config::built_source::build_one_synth_source;
use crate::config::scene::NormalizationConfig;
use crate::config::source::{SourceDef, SynthSourceDef};

/// Every synth kind must parse — with bare-integer YAML values, which
/// exercise serde's flatten/Content numeric path (a known footgun).
#[test]
fn synth_source_defs_parse_with_integer_values() {
    let cases = [
            (
                "synth: canopy_wind\nmin_speed: 2\nmax_speed: 8\nfoliage_density: 1\nreference_spl: 48\n",
                "canopy_wind",
            ),
            (
                "synth: field_wind\nmin_speed: 1\nmax_speed: 8\nchange_time_min: 20\nchange_time_max: 50\nreference_spl: 55\n",
                "field_wind",
            ),
            (
                "synth: soft_wind\nmin_speed: 1\nmax_speed: 5\ngust_brightness: 1\nreference_spl: 42\n",
                "soft_wind",
            ),
            (
                "synth: storm_wind\nmin_speed: 8\nmax_speed: 18\nturbulence_depth: 1\nreference_spl: 60\n",
                "storm_wind",
            ),
            ("synth: rain\nintensity: 0.5\nreference_spl: 55\n", "rain"),
            (
                "synth: rain_v2\nintensity: 0.5\nreference_spl: 55\n",
                "rain_v2",
            ),
            (
                "synth: river\nmin_flow_speed: 0.3\nmax_flow_speed: 1.2\nreference_spl: 45\n",
                "river",
            ),
            (
                "synth: waves\nperiod: 6\ncrash_prob: 0.25\nreference_spl: 60\n",
                "waves",
            ),
            ("synth: wisp\nrange: 6\nmax_speed: 7\nreference_spl: 55\n", "wisp"),
            (
                "synth: thunder\nmaster_gain: 1\nseed: 3\nreference_spl: 75\n",
                "thunder",
            ),
            (
                "synth: focus_tone\npitch: 333\nshimmer_rate_max: 1\nreference_spl: 55\n",
                "focus_tone",
            ),
        ];
    for (yaml, expected_kind) in cases {
        match serde_yaml::from_str::<SourceDef>(yaml)
            .unwrap_or_else(|e| panic!("{expected_kind} def should parse: {e}"))
        {
            SourceDef::Synth(synth) => {
                assert_eq!(synth.spec.kind_name(), expected_kind);
                assert_eq!(
                    synth.reference_spl as i32,
                    match expected_kind {
                        "waves" => 60,
                        "canopy_wind" => 48,
                        "soft_wind" => 42,
                        "river" => 45,
                        "storm_wind" => 60,
                        "thunder" => 75,
                        _ => 55,
                    }
                );
            }
            SourceDef::Sample(_) => panic!("{expected_kind} parsed as sample"),
        }
    }
}

#[test]
fn storm_wind_controls_parse_and_build_as_a_field() {
    let source: SynthSourceDef = serde_yaml::from_str(
        "synth: storm_wind\n\
             min_speed: 8\n\
             max_speed: 18\n\
             change_time_min: 12\n\
             change_time_max: 40\n\
             gust_duration_min: 1.5\n\
             gust_duration_max: 14\n\
             gust_strength: 0.55\n\
             turbulence_depth: 0.65\n\
             debris_level: 0.06\n\
             structure_level: 0.05\n\
             pressure_gain: 1.03\n\
             roar_gain: 1.50\n\
             shear_gain: 0.70\n\
             tear_gain: 0.35\n\
             reference_spl: 60\n",
    )
    .unwrap();
    let SynthSpec::StormWind(params) = &source.spec else {
        panic!("expected storm_wind");
    };
    assert_eq!(params.min_speed, Some(8.0));
    assert_eq!(params.max_speed, Some(18.0));
    assert_eq!(params.gust_strength, Some(0.55));
    assert_eq!(params.turbulence_depth, Some(0.65));
    assert_eq!(params.debris_level, Some(0.06));
    assert_eq!(params.structure_level, Some(0.05));
    assert_eq!(params.pressure_gain, Some(1.03));
    assert_eq!(params.roar_gain, Some(1.50));
    assert_eq!(params.shear_gain, Some(0.70));
    assert_eq!(params.tear_gain, Some(0.35));

    let built = build_one_synth_source(
        &source.spec,
        source.reference_spl,
        DirectivityPattern::Omni,
        1.0,
        Vec3::ZERO,
        45,
        &NormalizationConfig::default(),
        1.0,
        40.0,
    )
    .expect("storm wind should build");
    assert_eq!(
        built.source.emitter_kind(),
        atrium_core::source::EmitterKind::Field
    );
}

#[test]
fn canopy_wind_controls_parse_and_build_as_a_field() {
    let source: SynthSourceDef = serde_yaml::from_str(
        "synth: canopy_wind\n\
             min_speed: 2\n\
             max_speed: 8\n\
             change_time_min: 15\n\
             change_time_max: 45\n\
             gust_duration_min: 2\n\
             gust_duration_max: 8\n\
             foliage_density: 0.75\n\
             leaf_dryness: 0.25\n\
             branch_level: 0.12\n\
             reference_spl: 48\n",
    )
    .unwrap();
    let SynthSpec::CanopyWind(params) = &source.spec else {
        panic!("expected canopy_wind");
    };
    assert_eq!(params.min_speed, Some(2.0));
    assert_eq!(params.max_speed, Some(8.0));
    assert_eq!(params.foliage_density, Some(0.75));
    assert_eq!(params.leaf_dryness, Some(0.25));
    assert_eq!(params.branch_level, Some(0.12));

    let built = build_one_synth_source(
        &source.spec,
        source.reference_spl,
        DirectivityPattern::Omni,
        1.0,
        Vec3::ZERO,
        42,
        &NormalizationConfig::default(),
        1.0,
        40.0,
    )
    .expect("canopy wind should build");
    assert_eq!(
        built.source.emitter_kind(),
        atrium_core::source::EmitterKind::Field
    );
}

#[test]
fn field_wind_bounded_driver_controls_parse() {
    let source: SynthSourceDef = serde_yaml::from_str(
        "synth: field_wind\n\
             min_speed: 2\n\
             max_speed: 5\n\
             change_time_min: 20\n\
             change_time_max: 50\n\
             gust_duration_min: 3\n\
             gust_duration_max: 10\n\
             turbulence_time_min: 0.12\n\
             turbulence_time_max: 0.80\n\
             gust_strength: 0.35\n\
             rise_bias: 0.25\n\
             gust_brightness: 0.20\n\
             turbulence_brightness: 0.10\n\
             reference_spl: 55\n",
    )
    .unwrap();
    let SynthSpec::FieldWind(params) = source.spec else {
        panic!("expected field_wind");
    };
    assert_eq!(params.min_speed, Some(2.0));
    assert_eq!(params.max_speed, Some(5.0));
    assert_eq!(params.change_time_min, Some(20.0));
    assert_eq!(params.change_time_max, Some(50.0));
    assert_eq!(params.gust_duration_min, Some(3.0));
    assert_eq!(params.gust_duration_max, Some(10.0));
    assert_eq!(params.turbulence_time_min, Some(0.12));
    assert_eq!(params.turbulence_time_max, Some(0.80));
    assert_eq!(params.gust_strength, Some(0.35));
    assert_eq!(params.rise_bias, Some(0.25));
    assert_eq!(params.gust_brightness, Some(0.20));
    assert_eq!(params.turbulence_brightness, Some(0.10));
}

#[test]
fn soft_wind_controls_parse_and_build_as_a_field() {
    let source: SynthSourceDef = serde_yaml::from_str(
        "synth: soft_wind\n\
             min_speed: 1\n\
             max_speed: 5\n\
             change_time_min: 10\n\
             change_time_max: 24\n\
             gust_duration_min: 0.8\n\
             gust_duration_max: 4\n\
             turbulence_time_min: 0.12\n\
             turbulence_time_max: 0.8\n\
             gust_brightness: 0.18\n\
             turbulence_brightness: 0.10\n\
             reference_spl: 42\n",
    )
    .unwrap();
    let SynthSpec::SoftWind(params) = &source.spec else {
        panic!("expected soft_wind");
    };
    assert_eq!(params.min_speed, Some(1.0));
    assert_eq!(params.max_speed, Some(5.0));
    assert_eq!(params.turbulence_time_min, Some(0.12));
    assert_eq!(params.turbulence_time_max, Some(0.8));
    assert_eq!(params.gust_brightness, Some(0.18));
    assert_eq!(params.turbulence_brightness, Some(0.10));

    let built = build_one_synth_source(
        &source.spec,
        source.reference_spl,
        DirectivityPattern::Omni,
        1.0,
        Vec3::ZERO,
        46,
        &NormalizationConfig::default(),
        1.0,
        40.0,
    )
    .expect("soft wind should build");
    assert_eq!(
        built.source.emitter_kind(),
        atrium_core::source::EmitterKind::Field
    );
}

#[test]
fn river_controls_parse_and_build_as_an_object() {
    let source: SynthSourceDef = serde_yaml::from_str(
        "synth: river\n\
             min_flow_speed: 0.3\n\
             max_flow_speed: 1.2\n\
             change_time_min: 15\n\
             change_time_max: 110\n\
             eddy_time_min: 0.35\n\
             eddy_time_max: 1.25\n\
             eddy_depth: 0.65\n\
             bubble_activity: 0.75\n\
             splash_rate: 0.45\n\
             reference_spl: 45\n",
    )
    .unwrap();
    let SynthSpec::River(params) = &source.spec else {
        panic!("expected river");
    };
    assert_eq!(params.min_flow_speed, Some(0.3));
    assert_eq!(params.max_flow_speed, Some(1.2));
    assert_eq!(params.eddy_time_min, Some(0.35));
    assert_eq!(params.eddy_time_max, Some(1.25));
    assert_eq!(params.eddy_depth, Some(0.65));
    assert_eq!(params.bubble_activity, Some(0.75));
    assert_eq!(params.splash_rate, Some(0.45));

    let built = build_one_synth_source(
        &source.spec,
        source.reference_spl,
        DirectivityPattern::Omni,
        1.0,
        Vec3::ZERO,
        47,
        &NormalizationConfig::default(),
        1.0,
        40.0,
    )
    .expect("river should build");
    assert_eq!(
        built.source.emitter_kind(),
        atrium_core::source::EmitterKind::Object
    );
}

#[test]
fn field_wind_builds_as_a_field_emitter() {
    let source: SynthSourceDef = serde_yaml::from_str(
            "synth: field_wind\nmin_speed: 1\nmax_speed: 8\nchange_time_min: 20\nchange_time_max: 50\nreference_spl: 55\n",
        )
        .unwrap();
    let built = build_one_synth_source(
        &source.spec,
        source.reference_spl,
        DirectivityPattern::Omni,
        1.0,
        Vec3::ZERO,
        42,
        &NormalizationConfig::default(),
        1.0,
        40.0,
    )
    .expect("field wind should build");
    assert_eq!(
        built.source.emitter_kind(),
        atrium_core::source::EmitterKind::Field
    );
}

/// Storm speed bounds must reach the DSP, including its high-speed tearing.
#[test]
fn storm_wind_speed_range_reaches_the_dsp() {
    let calm_spec: SynthSourceDef = serde_yaml::from_str(
        "synth: storm_wind\nmin_speed: 10\nmax_speed: 10\nreference_spl: 60\n",
    )
    .unwrap();
    let fast_spec: SynthSourceDef = serde_yaml::from_str(
        "synth: storm_wind\nmin_speed: 25\nmax_speed: 25\nreference_spl: 60\n",
    )
    .unwrap();

    let bright_fraction = |spec: &SynthSpec| {
        let mut generator = spec.build_generator(Vec3::ZERO, 7);
        for _ in 0..48_000 {
            generator.next_sample(48_000.0); // warm up filters
        }
        let mut hp = crate::synth::noise::OnePoleHP::new(1_800.0, 48_000.0);
        let (mut full, mut high) = (0.0_f64, 0.0_f64);
        for _ in 0..192_000 {
            let s = generator.next_sample(48_000.0);
            full += (s * s) as f64;
            let h = hp.process(s);
            high += (h * h) as f64;
        }
        (high / full.max(1e-12)) as f32
    };

    let calm = bright_fraction(&calm_spec.spec);
    let fast = bright_fraction(&fast_spec.spec);
    assert!(
        fast > calm * 1.5,
        "storm force should have more tearing energy (calm {calm}, fast {fast})"
    );
}

/// A synth def whose parameters produce silence must fail loudly at build
/// time, not play an inaudible source.
#[test]
fn silent_synth_source_is_a_build_error() {
    let spec = SynthSpec::Rain(RainSynthParams {
        intensity: Some(0.0),
        ..Default::default()
    });
    let result = build_one_synth_source(
        &spec,
        55.0,
        DirectivityPattern::Omni,
        0.9,
        Vec3::ZERO,
        1,
        &NormalizationConfig::default(),
        1.0,
        40.0,
    );
    assert!(result.is_err(), "zero-intensity rain should fail the build");
}

/// Thunder builds: its preview books one clap to measure, while the built
/// source is silent until the plan strikes it, and then keeps that strike.
/// It has a rhythm, so the plan can strike it: its distances are its takes.
#[test]
fn thunder_is_silent_until_the_plan_strikes_it() {
    use crate::synth::thunder::{clap_seconds, KIND_CRACK};
    use atrium_core::source::{EmitterKind, Utterance};

    let source: SynthSourceDef =
        serde_yaml::from_str(&std::fs::read_to_string("sources/thunder.yaml").unwrap()).unwrap();
    assert_eq!(source.spec.kind_name(), "thunder");
    let mut built = build_one_synth_source(
        &source.spec,
        source.reference_spl,
        DirectivityPattern::Omni,
        source.spread,
        Vec3::ZERO,
        7,
        &NormalizationConfig::default(),
        1.0,
        40.0,
    )
    .expect("thunder should build");
    assert_eq!(built.source.emitter_kind(), EmitterKind::Object);
    assert_eq!(built.source.spread(), 0.9);
    let rhythm = built.rhythm.as_ref().expect("thunder has a rhythm");
    assert_eq!(rhythm.takes.len(), 4);
    assert_eq!(rhythm.takes[0].seconds, clap_seconds(KIND_CRACK));
    assert_eq!(rhythm.rests.gap_seconds, (8.0, 40.0));

    let sample_rate = 48_000.0;
    for _ in 0..4_800 {
        assert_eq!(
            built.source.next_sample(sample_rate),
            0.0,
            "silent by itself"
        );
    }
    built.source.set_clock(10_000);
    built.source.schedule(Utterance {
        take: KIND_CRACK,
        at_sample: 10_100,
        gain: 1.0,
        rate: 1.0,
        skip_seconds: 0.0,
    });
    for _ in 0..100 {
        built.source.next_sample(sample_rate);
    }
    assert_eq!(built.source.sounding_take(), None, "not yet");
    built.source.next_sample(sample_rate);
    assert_eq!(
        built.source.sounding_take(),
        Some(KIND_CRACK),
        "the plan's strike"
    );
}

/// The focus tone preset is calibrated like every other generator: the built
/// source's output RMS sits at `target_rms · 10^((spl − spl_reference) / 20)`,
/// so its level in the engine's full scale differs from the soft wind's by
/// exactly their reference SPLs (52 against 42 dB). The steady tone after
/// the preview plays within a quarter of a decibel of the calibrated level:
/// the half-second fade-in lowers the preview by 0.05 dB (0.25 s of power
/// lost of 20), and 20 s holds a fractional number of shimmer beats (0.09 dB
/// apart in all, measured).
#[test]
fn focus_tone_is_calibrated_like_the_other_generators() {
    use crate::synth::test_support::root_mean_square;

    let normalization = NormalizationConfig::default();
    let calibrated_db = |path: &str| {
        let source: SynthSourceDef =
            serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut built = build_one_synth_source(
            &source.spec,
            source.reference_spl,
            DirectivityPattern::Omni,
            source.spread,
            Vec3::ZERO,
            11,
            &normalization,
            1.0,
            40.0,
        )
        .unwrap_or_else(|error| panic!("{path} should build: {error}"));
        let samples: Vec<f32> = (0..48_000 * 40)
            .map(|_| built.source.next_sample(48_000.0))
            .collect();
        let preview_db = 20.0 * root_mean_square(&samples[..48_000 * 20]).log10();
        let later_db = 20.0 * root_mean_square(&samples[48_000 * 20..]).log10();
        (source.reference_spl as f64, preview_db, later_db)
    };
    let expected_db = |spl: f64| {
        20.0 * (normalization.target_rms as f64).log10() + spl - normalization.spl_reference as f64
    };

    let (tone_spl, tone_db, tone_later_db) = calibrated_db("sources/focus-tone-333.yaml");
    let (wind_spl, wind_db, _) = calibrated_db("sources/soft-wind.yaml");
    assert_eq!((tone_spl, wind_spl), (52.0, 42.0));
    assert!(
        (tone_db - expected_db(tone_spl)).abs() < 0.01,
        "tone {tone_db:.3} dB"
    );
    assert!(
        (wind_db - expected_db(wind_spl)).abs() < 0.01,
        "wind {wind_db:.3} dB"
    );
    assert!(((tone_db - wind_db) - 10.0).abs() < 0.02);
    assert!(
        (tone_later_db - tone_db).abs() < 0.25,
        "steady tone {tone_later_db:.3} dB against the calibrated {tone_db:.3} dB"
    );
}
