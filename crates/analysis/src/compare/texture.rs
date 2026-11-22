//! Texture: McDermott and Simoncelli's (2011) statistic classes, each
//! compared by the error measure the paper judged its synthesis with
//! (Experimental Procedures, "Convergence"): SNR = 10·log10(Σ s² /
//! Σ (s − s′)²) over the class's statistics, s the original's and s′ the
//! synthetic's. Here s is the reference's. They imposed statistics until each
//! class reached 30 dB and counted 20 dB as converged.

use super::comparison::{
    ClassDifference, Difference, OffsetDifference, Place, RateDifference, TextureClass,
    TextureDifferences,
};
use super::Compared;
use crate::report::{Report, Texture};
use crate::statistics::{mean, power_decibels};
use crate::texture::COMPRESSION;

/// The paper's threshold of convergence (dB).
pub(crate) const CONVERGED_DECIBELS: f64 = 20.0;

/// How far below its scatter floor a class may lie and still count as
/// within it (dB). The floor is itself estimated, from one pair of halves:
/// between two 30 s takes of one white noise each class's SNR lay from
/// 1.2 dB below its floor to 3.4 dB above it.
const SCATTER_MARGIN_DECIBELS: f64 = 3.0;

/// How many places a class names.
const PLACES: usize = 3;

pub(super) fn compare(reference: &Compared, other: &Compared) -> TextureDifferences {
    let (first, second) = (&reference.report.texture, &other.report.texture);
    let level_removed_decibels = level_difference(reference.report, other.report);
    let classes = TextureClass::ALL
        .iter()
        .map(|class| {
            let (statistics, places) =
                class_statistics(*class, first, second, mean_scale(level_removed_decibels));
            let floor = scatter_floor(*class, &statistics, reference, other);
            judge(*class, &statistics, places, floor)
        })
        .collect();
    let first_spectrum = modulation_spectrum(first);
    let second_spectrum = modulation_spectrum(second);
    TextureDifferences {
        classes,
        level_removed_decibels,
        modulation_spectrum: first
            .modulation_centres_hertz
            .iter()
            .zip(first_spectrum.iter().zip(&second_spectrum))
            .map(|(rate, (one, two))| RateDifference {
                modulation_hertz: *rate,
                values: Difference::of(*one, *two),
            })
            .collect(),
        correlation_by_offset: first
            .correlations
            .iter()
            .filter_map(|correlation| {
                Some(OffsetDifference {
                    offset: correlation.offset,
                    values: Difference::of(
                        correlation_at_offset(first, correlation.offset)?,
                        correlation_at_offset(second, correlation.offset)?,
                    ),
                })
            })
            .collect(),
        modulation_correlation_by_band: first
            .c1
            .iter()
            .zip(&second.c1)
            .map(|(one, two)| RateDifference {
                modulation_hertz: one.modulation_centre_hertz,
                values: Difference::of(mean(&one.neighbours), mean(&two.neighbours)),
            })
            .collect(),
    }
}

/// The overall level difference, other minus reference (dB), as the
/// envelopes see it.
fn level_difference(reference: &Report, other: &Report) -> f64 {
    full_scale_leq(other) - full_scale_leq(reference)
}

/// What the other file's envelope means are multiplied by to take out a
/// level difference: a gain g on the samples scales every compressed
/// envelope, and so its mean, by g^COMPRESSION.
fn mean_scale(level_difference_decibels: f64) -> f64 {
    10f64.powf(-COMPRESSION * level_difference_decibels / 20.0)
}

/// The SNR the class would reach between two independent takes of the
/// reference's sound, as long as these two files.
///
/// A statistic estimated over T seconds scatters about its true value with a
/// variance c/T. Between the reference's halves the squared error is then
/// c/(T/2) + c/(T/2) = 4c/T summed over the class; between two whole takes
/// of T and T′ seconds it is c/T + c/T′, the halves' error times
/// (1 + T/T′)/4. The other file is taken to scatter as the reference does:
/// the reference says what the sound is, and only the reference's halves are
/// needed, so a file read from its report JSON compares as itself. The
/// signal is the reference's whole Σ s², as in the SNR itself.
fn scatter_floor(
    class: TextureClass,
    statistics: &[Statistic],
    reference: &Compared,
    other: &Compared,
) -> Option<f64> {
    let [first, second] = reference.halves?;
    let scale = mean_scale(level_difference(first, second));
    let (halves, _) = class_statistics(class, &first.texture, &second.texture, scale);
    let lengths = reference.report.signal.seconds / other.report.signal.seconds;
    let expected = squared_error(&halves) * (1.0 + lengths) / 4.0;
    let signal: f64 = statistics.iter().map(|found| found.reference.powi(2)).sum();
    (expected > 0.0).then(|| power_decibels(signal / expected))
}

