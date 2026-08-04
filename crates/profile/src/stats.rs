//! Rolling sample statistics: the shared reader-side machinery behind both
//! [`crate::cpu`] spans and the [`crate::frame`] clock.
//!
//! Everything here is allocated once in [`RollingWindow::new`] and never again —
//! percentiles sort into a preallocated scratch buffer rather than collecting into
//! a fresh `Vec` each frame. This matters because the readout runs every frame.
//!
//! The reason this module exists at all, rather than the callers each keeping a
//! mean: **a mean hides oscillation.** A frame cost that alternates 4 ms / 20 ms
//! and one that sits flat at 12 ms have the same mean and feel nothing alike. So
//! the window keeps raw history and offers three views the mean cannot give:
//! percentiles (how bad is the bad case), the chronological series (draw it), and
//! [`RollingWindow::dominant_period`] (is the bad case *periodic*, and at what
//! rate).

/// A fixed-capacity ring of recent samples plus the statistics worth reading off
/// it. Overwrites oldest-first once full; never reallocates after construction.
pub struct RollingWindow {
    /// Ring storage. `samples[write]` is the next slot to be written.
    samples: Vec<f32>,
    write: usize,
    /// Number of valid samples, saturating at `samples.len()`.
    filled: usize,
    /// Preallocated sort/analysis space, so `percentile` and `dominant_period`
    /// never allocate. Length equals `samples.len()`.
    scratch: Vec<f32>,
    /// Preallocated per-lag correlation strengths for `dominant_period`. Held
    /// alongside `scratch` because the analysis needs the centred samples and
    /// the correlations at the same time.
    correlations: Vec<f32>,
}

