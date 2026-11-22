//! The comparison's acceptance checks (docs/compare-plan.md, "How we know it
//! is right") that need no render or recording: signals whose differences
//! are known by hand. The ones that render the meadow's wind or read a
//! blackbird take are in the engine, `src/offline/tests/comparison.rs`.

use serde_json::Value;

use super::*;
use crate::signal::Settings;
use crate::test_signals::{interleave, Noise, RATE};

fn report(interleaved: &[f32], channels: usize, full_scale_db_spl: Option<f64>) -> Report {
    crate::analyse(
        &Signal {
            interleaved,
            channels,
            sample_rate: RATE,
            channel_names: Vec::new(),
        },
        &Settings { full_scale_db_spl },
    )
}

fn compared<'a>(name: &str, report: &'a Report, timeline: Option<&str>) -> Compared<'a> {
    Compared {
        name: name.to_owned(),
        heading: format!("{name}, a test signal"),
        report,
        halves: None,
        timeline: timeline.map(str::to_owned),
        signal: None,
    }
}

/// The two halves of one channel, analysed apart.
fn halves(samples: &[f32]) -> [Report; 2] {
    let middle = samples.len() / 2;
    [
        report(&samples[..middle], 1, None),
        report(&samples[middle..], 1, None),
    ]
}

/// Two independent ears of white noise, `seconds` long.
fn stereo_noise(seed: u64, seconds: f64) -> Vec<f32> {
    let mut noise = Noise::new(seed);
    let left = noise.white(0.05, seconds);
    let right = noise.white(0.05, seconds);
    interleave(&[&left, &right])
}

/// Every number named `name` anywhere in `value`.
fn every(value: &Value, name: &str, found: &mut Vec<f64>) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields {
                if key == name {
                    if let Some(number) = field.as_f64() {
                        found.push(number);
                    }
                }
                every(field, name, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| every(item, name, found)),
        _ => {}
    }
}

/// The same file against itself: every difference in the JSON is 0, every
/// texture class identical (no finite SNR), and the two line up, the
/// largest difference over time 0.
#[test]
fn a_file_against_itself() {
    let samples = stereo_noise(3, 20.0);
    let found = report(&samples, 2, None);
    let file = compared("noise", &found, Some("scene, seed 1, 0-20 s"));
    let comparison = compare(&file, std::slice::from_ref(&file));
    let other = &comparison.others[0];
    let json = serde_json::to_value(&comparison).expect("serialises");

    let mut differences = Vec::new();
    every(&json, "difference", &mut differences);
    assert!(differences.len() > 100, "{} differences", differences.len());
    assert!(differences.iter().all(|difference| *difference == 0.0));
    assert_eq!(other.colour.rms_difference_decibels, 0.0);
    assert_eq!(other.colour.centroid_hertz.ratio, Some(1.0));
    assert_eq!(other.texture.level_removed_decibels, 0.0);
    for class in &other.texture.classes {
        assert_eq!(class.snr_decibels, None, "{:?}", class.class);
        assert!(class.converged && class.largest.is_empty());
    }
    assert!(other.lines_up);
    assert_eq!(other.over_time.largest_laf.unwrap().values.difference, 0.0);
    assert_eq!(
        other.over_time.largest_width.unwrap().values.difference,
        0.0
    );
    assert!(other
        .space
        .as_ref()
        .is_some_and(|space| !space.width_noticeable));
    assert_eq!(other.level.absolute, None);
}

