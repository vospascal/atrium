//! Step 1: the band the events are found in, and the level in it.
//!
//! The band is the third octaves where the recording's events stand furthest
//! above its bed: L1 − L90 of each band's level, and every band within
//! [`WITHIN_LARGEST_DECIBELS`] of the largest. A bird at 2–8 kHz is then
//! found under wind that sits below 500 Hz. Raven's band-limited energy
//! detector (Cornell Lab) detects in a chosen band for the same reason.
//!
//! L1, the level exceeded 1 % of the time, not L10: L10 sees the events only
//! when they sound more than a tenth of the time. Chirps of 150 ms every 4 s
//! sound 4 % of the time, and by L10 − L90 their band was as eventful as any
//! other, the bed's own scatter.
//!
//! Each level reading averages a frame of B·T = [`TIME_BANDWIDTH`], B the
//! band's width, so every band's level scatters by the same amount by
//! chance: a reading of band-limited noise over T seconds has 2·B·T degrees
//! of freedom. With frames of one length for all bands, the narrow low bands
//! would scatter most and noise alone would make them look eventful. Ten
//! degrees of freedom put the noise's 10th percentile 3.1 dB below its mean
//! and its readings above the start threshold (15 dB over that percentile) at
//! a chance below 10⁻¹⁰ each.
//!
//! The level is of the channels' mean, as the engine plays a take (it mixes
//! a recording's channels to one).

use super::record::{BandSpread, DetectionBand};
use crate::bands::third_octaves;
use crate::spectrum::Spectrum;
use crate::statistics::{power_decibels, quantile};

/// B·T of one level reading: ten degrees of freedom.
const TIME_BANDWIDTH: f64 = 5.0;
/// The shortest reading (s), for a wide band.
const SHORTEST_FRAME_SECONDS: f64 = 0.002;
/// One reading every millisecond, centred on it.
pub(super) const STEP_SECONDS: f64 = 0.001;
/// Bands whose L1 − L90 lies within this of the largest are detected in (dB).
pub(super) const WITHIN_LARGEST_DECIBELS: f64 = 6.0;
/// Outside its band a detection filter falls to nothing over a third of an
/// octave, as half a raised cosine: a brick-wall edge would ring before an
/// onset and move the start early.
const ROLL_OFF_OCTAVES: f64 = 1.0 / 3.0;

/// The band level, a reading every [`STEP_SECONDS`] from 0, each the mean
/// power over `frame_seconds` centred on it, in dB re full scale.
pub(super) struct BandReadings {
    pub(super) decibels: Vec<f64>,
    pub(super) frame_seconds: f64,
}

/// The band to detect in: `given` (Hz), or the third octaves where the
/// events stand furthest above the bed.
pub(super) fn choose(spectrum: &Spectrum, given: Option<(f64, f64)>) -> DetectionBand {
    let nyquist = spectrum.sample_rate() as f64 / 2.0;
    if let Some((low, high)) = given {
        let high = high.min(nyquist);
        return detection_band(vec![[low, high]], true, Vec::new());
    }
    let bands: Vec<BandSpread> = third_octaves(nyquist)
        .into_iter()
        .map(|band| {
            let filtered = spectrum.filtered(|frequency| {
                f64::from(u8::from((band.low..band.high).contains(&frequency)))
            });
            let frame_seconds = frame_seconds(band.high - band.low);
            let levels = frame_levels(
                &filtered,
                spectrum.sample_rate(),
                frame_seconds,
                frame_seconds / 2.0,
            );
            let spread = match (quantile(&levels, 0.99), quantile(&levels, 0.1)) {
                (Some(l1), Some(l90)) => l1 - l90,
                _ => 0.0,
            };
            BandSpread {
                nominal_hertz: band.nominal,
                low_hertz: band.low,
                high_hertz: band.high,
                l1_minus_l90: spread,
                chosen: false,
            }
        })
        .collect();
    let largest = bands
        .iter()
        .map(|band| band.l1_minus_l90)
        .fold(f64::NEG_INFINITY, f64::max);
    let bands: Vec<BandSpread> = bands
        .into_iter()
        .map(|band| BandSpread {
            chosen: band.l1_minus_l90 >= largest - WITHIN_LARGEST_DECIBELS,
            ..band
        })
        .collect();
    // Neighbouring chosen bands make one range.
    let mut ranges: Vec<[f64; 2]> = Vec::new();
    for band in bands.iter().filter(|band| band.chosen) {
        match ranges.last_mut() {
            Some(last) if (band.low_hertz - last[1]).abs() <= 1e-6 * band.low_hertz => {
                last[1] = band.high_hertz;
            }
            _ => ranges.push([band.low_hertz, band.high_hertz]),
        }
    }
    detection_band(ranges, false, bands)
}

fn detection_band(ranges: Vec<[f64; 2]>, given: bool, bands: Vec<BandSpread>) -> DetectionBand {
    let bandwidth: f64 = ranges.iter().map(|[low, high]| high - low).sum();
    DetectionBand {
        low_hertz: ranges.first().map_or(0.0, |range| range[0]),
        high_hertz: ranges.last().map_or(0.0, |range| range[1]),
        frame_seconds: frame_seconds(bandwidth),
        bandwidth_hertz: bandwidth,
        ranges,
        given,
        bands,
    }
}

