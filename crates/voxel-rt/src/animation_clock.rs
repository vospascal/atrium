//! The animation clock: pure math, no wgpu, no windowing.
//!
//! Material animation needs a time value on the GPU, and a naive `elapsed
//! seconds as f32` breaks in three separate ways. This module is the three
//! answers, kept together because they share one representation.
//!
//! 1. **A live speed slider must not jump the phase.** Multiplying an absolute
//!    time by the scale means changing the scale teleports every wave. The
//!    clock therefore accumulates `delta * speed`: the slider changes the rate
//!    of advance, not the position in the wave.
//! 2. **f32 loses the fraction.** A single monotonic counter in an f32 has
//!    ~24 bits of mantissa, so `sin(rate * t)` degrades within hours of uptime.
//!    The clock splits into a whole-[`EPOCH_SECONDS`] counter and a remainder
//!    inside it, and an oscillator recombines them per rate — see
//!    [`AnimationClockSample::oscillator_phase`].
//! 3. **A wrapped clock produces a visible discontinuity.** Wrapping a single
//!    `time` at any period steps every oscillator whose rate is not harmonic
//!    with it. The split form has no such step: the epoch term is continuous
//!    because `fract(rate * EPOCH_SECONDS)` is a per-rate constant.
//!
//! Everything here is mirrored by generated WGSL in
//! [`crate::material_graph`], so the arithmetic is written the way the shader
//! writes it — see [`fract`] on why Rust's `f32::fract` is not used.

/// One epoch. The remainder stays inside it, so `rate * remainder` keeps full
/// f32 precision for every rate the oscillator declaration allows.
pub const EPOCH_SECONDS: f32 = 64.0;

/// WGSL's `fract`, which is `x - floor(x)` and therefore always in `[0, 1)`.
///
/// Rust's `f32::fract` truncates toward zero instead, so it returns a NEGATIVE
/// fraction for a negative input. The two disagree for any negative phase, and
/// this clock's whole job is to agree with the shader bit for bit.
#[inline]
pub fn fract(value: f32) -> f32 {
    value - value.floor()
}

/// The accumulating clock. Owned by the platform layer, advanced once per
/// frame, sampled into the frame uniform.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnimationClock {
    /// Whole epochs elapsed. A `u32` at 64 s wraps after ~8,700 years.
    epoch: u32,
    /// Seconds within the epoch, always in `[0, EPOCH_SECONDS)`.
    remainder_seconds: f32,
}

impl AnimationClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance by one frame. `speed_scale` scales the DELTA, never the total.
    ///
    /// A negative or non-finite delta is ignored rather than rewinding the
    /// clock: a frame-time spike or a debugger pause should not run animation
    /// backwards.
    pub fn advance(&mut self, delta_seconds: f32, speed_scale: f32) {
        if !delta_seconds.is_finite() || !speed_scale.is_finite() {
            return;
        }
        let step = delta_seconds.max(0.0) * speed_scale.max(0.0);
        if step <= 0.0 {
            return;
        }
        let advanced = self.remainder_seconds + step;
        // A loop rather than a single modulo: a very long stall (a breakpoint,
        // a window drag) can cross many epochs, and the epoch count must stay
        // exact for event elapsed times to survive the boundary.
        let whole_epochs = (advanced / EPOCH_SECONDS).floor();
        if whole_epochs > 0.0 {
            self.epoch = self.epoch.wrapping_add(whole_epochs as u32);
            self.remainder_seconds = advanced - whole_epochs * EPOCH_SECONDS;
        } else {
            self.remainder_seconds = advanced;
        }
    }

    /// Rewind to zero. Used by the deterministic/bench mode, which needs the
    /// clock pinned rather than merely stopped.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn sample(&self) -> AnimationClockSample {
        AnimationClockSample {
            epoch: self.epoch as f32,
            remainder_seconds: self.remainder_seconds,
        }
    }
}

/// A clock reading, in the exact form the GPU receives it.
///
/// Both the CPU material backend and the generated WGSL take this and derive
/// everything from it, so a preview and a rendered pixel agree.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnimationClockSample {
    /// Whole epochs, as f32. Integer-exact to 2^24 epochs (~34 years).
    pub epoch: f32,
    /// Seconds within the epoch, `[0, EPOCH_SECONDS)`.
    pub remainder_seconds: f32,
}

