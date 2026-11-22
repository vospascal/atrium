//! The headphone equaliser: each ear's diffuse-field response, inverted.
//!
//! The measured responses carry, besides what depends on direction, what
//! every direction shares: the measurement loudspeaker's response and the
//! dummy head's ear canal, measured at its eardrum. On headphones the
//! listener's own ear canal adds its resonance again, so a sound would be
//! heard through two ear canals. Dividing every direction's response by the
//! set's diffuse-field response, the power average over all directions,
//! takes out exactly what all directions share and leaves what makes them
//! differ: Møller's transfer function "referenced to diffuse field"
//! ("Fundamentals of binaural technology", Applied Acoustics 36, 1992,
//! §2.2, eq. 9), the equalisation Larcher, Jot and Vandernoot recommend for
//! binaural synthesis (AES 105th Convention, 1998, preprint 4858), and the
//! way the set's own authors equalised its compact version (Gardner and
//! Martin, KEMAR FAQ, Q14). Because it is one filter per ear, the same for
//! every direction, it is applied once to the sum of all point sources.
//!
//! The inverse is of the third-octave smoothed response (Møller's
//! reference is a magnitude only, so it is realised minimum-phase, his
//! p. 198), with three limits on what it may do:
//!
//! - **Held below 100 Hz.** Each measured response is 512 samples at
//!   44.1 kHz, 11.6 ms, and resolves nothing finer than 86 Hz. Below that
//!   the set's diffuse response stops being acoustics: it falls 13 dB an
//!   octave with the loudspeaker's bass down to 40 Hz, then lies flat at
//!   −33 dB, which is the response's truncation, not a sound. 100 Hz is the
//!   lowest third octave above that resolution; below it the correction is
//!   held at its value there.
//! - **Held above 16 kHz.** From 12.7 to 16 kHz the set's response falls
//!   9 dB in one third octave, more steeply than anywhere else, and how much
//!   of that is the head and how much the tweeter and the probe microphone
//!   is not known; resampling ends it at 21 kHz. The set's authors held
//!   their own inverse of this loudspeaker above 15 kHz (FAQ, Q15; the
//!   documentation says 18 kHz); 16 kHz is the third octave between.
//! - **Never more than 20 dB above its typical gain.** The regularised
//!   inverse of Kirkeby and Nelson ("Digital filter design for inversion
//!   problems in sound reproduction", JAES 47, 1999), C* / (C* C + β), can
//!   never exceed 1/(2√β). β is set from that cap, so the regularisation is
//!   the cap and nothing else: 20 dB, as the set's authors clipped their own
//!   inverse (FAQ, Q15), above the equaliser's geometric-mean gain from
//!   100 Hz to 16 kHz. Within that range it changes the inverse by at most
//!   0.21 dB, at 100 Hz, where the set has least energy; it never reaches
//!   the cap there. Below that much energy the regularised inverse falls
//!   again instead of filling a hole.
//!
//! The level is set by the target: a diffuse field reaches each ear at unit
//! gain, as loud as the mono signal that made it.

use super::minimum_phase::{minimum_phase_spectrum, to_time, truncated_with_fade_out};
use super::statistics::DiffuseStatistics;
use super::third_octave::{third_octave_bins, RunningSum};

/// Below this the correction is held: the lowest third octave above the
/// 86 Hz that a 512-sample response at 44.1 kHz resolves.
pub(in crate::pipeline::renderers::binaural) const HELD_BELOW_HZ: f64 = 100.0;

/// Above this the correction is held: where the set's response falls
/// faster than anywhere else, into the end of what was measured.
pub(in crate::pipeline::renderers::binaural) const HELD_ABOVE_HZ: f64 = 16_000.0;

/// The most the equaliser may lift any frequency above its typical gain,
/// in dB: the set's authors' own clip when they inverted its loudspeaker.
pub(in crate::pipeline::renderers::binaural) const LARGEST_BOOST_DB: f64 = 20.0;

/// How long the equaliser is. Its magnitude is smooth, so its minimum-phase
/// response is short: against one twice as long, its third-octave response
/// differs by at most 0.12 dB, at 100 Hz, and by less everywhere above.
const EQUALISER_SECONDS: f64 = 0.02;