/// The same noise 10 dB louder, both uncalibrated: shape, colour, texture
/// and space the same (to rounding: the louder file's samples are the
/// quieter's times 10^0.5 in f32), absolute level not compared. The texture
/// is the same once the 10 dB is taken out of M1: every class's SNR far
/// above the paper's 30 dB.
///
/// Both calibrated at 100 dB SPL for RMS 1.0, LAeq differs by 10.0 dB, beyond
/// the 1 dB just-noticeable difference.
#[test]
fn the_same_noise_ten_decibels_louder() {
    let quiet = stereo_noise(4, 20.0);
    let gain = 10f32.powf(0.5);
    let loud: Vec<f32> = quiet.iter().map(|sample| sample * gain).collect();
    let (first, second) = (report(&quiet, 2, None), report(&loud, 2, None));
    let comparison = compare(
        &compared("quiet", &first, None),
        &[compared("loud", &second, None)],
    );
    let other = &comparison.others[0];
    assert_eq!(other.level.absolute, None);
    let level = &other.level;
    for shape in [
        level.l10_minus_l90,
        level.lafmax_over_while_sounding,
        level.crest_decibels,
        level.lc_minus_la,
    ] {
        assert!(shape.difference.abs() < 1e-4, "{shape:?}");
    }
    assert!(other.colour.rms_difference_decibels < 1e-4);
    assert!(other.colour.tilt_decibels_per_octave.difference.abs() < 1e-4);
    assert!((other.colour.centroid_hertz.ratio.unwrap() - 1.0).abs() < 1e-6);
    assert!((other.texture.level_removed_decibels - 10.0).abs() < 1e-4);
    for class in &other.texture.classes {
        assert!(
            class.snr_decibels.is_none_or(|snr| snr > 60.0),
            "{:?}: {:?}",
            class.class,
            class.snr_decibels
        );
    }
    let space = other.space.as_ref().expect("two channels");
    assert!(space.width.unwrap().difference.abs() < 1e-4);
    for band in &space.octave_bands {
        assert!(band.iacc.unwrap().difference.abs() < 1e-4);
        assert!(band.envelope_coherence.unwrap().difference.abs() < 1e-4);
    }
    for percentile in &other.over_time.laf_exceeded {
        assert!(percentile.values.difference.abs() < 1e-4);
    }
    assert!(other.over_time.laf_relative_to_l50);

    let (first, second) = (
        report(&quiet, 2, Some(100.0)),
        report(&loud, 2, Some(100.0)),
    );
    let comparison = compare(
        &compared("quiet", &first, None),
        &[compared("loud", &second, None)],
    );
    let absolute = comparison.others[0]
        .level
        .absolute
        .clone()
        .expect("both calibrated");
    assert!(
        (absolute.laeq.difference - 10.0).abs() < 1e-3,
        "{absolute:?}"
    );
    assert!(absolute.laeq_noticeable && absolute.leq_noticeable);
    assert!(!comparison.others[0].over_time.laf_relative_to_l50);
}

/// White against pink noise. White's third octaves rise 10·log10(2) =
/// 3.01 dB per octave (a band's power is its width), pink's are flat, so
/// the tilt differs by −3.0 dB per octave.
///
/// Both are Gaussian, so the spread and peakedness of every subband
/// envelope are alike: M2 and M4 at or above 20 dB. M1 is far below and
/// beyond the scatter: the envelope means are the spectrum (the paper's own
/// reading of them), and pink against white is a different spectrum. M3
/// lies between (15.5 dB against a scatter floor of 19.3 when measured):
/// most subbands' skewness is near zero (−0.04 at 6 kHz), so a small shift
/// is a large part of Σ s², and skewness does shift with the spectrum within
/// a band: in the lowpass subband pink's power crowds at the bottom (35 Hz:
/// −0.40 against −0.50), and in the wide ones the 200 Hz envelope lowpass
/// smooths a tilted band differently. Expected here: above 10 dB, well
/// clear of M1.
#[test]
fn white_against_pink_noise() {
    let mut noise = Noise::new(9);
    let white = noise.white(0.1, 30.0);
    let pink = noise.pink(0.1, 30.0);
    let (first, second) = (report(&white, 1, None), report(&pink, 1, None));
    let white_halves = halves(&white);
    let comparison = compare(
        &Compared {
            halves: Some([&white_halves[0], &white_halves[1]]),
            ..compared("white", &first, None)
        },
        &[compared("pink", &second, None)],
    );
    let other = &comparison.others[0];
    let tilt = other.colour.tilt_decibels_per_octave;
    assert!((tilt.difference + 3.0).abs() <= 0.1, "{tilt:?}");
    let class = |wanted: TextureClass| {
        other
            .texture
            .classes
            .iter()
            .find(|found| found.class == wanted)
            .expect("every class")
    };
    for wanted in [TextureClass::Variance, TextureClass::Kurtosis] {
        assert!(class(wanted).converged, "{:?}", class(wanted));
    }
    let mean = class(TextureClass::EnvelopeMean);
    assert!(!mean.converged && !mean.within_scatter, "{mean:?}");
    let skewness = class(TextureClass::Skewness);
    assert!(
        skewness.snr_decibels.unwrap() > 10.0
            && skewness.snr_decibels.unwrap() > mean.snr_decibels.unwrap(),
        "{skewness:?}"
    );
}

