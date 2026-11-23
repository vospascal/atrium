//! Each colour's spectrum, measured: its slope against its definition, the
//! same at 44.1, 48 and 96 kHz; brown, blue, grey, green and orange against
//! the example recordings they follow; red's and brown's floor; and what
//! every colour shares (determinism, the gain's glide).

use atrium_core::commands::SynthParam;
use atrium_core::layers::LayerMix;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use realfft::RealFftPlanner;

use super::brown::LOW_PASS_HZ;
use super::dispatch::Generator;
use super::*;
use crate::synth::test_support::{largest_step, mean, peak, root_mean_square};

const SAMPLE_RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];
const SEED: u64 = 42;

const EVERY_COLOUR: [NoiseColour; 9] = [
    NoiseColour::White,
    NoiseColour::Pink,
    NoiseColour::Brown,
    NoiseColour::Red,
    NoiseColour::Blue,
    NoiseColour::Violet,
    NoiseColour::Grey,
    NoiseColour::Green,
    NoiseColour::Orange,
];

fn source(colour: NoiseColour, seed: u64) -> NoiseColourSource {
    NoiseColourSource::new(
        Vec3::ZERO,
        NoiseColourSettings {
            colour,
            master_gain: 1.0,
        },
        seed,
    )
}

/// `seconds` of `colour` at `sample_rate`, after its half-second fade-in.
fn render(colour: NoiseColour, sample_rate: f32, seconds: f32) -> Vec<f32> {
    let mut noise = source(colour, SEED);
    for _ in 0..sample_rate as usize {
        noise.next_sample(sample_rate);
    }
    (0..(seconds * sample_rate) as usize)
        .map(|_| noise.next_sample(sample_rate))
        .collect()
}

/// A Welch estimate of the one-sided power spectral density per hertz:
/// Hann windows of 2^16 or 2^17 samples (0.7 Hz bins), half overlapping. Bins
/// that fine keep a band's mean true on a steep slope, whichever bins its
/// edges happen to take in.
struct Density {
    bin_hz: f64,
    per_hertz: Vec<f64>,
}

impl Density {
    fn of(samples: &[f32], sample_rate: f32) -> Self {
        let length = (sample_rate as usize).next_power_of_two();
        let mut planner = RealFftPlanner::<f64>::new();
        let transform = planner.plan_fft_forward(length);
        let window: Vec<f64> = (0..length)
            .map(|index| 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / length as f64).cos())
            .collect();
        let window_power: f64 = window.iter().map(|weight| weight * weight).sum();
        let mut input = vec![0.0; length];
        let mut output = transform.make_output_vec();
        let mut sum = vec![0.0; length / 2 + 1];
        let mut frames = 0;
        let mut start = 0;
        while start + length <= samples.len() {
            for (index, value) in input.iter_mut().enumerate() {
                *value = samples[start + index] as f64 * window[index];
            }
            transform.process(&mut input, &mut output).unwrap();
            for (total, value) in sum.iter_mut().zip(&output) {
                *total += value.norm_sqr();
            }
            frames += 1;
            start += length / 2;
        }
        // One-sided: twice the two-sided density, spread over the rate.
        let scale = 2.0 / (frames as f64 * window_power * sample_rate as f64);
        Self {
            bin_hz: sample_rate as f64 / length as f64,
            per_hertz: sum.into_iter().map(|total| total * scale).collect(),
        }
    }

    /// Mean density per hertz (dB) of the bins whose centres lie in
    /// `[low_hz, high_hz)`.
    fn band_db(&self, low_hz: f64, high_hz: f64) -> f64 {
        let (mut total, mut bins) = (0.0, 0);
        for (bin, &density) in self.per_hertz.iter().enumerate() {
            let centre = bin as f64 * self.bin_hz;
            if centre >= low_hz && centre < high_hz {
                total += density;
                bins += 1;
            }
        }
        assert!(bins > 0, "no bin in {low_hz}–{high_hz} Hz");
        10.0 * (total / bins as f64).log10()
    }

    /// Density (dB) in the band `fraction` of an octave wide centred on
    /// `centre_hz`.
    fn around_db(&self, centre_hz: f64, fraction: f64) -> f64 {
        let half = 2.0_f64.powf(fraction / 2.0);
        self.band_db(centre_hz / half, centre_hz * half)
    }
}