impl AnimationClockSample {
    /// The pinned reading for deterministic mode and for the representative
    /// sample the material table bakes for GI.
    pub const FROZEN: Self = Self {
        epoch: 0.0,
        remainder_seconds: 0.0,
    };

    /// Monotone seconds since start — what `material.time` returns.
    ///
    /// Deliberately NOT the remainder alone: a node returning the remainder
    /// would jump backwards every [`EPOCH_SECONDS`], silently breaking any
    /// graph that does arithmetic on it. The cost is ordinary f32 precision
    /// (integer seconds stay exact to 2^24 ≈ 194 days), which is why the
    /// split form is kept for oscillator phase, where it actually helps.
    pub fn monotone_seconds(self) -> f32 {
        self.epoch * EPOCH_SECONDS + self.remainder_seconds
    }

    /// An oscillator's phase in turns, `[0, 1)`.
    ///
    /// The split recombination: `fract(rate * remainder + fract(rate * EPOCH)
    /// * epoch)`. Both products stay far smaller than `rate * total_seconds`
    /// would, and the epoch term is continuous across an epoch boundary
    /// because `fract(rate * EPOCH_SECONDS)` is constant for a given rate.
    ///
    /// Precision budget: the epoch term grows linearly with uptime, so phase
    /// holds to ~0.001 turn while `epoch < 1e-3 * 2^24 ≈ 16,700` — about
    /// **12 days of continuous runtime** — and degrades gracefully after.
    pub fn oscillator_phase(self, rate_hz: f32) -> f32 {
        let per_epoch = fract(rate_hz * EPOCH_SECONDS);
        fract(rate_hz * self.remainder_seconds + per_epoch * self.epoch)
    }

