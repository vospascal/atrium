//! Texture: what makes rain rain and wind wind. McDermott and Simoncelli
//! (2011) showed that a handful of statistics of a sound's cochlear subband
//! envelopes is what people recognise a texture by: synthetic sounds that
//! share them are heard as the same rain, the same fire. These are those
//! statistics, computed as their Experimental Procedures define them.
//!
//! - `filters`: the cochlear bank and the two modulation banks
//! - `statistics`: the window, the moments, the correlations
//!
//! C2 (the phase relations between modulation bands an octave apart) is left
//! out: it needs analytic modulation bands and phase doubling for every
//! subband, and it distinguishes crackling from noisy textures more than it
//! describes wind or rain.

mod filters;
mod statistics;

use crate::envelope::band_envelope;
use crate::report::{ModulationCorrelation, Subband, SubbandCorrelation, Texture};
use crate::spectrum::Spectrum;
use filters::{c1_filters, modulation_power_filters, CochlearBank};
use statistics::{
    band_correlation, envelope_correlation, modulation_power, moments, window, ModulationBands,
};

/// The compression of the envelopes, as the paper's: basilar-membrane
/// compression. The comparison undoes a level difference through it.
pub(crate) const COMPRESSION: f64 = 0.3;

/// The subband offsets C is reported at (the paper's).
const CORRELATION_OFFSETS: [usize; 8] = [1, 2, 3, 5, 8, 11, 16, 21];

/// Envelopes shorter than this (0.16 s at 400 Hz) have no meaningful
/// statistics; their texture is reported as zeros.
const SHORTEST_ENVELOPE: usize = 64;

/// The texture of one channel.
pub(crate) fn channel_texture(spectrum: &Spectrum) -> Texture {
    let bank = CochlearBank::new(spectrum.sample_rate());
    let shapes = bank.shapes();
    let power_filters = modulation_power_filters();
    // C1 uses the octave bands from the second on.
    let octave_filters: Vec<_> = c1_filters().into_iter().skip(1).collect();

    let envelopes: Vec<Vec<f64>> = shapes
        .iter()
        .map(|shape| {
            let extent = bank.extent(*shape);
            band_envelope(
                spectrum,
                extent.low,
                extent.high,
                |frequency| bank.response(*shape, frequency),
                COMPRESSION,
            )
        })
        .collect();
    let length = envelopes.first().map_or(0, Vec::len);
    let usable = length >= SHORTEST_ENVELOPE;
    let weights = window(length);

    let all_moments: Vec<_> = envelopes
        .iter()
        .map(|envelope| moments(envelope, &weights))
        .collect();
    let mut octave_bands: Vec<Vec<Vec<f64>>> = Vec::with_capacity(shapes.len());
    let mut subbands = Vec::with_capacity(shapes.len());
    for ((shape, envelope), found) in shapes.iter().zip(&envelopes).zip(&all_moments) {
        let extent = bank.extent(*shape);
        let (modulation, octaves) = if usable {
            let bands = ModulationBands::of(envelope);
            (
                power_filters
                    .iter()
                    .map(|filter| modulation_power(&bands.band(filter), &weights, found.variance))
                    .collect(),
                octave_filters
                    .iter()
                    .map(|filter| bands.band(filter))
                    .collect(),
            )
        } else {
            (vec![0.0; power_filters.len()], Vec::new())
        };
        octave_bands.push(octaves);
        subbands.push(Subband {
            low_hertz: extent.low,
            centre_hertz: extent.centre,
            high_hertz: extent.high,
            mean: found.mean,
            variance_over_mean_squared: found.variance_over_mean_squared,
            skewness: found.skewness,
            kurtosis: found.kurtosis,
            modulation_power: modulation,
        });
    }

    let correlations = CORRELATION_OFFSETS
        .iter()
        .map(|&offset| SubbandCorrelation {
            offset,
            values: (0..shapes.len().saturating_sub(offset))
                .map(|first| {
                    envelope_correlation(
                        &envelopes[first],
                        &all_moments[first],
                        &envelopes[first + offset],
                        &all_moments[first + offset],
                        &weights,
                    )
                })
                .collect(),
        })
        .collect();

    let c1 = octave_filters
        .iter()
        .enumerate()
        .map(|(band, filter)| {
            let pairs = |offset: usize| -> Vec<f64> {
                (0..shapes.len().saturating_sub(offset))
                    .map(|first| {
                        match (
                            octave_bands[first].get(band),
                            octave_bands[first + offset].get(band),
                        ) {
                            (Some(a), Some(b)) => band_correlation(a, b, &weights),
                            _ => 0.0,
                        }
                    })
                    .collect()
            };
            ModulationCorrelation {
                modulation_centre_hertz: filter.centre,
                neighbours: pairs(1),
                second_neighbours: pairs(2),
            }
        })
        .collect();

    Texture {
        subbands,
        modulation_centres_hertz: power_filters.iter().map(|filter| filter.centre).collect(),
        correlations,
        c1,
    }
}

