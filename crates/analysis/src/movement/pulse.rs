//! A periodic pulse in the envelope (an isochronic tone, a tremolo, a
//! beating pair), and the shape of its average cycle.
//!
//! The envelope is the square root of the channels' summed power,
//! lowpassed at 200 Hz and sampled at 400 Hz, from one transform of the
//! squared signal. A boxcar average would alias the squared carrier's 2f
//! ripple into the pulse range: a steady 210 Hz tone would pulse at 20 Hz.
//! The pulse is the strongest rate from 1 to 50 Hz in the envelope's
//! spectrum, if it stands at least 20 dB over the median there (an
//! irregular envelope's strongest rate stands about 11 dB over it in a
//! minute). A narrow pulse's harmonics can outrank its fundamental, so a
//! rate a half to a fifth as fast that stands within 10 dB of it and 20 dB
//! over the median is taken instead. The envelope is then folded at that
//! rate into its average cycle, which gives the depth and the 10–90 % rise
//! and 90–10 % decay. A cycle shallower than 0.03, half the smallest depth
//! heard on noise, is no pulse.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use crate::report::Pulse;
use crate::spectrum::{transform_size, ENVELOPE_RATE};
use crate::statistics::{power_decibels, quantile};

/// The pulse rates looked for (Hz).
const LOWEST_HERTZ: f64 = 1.0;
const HIGHEST_HERTZ: f64 = 50.0;
/// A pulse stands this far over the median modulation power (dB).
const PROMINENCE_DECIBELS: f64 = 20.0;
/// A slower rate this close to the strongest is the fundamental (dB).
const FUNDAMENTAL_WITHIN_DECIBELS: f64 = 10.0;
/// A pulse shallower than this is not reported: half the smallest depth
/// heard on noise (m ≈ 0.06, −25 dB; Viemeister 1979). The power, as the
/// square of the amplitude, swings twice as deep: its line at the rate must
/// reach twice this share of the mean power.
const MINIMUM_DEPTH: f64 = 0.03;
/// A rate this close (a share of it) to a partial is that tone's pitch. 5 %
/// and not less: a drone's detuned second voices repeat a few per cent off
/// its fundamental (a 38.91 Hz drone gave 37.87 Hz, 116.20 − 78.25 Hz).
const PITCH_TOLERANCE: f64 = 0.05;
/// Envelope samples left out at each end (0.5 s): the lowpass rings there.
const TRIM: usize = ENVELOPE_RATE as usize / 2;
/// At least this much envelope (s) is needed.
const SHORTEST_SECONDS: f64 = 2.0;

/// The channels' power envelope at 400 Hz.
pub(crate) fn power_envelope(samples: &[f32], sample_rate: u32) -> Vec<f64> {
    let size = transform_size(samples.len(), sample_rate);
    let spacing = sample_rate as f64 / size as f64;
    let points = (ENVELOPE_RATE as f64 / spacing).round() as usize;
    let length =
        (samples.len() as f64 * ENVELOPE_RATE as f64 / sample_rate as f64).floor() as usize;
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(size);
    let mut input = forward.make_input_vec();
    for (slot, sample) in input.iter_mut().zip(samples) {
        *slot = (*sample as f64).powi(2);
    }
    let mut bins = forward.make_output_vec();
    forward
        .process(&mut input, &mut bins)
        .expect("buffers are the planner's own");
    let inverse = planner.plan_fft_inverse(points);
    let mut kept = inverse.make_input_vec();
    let kept_length = kept.len();
    // Below 200 Hz; the 200 Hz bin is the new Nyquist and is dropped.
    kept[..kept_length - 1].copy_from_slice(&bins[..kept_length - 1]);
    kept[0].im = 0.0;
    kept[kept_length - 1] = Complex::new(0.0, 0.0);
    let mut envelope = inverse.make_output_vec();
    inverse
        .process(&mut kept, &mut envelope)
        .expect("DC and Nyquist are real");
    let scale = 1.0 / size as f64;
    envelope
        .iter()
        .take(length)
        .map(|value| value * scale)
        .collect()
}