impl RollingWindow {
    /// `capacity` is how much history to keep, in samples. For a per-frame
    /// window, 240 samples is four seconds at 60 Hz — long enough to show
    /// several cycles of anything slower than a ~2 Hz wave.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            samples: vec![0.0; capacity],
            write: 0,
            filled: 0,
            scratch: vec![0.0; capacity],
            correlations: vec![0.0; capacity],
        }
    }

    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    pub fn len(&self) -> usize {
        self.filled
    }

    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    pub fn clear(&mut self) {
        self.write = 0;
        self.filled = 0;
    }

    /// Non-finite samples are dropped rather than stored: one NaN in the ring
    /// would poison every percentile and the autocorrelation for as long as it
    /// stayed resident, and a NaN frame time is a bug elsewhere, not data.
    pub fn push(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
        self.samples[self.write] = value;
        self.write = (self.write + 1) % self.samples.len();
        self.filled = (self.filled + 1).min(self.samples.len());
    }

    /// The most recently pushed sample.
    pub fn latest(&self) -> Option<f32> {
        if self.filled == 0 {
            return None;
        }
        let last = (self.write + self.samples.len() - 1) % self.samples.len();
        Some(self.samples[last])
    }

    /// Samples oldest-first. This is the order to plot; the raw ring is rotated
    /// and graphing it directly would draw a discontinuity that moves every frame.
    pub fn chronological(&self) -> impl Iterator<Item = f32> + '_ {
        let capacity = self.samples.len();
        // Once full the oldest sample sits at `write`; while filling, at 0.
        let start = if self.filled == capacity {
            self.write
        } else {
            0
        };
        (0..self.filled).map(move |offset| self.samples[(start + offset) % capacity])
    }

    pub fn mean(&self) -> Option<f32> {
        if self.filled == 0 {
            return None;
        }
        let total: f32 = self.chronological().sum();
        Some(total / self.filled as f32)
    }

    pub fn min(&self) -> Option<f32> {
        self.chronological().reduce(f32::min)
    }

    pub fn max(&self) -> Option<f32> {
        self.chronological().reduce(f32::max)
    }

    /// Nearest-rank percentile, `fraction` in 0.0..=1.0. Sorts into the scratch
    /// buffer, so this needs `&mut self` despite being a read.
    pub fn percentile(&mut self, fraction: f32) -> Option<f32> {
        if self.filled == 0 {
            return None;
        }
        let capacity = self.samples.len();
        let start = if self.filled == capacity {
            self.write
        } else {
            0
        };
        for offset in 0..self.filled {
            self.scratch[offset] = self.samples[(start + offset) % capacity];
        }
        let window = &mut self.scratch[..self.filled];
        window.sort_unstable_by(f32::total_cmp);
        let rank = (fraction.clamp(0.0, 1.0) * (self.filled - 1) as f32).round() as usize;
        Some(window[rank])
    }

    pub fn median(&mut self) -> Option<f32> {
        self.percentile(0.5)
    }

    /// The 95th percentile — the "how bad does it get" number. Preferred over
    /// `max` for a live readout: a single outlier owns `max` for the whole
    /// window length, which makes the display look stuck.
    pub fn p95(&mut self) -> Option<f32> {
        self.percentile(0.95)
    }

    /// Estimate the dominant periodic component in the window, if any.
    ///
    /// This is the wave detector. Mean-centres the history, correlates it against
    /// lagged copies of itself, and reports the cycle length behind the strongest
    /// match. A cost that oscillates on a fixed cadence — a cascade update every
    /// N frames, a swapchain that starves every N frames — produces a sharp peak
    /// at that N. Broadband noise produces no peak.
    ///
    /// Lags start at 2 because lag 1 measures frame-to-frame roughness, not
    /// periodicity, and stop at a quarter of the window so every reported period
    /// is backed by at least four observed cycles. Returns `None` below
    /// [`MINIMUM_PERIOD_SAMPLES`] of history or when nothing clears
    /// `minimum_strength`.
    ///
    /// # Two traps this had to be written around
    ///
    /// **Normalisation must not favour long lags.** Dividing by a lag-independent
    /// total energy inflates long lags, because fewer terms are summed there. So
    /// each lag is normalised by the energy of exactly the two overlapping
    /// windows it correlated — a Pearson coefficient, comparable across lags.
    ///
    /// **Harmonics.** A period-12 signal correlates just as strongly at 24, 36,
    /// 48; picking the numerically largest peak is a coin flip between them and
    /// routinely reported double the true period. The fundamental is the
    /// *smallest* lag that is a local peak and within
    /// [`FUNDAMENTAL_PEAK_TOLERANCE`] of the best, so that is what is chosen.
    pub fn dominant_period(&mut self, minimum_strength: f32) -> Option<PeriodEstimate> {
        if self.filled < MINIMUM_PERIOD_SAMPLES {
            return None;
        }
        let capacity = self.samples.len();
        let start = if self.filled == capacity {
            self.write
        } else {
            0
        };
        let count = self.filled;

        let mut total = 0.0;
        for offset in 0..count {
            let sample = self.samples[(start + offset) % capacity];
            self.scratch[offset] = sample;
            total += sample;
        }
        let mean = total / count as f32;
        for offset in 0..count {
            self.scratch[offset] -= mean;
        }
        let centred = &self.scratch[..count];

        // A perfectly flat window has zero variance and no periodicity to find —
        // bail rather than divide by it.
        let variance: f32 = centred.iter().map(|value| value * value).sum();
        if variance <= f32::EPSILON {
            return None;
        }

        let longest_lag = count / 4;
        if longest_lag < 2 {
            return None;
        }

        // Pearson correlation per lag, over just the overlapping region.
        for lag in 2..=longest_lag {
            let overlap = count - lag;
            let mut correlation = 0.0;
            let mut leading_energy = 0.0;
            let mut lagging_energy = 0.0;
            for index in 0..overlap {
                let leading = centred[index];
                let lagging = centred[index + lag];
                correlation += leading * lagging;
                leading_energy += leading * leading;
                lagging_energy += lagging * lagging;
            }
            let normaliser = (leading_energy * lagging_energy).sqrt();
            self.correlations[lag] = if normaliser > f32::EPSILON {
                correlation / normaliser
            } else {
                0.0
            };
        }

        let strengths = &self.correlations[..=longest_lag];
        let best_strength = strengths[2..]
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        if best_strength < minimum_strength {
            return None;
        }

        // Walk up from the shortest lag and take the first local peak that is
        // essentially as strong as the best one: that is the fundamental, not one
        // of its harmonics.
        let acceptable = best_strength * FUNDAMENTAL_PEAK_TOLERANCE;
        for lag in 2..=longest_lag {
            let strength = strengths[lag];
            if strength < acceptable {
                continue;
            }
            let rises_from_previous = lag == 2 || strength >= strengths[lag - 1];
            let falls_to_next = lag == longest_lag || strength >= strengths[lag + 1];
            if rises_from_previous && falls_to_next {
                return Some(PeriodEstimate {
                    period_samples: lag,
                    strength,
                });
            }
        }
        None
    }
}

/// How close to the strongest peak a shorter lag must be to be preferred as the
/// fundamental. At 0.9, a period-12 signal reports 12 rather than its equally
/// strong harmonic at 24.
pub const FUNDAMENTAL_PEAK_TOLERANCE: f32 = 0.9;

/// Minimum history before [`RollingWindow::dominant_period`] will answer. Eight
/// samples per cycle × four cycles: below this, noise readily fakes a peak.
pub const MINIMUM_PERIOD_SAMPLES: usize = 32;

