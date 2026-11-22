//! Averaging a spectrum over a third octave, as the ear averages it.
//!
//! The ear resolves frequency in bands about a third of an octave wide, so
//! what a head does is judged, and smoothed, over the third octave around
//! each frequency: the bins from f / 2^(1/6) to f · 2^(1/6).

/// Running sums of one spectrum, so the sum over any band is two lookups.
pub(super) struct RunningSum {
    sums: Vec<f64>,
}

impl RunningSum {
    pub(super) fn new(values: &[f64]) -> Self {
        let mut sums = Vec::with_capacity(values.len() + 1);
        sums.push(0.0);
        for value in values {
            sums.push(sums.last().copied().unwrap_or(0.0) + value);
        }
        Self { sums }
    }

    /// The sum over the bins `lowest..highest`.
    pub(super) fn over(&self, (lowest, highest): (usize, usize)) -> f64 {
        self.sums[highest] - self.sums[lowest]
    }
}

/// The bins `lowest..highest` of the third octave around `centre` Hz, on a
/// grid of `bins` bins `bin_hz` apart; never empty, never past the last bin.
pub(super) fn third_octave_bins(centre: f64, bin_hz: f64, bins: usize) -> (usize, usize) {
    let half_band = 2f64.powf(1.0 / 6.0);
    let lowest = ((centre / half_band / bin_hz).ceil() as usize).min(bins - 1);
    let highest = ((centre * half_band / bin_hz).floor() as usize).clamp(lowest, bins - 1) + 1;
    (lowest, highest)
}
