//! How each partial moves over the file: its level and its pitch, from its
//! band at baseband in every channel, and the phase between the ears.
//!
//! The level is the channels' summed band power, smoothed over 0.25 s. The
//! pitch is the instantaneous frequency, each channel's weighted by its
//! power, smoothed the same way; where the band is more than 30 dB under its
//! median power its phase is noise, and those moments are left out. For a
//! cluster of voices the instantaneous frequency is their beating as well as
//! their drift, which is what the ear hears as the cluster's wander. The
//! first and last two seconds are left out: the band's 0.5 Hz roll-off
//! takes that long to settle where the file starts and stops.

use realfft::num_complex::Complex;

use super::baseband::Baseband;
use crate::report::PartialInteraural;
use crate::spectrum::ENVELOPE_RATE;
use crate::statistics::{power_decibels, quantile, standard_deviation};

/// Samples left out at each end (2 s at 400 Hz).
const TRIM: usize = 2 * ENVELOPE_RATE as usize;
/// The smoothing window (0.25 s at 400 Hz).
const SMOOTHING: usize = ENVELOPE_RATE as usize / 4;
/// Below this share of its median power a band's phase is not read.
const PITCH_GATE: f64 = 1e-3;
/// The lag the interaural rotation is read over (0.1 s): rates within
/// ±5 Hz are unambiguous.
const ROTATION_LAG: usize = ENVELOPE_RATE as usize / 10;
/// The rate's spread is taken over stretches this long (10 s).
const STRETCH_SAMPLES: usize = 10 * ENVELOPE_RATE as usize;

/// One partial's band summed over the channels.
#[derive(Default)]
pub(crate) struct Track {
    power: Vec<f64>,
    /// Σ power·instantaneous frequency.
    weighted_frequency: Vec<f64>,
}

impl Track {
    pub(crate) fn add(&mut self, band: &Baseband) {
        let length = band.values.len();
        if self.power.is_empty() {
            self.power = vec![0.0; length];
            self.weighted_frequency = vec![0.0; length];
        }
        for index in 1..length.min(self.power.len()) {
            let power = band.values[index].norm_sqr() as f64;
            self.power[index] += power;
            self.weighted_frequency[index] += power * band.frequency_at(index);
        }
    }

    /// The trimmed, smoothed band power; None when under two seconds remain.
    fn smoothed_power(&self) -> Option<Vec<f64>> {
        let kept = trimmed(&self.power)?;
        Some(moving_sum(kept, SMOOTHING))
    }

    /// The 90th minus the 10th percentile of the smoothed level (dB).
    pub(crate) fn level_p10_p90(&self) -> Option<f64> {
        let levels: Vec<f64> = self
            .smoothed_power()?
            .iter()
            .map(|power| power_decibels(power / SMOOTHING as f64))
            .collect();
        Some(quantile(&levels, 0.9)? - quantile(&levels, 0.1)?)
    }

    /// The smoothed pitch around its median: standard deviation and 90th
    /// minus 10th percentile (cents).
    pub(crate) fn pitch(&self) -> Option<(f64, f64)> {
        let power = self.smoothed_power()?;
        let weighted = moving_sum(trimmed(&self.weighted_frequency)?, SMOOTHING);
        let median_power = quantile(&power, 0.5)?;
        let frequencies: Vec<f64> = power
            .iter()
            .zip(&weighted)
            .filter(|(power, _)| **power > PITCH_GATE * median_power && **power > 0.0)
            .map(|(power, weighted)| weighted / power)
            .filter(|frequency| *frequency > 0.0)
            .collect();
        let median = quantile(&frequencies, 0.5)?;
        let cents: Vec<f64> = frequencies
            .iter()
            .map(|frequency| 1200.0 * (frequency / median).log2())
            .collect();
        Some((
            standard_deviation(&cents),
            quantile(&cents, 0.9)? - quantile(&cents, 0.1)?,
        ))
    }
}

/// The partial at the two ears, from its band at each; None when under two
/// seconds remain.
pub(crate) fn interaural(
    left: &[Complex<f32>],
    right: &[Complex<f32>],
) -> Option<PartialInteraural> {
    let length = left.len().min(right.len());
    let left = trimmed(&left[..length])?;
    let right = trimmed(&right[..length])?;
    let products: Vec<Complex<f64>> = left
        .iter()
        .zip(right)
        .map(|(left, right)| {
            Complex::new(left.re as f64, left.im as f64)
                * Complex::new(right.re as f64, -(right.im as f64))
        })
        .collect();
    let left_power: f64 = left.iter().map(|value| value.norm_sqr() as f64).sum();
    let right_power: f64 = right.iter().map(|value| value.norm_sqr() as f64).sum();

    let rotation_hertz = rotation(&products);
    // The rate in each 10 s, weighted by the stretch's power: a detune that
    // drifts a little over minutes keeps its rate from stretch to stretch,
    // voices wandering apart at the two ears do not.
    let stretches: Vec<(f64, f64)> = products
        .chunks(STRETCH_SAMPLES)
        .filter(|part| part.len() >= STRETCH_SAMPLES / 2)
        .map(|part| {
            (
                rotation(part),
                part.iter().map(|product| product.norm()).sum(),
            )
        })
        .collect();
    let weight: f64 = stretches.iter().map(|(_, weight)| weight).sum();
    let rotation_spread_hertz = (stretches.len() >= 2 && weight > 0.0).then(|| {
        let average = stretches
            .iter()
            .map(|(rate, weight)| rate * weight)
            .sum::<f64>()
            / weight;
        (stretches
            .iter()
            .map(|(rate, weight)| weight * (rate - average).powi(2))
            .sum::<f64>()
            / weight)
            .sqrt()
    });
    Some(PartialInteraural {
        level_difference_decibels: power_decibels(right_power) - power_decibels(left_power),
        rotation_hertz,
        rotation_spread_hertz,
    })
}

