//! Every ear correction the renderer can ask for, worked out once.
//!
//! The sphere's series takes some tens of terms per frequency and the fit a
//! few dozen frequencies, too much to redo for every source every buffer. So
//! the corrections are fitted on a grid once, the first time a renderer is
//! set up, and the renderer reads between grid points: every
//! 5° of incidence, and 40 distances spaced evenly in their logarithm from
//! the nearest the sphere is taken at to the distance the HRIRs were measured
//! at. Gains are read between in decibels and the corner in its logarithm.
//!
//! Spagnol et al. publish rational functions of distance, one set per 10° of
//! incidence, for the same three parameters. Their table is not used: checked
//! against the sphere, the 90° row's q11 carries a sign that puts its
//! zero-frequency gain 0.75 dB off at ρ = 1.25 where its neighbours are within
//! 0.005 dB, the 120° high-gain row has a pole at ρ ≈ 1.96 inside the range,
//! the unit of the corner is not stated, and the coefficients are fitted
//! against a source infinitely far, where these must be relative to 1.4 m.
//! Fitting the sphere directly avoids all four.

use super::fit::{self, decibels_to_gain};
use super::shelf::EarCorrection;
use super::sphere::{response_at_zero_frequency, response_magnitude};
use super::{NEAREST_NORMALISED_DISTANCE, SPEED_OF_SOUND};

const INCIDENCE_STEP_DEGREES: usize = 5;
const INCIDENCES: usize = 180 / INCIDENCE_STEP_DEGREES + 1;
const DISTANCES: usize = 40;

/// One grid point, in the forms that are read between.
#[derive(Clone, Copy, Debug)]
struct Fitted {
    low_decibels: f32,
    high_decibels: f32,
    log_corner: f32,
}

/// The fitted corrections for one head and one measurement distance.
pub(crate) struct NearFieldTable {
    nearest_distance: f32,
    reference_distance: f32,
    log_nearest: f32,
    log_step: f32,
    /// `DISTANCES` per incidence, incidence-major.
    grid: Vec<Fitted>,
}

impl NearFieldTable {
    /// Fit the corrections for a head of `head_radius` (m) whose HRIRs were
    /// measured `reference_distance` (m) from its centre. Some tens of
    /// milliseconds; never call it on the audio thread.
    pub(crate) fn new(head_radius: f32, reference_distance: f32) -> Self {
        let radius = head_radius as f64;
        let nearest = NEAREST_NORMALISED_DISTANCE as f64;
        let reference = (reference_distance as f64 / radius).max(nearest);
        let frequencies = fit::frequencies();
        let normalised = frequencies.map(|hz| std::f64::consts::TAU * hz * radius / SPEED_OF_SOUND);
        let log_step = (reference / nearest).ln() / (DISTANCES - 1) as f64;

        let mut grid = Vec::with_capacity(INCIDENCES * DISTANCES);
        for incidence_index in 0..INCIDENCES {
            let incidence = ((incidence_index * INCIDENCE_STEP_DEGREES) as f64).to_radians();
            let reference_decibels = normalised
                .map(|frequency| decibels(response_magnitude(reference, frequency, incidence)));
            let reference_at_zero = response_at_zero_frequency(reference, incidence);
            for distance_index in 0..DISTANCES {
                let distance = if distance_index == DISTANCES - 1 {
                    reference
                } else {
                    nearest * (log_step * distance_index as f64).exp()
                };
                let low_decibels =
                    decibels(response_at_zero_frequency(distance, incidence) / reference_at_zero);
                let target: [f64; fit::POINTS] = std::array::from_fn(|index| {
                    decibels(response_magnitude(distance, normalised[index], incidence))
                        - reference_decibels[index]
                });
                let (high_decibels, corner_hz) =
                    fit::fit_shelf(low_decibels, &frequencies, &target);
                grid.push(Fitted {
                    low_decibels: low_decibels as f32,
                    high_decibels: high_decibels as f32,
                    log_corner: corner_hz.ln() as f32,
                });
            }
        }
        Self {
            nearest_distance: (nearest * radius) as f32,
            reference_distance: (reference * radius) as f32,
            log_nearest: (nearest * radius).ln() as f32,
            log_step: log_step as f32,
            grid,
        }
    }

    /// The correction for an ear `incidence` radians from the direction to a
    /// source `distance` metres from the head's centre: unity from the
    /// measurement distance on, and the nearest distance's for anything
    /// nearer.
    pub(crate) fn correction(&self, incidence: f32, distance: f32) -> EarCorrection {
        if distance >= self.reference_distance {
            return EarCorrection::UNITY;
        }
        let distance = distance.max(self.nearest_distance);
        let along_distance =
            ((distance.ln() - self.log_nearest) / self.log_step).clamp(0.0, (DISTANCES - 1) as f32);
        let along_incidence = (incidence.to_degrees().clamp(0.0, 180.0)
            / INCIDENCE_STEP_DEGREES as f32)
            .clamp(0.0, (INCIDENCES - 1) as f32);
        let (distance_index, distance_share) = split(along_distance, DISTANCES);
        let (incidence_index, incidence_share) = split(along_incidence, INCIDENCES);
        let at = |incidence: usize, distance: usize| self.grid[incidence * DISTANCES + distance];
        let blend = |read: fn(&Fitted) -> f32| {
            let near = read(&at(incidence_index, distance_index)) * (1.0 - distance_share)
                + read(&at(incidence_index, distance_index + 1)) * distance_share;
            let far = read(&at(incidence_index + 1, distance_index)) * (1.0 - distance_share)
                + read(&at(incidence_index + 1, distance_index + 1)) * distance_share;
            near * (1.0 - incidence_share) + far * incidence_share
        };
        EarCorrection {
            low_gain: decibels_to_gain(blend(|fitted| fitted.low_decibels) as f64) as f32,
            high_gain: decibels_to_gain(blend(|fitted| fitted.high_decibels) as f64) as f32,
            corner_hz: blend(|fitted| fitted.log_corner).exp(),
        }
    }
}

/// A position along a grid of `count` points as the lower point's index and
/// the share of the way to the next, the last pair included.
fn split(position: f32, count: usize) -> (usize, f32) {
    let index = (position.floor() as usize).min(count - 2);
    (index, position - index as f32)
}

fn decibels(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}
