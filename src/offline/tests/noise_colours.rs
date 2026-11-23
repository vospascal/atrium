//! The noise colours as a listener hears them, through the whole engine:
//! `scenes/noise-colours-test.yaml` with each colour's preset in turn,
//! heard all around (a field), on headphones and on the scene's 5.1
//! speakers. Each is set to 50 dB(A) (`config::synth` tests); measured here
//! is what reaches the ears and the listening spot, and how alike the two
//! ears' noise is.

use realfft::RealFftPlanner;

use super::on_headphones;
use crate::config::SceneConfig;
use crate::offline::{Output, RenderSettings, Rendered, Rendering};

const SCENE: &str = "scenes/noise-colours-test.yaml";
const PRESET: &str = "sources/noise/pink.yaml";
const COLOURS: [&str; 9] = [
    "white", "pink", "red", "brown", "blue", "violet", "grey", "green", "orange",
];
/// Measured from here, past the half-second fade-in and the render's
/// settling.
const FROM_SECONDS: f64 = 2.0;
const MEASURED_SECONDS: f64 = 10.0;

/// The scene with `colour`'s preset in place of its pink noise, rendered
/// with `settings`.
fn rendered(colour: &str, settings: &RenderSettings) -> Rendered {
    let text = std::fs::read_to_string(SCENE).unwrap_or_else(|error| panic!("{SCENE}: {error}"));
    assert!(text.contains(PRESET), "the scene plays {PRESET}");
    let text = text.replace(PRESET, &format!("sources/noise/{colour}.yaml"));
    let config: SceneConfig = serde_yaml::from_str(&text).expect("the scene parses");
    Rendering::prepare(config, settings)
        .unwrap_or_else(|error| panic!("{colour}: {error}"))
        .run()
}

/// The analyser's levels of the measured stretch of `rendered`: on two
/// channels the mean of the ears' powers, on six the sum of the speakers'.
fn levels(rendered: &Rendered) -> atrium_analysis::Levels {
    let channels = rendered.channels();
    let first = (FROM_SECONDS * rendered.sample_rate() as f64) as usize * channels;
    let report = atrium_analysis::analyse(
        &atrium_analysis::Signal {
            interleaved: &rendered.frames()[first..],
            channels,
            sample_rate: rendered.sample_rate(),
            channel_names: rendered
                .channel_names()
                .into_iter()
                .map(str::to_string)
                .collect(),
        },
        &atrium_analysis::Settings {
            full_scale_db_spl: Some(rendered.full_scale_db_spl()),
        },
    );
    report.level.overall
}

/// One channel of the measured stretch of an interleaved stereo render.
fn ear(rendered: &Rendered, index: usize) -> Vec<f64> {
    let first = (FROM_SECONDS * rendered.sample_rate() as f64) as usize;
    rendered
        .frames()
        .iter()
        .skip(index)
        .step_by(2)
        .skip(first)
        .map(|&sample| sample as f64)
        .collect()
}

/// The two ears' zero-lag correlation over everything, and in each octave
/// band 63 Hz to 8 kHz from a Welch cross-spectrum (Hann, 2^14 samples,
/// half overlapping): the real part of the cross-spectrum over the
/// geometric mean of the two powers, summed over the band.
fn correlations(left: &[f64], right: &[f64], sample_rate: f64) -> (f64, Vec<(f64, f64)>) {
    let dot = |first: &[f64], second: &[f64]| -> f64 {
        first.iter().zip(second).map(|(a, b)| a * b).sum()
    };
    let broadband = dot(left, right) / (dot(left, left) * dot(right, right)).sqrt();

    let length = 1 << 14;
    let mut planner = RealFftPlanner::<f64>::new();
    let transform = planner.plan_fft_forward(length);
    let window: Vec<f64> = (0..length)
        .map(|index| 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / length as f64).cos())
        .collect();
    let bins = length / 2 + 1;
    let (mut left_power, mut right_power, mut cross) =
        (vec![0.0; bins], vec![0.0; bins], vec![0.0; bins]);
    let mut start = 0;
    while start + length <= left.len() {
        let spectrum = |samples: &[f64]| {
            let mut input: Vec<f64> = (0..length)
                .map(|index| samples[start + index] * window[index])
                .collect();
            let mut output = transform.make_output_vec();
            transform.process(&mut input, &mut output).unwrap();
            output
        };
        let (left_spectrum, right_spectrum) = (spectrum(left), spectrum(right));
        for bin in 0..bins {
            left_power[bin] += left_spectrum[bin].norm_sqr();
            right_power[bin] += right_spectrum[bin].norm_sqr();
            cross[bin] += (left_spectrum[bin] * right_spectrum[bin].conj()).re;
        }
        start += length / 2;
    }
    let bin_hz = sample_rate / length as f64;
    let bands = (-4..=3)
        .map(|octave| {
            let centre = 1_000.0 * 2.0_f64.powi(octave);
            let in_band = |bin: &usize| {
                let frequency = *bin as f64 * bin_hz;
                frequency >= centre / std::f64::consts::SQRT_2
                    && frequency < centre * std::f64::consts::SQRT_2
            };
            let sum = |values: &[f64]| {
                (0..bins)
                    .filter(in_band)
                    .map(|bin| values[bin])
                    .sum::<f64>()
            };
            (
                centre,
                sum(&cross) / (sum(&left_power) * sum(&right_power)).sqrt(),
            )
        })
        .collect();
    (broadband, bands)
}