/// The octave bands 63 Hz to 8 kHz, by their exact centres.
fn octave_centres() -> impl Iterator<Item = f64> {
    (-4..=3).map(|octave| 1_000.0 * 2.0_f64.powi(octave))
}

/// Least-squares slope (dB per octave) of `(log2 frequency, dB)` points.
fn slope(points: &[(f64, f64)]) -> f64 {
    let count = points.len() as f64;
    let mean_x = points.iter().map(|(x, _)| x).sum::<f64>() / count;
    let mean_y = points.iter().map(|(_, y)| y).sum::<f64>() / count;
    let covariance: f64 = points
        .iter()
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum();
    let variance: f64 = points.iter().map(|(x, _)| (x - mean_x).powi(2)).sum();
    covariance / variance
}

/// The slope of the density per hertz over the octave bands 63 Hz to 8 kHz.
fn octave_slope(density: &Density) -> f64 {
    let points: Vec<(f64, f64)> = octave_centres()
        .map(|centre| (centre.log2(), density.around_db(centre, 1.0)))
        .collect();
    slope(&points)
}

/// Each colour's density per hertz falls or rises by its definition's
/// slope, fitted over the octave bands 63 Hz to 8 kHz, within 0.5 dB per
/// octave at 44.1, 48 and 96 kHz: white 0, pink −3, red −6, violet +6.
/// (Brown, blue and grey follow example recordings instead; see their own
/// tests.) Returns the slopes measured.
#[test]
fn every_colour_has_its_slope_at_every_rate() {
    let targets = [
        (NoiseColour::White, 0.0),
        (NoiseColour::Pink, -3.0),
        (NoiseColour::Red, -6.0),
        (NoiseColour::Violet, 6.0),
    ];
    for (colour, target) in targets {
        let measured: Vec<f64> = SAMPLE_RATES
            .iter()
            .map(|&sample_rate| {
                octave_slope(&Density::of(
                    &render(colour, sample_rate, 20.0),
                    sample_rate,
                ))
            })
            .collect();
        println!(
            "{colour:?}: {:+.2} / {:+.2} / {:+.2} dB per octave at 44.1 / 48 / 96 kHz (target {target:+.0})",
            measured[0], measured[1], measured[2]
        );
        for (slope, sample_rate) in measured.iter().zip(SAMPLE_RATES) {
            assert!(
                (slope - target).abs() < 0.5,
                "{colour:?} at {sample_rate} Hz: {slope:+.2} dB per octave, not {target:+.0}"
            );
        }
    }
}

/// Every colour has the same density per hertz in every octave band from
/// 63 Hz to 8 kHz at 44.1 and 96 kHz as at 48 kHz, within 0.5 dB: corners in
/// hertz, white noise scaled to its density per hertz. Returns the largest
/// difference.
#[test]
fn every_colour_is_the_same_spectrum_at_every_rate() {
    for colour in EVERY_COLOUR {
        let densities: Vec<Density> = SAMPLE_RATES
            .iter()
            .map(|&sample_rate| Density::of(&render(colour, sample_rate, 20.0), sample_rate))
            .collect();
        let mut largest: f64 = 0.0;
        for centre in octave_centres() {
            let reference = densities[1].around_db(centre, 1.0);
            for other in [&densities[0], &densities[2]] {
                let difference = other.around_db(centre, 1.0) - reference;
                largest = largest.max(difference.abs());
            }
        }
        println!("{colour:?}: octave bands at most {largest:.2} dB from 48 kHz");
        assert!(largest < 0.5, "{colour:?}: {largest:.2} dB apart");
    }
}