fn squared_error(statistics: &[Statistic]) -> f64 {
    statistics
        .iter()
        .map(|found| (found.reference - found.other).powi(2))
        .sum()
}

/// The file's unweighted Leq in dBFS, whatever its report's unit: the
/// envelopes are taken from the samples, so it is the samples' level that
/// moves them.
fn full_scale_leq(report: &Report) -> f64 {
    report.level.overall.leq - report.signal.full_scale_db_spl.unwrap_or(0.0)
}

/// One statistic of a class: the reference's value, the other's, and which
/// place it belongs to.
struct Statistic {
    reference: f64,
    other: f64,
    place: usize,
}

/// A place as the summary shows it: its hertz, its partner's, and the
/// values there.
type Shown = (f64, Option<f64>, Difference);

/// A class's statistics, both files', and the places they are grouped into
/// (each place's hertz, partner and the values it is shown with). Only what
/// both files have is compared.
fn class_statistics(
    class: TextureClass,
    first: &Texture,
    second: &Texture,
    mean_scale: f64,
) -> (Vec<Statistic>, Vec<Shown>) {
    let subbands: Vec<_> = first.subbands.iter().zip(&second.subbands).collect();
    let per_subband = |value: &dyn Fn(&crate::report::Subband) -> f64, scale: f64| {
        let statistics = subbands
            .iter()
            .enumerate()
            .map(|(place, (one, two))| Statistic {
                reference: value(one),
                other: value(two) * scale,
                place,
            })
            .collect();
        let places = subbands
            .iter()
            .map(|(one, two)| {
                (
                    one.centre_hertz,
                    None,
                    Difference::of(value(one), value(two) * scale),
                )
            })
            .collect();
        (statistics, places)
    };
    match class {
        TextureClass::EnvelopeMean => per_subband(&|subband| subband.mean, mean_scale),
        TextureClass::Variance => per_subband(&|subband| subband.variance_over_mean_squared, 1.0),
        TextureClass::Skewness => per_subband(&|subband| subband.skewness, 1.0),
        TextureClass::Kurtosis => per_subband(&|subband| subband.kurtosis, 1.0),
        TextureClass::Correlation => {
            let mut statistics = Vec::new();
            let mut places = Vec::new();
            for one in &first.correlations {
                let Some(two) = second
                    .correlations
                    .iter()
                    .find(|two| two.offset == one.offset)
                else {
                    continue;
                };
                for (pair, (reference, other)) in one.values.iter().zip(&two.values).enumerate() {
                    let (Some(lower), Some(upper)) = (
                        first.subbands.get(pair),
                        first.subbands.get(pair + one.offset),
                    ) else {
                        continue;
                    };
                    statistics.push(Statistic {
                        reference: *reference,
                        other: *other,
                        place: places.len(),
                    });
                    places.push((
                        lower.centre_hertz,
                        Some(upper.centre_hertz),
                        Difference::of(*reference, *other),
                    ));
                }
            }
            (statistics, places)
        }
        TextureClass::ModulationPower => {
            let first_spectrum = modulation_spectrum(first);
            let second_spectrum = modulation_spectrum(second);
            let statistics = subbands
                .iter()
                .flat_map(|(one, two)| {
                    one.modulation_power
                        .iter()
                        .zip(&two.modulation_power)
                        .enumerate()
                        .map(|(place, (reference, other))| Statistic {
                            reference: *reference,
                            other: *other,
                            place,
                        })
                })
                .collect();
            let places = first
                .modulation_centres_hertz
                .iter()
                .zip(first_spectrum.iter().zip(&second_spectrum))
                .map(|(rate, (one, two))| (*rate, None, Difference::of(*one, *two)))
                .collect();
            (statistics, places)
        }
        TextureClass::ModulationCorrelation => {
            let mut statistics = Vec::new();
            let mut places = Vec::new();
            for (place, (one, two)) in first.c1.iter().zip(&second.c1).enumerate() {
                let pairs = one
                    .neighbours
                    .iter()
                    .zip(&two.neighbours)
                    .chain(one.second_neighbours.iter().zip(&two.second_neighbours));
                for (reference, other) in pairs {
                    statistics.push(Statistic {
                        reference: *reference,
                        other: *other,
                        place,
                    });
                }
                places.push((
                    one.modulation_centre_hertz,
                    None,
                    Difference::of(mean(&one.neighbours), mean(&two.neighbours)),
                ));
            }
            (statistics, places)
        }
    }
}