/// The pulse in `power` (the channels' summed power envelope at 400 Hz).
/// Rates within 5 % of a partial in `partials_hertz` are not pulses: a
/// harmonic tone's waveform repeats at its fundamental, which is its pitch.
pub(crate) fn pulse(power: &[f64], partials_hertz: &[f64]) -> Option<Pulse> {
    if power.len() < 2 * TRIM + (SHORTEST_SECONDS * ENVELOPE_RATE as f64) as usize {
        return None;
    }
    // The rate from the power, which is band-limited below 200 Hz; the
    // square root that gives the amplitude makes harmonics that fold back at
    // 400 Hz (351 Hz onto 49). The amplitude is used only for the cycle.
    let kept = &power[TRIM..power.len() - TRIM];
    let amplitude: Vec<f64> = kept.iter().map(|power| power.max(0.0).sqrt()).collect();
    let length = kept.len();
    let average = kept.iter().sum::<f64>() / length as f64;
    if average <= 0.0 {
        return None;
    }
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(length);
    let mut input: Vec<f64> = kept
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let taper = 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / length as f64).cos();
            (value - average) * taper
        })
        .collect();
    let mut spectrum = forward.make_output_vec();
    forward
        .process(&mut input, &mut spectrum)
        .expect("buffers are the planner's own");
    let spacing = ENVELOPE_RATE as f64 / length as f64;
    let first = ((LOWEST_HERTZ / spacing).ceil() as usize).max(1);
    let last = ((HIGHEST_HERTZ / spacing).floor() as usize).min(spectrum.len() - 2);
    if first >= last {
        return None;
    }
    let decibels: Vec<f64> = spectrum
        .iter()
        .enumerate()
        .map(|(index, bin)| {
            let rate = index as f64 * spacing;
            let pitch = partials_hertz
                .iter()
                .any(|partial| (rate - partial).abs() <= PITCH_TOLERANCE * partial);
            if pitch {
                f64::NEG_INFINITY
            } else {
                power_decibels(bin.norm_sqr())
            }
        })
        .collect();
    let counted: Vec<f64> = decibels[first..=last]
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect();
    let median = quantile(&counted, 0.5)?;
    let strongest = (first..=last).max_by(|a, b| decibels[*a].total_cmp(&decibels[*b]))?;
    if decibels[strongest] - median < PROMINENCE_DECIBELS {
        return None;
    }
    // The fundamental: the slowest of rate/2 … rate/5 that is strong too.
    let mut chosen = strongest;
    for divisor in (2..=5).rev() {
        let target = strongest as f64 / divisor as f64;
        if target * spacing < LOWEST_HERTZ {
            continue;
        }
        let around = (target.round() as usize).saturating_sub(2)..=(target.round() as usize + 2);
        if let Some(candidate) = around
            .filter(|index| *index >= 1 && *index < decibels.len() - 1)
            .max_by(|a, b| decibels[*a].total_cmp(&decibels[*b]))
        {
            if decibels[candidate] >= decibels[strongest] - FUNDAMENTAL_WITHIN_DECIBELS
                && decibels[candidate] - median >= PROMINENCE_DECIBELS
            {
                chosen = candidate;
                break;
            }
        }
    }
    let offset = vertex(decibels[chosen - 1], decibels[chosen], decibels[chosen + 1]);
    let rate = (chosen as f64 + offset) * spacing;
    // The rate's own sinusoid in the power, as a share of the mean power
    // (a Hann window passes half a sinusoid's amplitude). With no noise
    // floor, as in a clean render, the median says nothing; this does.
    let line_amplitude = 2.0 * spectrum[chosen].norm() / (0.5 * length as f64);
    if line_amplitude / average < 2.0 * MINIMUM_DEPTH {
        return None;
    }

    // The average cycle, one envelope sample a step where the rate allows.
    let steps = ((ENVELOPE_RATE as f64 / rate).round() as usize).clamp(8, 400);
    let mut sums = vec![0.0f64; steps];
    let mut counts = vec![0usize; steps];
    for (index, value) in amplitude.iter().enumerate() {
        let time = (index as f64 + 0.5) / ENVELOPE_RATE as f64;
        let phase = (time * rate).fract();
        let step = ((phase * steps as f64) as usize).min(steps - 1);
        sums[step] += value;
        counts[step] += 1;
    }
    let cycle: Vec<f64> = sums
        .iter()
        .zip(&counts)
        .map(|(sum, count)| sum / (*count).max(1) as f64)
        .collect();
    let peak = (0..steps).max_by(|a, b| cycle[*a].total_cmp(&cycle[*b]))?;
    let highest = cycle[peak];
    let lowest = cycle.iter().copied().fold(f64::INFINITY, f64::min);
    let depth = (highest - lowest) / (highest + lowest);
    if depth < MINIMUM_DEPTH {
        return None;
    }
    let normalised: Vec<f64> = cycle
        .iter()
        .map(|value| (value - lowest) / (highest - lowest))
        .collect();
    let step_seconds = 1.0 / (rate * steps as f64);
    let rise = crossing(&normalised, peak, -1, 0.9, 0.1) * step_seconds;
    let decay = crossing(&normalised, peak, 1, 0.9, 0.1) * step_seconds;
    Some(Pulse {
        rate_hertz: rate,
        prominence_decibels: decibels[chosen] - median,
        depth,
        rise_seconds: rise,
        decay_seconds: decay,
    })
}