/// Every colour leaves its generator at an RMS of about 0.1 at 48 kHz.
#[test]
fn every_colour_leaves_its_generator_at_about_a_tenth() {
    for colour in EVERY_COLOUR {
        let level = root_mean_square(&render(colour, 48_000.0, 20.0));
        println!("{colour:?}: RMS {level:.4}");
        assert!((level - 0.1).abs() < 0.01, "{colour:?}: RMS {level:.4}");
    }
}

/// The third-octave centres (base 10, as nominal) from `lowest` to
/// `highest` band numbers, 1 kHz being 0.
fn third_octave_centres(lowest: i32, highest: i32) -> impl Iterator<Item = f64> {
    (lowest..=highest).map(|band| 1_000.0 * 10.0_f64.powf(band as f64 / 10.0))
}

/// How far the measured third-octave band densities are from `model`'s
/// power gain, at each centre, once their mean difference (the overall
/// level) is taken out.
fn errors_from(density: &Density, model: fn(f64) -> f64, centres: &[f64]) -> Vec<(f64, f64)> {
    let differences: Vec<(f64, f64)> = centres
        .iter()
        .map(|&centre| {
            (
                centre,
                density.around_db(centre, 1.0 / 3.0) - 10.0 * model(centre).log10(),
            )
        })
        .collect();
    let offset = differences
        .iter()
        .map(|(_, difference)| difference)
        .sum::<f64>()
        / differences.len() as f64;
    differences
        .into_iter()
        .map(|(centre, difference)| (centre, difference - offset))
        .collect()
}

fn worst(errors: &[(f64, f64)]) -> f64 {
    errors
        .iter()
        .map(|(_, error)| error.abs())
        .fold(0.0, f64::max)
}

/// Each measured third-octave band density against 1 kHz's, beside an
/// example recording's at the same centres: the differences.
fn against_example(density: &Density, example: &[(f64, f64)]) -> Vec<(f64, f64, f64)> {
    let reference = density.around_db(1_000.0, 1.0 / 3.0);
    example
        .iter()
        .map(|&(centre, recorded)| {
            let made = density.around_db(centre, 1.0 / 3.0) - reference;
            (centre, made, made - recorded)
        })
        .collect()
}

