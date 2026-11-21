use super::*;
use atrium_core::speaker::MAX_CHANNELS;
use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

const RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];
/// Mean magnitude of the zero-lag coherence below 200 Hz over the 28 channel
/// pairs, as the root README records it.
const RECORDED_MEAN_MAGNITUDE: f64 = 0.64;

// ── Hand-derived values ───────────────────────────────────────────────

#[test]
fn stage_impulse_response_is_hand_derived_and_lossless() {
    // H(z) = (−g + z^−M)/(1 − g·z^−M) with g = ½: h[0] = −½, h[M] = 1 − g² = ¾,
    // h[2M] = g(1 − g²) = ⅜, … and Σh² = g² + (1 − g²)²/(1 − g²) = 1.
    let delay = 5;
    let mut stage = AllPass::new(delay, STAGE_GAIN);
    let response: Vec<f32> = (0..2000)
        .map(|n| stage.process(if n == 0 { 1.0 } else { 0.0 }))
        .collect();
    assert_eq!(response[0], -0.5);
    assert_eq!(response[delay], 0.75);
    assert_eq!(response[2 * delay], 0.375);
    for (index, value) in response.iter().enumerate() {
        if index % delay != 0 {
            assert_eq!(*value, 0.0, "only multiples of M carry energy");
        }
    }
    let energy: f32 = response.iter().map(|value| value * value).sum();
    assert!((energy - 1.0).abs() < 1e-6, "all-pass energy {energy}");
}

#[test]
fn stages_preserve_energy_but_diverge_in_time() {
    let mut first = AllPass::new(17, STAGE_GAIN);
    let mut second = AllPass::new(29, STAGE_GAIN);
    let mut input_energy = 0.0;
    let mut first_energy = 0.0;
    let mut second_energy = 0.0;
    let mut difference = 0.0;
    for n in 0..20_000 {
        let input = ((n as f32 * 0.731).sin() + (n as f32 * 1.117).sin()) * 0.5;
        let first_output = first.process(input);
        let second_output = second.process(input);
        if n > 200 {
            input_energy += input * input;
            first_energy += first_output * first_output;
            second_energy += second_output * second_output;
            difference += (first_output - second_output).abs();
        }
    }
    assert!((first_energy / input_energy - 1.0).abs() < 0.03);
    assert!((second_energy / input_energy - 1.0).abs() < 0.03);
    assert!(
        difference > 100.0,
        "field voices should not remain coherent"
    );
}

#[test]
fn delays_round_to_expected_samples_at_48k() {
    // ms = 0.5 + 1.5·u, u the first four draws of Rng::new(80 + channel);
    // samples = round(ms × 48). Channel 0: 1.6932, 0.5667, 1.3645, 1.4589 ms
    // → 81.27, 27.20, 65.50 (65.4978), 70.03 → 81, 27, 65, 70.
    let expected: [[usize; STAGES]; MAX_CHANNELS] = [
        [81, 27, 65, 70],
        [75, 82, 72, 84],
        [39, 50, 53, 31],
        [78, 51, 25, 42],
        [94, 58, 75, 52],
        [80, 30, 93, 64],
        [70, 45, 62, 65],
        [85, 83, 45, 27],
    ];
    for (channel, expected_delays) in expected.iter().enumerate() {
        let decorrelator = Decorrelator::new(channel, 48_000.0);
        let delays: Vec<usize> = decorrelator
            .stages
            .iter()
            .map(|stage| stage.buffer.len())
            .collect();
        assert_eq!(delays, expected_delays, "channel {channel}");
        for stage in &decorrelator.stages {
            assert_eq!(stage.feedback, STAGE_GAIN);
        }
    }
}

#[test]
fn first_taps_of_channel_zero_are_hand_derived() {
    // Channel 0 at 48 kHz has delays 81, 27, 65 and 70 samples. At n = 0 every
    // stage passes −g: (−½)⁴ = 1/16. Nothing arrives before the shortest
    // delay. At n = 27 that stage passes 1 − g² and the others −g:
    // ¾·(−½)³ = −3/32. At n = 54 it passes g(1 − g²) = ⅜: −3/64. No sum of
    // the other delays (65, 70, 81) is that short.
    let response = impulse_response(0, 48_000.0, 0.01);
    assert_eq!(response[0], 0.0625);
    assert!(response[1..27].iter().all(|&value| value == 0.0));
    assert_eq!(response[27], -0.09375);
    assert!(response[28..54].iter().all(|&value| value == 0.0));
    assert_eq!(response[54], -0.046875);
}