    /// Seconds from a stamped instant to now. May be negative if the stamp is
    /// in the future, which callers use to mean "not started yet".
    pub fn elapsed_since(self, epoch: f32, remainder_seconds: f32) -> f32 {
        (self.epoch - epoch) * EPOCH_SECONDS + (self.remainder_seconds - remainder_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure a naive `time * speed` would show: changing the scale
    /// mid-stream must change the RATE, not teleport the phase.
    #[test]
    fn changing_speed_advances_the_rate_without_jumping_the_phase() {
        let mut clock = AnimationClock::new();
        for _ in 0..60 {
            clock.advance(1.0 / 60.0, 1.0);
        }
        let before = clock.sample().monotone_seconds();
        assert!(
            (before - 1.0).abs() < 1e-4,
            "one second at 1x, got {before}"
        );

        // Double the speed. The very next reading must continue from where we
        // were, not leap to 2 s.
        clock.advance(1.0 / 60.0, 2.0);
        let after = clock.sample().monotone_seconds();
        let step = after - before;
        assert!(
            step > 0.0 && step < 0.05,
            "speed change teleported the clock: {before} -> {after}"
        );
    }

    #[test]
    fn speed_zero_holds_the_clock_still() {
        let mut clock = AnimationClock::new();
        clock.advance(1.0, 1.0);
        let held = clock.sample();
        for _ in 0..10 {
            clock.advance(1.0, 0.0);
        }
        assert_eq!(clock.sample(), held);
    }

    #[test]
    fn a_negative_or_non_finite_delta_never_rewinds_the_clock() {
        let mut clock = AnimationClock::new();
        clock.advance(2.0, 1.0);
        let held = clock.sample();
        clock.advance(-5.0, 1.0);
        clock.advance(f32::NAN, 1.0);
        clock.advance(1.0, f32::INFINITY);
        assert_eq!(clock.sample(), held);
    }

    #[test]
    fn crossing_an_epoch_keeps_the_remainder_in_range_and_counts_the_epoch() {
        let mut clock = AnimationClock::new();
        clock.advance(EPOCH_SECONDS * 2.5, 1.0);
        let sample = clock.sample();
        assert_eq!(sample.epoch, 2.0);
        assert!(
            sample.remainder_seconds >= 0.0 && sample.remainder_seconds < EPOCH_SECONDS,
            "remainder escaped its epoch: {}",
            sample.remainder_seconds
        );
        assert!((sample.monotone_seconds() - EPOCH_SECONDS * 2.5).abs() < 1e-2);
    }

    /// `material.time` must never step backwards — the bug a remainder-only
    /// clock would have shipped, and the one that would have made a lava drift
    /// snap back to its origin every 64 seconds.
    #[test]
    fn monotone_seconds_never_steps_backwards_across_an_epoch_boundary() {
        let mut clock = AnimationClock::new();
        let mut previous = clock.sample().monotone_seconds();
        // Walk right through a boundary in small steps.
        for _ in 0..((EPOCH_SECONDS as usize + 4) * 10) {
            clock.advance(0.1, 1.0);
            let now = clock.sample().monotone_seconds();
            assert!(
                now >= previous,
                "time went backwards at epoch {}: {previous} -> {now}",
                clock.sample().epoch
            );
            previous = now;
        }
        assert_eq!(clock.sample().epoch, 1.0);
    }

    /// The reason the split form exists: a plain wrapped clock steps here for
    /// any rate that is not harmonic with the wrap period.
    #[test]
    fn oscillator_phase_is_continuous_across_an_epoch_boundary() {
        // Deliberately non-harmonic with a 64 s epoch.
        for rate_hz in [0.07_f32, 0.31, 1.3, 7.77] {
            let before = AnimationClockSample {
                epoch: 0.0,
                remainder_seconds: EPOCH_SECONDS - 0.001,
            }
            .oscillator_phase(rate_hz);
            let after = AnimationClockSample {
                epoch: 1.0,
                remainder_seconds: 0.0,
            }
            .oscillator_phase(rate_hz);
            // Phase advances by rate * 0.001 turns across the boundary, modulo 1.
            let expected_step = fract(rate_hz * 0.001);
            let step = fract(after - before);
            assert!(
                (step - expected_step).abs() < 1e-3,
                "rate {rate_hz} stepped {step} across the epoch, expected {expected_step}"
            );
        }
    }

    /// Continuity is not enough on its own — the phase must also match what a
    /// single unwrapped clock would have produced, or the wave is merely
    /// smooth and wrong.
    #[test]
    fn oscillator_phase_matches_an_unwrapped_reference_within_precision() {
        let rate_hz = 0.31_f32;
        for epoch in [0.0_f32, 1.0, 5.0, 100.0] {
            let remainder = 12.5_f32;
            let split = AnimationClockSample {
                epoch,
                remainder_seconds: remainder,
            }
            .oscillator_phase(rate_hz);
            let total = f64::from(epoch) * f64::from(EPOCH_SECONDS) + f64::from(remainder);
            let reference = (f64::from(rate_hz) * total).rem_euclid(1.0) as f32;
            let error = fract(split - reference + 0.5) - 0.5;
            assert!(
                error.abs() < 1e-3,
                "epoch {epoch}: split {split} vs reference {reference}"
            );
        }
    }

    /// WGSL `fract` is `x - floor(x)`; Rust's truncates toward zero. A phase
    /// offset can be negative, so the two must not be confused.
    #[test]
    fn fract_matches_wgsl_for_negative_inputs() {
        assert!((fract(-0.25) - 0.75).abs() < 1e-6);
        assert!((fract(-1.75) - 0.25).abs() < 1e-6);
        assert_ne!(fract(-0.25), (-0.25_f32).fract());
    }

    #[test]
    fn elapsed_is_exact_for_an_event_stamped_just_before_an_epoch_boundary() {
        let stamped_epoch = 0.0;
        let stamped_remainder = EPOCH_SECONDS - 0.25;
        let now = AnimationClockSample {
            epoch: 1.0,
            remainder_seconds: 0.5,
        };
        let elapsed = now.elapsed_since(stamped_epoch, stamped_remainder);
        assert!(
            (elapsed - 0.75).abs() < 1e-4,
            "elapsed across the boundary was {elapsed}, expected 0.75"
        );
    }

    #[test]
    fn frozen_reads_zero_everywhere() {
        assert_eq!(AnimationClockSample::FROZEN.monotone_seconds(), 0.0);
        assert_eq!(AnimationClockSample::FROZEN.oscillator_phase(3.0), 0.0);
    }
}