/// Going round the cycle from `peak` in `direction`, the time (in steps)
/// between where it crosses `upper` and where it crosses `lower`, each
/// between steps by linear interpolation; at most a cycle.
fn crossing(cycle: &[f64], peak: usize, direction: isize, upper: f64, lower: f64) -> f64 {
    let steps = cycle.len() as isize;
    let at = |offset: isize| cycle[(peak as isize + direction * offset).rem_euclid(steps) as usize];
    let find = |level: f64, from: isize| -> Option<f64> {
        (from..steps).find_map(|offset| {
            let (near, far) = (at(offset - 1), at(offset));
            (far <= level && near > level)
                .then(|| (offset - 1) as f64 + (near - level) / (near - far))
        })
    };
    let Some(upper_at) = find(upper, 1) else {
        return steps as f64;
    };
    match find(lower, upper_at.floor() as isize + 1) {
        Some(lower_at) => lower_at - upper_at,
        None => steps as f64 - upper_at,
    }
}

/// The offset (−0.5 to 0.5) of the vertex of the parabola through three
/// equally spaced values.
fn vertex(left: f64, centre: f64, right: f64) -> f64 {
    let curvature = left - 2.0 * centre + right;
    if curvature >= 0.0 {
        0.0
    } else {
        (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{sine, Noise, RATE};

    /// A 500 Hz tone pulsed at 4 Hz: a 10 ms raised-cosine rise from a floor
    /// of 0.2 to 1, then an exponential fall with τ = 40 ms back towards the
    /// floor. Depth (1 − 0.2) / (1 + 0.2) = 0.667; the raised cosine goes
    /// from 10 % to 90 % in 10·(0.795 − 0.205) = 5.9 ms; the fall from 90 %
    /// to 10 % takes τ·ln 9 = 87.9 ms. The envelope's 200 Hz lowpass and its
    /// 2.5 ms steps blur the rise by a step or two.
    #[test]
    fn a_damped_pulse() {
        let rate = 4.0;
        let samples: Vec<f32> = sine(500.0, 1.0, 30.0)
            .iter()
            .enumerate()
            .map(|(index, carrier)| {
                let time = (index as f64 / RATE as f64) % (1.0 / rate);
                let shape = if time < 0.010 {
                    0.5 - 0.5 * (std::f64::consts::PI * time / 0.010).cos()
                } else {
                    (-(time - 0.010) / 0.040).exp()
                };
                (*carrier as f64 * (0.2 + 0.8 * shape)) as f32
            })
            .collect();
        let found = pulse(&power_envelope(&samples, RATE), &[]).expect("a pulse");
        assert!((found.rate_hertz - 4.0).abs() < 0.005, "{found:?}");
        assert!((found.depth - 0.667).abs() < 0.02, "{found:?}");
        assert!((found.rise_seconds - 0.0059).abs() < 0.003, "{found:?}");
        assert!((found.decay_seconds - 0.0879).abs() < 0.005, "{found:?}");
    }

    /// A steady 210 Hz tone (whose squared carrier, at 420 Hz, a boxcar
    /// envelope would alias to 20 Hz) and steady noise have no pulse.
    #[test]
    fn steady_sounds_have_no_pulse() {
        assert!(pulse(&power_envelope(&sine(210.0, 0.5, 30.0), RATE), &[210.0]).is_none());
        let mut noise = Noise::new(4);
        assert!(pulse(&power_envelope(&noise.white(0.1, 60.0), RATE), &[]).is_none());
    }

    /// A harmonic tone on 39 Hz (39, 78, 117 Hz) repeats 39 times a second:
    /// its pitch, not a pulse, once its partials are known.
    #[test]
    fn a_low_harmonic_tone_is_not_a_pulse() {
        let samples: Vec<f32> = sine(39.0, 0.3, 30.0)
            .iter()
            .zip(sine(78.0, 0.2, 30.0))
            .zip(sine(117.0, 0.1, 30.0))
            .map(|((first, second), third)| first + second + third)
            .collect();
        let envelope = power_envelope(&samples, RATE);
        assert!(pulse(&envelope, &[]).is_some_and(|found| (found.rate_hertz - 39.0).abs() < 0.01));
        let left = pulse(&envelope, &[39.0, 78.0, 117.0]);
        assert!(left.is_none(), "{left:?}");
    }

    /// Two equal tones 6 Hz apart beat at 6 Hz with nulls: depth 1.
    #[test]
    fn a_monaural_beat() {
        let samples: Vec<f32> = sine(400.0, 0.3, 30.0)
            .iter()
            .zip(sine(406.0, 0.3, 30.0))
            .map(|(first, second)| first + second)
            .collect();
        let found = pulse(&power_envelope(&samples, RATE), &[]).expect("a beat");
        assert!((found.rate_hertz - 6.0).abs() < 0.005, "{found:?}");
        assert!(found.depth > 0.95, "{found:?}");
    }
}