/// Every colour reaches the listener at about the same A-weighted level: on
/// headphones each ear hears it at 50 dB(A) within 1.5 dB, and the colours
/// are within 1.5 dB of one another; on the 5.1 speakers the listening
/// spot gets 50 dB(A) within 2 dB. The two ears' noise is not one signal:
/// from 1 kHz up they are uncorrelated (within ±0.2 in every octave band),
/// but a field all round reaches them alike where its wavelength is long
/// against the head, as it does a real head (the measured KEMAR head's
/// diffuse-field coherence, `pipeline::tests::ambient_headphones`): above
/// 0.8 at 63 Hz. So the broadband correlation follows the colour: near 0 for
/// white, high for the dark colours, whose power is mostly below 250 Hz.
/// Measured 2026-11-23; returns every number.
#[test]
fn every_noise_colour_reaches_the_listener_at_fifty_decibels_a() {
    let headphones = on_headphones(FROM_SECONDS + MEASURED_SECONDS);
    let speakers = RenderSettings {
        seconds: FROM_SECONDS + MEASURED_SECONDS,
        output: Output::Scene,
        ..RenderSettings::default()
    };
    let mut on_headphones_a = Vec::new();
    for colour in COLOURS {
        let ears = rendered(colour, &headphones);
        assert_eq!(ears.channels(), 2);
        let heard = levels(&ears);
        let (broadband, bands) =
            correlations(&ear(&ears, 0), &ear(&ears, 1), ears.sample_rate() as f64);
        let room = rendered(colour, &speakers);
        assert_eq!(room.channels(), 6);
        let spot = levels(&room);
        println!(
            "{colour}: headphones {:.1} dB(A) ({:.1} dB unweighted), ears correlated {broadband:+.2} \
             ({}); 5.1 speakers {:.1} dB(A) ({:.1} dB unweighted)",
            heard.laeq,
            heard.leq,
            bands
                .iter()
                .map(|(centre, correlation)| format!("{centre:.0} Hz {correlation:+.2}"))
                .collect::<Vec<_>>()
                .join(", "),
            spot.laeq,
            spot.leq,
        );
        assert!(
            (heard.laeq - 50.0).abs() < 1.5,
            "{colour}: {:.1} dB(A)",
            heard.laeq
        );
        assert!(
            (spot.laeq - 50.0).abs() < 2.0,
            "{colour}: {:.1} dB(A)",
            spot.laeq
        );
        for &(centre, correlation) in &bands {
            if centre >= 1_000.0 {
                assert!(
                    correlation.abs() < 0.2,
                    "{colour}: {correlation:+.2} at {centre} Hz"
                );
            }
        }
        assert!(bands[0].1 > 0.8, "{colour}: {:+.2} at 63 Hz", bands[0].1);
        on_headphones_a.push(heard.laeq);
    }
    let loudest = on_headphones_a
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let softest = on_headphones_a
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    println!(
        "on headphones the colours span {:.2} dB(A)",
        loudest - softest
    );
    assert!(loudest - softest < 1.5);
}
