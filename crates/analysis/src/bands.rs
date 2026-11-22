//! Third-octave and octave bands, IEC 61260-1: base-ten centres
//! 1000·10^(n/10) Hz, edges a twentieth of a decade (third octave) or three
//! twentieths (octave) either side, named by their nominal centres.

use crate::spectrum::Spectrum;

/// One band: its nominal centre and its exact edges (Hz).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Band {
    pub(crate) nominal: f64,
    pub(crate) low: f64,
    pub(crate) high: f64,
}

/// The nominal centres from 25 Hz to 20 kHz (IEC 61260-1 Annex E).
const THIRD_OCTAVE_NOMINALS: [f64; 30] = [
    25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0,
    630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0, 8000.0,
    10_000.0, 12_500.0, 16_000.0, 20_000.0,
];

/// The band index n of the first nominal, 25 Hz: 1000·10^(−16/10) = 25.1 Hz.
const FIRST_THIRD_OCTAVE_INDEX: i32 = -16;

/// The octave bands the space measures use, 125 Hz to 8 kHz (ISO 3382-1).
const OCTAVE_NOMINALS: [f64; 7] = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

/// The third octaves from 25 Hz to 20 kHz that lie wholly below `nyquist`.
pub(crate) fn third_octaves(nyquist: f64) -> Vec<Band> {
    THIRD_OCTAVE_NOMINALS
        .iter()
        .enumerate()
        .map(|(offset, nominal)| {
            band(
                *nominal,
                FIRST_THIRD_OCTAVE_INDEX + offset as i32,
                1.0 / 20.0,
            )
        })
        .filter(|band| band.high <= nyquist)
        .collect()
}

/// The octaves from 125 Hz to 8 kHz that lie wholly below `nyquist`.
pub(crate) fn octaves(nyquist: f64) -> Vec<Band> {
    OCTAVE_NOMINALS
        .iter()
        .enumerate()
        .map(|(offset, nominal)| band(*nominal, -9 + 3 * offset as i32, 3.0 / 20.0))
        .filter(|band| band.high <= nyquist)
        .collect()
}

fn band(nominal: f64, index: i32, half_width_decades: f64) -> Band {
    let centre = 1000.0 * 10f64.powf(index as f64 / 10.0);
    Band {
        nominal,
        low: centre * 10f64.powf(-half_width_decades),
        high: centre * 10f64.powf(half_width_decades),
    }
}

/// The mean square of `spectrum` in each band.
pub(crate) fn band_powers(spectrum: &Spectrum, bands: &[Band]) -> Vec<f64> {
    bands
        .iter()
        .map(|band| spectrum.mean_square(band.low, band.high, |_| 1.0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::power_decibels;
    use crate::test_signals::{sine, Noise, RATE};

    #[test]
    fn centres_and_edges_follow_the_standard() {
        let bands = third_octaves(24_000.0);
        assert_eq!(bands.len(), 30);
        let thousand = bands[16];
        assert_eq!(thousand.nominal, 1000.0);
        assert!((thousand.low - 891.25).abs() < 0.01 && (thousand.high - 1122.02).abs() < 0.01);
        // A 44.1 kHz file has no 20 kHz band: its upper edge is 22.4 kHz.
        assert_eq!(third_octaves(22_050.0).len(), 29);
        assert_eq!(octaves(24_000.0).len(), 7);
    }

    /// Pink noise has the same power in every band of constant relative
    /// width: 100 Hz to 16 kHz each within ±0.5 dB of their mean.
    #[test]
    fn pink_noise_is_flat_in_third_octaves() {
        let pink = Noise::new(3).pink(0.1, 60.0);
        let spectrum = Spectrum::of(&pink, RATE);
        let bands: Vec<Band> = third_octaves(24_000.0)
            .into_iter()
            .filter(|band| band.nominal >= 100.0 && band.nominal <= 16_000.0)
            .collect();
        let levels: Vec<f64> = band_powers(&spectrum, &bands)
            .into_iter()
            .map(power_decibels)
            .collect();
        let average = crate::statistics::mean(&levels);
        for (band, level) in bands.iter().zip(&levels) {
            assert!(
                (level - average).abs() <= 0.5,
                "{} Hz at {level:.2} dB, mean {average:.2}",
                band.nominal
            );
        }
    }

    /// A 1 kHz sine at −20 dBFS RMS reads −20.0 ± 0.1 in the 1 kHz band and
    /// at least 30 dB less two or more bands away.
    #[test]
    fn a_sine_stays_in_its_band() {
        let tone = sine(1000.0, 0.1 * 2f64.sqrt(), 5.0);
        let spectrum = Spectrum::of(&tone, RATE);
        let bands = third_octaves(24_000.0);
        let levels: Vec<f64> = band_powers(&spectrum, &bands)
            .into_iter()
            .map(power_decibels)
            .collect();
        assert!((levels[16] + 20.0).abs() <= 0.1, "{}", levels[16]);
        for (index, level) in levels.iter().enumerate() {
            if index.abs_diff(16) >= 2 {
                assert!(*level <= -50.0, "{} Hz at {level:.1}", bands[index].nominal);
            }
        }
    }
}