// ── Acceptance: an attack heard once, decorrelated, lossless, flat ────

fn impulse_response(channel: usize, sample_rate: f32, seconds: f32) -> Vec<f64> {
    let mut decorrelator = Decorrelator::new(channel, sample_rate);
    (0..(seconds * sample_rate) as usize)
        .map(|n| decorrelator.process(if n == 0 { 1.0 } else { 0.0 }) as f64)
        .collect()
}

fn filtered(channel: usize, sample_rate: f32, input: &[f32]) -> Vec<f64> {
    let mut decorrelator = Decorrelator::new(channel, sample_rate);
    input
        .iter()
        .map(|&sample| decorrelator.process(sample) as f64)
        .collect()
}

fn white_noise(seed: u64, frames: usize) -> Vec<f32> {
    let mut generator = Rng::new(seed);
    (0..frames).map(|_| generator.next_bipolar()).collect()
}

fn milliseconds_to_samples(milliseconds: f64, sample_rate: f32) -> usize {
    (milliseconds * 0.001 * sample_rate as f64).round() as usize
}

/// Share of the energy of `response` that arrives before `milliseconds`.
fn energy_share_before(response: &[f64], sample_rate: f32, milliseconds: f64) -> f64 {
    let end = milliseconds_to_samples(milliseconds, sample_rate);
    let total: f64 = response.iter().map(|value| value * value).sum();
    response[..end]
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        / total
}

/// The strongest tap later than `milliseconds`, in dB relative to the
/// strongest tap of all.
fn strongest_tap_after_db(response: &[f64], sample_rate: f32, milliseconds: f64) -> f64 {
    let strongest = response
        .iter()
        .fold(0f64, |peak, value| peak.max(value.abs()));
    let start = milliseconds_to_samples(milliseconds, sample_rate) + 1;
    let late = response[start..]
        .iter()
        .fold(0f64, |peak, value| peak.max(value.abs()));
    20.0 * (late / strongest).log10()
}

/// Normalised cross-correlation Σ a[n]·b[n + lag] / √(Σa²·Σb²) for lags
/// −max_lag..=max_lag; index `max_lag` is lag 0.
fn normalised_cross_correlation(first: &[f64], second: &[f64], max_lag: usize) -> Vec<f64> {
    let length = (first.len() + second.len()).next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(length);
    let inverse = planner.plan_fft_inverse(length);
    let spectrum_of = |signal: &[f64]| {
        let mut input = forward.make_input_vec();
        input[..signal.len()].copy_from_slice(signal);
        let mut spectrum = forward.make_output_vec();
        forward
            .process(&mut input, &mut spectrum)
            .expect("forward fft");
        spectrum
    };
    let first_spectrum = spectrum_of(first);
    let second_spectrum = spectrum_of(second);
    let mut product: Vec<Complex<f64>> = first_spectrum
        .iter()
        .zip(&second_spectrum)
        .map(|(first_bin, second_bin)| first_bin.conj() * second_bin)
        .collect();
    let last = product.len() - 1;
    product[0].im = 0.0;
    product[last].im = 0.0;
    let mut correlation = inverse.make_output_vec();
    inverse
        .process(&mut product, &mut correlation)
        .expect("inverse fft");
    let first_energy: f64 = first.iter().map(|value| value * value).sum();
    let second_energy: f64 = second.iter().map(|value| value * value).sum();
    let scale = 1.0 / (length as f64 * (first_energy * second_energy).sqrt());
    (0..=2 * max_lag)
        .map(|index| {
            let lag = index as isize - max_lag as isize;
            correlation[lag.rem_euclid(length as isize) as usize] * scale
        })
        .collect()
}

