//! Fitting one ear's shelf to the sphere.
//!
//! The gain at zero frequency is the sphere's own, from its closed form; the
//! fit only chooses the high gain and the corner. It minimises the squared
//! error in decibels over [`LOWEST_HZ`] up to [`HIGHEST_HZ`], log-spaced, by
//! Levenberg–Marquardt: two unknowns, a smooth error, a good first guess.
//! Read back through the table and run as the filter at 48 kHz, it is within
//! 0.81 dB of the sphere from 100 Hz to 8 kHz at every distance and angle the
//! tests try, down to 12 cm; the worst is at 90°, 6.4 kHz, nearest.

use super::shelf::analogue_magnitude;

/// The band the shelf is fitted over (Hz). Spagnol et al. fit up to 15 kHz;
/// above 16 kHz the pinna, not the head, shapes what an ear hears.
const LOWEST_HZ: f64 = 50.0;
const HIGHEST_HZ: f64 = 16_000.0;
/// How many frequencies the band is sampled at.
pub(super) const POINTS: usize = 40;
/// The corner is kept between these (Hz).
const LOWEST_CORNER_HZ: f64 = 20.0;
const HIGHEST_CORNER_HZ: f64 = 40_000.0;
const ITERATIONS: usize = 60;

/// The frequencies the sphere is sampled at for a fit.
pub(super) fn frequencies() -> [f64; POINTS] {
    let ratio = (HIGHEST_HZ / LOWEST_HZ).ln() / (POINTS - 1) as f64;
    std::array::from_fn(|index| LOWEST_HZ * (ratio * index as f64).exp())
}

/// The high gain (dB) and the corner (Hz) that bring a shelf whose low gain
/// is `low_decibels` closest to `target_decibels`, sampled at `frequencies`.
pub(super) fn fit_shelf(
    low_decibels: f64,
    frequencies: &[f64; POINTS],
    target_decibels: &[f64; POINTS],
) -> (f64, f64) {
    let highest_corner = HIGHEST_CORNER_HZ;
    let low = decibels_to_gain(low_decibels);
    let error = |high_decibels: f64, log_corner: f64, frequency: f64, target: f64| {
        let shelf = analogue_magnitude(
            low,
            decibels_to_gain(high_decibels),
            log_corner.exp(),
            frequency,
        );
        20.0 * shelf.log10() - target
    };
    let cost = |high_decibels: f64, log_corner: f64| -> f64 {
        frequencies
            .iter()
            .zip(target_decibels)
            .map(|(&frequency, &target)| {
                error(high_decibels, log_corner, frequency, target).powi(2)
            })
            .sum()
    };

    // First guess: the top of the band for the high gain, and the corner
    // where the target is halfway between the two gains.
    let mut high_decibels = target_decibels[POINTS - 1];
    let halfway = (low_decibels + high_decibels) / 2.0;
    let crossing = target_decibels
        .windows(2)
        .position(|pair| (pair[0] - halfway) * (pair[1] - halfway) <= 0.0)
        .map_or(1000.0, |index| frequencies[index]);
    let mut log_corner = crossing.clamp(LOWEST_CORNER_HZ, highest_corner).ln();

    let mut current = cost(high_decibels, log_corner);
    let mut damping = 1e-3;
    let nudge = 1e-5;
    for _ in 0..ITERATIONS {
        // Gauss–Newton normal equations, Jacobian by central differences.
        let (mut jtj, mut jtr) = ([[0.0f64; 2]; 2], [0.0f64; 2]);
        for (&frequency, &target) in frequencies.iter().zip(target_decibels) {
            let residual = error(high_decibels, log_corner, frequency, target);
            let by_high = (error(high_decibels + nudge, log_corner, frequency, target)
                - error(high_decibels - nudge, log_corner, frequency, target))
                / (2.0 * nudge);
            let by_corner = (error(high_decibels, log_corner + nudge, frequency, target)
                - error(high_decibels, log_corner - nudge, frequency, target))
                / (2.0 * nudge);
            let row = [by_high, by_corner];
            for a in 0..2 {
                jtr[a] += row[a] * residual;
                for b in 0..2 {
                    jtj[a][b] += row[a] * row[b];
                }
            }
        }
        let mut improved = false;
        while damping < 1e8 {
            let a = jtj[0][0] * (1.0 + damping) + 1e-12;
            let d = jtj[1][1] * (1.0 + damping) + 1e-12;
            let (b, c) = (jtj[0][1], jtj[1][0]);
            let determinant = a * d - b * c;
            let step_high = -(d * jtr[0] - b * jtr[1]) / determinant;
            let step_corner = -(a * jtr[1] - c * jtr[0]) / determinant;
            let next_high = high_decibels + step_high;
            let next_corner =
                (log_corner + step_corner).clamp(LOWEST_CORNER_HZ.ln(), highest_corner.ln());
            let next = cost(next_high, next_corner);
            if next < current {
                (high_decibels, log_corner, current) = (next_high, next_corner, next);
                damping = (damping / 3.0).max(1e-9);
                improved = true;
                break;
            }
            damping *= 4.0;
        }
        if !improved {
            break;
        }
    }
    (high_decibels, log_corner.exp())
}

pub(super) fn decibels_to_gain(decibels: f64) -> f64 {
    10f64.powf(decibels / 20.0)
}