/// The weighted mean of several channels' textures, statistic by statistic.
/// The weights are the channels' powers, so a silent channel (quad's centre)
/// counts for nothing and a loud one most.
pub(crate) fn average(textures: &[(Texture, f64)]) -> Texture {
    let total: f64 = textures.iter().map(|(_, weight)| weight).sum();
    let shares: Vec<f64> = textures
        .iter()
        .map(|(_, weight)| {
            if total > 0.0 {
                weight / total
            } else {
                1.0 / textures.len() as f64
            }
        })
        .collect();
    let mix = |value: &dyn Fn(&Texture) -> f64| -> f64 {
        textures
            .iter()
            .zip(&shares)
            .map(|((texture, _), share)| share * value(texture))
            .sum()
    };
    let first = &textures[0].0;
    Texture {
        subbands: first
            .subbands
            .iter()
            .enumerate()
            .map(|(index, subband)| Subband {
                low_hertz: subband.low_hertz,
                centre_hertz: subband.centre_hertz,
                high_hertz: subband.high_hertz,
                mean: mix(&|texture| texture.subbands[index].mean),
                variance_over_mean_squared: mix(&|texture| {
                    texture.subbands[index].variance_over_mean_squared
                }),
                skewness: mix(&|texture| texture.subbands[index].skewness),
                kurtosis: mix(&|texture| texture.subbands[index].kurtosis),
                modulation_power: (0..subband.modulation_power.len())
                    .map(|band| mix(&|texture| texture.subbands[index].modulation_power[band]))
                    .collect(),
            })
            .collect(),
        modulation_centres_hertz: first.modulation_centres_hertz.clone(),
        correlations: first
            .correlations
            .iter()
            .enumerate()
            .map(|(index, correlation)| SubbandCorrelation {
                offset: correlation.offset,
                values: (0..correlation.values.len())
                    .map(|pair| mix(&|texture| texture.correlations[index].values[pair]))
                    .collect(),
            })
            .collect(),
        c1: first
            .c1
            .iter()
            .enumerate()
            .map(|(index, band)| ModulationCorrelation {
                modulation_centre_hertz: band.modulation_centre_hertz,
                neighbours: (0..band.neighbours.len())
                    .map(|pair| mix(&|texture| texture.c1[index].neighbours[pair]))
                    .collect(),
                second_neighbours: (0..band.second_neighbours.len())
                    .map(|pair| mix(&|texture| texture.c1[index].second_neighbours[pair]))
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{Noise, RATE};

    fn modulated_noise(rate_hertz: f64, seconds: f64, seed: u64) -> Vec<f32> {
        Noise::new(seed)
            .white(0.1, seconds)
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                let time = index as f64 / RATE as f64;
                let gain = 1.0 + (std::f64::consts::TAU * rate_hertz * time).sin();
                (*sample as f64 * gain) as f32
            })
            .collect()
    }

    /// The modulation filter whose centre is nearest `rate` on a log scale.
    fn nearest_filter(centres: &[f64], rate: f64) -> usize {
        (0..centres.len())
            .min_by(|a, b| {
                (centres[*a] / rate)
                    .ln()
                    .abs()
                    .total_cmp(&(centres[*b] / rate).ln().abs())
            })
            .unwrap()
    }

    /// The Hilbert envelope of Gaussian noise in a narrow band is Rayleigh
    /// distributed, whose variance over squared mean is 4/π − 1 = 0.2732.
    /// The subband centred nearest 300 Hz is 130 Hz wide, so the 200 Hz
    /// lowpass keeps its envelope whole.
    #[test]
    fn a_noise_envelope_is_rayleigh() {
        let noise = Noise::new(11).white(0.1, 30.0);
        let spectrum = Spectrum::of(&noise, RATE);
        let bank = CochlearBank::new(RATE);
        let shape = bank
            .shapes()
            .into_iter()
            .min_by(|a, b| {
                (bank.extent(*a).centre - 300.0)
                    .abs()
                    .total_cmp(&(bank.extent(*b).centre - 300.0).abs())
            })
            .unwrap();
        let extent = bank.extent(shape);
        let envelope = band_envelope(
            &spectrum,
            extent.low,
            extent.high,
            |frequency| bank.response(shape, frequency),
            1.0,
        );
        let found = moments(&envelope, &window(envelope.len()));
        let expected = 4.0 / std::f64::consts::PI - 1.0;
        assert!(
            (found.variance_over_mean_squared - expected).abs() <= 0.03,
            "{} against {expected}",
            found.variance_over_mean_squared
        );
    }

    /// Noise whose amplitude swings fully at a rate puts the most modulation
    /// power in the filter centred nearest that rate, in every subband wide
    /// enough to carry the modulation (1 to 8 kHz).
    #[test]
    fn modulation_power_peaks_at_the_modulation_rate() {
        for rate in [4.0, 64.0] {
            let texture = channel_texture(&Spectrum::of(&modulated_noise(rate, 20.0, 5), RATE));
            let expected = nearest_filter(&texture.modulation_centres_hertz, rate);
            for subband in texture
                .subbands
                .iter()
                .filter(|subband| (1000.0..=8000.0).contains(&subband.centre_hertz))
            {
                let largest = (0..subband.modulation_power.len())
                    .max_by(|a, b| {
                        subband.modulation_power[*a].total_cmp(&subband.modulation_power[*b])
                    })
                    .unwrap();
                assert_eq!(
                    largest, expected,
                    "{rate} Hz modulation, subband at {:.0} Hz: {:?}",
                    subband.centre_hertz, subband.modulation_power
                );
            }
        }
    }

    /// Subbands two apart do not overlap, so in white noise their envelopes
    /// are independent: C and C1 near zero, within the scatter of an
    /// estimate.
    ///
    /// One amplitude swing m = 1 + sin(2π·2t) shared by every subband makes
    /// them move together. By hand, each compressed envelope is m^0.3·R^0.3
    /// with R Rayleigh and independent between the subbands, so
    /// C = var(m^0.3)·E[R^0.3]² / (E[m^0.6]·E[R^0.6] − E[m^0.3]²·E[R^0.3]²).
    /// With E[m^0.3] = 0.9011, E[m^0.6] = 0.9105, E[R^0.3] = 2^0.15·Γ(1.15)
    /// and E[R^0.6] = 2^0.3·Γ(1.3) (σ = 1) that is 0.778; the 200 Hz lowpass
    /// can only raise it, by smoothing R in subbands wider than 200 Hz.
    /// C1 in the 3.125 Hz band (0.9 to 5.3 Hz) holds the swing and almost
    /// none of R's fluctuation, which spreads over the subband's width: near
    /// one.
    #[test]
    fn correlations_follow_shared_modulation() {
        let steady = channel_texture(&Spectrum::of(&Noise::new(7).white(0.1, 20.0), RATE));
        let swinging = channel_texture(&Spectrum::of(&modulated_noise(2.0, 20.0, 7), RATE));
        let mid = 8..24;
        let offset_two = |texture: &Texture| texture.correlations[1].values[mid.clone()].to_vec();
        for value in offset_two(&steady) {
            assert!(value.abs() < 0.1, "steady C {value}");
        }
        for value in offset_two(&swinging) {
            assert!(value > 0.778 - 0.05, "swinging C {value}");
        }
        // A correlation estimated over T seconds of a band B Hz wide scatters
        // by about 1/√(B·T): the 50 Hz octave band (B ≈ 35 Hz) over 20 s, by
        // 0.04, so each value stays within 0.15 and their mean within 0.05.
        let steady_c1 = &steady.c1[4];
        assert_eq!(steady_c1.modulation_centre_hertz, 50.0);
        for value in &steady_c1.second_neighbours[mid.clone()] {
            assert!(value.abs() < 0.15, "steady C1 {value}");
        }
        let average = crate::statistics::mean(&steady_c1.second_neighbours[mid.clone()]);
        assert!(average.abs() < 0.05, "steady C1 mean {average}");
        for value in &swinging.c1[0].second_neighbours[mid.clone()] {
            assert!(*value > 0.9, "swinging C1 {value}");
        }
    }
}
