//! Colour: the third-octave levels relative to each file's Leq, so a
//! recording in dBFS and a render in dB SPL compare as shapes.

use super::comparison::{BandDifference, ColourDifferences, Difference, Ratio};
use crate::report::Report;

/// A band this far below the file's Leq counts as silent: below it a
/// difference is not one of colour, and an empty band would otherwise
/// dominate the RMS with a hundred-odd dB (dB).
const FLOOR_DECIBELS: f64 = -100.0;

/// The bands the RMS difference is taken over (nominal centres, Hz): where
/// both a recording and a render have content worth comparing, below an
/// MP3's lowpass.
const RMS_LOWEST: f64 = 50.0;
const RMS_HIGHEST: f64 = 16_000.0;

/// The bands the tilt is fitted over (nominal centres, Hz).
const TILT_LOWEST: f64 = 100.0;
const TILT_HIGHEST: f64 = 10_000.0;

pub(super) fn compare(reference: &Report, other: &Report) -> ColourDifferences {
    let first = relative_band_levels(reference);
    let second = relative_band_levels(other);
    let bands: Vec<BandDifference> = first
        .iter()
        .filter_map(|(centre, level)| {
            let (_, other_level) = second.iter().find(|(other, _)| other == centre)?;
            Some(BandDifference {
                centre_hertz: *centre,
                decibels: Difference::of(*level, *other_level),
            })
        })
        .collect();
    let compared: Vec<f64> = bands
        .iter()
        .filter(|band| (RMS_LOWEST..=RMS_HIGHEST).contains(&band.centre_hertz))
        .map(|band| band.decibels.difference.powi(2))
        .collect();
    let rms_difference_decibels = if compared.is_empty() {
        0.0
    } else {
        (compared.iter().sum::<f64>() / compared.len() as f64).sqrt()
    };
    ColourDifferences {
        bands,
        rms_difference_decibels,
        tilt_decibels_per_octave: Difference::of(tilt(&first), tilt(&second)),
        centroid_hertz: Ratio::of(reference.colour.centroid_hertz, other.colour.centroid_hertz),
    }
}

/// Each third octave's nominal centre and its level relative to the file's
/// Leq, floored at [`FLOOR_DECIBELS`].
pub(crate) fn relative_band_levels(report: &Report) -> Vec<(f64, f64)> {
    report
        .colour
        .third_octaves
        .iter()
        .map(|band| {
            (
                band.centre_hertz,
                band.relative_decibels.max(FLOOR_DECIBELS),
            )
        })
        .collect()
}

/// The least-squares slope of the band levels over octaves, 100 Hz to
/// 10 kHz (dB per octave). On the bands' exact base-ten centres, not their
/// nominal ones: the nominal 125 Hz is 0.7 % off the exact 125.9, and white
/// noise is +3.01 dB per octave only on the exact ones.
fn tilt(bands: &[(f64, f64)]) -> f64 {
    let points: Vec<(f64, f64)> = bands
        .iter()
        .filter(|(centre, _)| (TILT_LOWEST..=TILT_HIGHEST).contains(centre))
        .map(|(centre, level)| (exact_centre(*centre).log2(), *level))
        .collect();
    if points.len() < 2 {
        return 0.0;
    }
    let count = points.len() as f64;
    let mean_octave = points.iter().map(|(octave, _)| octave).sum::<f64>() / count;
    let mean_level = points.iter().map(|(_, level)| level).sum::<f64>() / count;
    let covariance: f64 = points
        .iter()
        .map(|(octave, level)| (octave - mean_octave) * (level - mean_level))
        .sum();
    let spread: f64 = points
        .iter()
        .map(|(octave, _)| (octave - mean_octave).powi(2))
        .sum();
    covariance / spread
}

/// IEC 61260-1's exact centre, 1000·10^(n/10) Hz, of the band named
/// `nominal`.
fn exact_centre(nominal: f64) -> f64 {
    let index = (10.0 * (nominal / 1000.0).log10()).round();
    1000.0 * 10f64.powf(index / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_centres_are_base_ten() {
        assert!((exact_centre(125.0) - 125.89).abs() < 0.01);
        assert!((exact_centre(31.5) - 31.62).abs() < 0.01);
        assert_eq!(exact_centre(1000.0), 1000.0);
    }

    /// Levels rising exactly 10·log10(2) = 3.01 dB per octave on the exact
    /// centres (white noise's third octaves) fit a slope of 3.01; flat ones
    /// a slope of 0. Bands outside 100 Hz to 10 kHz do not count.
    #[test]
    fn the_tilt_is_the_slope_over_octaves() {
        let nominals = [
            50.0, 100.0, 125.0, 160.0, 200.0, 1000.0, 5000.0, 10_000.0, 16_000.0,
        ];
        let white: Vec<(f64, f64)> = nominals
            .iter()
            .map(|nominal| {
                let level = 10.0 * (exact_centre(*nominal) / 1000.0).log10();
                let outside = !(TILT_LOWEST..=TILT_HIGHEST).contains(nominal);
                (*nominal, if outside { 40.0 } else { level })
            })
            .collect();
        assert!((tilt(&white) - 10.0 * 2f64.log10()).abs() < 1e-9);
        let flat: Vec<(f64, f64)> = nominals.iter().map(|nominal| (*nominal, -20.0)).collect();
        assert!(tilt(&flat).abs() < 1e-12);
    }
}