/// Two takes of one white noise, different seeds, 30 s each: what the
/// scatter floor is for. C and C1 fall far below the paper's 20 dB by
/// chance alone (measured: C 7.8, C1 4.3 dB), and every class lies within
/// the floor the reference's halves give (measured: from 1.2 dB below it,
/// M2, to 3.4 dB above, M4).
#[test]
fn two_takes_of_one_noise_differ_only_by_scatter() {
    let first_samples = Noise::new(21).white(0.1, 30.0);
    let second_samples = Noise::new(22).white(0.1, 30.0);
    let (first, second) = (
        report(&first_samples, 1, None),
        report(&second_samples, 1, None),
    );
    let first_halves = halves(&first_samples);
    let comparison = compare(
        &Compared {
            halves: Some([&first_halves[0], &first_halves[1]]),
            ..compared("first", &first, None)
        },
        &[compared("second", &second, None)],
    );
    let classes = &comparison.others[0].texture.classes;
    let correlation = &classes[4];
    assert_eq!(correlation.class, TextureClass::Correlation);
    assert!(!correlation.converged, "{correlation:?}");
    for class in classes {
        assert!(class.within_scatter, "{class:?}");
    }
}

/// Noise against the same noise swung fully at 4 Hz: the modulation power
/// differs, most at the filter centred nearest 4 Hz, 0.5·400^(7/19) =
/// 4.55 Hz (its neighbour 3.32 Hz is further on a log scale).
#[test]
fn noise_against_noise_modulated_at_four_hertz() {
    let steady = Noise::new(5).white(0.1, 20.0);
    let swinging: Vec<f32> = steady
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let time = index as f64 / RATE as f64;
            (*sample as f64 * (1.0 + (std::f64::consts::TAU * 4.0 * time).sin())) as f32
        })
        .collect();
    let (first, second) = (report(&steady, 1, None), report(&swinging, 1, None));
    let comparison = compare(
        &compared("steady", &first, None),
        &[compared("swinging", &second, None)],
    );
    let power = comparison.others[0]
        .texture
        .classes
        .iter()
        .find(|class| class.class == TextureClass::ModulationPower)
        .expect("a class");
    assert!(power.snr_decibels.unwrap() < 20.0, "{power:?}");
    assert!(!power.converged);
    let expected = 0.5 * 400f64.powf(7.0 / 19.0);
    assert!((expected - 4.55).abs() < 0.01);
    assert!(
        (power.largest[0].hertz - expected).abs() < 1e-9,
        "{power:?}"
    );
}

/// A report read back from its JSON compares as the report itself: the
/// JSON carries every number the comparison reads, to the last bit.
#[test]
fn a_report_against_its_json() {
    let first_samples = stereo_noise(6, 10.0);
    let second_samples = stereo_noise(7, 10.0);
    let first = report(&first_samples, 2, None);
    let second = report(&second_samples, 2, Some(94.0));
    let text = serde_json::to_string(&second).expect("serialises");
    let read: Report = serde_json::from_str(&text).expect("reads");
    let reference = compared("first", &first, None);
    assert_eq!(
        compare(&reference, &[compared("second", &second, None)]),
        compare(&reference, &[compared("second", &read, None)])
    );
}

/// Files line up only when both name one timeline and are as long.
#[test]
fn what_lines_up() {
    let samples = stereo_noise(8, 2.0);
    let found = report(&samples, 2, None);
    let shorter = report(&samples[..samples.len() / 2], 2, None);
    let render = |report| compared("render", report, Some("meadow, seed 1, 0-2 s"));
    assert!(lines_up(&render(&found), &render(&found)));
    assert!(!lines_up(&render(&found), &render(&shorter)));
    assert!(!lines_up(
        &compared("recording", &found, None),
        &compared("recording", &found, None)
    ));
    assert!(!lines_up(
        &render(&found),
        &compared("render", &found, Some("meadow, seed 2, 0-2 s"))
    ));
}

/// The summary names every file and its colour, says what was not
/// compared and why, and gives each texture class a row.
#[test]
fn the_summary_reads_alone() {
    let first_samples = stereo_noise(10, 6.0);
    let second_samples = Noise::new(11).white(0.05, 6.0);
    let first = report(&first_samples, 2, None);
    let second = report(&second_samples, 1, None);
    let comparison = compare(
        &compared("stereo", &first, None),
        &[compared("mono", &second, None)],
    );
    let text = comparison_summary(&comparison);
    assert!(
        text.contains("ref (black)") && text.contains("1 (blue)"),
        "{text}"
    );
    assert!(text.contains("absolute level not compared for stereo, mono"));
    assert!(text.contains("space       not compared"));
    for label in [
        "M1 envelope mean",
        "M4 kurtosis",
        "C1 modulation",
        "LAF distribution",
    ] {
        assert!(text.contains(label), "{label}:\n{text}");
    }
}