/// How long a reading in a band `bandwidth` Hz wide averages (s).
fn frame_seconds(bandwidth: f64) -> f64 {
    (TIME_BANDWIDTH / bandwidth.max(1e-9)).max(SHORTEST_FRAME_SECONDS)
}

/// The detection filter's gain at `frequency`: 1 in a range, falling over
/// [`ROLL_OFF_OCTAVES`] outside it.
fn gain(ranges: &[[f64; 2]], frequency: f64) -> f64 {
    ranges
        .iter()
        .map(|[low, high]| {
            let outside_octaves = if frequency < *low {
                (low / frequency.max(1e-9)).log2()
            } else if frequency > *high {
                (frequency / high).log2()
            } else {
                0.0
            };
            if outside_octaves >= ROLL_OFF_OCTAVES {
                0.0
            } else {
                0.5 + 0.5 * (std::f64::consts::PI * outside_octaves / ROLL_OFF_OCTAVES).cos()
            }
        })
        .fold(0.0, f64::max)
}

/// The level in `band`, a reading every [`STEP_SECONDS`].
pub(super) fn level(spectrum: &Spectrum, band: &DetectionBand) -> BandReadings {
    let filtered = spectrum.filtered(|frequency| gain(&band.ranges, frequency));
    BandReadings {
        decibels: frame_levels(
            &filtered,
            spectrum.sample_rate(),
            band.frame_seconds,
            STEP_SECONDS,
        ),
        frame_seconds: band.frame_seconds,
    }
}

/// The mean power of `samples` over `frame_seconds` centred on every
/// `step_seconds` from 0 to the end (frames at the ends cut to the signal),
/// in dB.
fn frame_levels(
    samples: &[f32],
    sample_rate: u32,
    frame_seconds: f64,
    step_seconds: f64,
) -> Vec<f64> {
    let rate = sample_rate as f64;
    let mut cumulative = Vec::with_capacity(samples.len() + 1);
    cumulative.push(0.0f64);
    let mut sum = 0.0f64;
    for sample in samples {
        sum += (*sample as f64).powi(2);
        cumulative.push(sum);
    }
    let half = ((frame_seconds * rate).round() as usize).max(1) / 2;
    let seconds = samples.len() as f64 / rate;
    let readings = (seconds / step_seconds).floor() as usize + 1;
    (0..readings)
        .map(|index| {
            let centre = (index as f64 * step_seconds * rate).round() as usize;
            let start = centre.saturating_sub(half).min(samples.len());
            let end = (centre + half.max(1)).min(samples.len());
            if end <= start {
                return power_decibels(0.0);
            }
            power_decibels((cumulative[end] - cumulative[start]) / (end - start) as f64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{sine, Noise, RATE};

    /// A tone burst in noise is found in the third octave that holds it;
    /// noise alone has every band within a few dB of the others.
    #[test]
    fn the_band_follows_the_events() {
        let mut noise = Noise::new(4);
        let mut samples = noise.pink(0.01, 20.0);
        for burst in 0..8 {
            let start = ((1.0 + 2.3 * burst as f64) * RATE as f64) as usize;
            for (offset, value) in sine(3150.0, 0.2, 0.4).into_iter().enumerate() {
                samples[start + offset] += value;
            }
        }
        let band = choose(&Spectrum::of(&samples, RATE), None);
        assert!(
            band.low_hertz <= 3150.0 && band.high_hertz >= 3150.0,
            "{:?}",
            band.ranges
        );
        assert!(
            band.high_hertz - band.low_hertz < 3000.0,
            "{:?}",
            band.ranges
        );

        let quiet = Noise::new(5).pink(0.01, 20.0);
        let band = choose(&Spectrum::of(&quiet, RATE), None);
        let spreads: Vec<f64> = band.bands.iter().map(|band| band.l1_minus_l90).collect();
        let (lowest, highest) = spreads
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
                (low.min(*value), high.max(*value))
            });
        assert!(highest - lowest < WITHIN_LARGEST_DECIBELS, "{spreads:?}");
    }

    #[test]
    fn the_filter_falls_off_outside_its_range() {
        let ranges = [[1000.0, 2000.0]];
        assert_eq!(gain(&ranges, 1500.0), 1.0);
        assert!((gain(&ranges, 1000.0 / 2f64.powf(1.0 / 6.0)) - 0.5).abs() < 1e-9);
        assert_eq!(gain(&ranges, 500.0), 0.0);
        assert_eq!(gain(&ranges, 4000.0), 0.0);
    }

    /// A reading is the mean power of its frame: a sine of amplitude 0.5 has
    /// 0.125, −9.03 dB.
    #[test]
    fn readings_are_mean_powers() {
        let tone = sine(1000.0, 0.5, 1.0);
        let levels = frame_levels(&tone, RATE, 0.01, STEP_SECONDS);
        assert_eq!(levels.len(), 1001);
        assert!(
            (levels[500] - power_decibels(0.125)).abs() < 0.01,
            "{}",
            levels[500]
        );
    }
}