fn format_matrix(matrix: &[[f64; MAX_CHANNELS]; MAX_CHANNELS]) -> String {
    matrix
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| format!("{value:+.3}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Third-octave band levels (power averaged over each band, in dB) of the
/// magnitude response, bands centred 1000·2^(k/3) Hz from 99 Hz to 16 kHz.
fn third_octave_levels_db(response: &[f64], sample_rate: f32) -> Vec<(f64, f64)> {
    let length = 1 << 17;
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(length);
    let mut input = forward.make_input_vec();
    input[..response.len()].copy_from_slice(response);
    let mut spectrum = forward.make_output_vec();
    forward
        .process(&mut input, &mut spectrum)
        .expect("forward fft");
    let bin_hz = sample_rate as f64 / length as f64;
    (-10..=12)
        .map(|band| {
            let centre = 1000.0 * 2f64.powf(band as f64 / 3.0);
            let lower = (centre * 2f64.powf(-1.0 / 6.0) / bin_hz).ceil() as usize;
            let upper = (centre * 2f64.powf(1.0 / 6.0) / bin_hz).floor() as usize;
            let power = spectrum[lower..=upper]
                .iter()
                .map(|bin| bin.norm_sqr())
                .sum::<f64>()
                / (upper - lower + 1) as f64;
            (centre, 10.0 * power.log10())
        })
        .collect()
}

#[test]
fn an_attack_is_heard_once() {
    for sample_rate in RATES {
        for channel in 0..MAX_CHANNELS {
            let response = impulse_response(channel, sample_rate, 0.2);
            let within_10_ms = energy_share_before(&response, sample_rate, 10.0);
            let within_30_ms = energy_share_before(&response, sample_rate, 30.0);
            let late_tap_db = strongest_tap_after_db(&response, sample_rate, 15.0);
            assert!(
                within_10_ms >= 0.85,
                "channel {channel} at {sample_rate} Hz: {:.1} % of the energy in the first 10 ms",
                within_10_ms * 100.0
            );
            assert!(
                within_30_ms >= 0.99,
                "channel {channel} at {sample_rate} Hz: {:.2} % of the energy in the first 30 ms",
                within_30_ms * 100.0
            );
            assert!(
                late_tap_db <= -20.0,
                "channel {channel} at {sample_rate} Hz: a tap after 15 ms at {late_tap_db:.1} dB"
            );
        }
    }
}

#[test]
fn channels_are_decorrelated_for_white_noise() {
    for sample_rate in RATES {
        let noise = white_noise(11, (2.0 * sample_rate) as usize);
        let outputs: Vec<Vec<f64>> = (0..MAX_CHANNELS)
            .map(|channel| filtered(channel, sample_rate, &noise))
            .collect();
        let max_lag = milliseconds_to_samples(30.0, sample_rate);
        let mut zero_lag = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
        let mut largest = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
        for first in 0..MAX_CHANNELS {
            for second in first + 1..MAX_CHANNELS {
                let correlation =
                    normalised_cross_correlation(&outputs[first], &outputs[second], max_lag);
                let largest_magnitude = correlation
                    .iter()
                    .fold(0f64, |peak, value| peak.max(value.abs()));
                zero_lag[first][second] = correlation[max_lag];
                zero_lag[second][first] = correlation[max_lag];
                largest[first][second] = largest_magnitude;
                largest[second][first] = largest_magnitude;
            }
        }
        let worst_zero_lag = zero_lag
            .iter()
            .flatten()
            .fold(0f64, |peak, value| peak.max(value.abs()));
        let worst_largest = largest
            .iter()
            .flatten()
            .fold(0f64, |peak, &value| peak.max(value));
        let report = format!(
            "at {sample_rate} Hz, zero lag (worst {worst_zero_lag:.3}):\n{}\n\
             largest over ±30 ms (worst {worst_largest:.3}):\n{}",
            format_matrix(&zero_lag),
            format_matrix(&largest)
        );
        eprintln!("{report}");
        assert!(worst_zero_lag <= 0.15, "{report}");
        assert!(worst_largest <= 0.35, "{report}");
    }
}

#[test]
fn each_channel_is_lossless() {
    for sample_rate in RATES {
        for channel in 0..MAX_CHANNELS {
            let energy: f64 = impulse_response(channel, sample_rate, 0.2)
                .iter()
                .map(|value| value * value)
                .sum();
            assert!(
                (energy - 1.0).abs() <= 0.001,
                "channel {channel} at {sample_rate} Hz: impulse response energy {energy:.6}"
            );
        }
    }
}

#[test]
fn third_octave_response_is_flat_from_100_hz_to_16_khz() {
    for sample_rate in RATES {
        for channel in 0..MAX_CHANNELS {
            let levels =
                third_octave_levels_db(&impulse_response(channel, sample_rate, 0.2), sample_rate);
            let (worst_centre, worst_level) =
                levels
                    .iter()
                    .copied()
                    .fold((0.0, 0.0), |worst: (f64, f64), band| {
                        if band.1.abs() > worst.1.abs() {
                            band
                        } else {
                            worst
                        }
                    });
            assert!(
                worst_level.abs() <= 3.0,
                "channel {channel} at {sample_rate} Hz: {worst_level:+.2} dB in the {worst_centre:.0} Hz third octave"
            );
        }
    }
}

#[test]
fn energy_within_ten_ms_is_the_same_at_every_rate() {
    for channel in 0..MAX_CHANNELS {
        let shares: Vec<f64> = RATES
            .iter()
            .map(|&sample_rate| {
                energy_share_before(
                    &impulse_response(channel, sample_rate, 0.2),
                    sample_rate,
                    10.0,
                )
            })
            .collect();
        let spread = shares.iter().fold(f64::MIN, |high, &share| high.max(share))
            - shares.iter().fold(f64::MAX, |low, &share| low.min(share));
        assert!(
            spread * 100.0 < 2.0,
            "channel {channel}: {:.2} / {:.2} / {:.2} % within 10 ms at 44.1 / 48 / 96 kHz",
            shares[0] * 100.0,
            shares[1] * 100.0,
            shares[2] * 100.0
        );
    }
}

/// A filter a few milliseconds long cannot shift the phase of a 100 Hz wave
/// (10 ms) far, so the deep bass stays coherent between channels. The README
/// records the figures; this keeps them true.
#[test]
fn bass_below_200_hz_stays_as_coherent_as_recorded() {
    let sample_rate = 48_000.0;
    let length = (4.0 * sample_rate) as usize;
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(length);
    let inverse = planner.plan_fft_inverse(length);
    let mut noise: Vec<f64> = white_noise(3, length)
        .iter()
        .map(|&sample| sample as f64)
        .collect();
    let mut spectrum = forward.make_output_vec();
    forward
        .process(&mut noise, &mut spectrum)
        .expect("forward fft");
    let bin_hz = sample_rate as f64 / length as f64;
    for (bin, value) in spectrum.iter_mut().enumerate() {
        if bin as f64 * bin_hz >= 200.0 {
            *value = Complex::new(0.0, 0.0);
        }
    }
    spectrum[0].im = 0.0;
    let mut bass = inverse.make_output_vec();
    inverse
        .process(&mut spectrum, &mut bass)
        .expect("inverse fft");
    let bass: Vec<f32> = bass.iter().map(|&sample| sample as f32).collect();
    let outputs: Vec<Vec<f64>> = (0..MAX_CHANNELS)
        .map(|channel| filtered(channel, sample_rate, &bass))
        .collect();
    let mut coherences = Vec::new();
    for first in 0..MAX_CHANNELS {
        for second in first + 1..MAX_CHANNELS {
            let product: f64 = outputs[first]
                .iter()
                .zip(&outputs[second])
                .map(|(first_sample, second_sample)| first_sample * second_sample)
                .sum();
            let first_energy: f64 = outputs[first].iter().map(|value| value * value).sum();
            let second_energy: f64 = outputs[second].iter().map(|value| value * value).sum();
            coherences.push(product / (first_energy * second_energy).sqrt());
        }
    }
    // Anti-phase bass (a negative coherence) is as coherent as in-phase bass:
    // it cancels where in-phase bass adds. So the magnitude is what is kept.
    let mean_magnitude =
        coherences.iter().map(|value| value.abs()).sum::<f64>() / coherences.len() as f64;
    let lowest = coherences
        .iter()
        .fold(f64::MAX, |low, &value| low.min(value));
    let highest = coherences
        .iter()
        .fold(f64::MIN, |high, &value| high.max(value));
    let report = format!(
        "zero-lag coherence below 200 Hz over the 28 pairs: mean magnitude {mean_magnitude:.2}, \
         from {lowest:.2} to {highest:.2}"
    );
    eprintln!("{report}");
    assert!(
        (mean_magnitude - RECORDED_MEAN_MAGNITUDE).abs() <= 0.03,
        "{report}"
    );
}