/// A detected oscillation in a [`RollingWindow`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeriodEstimate {
    /// Cycle length in samples. For a per-frame window this is frames per cycle.
    pub period_samples: usize,
    /// Normalised autocorrelation at that lag. Roughly: 1.0 is a pure
    /// repeating signal, 0.0 is none. Treat below ~0.25 as "probably noise".
    pub strength: f32,
}

impl PeriodEstimate {
    /// Cycle rate in hertz, given the rate samples were taken at. For a
    /// per-frame window pass the frame rate.
    pub fn hertz(&self, samples_per_second: f32) -> f32 {
        samples_per_second / self.period_samples as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_overwrites_oldest_first_and_reports_chronological_order() {
        let mut window = RollingWindow::new(4);
        for value in [1.0, 2.0, 3.0] {
            window.push(value);
        }
        assert_eq!(window.len(), 3);
        assert_eq!(
            window.chronological().collect::<Vec<_>>(),
            vec![1.0, 2.0, 3.0]
        );

        // Past capacity: 1.0 and 2.0 fall off, order stays oldest-first.
        for value in [4.0, 5.0, 6.0] {
            window.push(value);
        }
        assert_eq!(window.len(), 4);
        assert_eq!(
            window.chronological().collect::<Vec<_>>(),
            vec![3.0, 4.0, 5.0, 6.0]
        );
        assert_eq!(window.latest(), Some(6.0));
    }

    #[test]
    fn percentiles_read_off_a_rotated_ring() {
        let mut window = RollingWindow::new(5);
        // Push 8 into a 5-ring so the ring is rotated: valid samples are 4..=8.
        for value in 1..=8 {
            window.push(value as f32);
        }
        assert_eq!(window.min(), Some(4.0));
        assert_eq!(window.max(), Some(8.0));
        assert_eq!(window.median(), Some(6.0));
        assert_eq!(window.p95(), Some(8.0));
    }

    /// The whole reason percentiles beat a mean: these two windows have the same
    /// mean and describe completely different experiences.
    #[test]
    fn a_mean_hides_the_oscillation_that_percentiles_expose() {
        let mut oscillating = RollingWindow::new(64);
        let mut flat = RollingWindow::new(64);
        for index in 0..64 {
            oscillating.push(if index % 2 == 0 { 4.0 } else { 20.0 });
            flat.push(12.0);
        }
        assert_eq!(oscillating.mean(), flat.mean());
        assert_eq!(oscillating.p95(), Some(20.0));
        assert_eq!(flat.p95(), Some(12.0));
    }

    #[test]
    fn a_periodic_cost_reports_its_period_and_flat_noise_reports_nothing() {
        // A spike every 12 frames — the shape a fixed-cadence background job
        // (cascade update, buffer upload) stamps onto the frame time.
        let mut spiky = RollingWindow::new(240);
        for index in 0..240 {
            spiky.push(if index % 12 == 0 { 9.0 } else { 4.0 });
        }
        let estimate = spiky
            .dominant_period(0.25)
            .expect("a 12-frame spike train must be detected");
        assert_eq!(estimate.period_samples, 12);
        // At 60 fps a 12-frame cycle is 5 Hz.
        assert!((estimate.hertz(60.0) - 5.0).abs() < 1e-3);

        let mut flat = RollingWindow::new(240);
        for _ in 0..240 {
            flat.push(4.0);
        }
        assert_eq!(flat.dominant_period(0.25), None);
    }

    #[test]
    fn a_sine_wave_is_detected_at_its_own_period() {
        let mut window = RollingWindow::new(240);
        for index in 0..240 {
            let phase = index as f32 / 20.0 * std::f32::consts::TAU;
            window.push(10.0 + 3.0 * phase.sin());
        }
        let estimate = window.dominant_period(0.25).expect("a sine is periodic");
        assert_eq!(estimate.period_samples, 20);
    }

    #[test]
    fn short_history_and_non_finite_samples_never_panic() {
        let mut window = RollingWindow::new(240);
        assert_eq!(window.dominant_period(0.25), None);
        assert_eq!(window.median(), None);
        assert_eq!(window.latest(), None);

        window.push(f32::NAN);
        window.push(f32::INFINITY);
        assert!(window.is_empty(), "non-finite samples must be dropped");

        window.push(5.0);
        assert_eq!(window.median(), Some(5.0));
    }

    #[test]
    fn a_single_slot_window_is_legal() {
        let mut window = RollingWindow::new(0);
        assert_eq!(window.capacity(), 1);
        window.push(2.0);
        window.push(3.0);
        assert_eq!(window.latest(), Some(3.0));
        assert_eq!(window.median(), Some(3.0));
    }
}