fn shown(rows: &[(f64, f64, f64)]) -> String {
    rows.iter()
        .map(|(centre, made, difference)| format!("{centre:.0} Hz {made:+.1} ({difference:+.1})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Green follows its fit to the example recording, 1/f^1.45 under a
/// third-order low-pass at 1640 Hz: third-octave band levels within 0.5 dB
/// of it from 63 Hz to 6.3 kHz at every rate, the overall level aside.
/// Returns the errors, and the bands beside the recording's.
#[test]
fn green_follows_the_example_recordings_fit() {
    let example = [
        (63.0, 16.3),
        (125.0, 13.2),
        (250.0, 9.9),
        (500.0, 7.1),
        (2_000.0, -13.4),
        (4_000.0, -30.7),
        (8_000.0, -52.6),
    ];
    let centres: Vec<f64> = third_octave_centres(-12, 8).collect();
    for sample_rate in SAMPLE_RATES {
        let density = Density::of(&render(NoiseColour::Green, sample_rate, 60.0), sample_rate);
        let errors = errors_from(&density, super::green::power_gain, &centres);
        println!(
            "green at {sample_rate} Hz: worst band {:.2} dB from 1/f^{} under {} Hz; against 1 kHz \
             (and against the recording): {}",
            worst(&errors),
            super::green::SLOPE_EXPONENT,
            super::green::LOW_PASS_HZ,
            shown(&against_example(&density, &example))
        );
        assert!(worst(&errors) < 0.5, "{sample_rate} Hz: {errors:?}");
    }
}

/// The third-octave centres the example recordings were measured at.
const EXAMPLE_CENTRES: [f64; 28] = [
    31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0, 630.0,
    800.0, 1_000.0, 1_250.0, 1_600.0, 2_000.0, 2_500.0, 3_150.0, 4_000.0, 5_000.0, 6_300.0,
    8_000.0, 10_000.0, 12_500.0, 16_000.0,
];

/// The example grey noise recording's density per hertz against 1 kHz, in
/// the third octaves of [`EXAMPLE_CENTRES`].
const GREY_EXAMPLE_DB: [f64; 28] = [
    25.4, 24.5, 24.0, 22.7, 21.7, 20.2, 18.5, 17.0, 14.8, 13.1, 11.4, 9.4, 7.0, 4.6, 2.4, 0.0,
    -2.4, -4.9, -5.2, -5.5, -5.2, -4.4, -3.7, -3.1, -2.6, -2.4, -2.3, -2.2,
];

/// The example blue noise recording's, likewise.
const BLUE_EXAMPLE_DB: [f64; 28] = [
    -15.8, -13.2, -11.9, -9.6, -8.5, -6.9, -6.1, -5.2, -4.9, -4.3, -3.7, -3.2, -2.6, -1.9, -0.8,
    0.0, 1.1, 2.8, 4.5, 5.6, 6.3, 7.0, 7.4, 7.8, 8.0, 8.0, 8.0, 8.1,
];

/// `colour`'s third-octave bands against 1 kHz beside the example's, at
/// every rate, within `tolerance_db` from `lowest_hz` to 12.5 kHz. Returns
/// the worst band at each rate and prints every band.
fn follows_example(colour: NoiseColour, example: &[f64; 28], lowest_hz: f64, tolerance_db: f64) {
    let rows: Vec<(f64, f64)> = EXAMPLE_CENTRES
        .iter()
        .copied()
        .zip(example.iter().copied())
        .collect();
    for sample_rate in SAMPLE_RATES {
        let density = Density::of(&render(colour, sample_rate, 60.0), sample_rate);
        let bands = against_example(&density, &rows);
        let tested: Vec<&(f64, f64, f64)> = bands
            .iter()
            .filter(|(centre, ..)| *centre >= lowest_hz && *centre <= 12_500.0)
            .collect();
        let worst = tested
            .iter()
            .map(|(_, _, difference)| difference.abs())
            .fold(0.0, f64::max);
        let error = (tested
            .iter()
            .map(|(_, _, difference)| difference * difference)
            .sum::<f64>()
            / tested.len() as f64)
            .sqrt();
        println!(
            "{colour:?} at {sample_rate} Hz: {error:.2} dB RMS, worst {worst:.2} dB from the example; \
             against 1 kHz (and against the example): {}",
            shown(&bands)
        );
        assert!(
            worst < tolerance_db,
            "{colour:?} at {sample_rate} Hz: {bands:?}"
        );
    }
}

/// Grey follows the example grey noise recording: third-octave band levels
/// against 1 kHz within 1.5 dB of it from 31.5 Hz to 12.5 kHz at every
/// rate.
#[test]
fn grey_follows_the_example_recording() {
    follows_example(NoiseColour::Grey, &GREY_EXAMPLE_DB, 31.5, 1.5);
}

/// Blue follows the example blue noise recording: third-octave band levels
/// against 1 kHz within 1.5 dB of it from 50 Hz (its lowest bands are thin)
/// to 12.5 kHz at every rate.
#[test]
fn blue_follows_the_example_recording() {
    follows_example(NoiseColour::Blue, &BLUE_EXAMPLE_DB, 50.0, 1.5);
}

/// Red and brown carry nothing at 0 Hz: over a minute their mean is more
/// than 50 dB under their RMS, and they never wander beyond six times
/// their RMS.
#[test]
fn red_and_brown_have_no_offset_and_stay_bounded() {
    for colour in [NoiseColour::Red, NoiseColour::Brown] {
        for sample_rate in SAMPLE_RATES {
            let samples = render(colour, sample_rate, 60.0);
            let level = root_mean_square(&samples);
            let offset = mean(&samples);
            let largest = peak(&samples) as f64;
            println!(
                "{colour:?} at {sample_rate} Hz: mean {offset:.2e}, RMS {level:.4}, peak {largest:.4} ({:.1} × RMS)",
                largest / level
            );
            assert!(offset.abs() < 3e-3 * level, "{colour:?}: mean {offset}");
            assert!(largest < 6.0 * level, "{colour:?}: peak {largest}");
        }
    }
}

/// Brown's third-octave band levels are its model's, 1/f² under a
/// second-order low-pass at 285 Hz (the best fit to the example brown noise
/// recording, 2.1 dB RMS error), within 0.5 dB from 63 Hz to 4 kHz at every
/// rate, its overall level aside; and over the octave bands 63 Hz to 8 kHz
/// it falls about as steeply as the recording (−15.5 dB per octave).
/// Returns the errors and the slope.
#[test]
fn brown_follows_the_example_recordings_fit() {
    for sample_rate in SAMPLE_RATES {
        let density = Density::of(&render(NoiseColour::Brown, sample_rate, 60.0), sample_rate);
        let differences: Vec<(f64, f64)> = (-12..=6)
            .map(|band| {
                let centre = 1_000.0 * 10.0_f64.powf(band as f64 / 10.0);
                let target_db = 10.0 * super::brown::power_gain(centre).log10();
                (centre, density.around_db(centre, 1.0 / 3.0) - target_db)
            })
            .collect();
        let offset = differences
            .iter()
            .map(|(_, difference)| difference)
            .sum::<f64>()
            / differences.len() as f64;
        let worst = differences
            .iter()
            .map(|(_, difference)| (difference - offset).abs())
            .fold(0.0, f64::max);
        let overall = octave_slope(&density);
        let at = |centre: f64| {
            density.around_db(centre, 1.0 / 3.0) - density.around_db(1_000.0, 1.0 / 3.0)
        };
        println!(
            "brown at {sample_rate} Hz: worst band {worst:.2} dB from 1/f² under {LOW_PASS_HZ} Hz; \
             {overall:+.1} dB per octave over 63 Hz–8 kHz; against 1 kHz {:+.1} dB at 63 Hz, {:+.1} dB at 5 kHz",
            at(63.0),
            at(5_000.0)
        );
        assert!(worst < 0.5, "{sample_rate} Hz: {differences:?}");
        assert!((overall + 15.5).abs() < 2.0, "{overall:+.2} dB per octave");
    }
}

/// Red is Brownian noise and brown is darker: they are not the same sound.
#[test]
fn red_and_brown_differ() {
    let red = Density::of(&render(NoiseColour::Red, 48_000.0, 10.0), 48_000.0);
    let brown = Density::of(&render(NoiseColour::Brown, 48_000.0, 10.0), 48_000.0);
    let tilt = |density: &Density| density.around_db(4_000.0, 1.0) - density.around_db(125.0, 1.0);
    assert!(tilt(&brown) < tilt(&red) - 20.0);
}

/// The same seed gives the same samples, to the bit; another seed others.
#[test]
fn every_colour_is_deterministic_from_its_seed() {
    for colour in EVERY_COLOUR {
        let run = |seed| {
            let mut noise = source(colour, seed);
            (0..96_000)
                .map(|_| noise.next_sample(48_000.0).to_bits())
                .collect::<Vec<u32>>()
        };
        assert_eq!(run(7), run(7), "{colour:?}");
        assert_ne!(run(7), run(8), "{colour:?}");
    }
}

/// A new master gain glides there over 50 ms: switched while it plays, from
/// half to full, it makes no step larger than the noise's own largest step
/// at full gain over the same stretch, and half a second later it is at the
/// new gain. Not a number, or a setting it has not got, changes nothing.
#[test]
fn the_master_gain_glides_without_a_click() {
    let sample_rate = 48_000.0;
    let half = (0.5 * sample_rate) as usize;
    let after = (0.05 * sample_rate) as usize;
    for colour in EVERY_COLOUR {
        let full = render(colour, sample_rate, 2.0);
        let mut noise = NoiseColourSource::new(
            Vec3::ZERO,
            NoiseColourSettings {
                colour,
                master_gain: 0.5,
            },
            SEED,
        );
        for _ in 0..sample_rate as usize {
            noise.next_sample(sample_rate);
        }
        let mut switched: Vec<f32> = (0..half).map(|_| noise.next_sample(sample_rate)).collect();
        noise.set_synth_param(SynthParam::MasterGain, 1.0);
        noise.set_synth_param(SynthParam::MasterGain, f32::NAN);
        noise.set_synth_param(SynthParam::Intensity, 0.0);
        switched.extend((0..3 * half).map(|_| noise.next_sample(sample_rate)));
        let own = largest_step(&full[half - after..half + after]);
        let at_switch = largest_step(&switched[half - 1..half + after]);
        assert!(at_switch <= own, "{colour:?}: {at_switch} > {own}");
        // Half a second on, 10 time constants: at full gain, to the bit.
        for (late, whole) in switched[2 * half..].iter().zip(&full[2 * half..]) {
            assert_eq!(late.to_bits(), whole.to_bits(), "{colour:?}");
        }
    }
}

/// The colours that are one layer say so; orange is two, deep and bright.
#[test]
fn only_orange_has_two_layers() {
    for colour in EVERY_COLOUR {
        let layers = source(colour, SEED).layer_count();
        let expected = if colour == NoiseColour::Orange { 2 } else { 1 };
        assert_eq!(layers, expected, "{colour:?}");
    }
}

/// Orange's two layers fitted to the example recording: the deep layer
/// carries about 22 dB more power than the bright one, their densities
/// meet near 520 Hz, and their sum's band levels against 1 kHz are within
/// 1.5 dB of the recording's from 63 Hz to 8 kHz (the task asked 3.5). At
/// 0 dB on both faders the sound is their sum, to the bit. Returns the
/// balance and the bands.
#[test]
fn orange_is_a_dark_brown_with_red_above_it() {
    let sample_rate = 48_000.0;
    let mut generator = Generator::new(NoiseColour::Orange, SEED);
    let (mut deep, mut bright) = (Vec::new(), Vec::new());
    for _ in 0..(60.0 * sample_rate) as usize {
        let [deep_sample, bright_sample] = generator.next_layers(sample_rate);
        deep.push(deep_sample);
        bright.push(bright_sample);
    }
    let (deep_density, bright_density) = (
        Density::of(&deep, sample_rate),
        Density::of(&bright, sample_rate),
    );
    let power = 20.0 * (root_mean_square(&deep) / root_mean_square(&bright)).log10();
    let meet = {
        let step = 2.0_f64.powf(1.0 / 24.0);
        let mut frequency = 125.0;
        while bright_density.around_db(frequency, 1.0 / 6.0)
            < deep_density.around_db(frequency, 1.0 / 6.0)
        {
            frequency *= step;
        }
        frequency
    };
    let sum: Vec<f32> = deep
        .iter()
        .zip(&bright)
        .map(|(deep, bright)| deep + bright)
        .collect();
    let example = [
        (31.5, 44.8),
        (50.0, 48.4),
        (63.0, 47.5),
        (250.0, 24.7),
        (500.0, 10.6),
        (2_000.0, -7.1),
        (4_000.0, -16.0),
        (8_000.0, -26.2),
    ];
    let bands = against_example(&Density::of(&sum, sample_rate), &example);
    println!(
        "orange: deep {power:.1} dB more power than bright; their densities meet at {meet:.0} Hz; \
         the sum against 1 kHz (and against the recording): {}",
        shown(&bands)
    );
    assert!((power - 22.3).abs() < 1.0, "{power:.1} dB");
    assert!(
        (meet / 520.0 - 1.0).abs() < 0.1,
        "they meet at {meet:.0} Hz"
    );
    for &(centre, _, difference) in &bands {
        if centre >= 63.0 {
            assert!(difference.abs() < 1.5, "{centre} Hz: {difference:+.1} dB");
        }
    }

    let mut orange = source(NoiseColour::Orange, SEED);
    let mut mix = LayerMix::untouched();
    for index in 0..(10.0 * sample_rate) as usize {
        mix.advance(sample_rate);
        let sample = orange.next_mixed(sample_rate, &mut mix);
        if index >= sample_rate as usize {
            assert_eq!(sample.to_bits(), (deep[index] + bright[index]).to_bits());
        }
    }
}