/// The rate (Hz) the phase of `products` turns at, from their mean turn
/// over [`ROTATION_LAG`].
fn rotation(products: &[Complex<f64>]) -> f64 {
    let turned: Complex<f64> = products
        .iter()
        .skip(ROTATION_LAG)
        .zip(products)
        .map(|(now, before)| now * before.conj())
        .sum();
    let lag_seconds = ROTATION_LAG as f64 / ENVELOPE_RATE as f64;
    turned.im.atan2(turned.re) / (std::f64::consts::TAU * lag_seconds)
}

/// `values` without its first and last two seconds; None when under two
/// seconds remain.
fn trimmed<T>(values: &[T]) -> Option<&[T]> {
    (values.len() >= 3 * TRIM).then(|| &values[TRIM..values.len() - TRIM])
}

/// The sums of every `window` consecutive values.
fn moving_sum(values: &[f64], window: usize) -> Vec<f64> {
    if values.len() < window || window == 0 {
        return Vec::new();
    }
    let mut sums = Vec::with_capacity(values.len() - window + 1);
    let mut sum: f64 = values[..window].iter().sum();
    sums.push(sum);
    for index in window..values.len() {
        sum += values[index] - values[index - window];
        sums.push(sum);
    }
    sums
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::baseband::baseband;
    use crate::spectrum::Spectrum;
    use crate::test_signals::{sine, RATE};

    /// `seconds` of a sine at `frequency` whose pitch swings `cents` either
    /// side at `rate` Hz and whose level swings `decibels` either side at
    /// `level_rate` Hz, both sinusoidally.
    fn moving_tone(
        frequency: f64,
        cents: f64,
        rate: f64,
        decibels: f64,
        level_rate: f64,
        seconds: f64,
    ) -> Vec<f32> {
        let mut phase = 0.0f64;
        (0..(seconds * RATE as f64) as usize)
            .map(|index| {
                let time = index as f64 / RATE as f64;
                let swing = (std::f64::consts::TAU * rate * time).sin();
                let now = frequency * 2f64.powf(cents * swing / 1200.0);
                phase += std::f64::consts::TAU * now / RATE as f64;
                let level = decibels * (std::f64::consts::TAU * level_rate * time).sin();
                (0.3 * 10f64.powf(level / 20.0) * phase.sin()) as f32
            })
            .collect()
    }

    fn track_of(samples: &[f32], low: f64, high: f64) -> Track {
        let mut track = Track::default();
        track.add(&baseband(&Spectrum::of(samples, RATE), low, high));
        track
    }

    /// A steady sine: its level moves under 0.1 dB and its pitch under
    /// 0.1 cent.
    #[test]
    fn a_steady_sine_does_not_move() {
        let track = track_of(&sine(300.0, 0.3, 30.0), 298.0, 302.0);
        let level = track.level_p10_p90().expect("long enough");
        assert!(level < 0.1, "{level}");
        let (spread, range) = track.pitch().expect("long enough");
        assert!(spread < 0.1 && range < 0.3, "{spread} {range}");
    }

    /// Pitch swinging ±10 cents at 0.2 Hz: a sine's standard deviation is
    /// its amplitude over √2, 7.07 cents, and its 10th to 90th percentile
    /// 2·10·sin(0.4π) = 19.0 cents (the 0.25 s smoothing takes 0.4 % off a
    /// 0.2 Hz swing). Level swinging ±6 dB at 0.05 Hz: 2·6·sin(0.4π) =
    /// 11.4 dB from the 10th to the 90th percentile.
    #[test]
    fn a_moving_tone_reads_its_swings() {
        let samples = moving_tone(300.0, 10.0, 0.2, 6.0, 0.05, 80.0);
        let track = track_of(&samples, 296.0, 304.0);
        let (spread, range) = track.pitch().expect("long enough");
        assert!((spread - 7.07).abs() < 0.3, "{spread}");
        assert!((range - 19.0).abs() < 0.8, "{range}");
        let level = track.level_p10_p90().expect("long enough");
        assert!((level - 11.4).abs() < 0.5, "{level}");
    }

    /// 200 Hz at the left ear and 200.5 Hz at the right: the phase between
    /// them turns at 200 − 200.5 = −0.5 Hz in every 10 s, and the levels
    /// match. The same tone at both ears does not turn.
    #[test]
    fn interaural_rotation_of_a_binaural_detune() {
        let left = sine(200.0, 0.3, 30.0);
        let right = sine(200.5, 0.3, 30.0);
        let band = |samples: &[f32]| baseband(&Spectrum::of(samples, RATE), 198.0, 203.0);
        let reading = interaural(&band(&left).values, &band(&right).values).expect("long");
        assert!((reading.rotation_hertz + 0.5).abs() < 0.01, "{reading:?}");
        assert!(
            reading.rotation_spread_hertz.expect("stretches") < 0.005,
            "{reading:?}"
        );
        assert!(reading.level_difference_decibels.abs() < 0.2, "{reading:?}");
        let same = interaural(&band(&left).values, &band(&left).values).expect("long");
        assert!(same.rotation_hertz.abs() < 1e-6);
        assert!(same.rotation_spread_hertz.expect("stretches") < 1e-6);
    }
}