/// The class's SNR, whether it converged or lies within the scatter
/// `floor`, and where its error lies.
fn judge(
    class: TextureClass,
    statistics: &[Statistic],
    places: Vec<Shown>,
    floor: Option<f64>,
) -> ClassDifference {
    let signal: f64 = statistics.iter().map(|found| found.reference.powi(2)).sum();
    let mut place_errors = vec![0.0f64; places.len()];
    for found in statistics {
        place_errors[found.place] += (found.reference - found.other).powi(2);
    }
    let error: f64 = place_errors.iter().sum();
    let snr_decibels = (error > 0.0).then(|| power_decibels(signal / error));
    let mut order: Vec<usize> = (0..places.len())
        .filter(|place| place_errors[*place] > 0.0)
        .collect();
    order.sort_by(|a, b| place_errors[*b].total_cmp(&place_errors[*a]));
    ClassDifference {
        class,
        statistics: statistics.len(),
        snr_decibels,
        converged: snr_decibels.is_none_or(|snr| snr >= CONVERGED_DECIBELS),
        scatter_floor_decibels: floor,
        within_scatter: floor.is_some_and(|floor| {
            snr_decibels.is_none_or(|snr| snr >= floor - SCATTER_MARGIN_DECIBELS)
        }),
        largest: order
            .into_iter()
            .take(PLACES)
            .map(|place| {
                let (hertz, partner_hertz, values) = places[place];
                Place {
                    hertz,
                    partner_hertz,
                    values,
                    share_of_error: place_errors[place] / error,
                }
            })
            .collect(),
    }
}

/// The modulation power at each filter, the mean over the subbands that
/// sound (those above Nyquist read zero and would pull it down), as the
/// analyser's summary gives it.
pub(crate) fn modulation_spectrum(texture: &Texture) -> Vec<f64> {
    let sounding: Vec<_> = texture
        .subbands
        .iter()
        .filter(|subband| subband.mean > 0.0)
        .collect();
    (0..texture.modulation_centres_hertz.len())
        .map(|filter| {
            mean(
                &sounding
                    .iter()
                    .map(|subband| subband.modulation_power[filter])
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// C at `offset`, the mean over every pair; None where it is not reported.
pub(crate) fn correlation_at_offset(texture: &Texture, offset: usize) -> Option<f64> {
    texture
        .correlations
        .iter()
        .find(|correlation| correlation.offset == offset)
        .map(|correlation| mean(&correlation.values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statistic(reference: f64, other: f64, place: usize) -> Statistic {
        Statistic {
            reference,
            other,
            place,
        }
    }

    /// Statistics 3 and 4 against 3 and 3.4: Σ s² = 25, Σ (s − s′)² = 0.36,
    /// so 10·log10(25 / 0.36) = 18.42 dB, short of 20; all the error at the
    /// second place.
    #[test]
    fn the_snr_is_the_papers() {
        let places = vec![
            (100.0, None, Difference::of(3.0, 3.0)),
            (200.0, None, Difference::of(4.0, 3.4)),
        ];
        let judged = judge(
            TextureClass::Variance,
            &[statistic(3.0, 3.0, 0), statistic(4.0, 3.4, 1)],
            places,
            None,
        );
        let expected = 10.0 * (25.0f64 / 0.36).log10();
        assert!((judged.snr_decibels.unwrap() - expected).abs() < 1e-9);
        assert!((expected - 18.42).abs() < 0.01);
        assert!(!judged.converged);
        assert_eq!(judged.largest.len(), 1);
        assert_eq!(judged.largest[0].hertz, 200.0);
        assert!((judged.largest[0].share_of_error - 1.0).abs() < 1e-12);
    }

    #[test]
    fn identical_statistics_have_no_snr() {
        let judged = judge(
            TextureClass::Skewness,
            &[statistic(0.5, 0.5, 0)],
            vec![(100.0, None, Difference::of(0.5, 0.5))],
            Some(10.0),
        );
        assert_eq!(judged.snr_decibels, None);
        assert!(judged.converged && judged.within_scatter);
        assert!(judged.largest.is_empty());
    }
}