/// The last part of the equaliser fades out over this.
const FADE_OUT_SECONDS: f64 = 0.002;

/// Each ear's equaliser, `EQUALISER_SECONDS` long at the sample rate it was
/// designed for.
pub(in crate::pipeline::renderers::binaural) struct EqualiserFilters {
    pub(in crate::pipeline::renderers::binaural) left: Vec<f32>,
    pub(in crate::pipeline::renderers::binaural) right: Vec<f32>,
}

/// Each ear's diffuse-field power, averaged over the third octave around
/// every bin and held outside `HELD_BELOW_HZ` to `HELD_ABOVE_HZ`: what the
/// equaliser divides out.
pub(in crate::pipeline::renderers::binaural) fn held_smoothed_power(
    power: &[f64],
    bin_hz: f64,
) -> Vec<f64> {
    let sums = RunningSum::new(power);
    let bins = power.len();
    (0..bins)
        .map(|bin| {
            let centre = (bin as f64 * bin_hz).clamp(HELD_BELOW_HZ, HELD_ABOVE_HZ);
            let band = third_octave_bins(centre, bin_hz, bins);
            sums.over(band) / (band.1 - band.0) as f64
        })
        .collect()
}

/// Kirkeby and Nelson's β for both ears: the regularisation whose largest
/// possible gain, 1/(2√β), is `LARGEST_BOOST_DB` above the inverse's
/// geometric-mean gain over the third octaves from `HELD_BELOW_HZ` to
/// `HELD_ABOVE_HZ`. One β for both ears, so it cannot make them differ.
pub(in crate::pipeline::renderers::binaural) fn regularisation(
    smoothed: [&[f64]; 2],
    bin_hz: f64,
) -> f64 {
    let lowest_band = (3.0 * (HELD_BELOW_HZ / 1000.0).log2()).round() as i32;
    let highest_band = (3.0 * (HELD_ABOVE_HZ / 1000.0).log2()).round() as i32;
    let mut log_sum = 0.0;
    let mut count = 0;
    for band in lowest_band..=highest_band {
        let centre = 1000.0 * 2f64.powf(band as f64 / 3.0);
        let bin = ((centre / bin_hz).round() as usize).min(smoothed[0].len() - 1);
        for ear in smoothed {
            log_sum += ear[bin].ln();
            count += 1;
        }
    }
    let typical_power = (log_sum / count as f64).exp();
    let cap = 10f64.powf(LARGEST_BOOST_DB / 20.0);
    typical_power / (4.0 * cap * cap)
}

/// The power of the regularised inverse of `smoothed`: |C|² / (|C|² + β)².
fn inverse_power(smoothed: &[f64], regularisation: f64) -> Vec<f64> {
    smoothed
        .iter()
        .map(|&power| power / (power + regularisation).powi(2))
        .collect()
}

/// Each ear's equaliser at `sample_rate` from the diffuse-field
/// `statistics`, measured on a grid of `grid_size` bins.
pub(in crate::pipeline::renderers::binaural) fn design_equaliser(
    statistics: &DiffuseStatistics,
    sample_rate: f32,
    grid_size: usize,
) -> EqualiserFilters {
    let bin_hz = sample_rate as f64 / grid_size as f64;
    let left = held_smoothed_power(&statistics.left_power, bin_hz);
    let right = held_smoothed_power(&statistics.right_power, bin_hz);
    let regularisation = regularisation([&left, &right], bin_hz);
    let length = (EQUALISER_SECONDS * sample_rate as f64).round() as usize;
    let fade_length = (FADE_OUT_SECONDS * sample_rate as f64).round() as usize;
    let ear = |smoothed: &[f64]| {
        let spectrum = minimum_phase_spectrum(&inverse_power(smoothed, regularisation), grid_size);
        truncated_with_fade_out(to_time(spectrum, grid_size), length, fade_length)
    };
    EqualiserFilters {
        left: ear(&left),
        right: ear(&right),
    }
}
