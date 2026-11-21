//! Per-line absorptive damping: the one-pole filter that sets each line's
//! frequency-dependent decay, solved from the two RT60s.

use crate::pipeline::room_acoustics;

/// Frequency (Hz) at which each line's damping filter meets the low-band RT60:
/// `room_acoustics::reverb_decay_seconds`: Sabine at 500 Hz, or the explicit RT60 override.
const LOW_DECAY_FREQUENCY_HZ: f64 = 500.0;
/// Frequency (Hz) at which each line's damping filter meets the high-band RT60:
/// Sabine band 5 (`RT60_HIGH_BAND`), or the override × `HF_DECAY_RATIO`.
const HIGH_DECAY_FREQUENCY_HZ: f64 = 4000.0;
/// Passivity ceiling on any line's loop gain at any frequency. An infinite RT60
/// would make the network lossless (gain exactly 1); 0.9999 per pass still
/// decays over minutes (≥ 3 min on the shortest line) but can never build up.
pub(super) const MAXIMUM_LOOP_GAIN: f64 = 0.9999;
/// Longest decay the damping filter may give DC, as a multiple of the 500 Hz
/// RT60. A one-pole steep enough to reach a short 4 kHz decay lifts its DC gain
/// above the 500 Hz gain; unchecked, a strongly HF-absorbing room would turn
/// into a sub-bass drone. Typical rooms stay well inside it (carpeted
/// 6×4×3 m: DC decays at 1.1–2.0× the 500 Hz RT60, only the longest line binds).
const MAXIMUM_LOW_FREQUENCY_DECAY_RATIO: f64 = 2.0;

/// Jot's one-pole absorptive filter, `y[n] = k·x[n] + p·y[n−1]`, with
/// `|H(e^{jω})| = k / √(1 + p² − 2p·cos ω)`.
#[derive(Clone, Copy)]
pub(super) struct DampingFilter {
    pub(super) k: f32,
    pub(super) p: f32,
    pub(super) state: f32,
}

impl DampingFilter {
    pub(super) fn new() -> Self {
        Self {
            k: 0.0,
            p: 0.0,
            state: 0.0,
        }
    }

    /// Damping filter for a line of `delay_samples`, solved in closed form so
    /// its gain equals the Jot per-pass gain `10^(−3·delay / (RT60·fs))` of
    /// `rt60_low` at 500 Hz and of `rt60_high` at 4 kHz — the frequencies the
    /// two RT60s are defined at.
    ///
    /// Writing `1 + p² − 2p·cos ω = (1 − p)² + 2p·v(ω)` with the versine
    /// `v(ω) = 1 − cos ω`, the ratio of the squared target gains
    /// `r = (g_low / g_high)²` fixes the pole through
    /// `(1 − p)² / (2p) = (v_high − r·v_low) / (r − 1)`; the stable root of that
    /// quadratic is `p = 1 + s − √(s·(2 + s))` (see `pole_with_shape`), and then
    /// `k = g_low · √((1 − p)² + 2p·v_low)`.
    ///
    /// Edge cases, all keeping the 500 Hz gain exact:
    /// - `rt60_high ≥ rt60_low`: no damping, `p = 0`, `k = g_low`.
    /// - A ratio no one-pole can reach (`v_high ≤ r·v_low`, ≈ 18 dB per pass
    ///   between 500 Hz and 4 kHz): the pole goes as far as the DC cap allows.
    /// - Passivity: the gain peaks at DC, `k / (1 − p)`, and is capped at
    ///   `min(g_low^(1/MAXIMUM_LOW_FREQUENCY_DECAY_RATIO), MAXIMUM_LOOP_GAIN)`
    ///   by lowering the pole, so every line's loop gain stays below 1.
    ///
    /// Solved in f64: at 96 kHz `v_low` is 5.4e-4, close to f32 rounding of
    /// `cos ω`. Runs once per line at init.
    pub(super) fn design(
        delay_samples: usize,
        sample_rate: f32,
        rt60_low: f32,
        rt60_high: f32,
    ) -> Self {
        let delay_seconds = delay_samples as f32 / sample_rate;
        let low_gain = f64::from(room_acoustics::jot_feedback_gain(delay_seconds, rt60_low));
        if low_gain.is_nan() || low_gain <= 0.0 {
            return Self::new();
        }
        let low_gain = low_gain.min(MAXIMUM_LOOP_GAIN);
        let high_gain =
            f64::from(room_acoustics::jot_feedback_gain(delay_seconds, rt60_high)).min(low_gain);

        let sample_rate = f64::from(sample_rate);
        let high_angle = (std::f64::consts::TAU * HIGH_DECAY_FREQUENCY_HZ / sample_rate)
            .min(std::f64::consts::PI);
        let low_angle =
            (std::f64::consts::TAU * LOW_DECAY_FREQUENCY_HZ / sample_rate).min(high_angle);
        // 1 − cos ω = 2·sin²(ω/2), computed without the cancellation of 1 − cos ω.
        let low_versine = 2.0 * (low_angle / 2.0).sin().powi(2);
        let high_versine = 2.0 * (high_angle / 2.0).sin().powi(2);

        let mut pole = if high_gain >= low_gain || high_versine <= low_versine {
            0.0
        } else {
            let squared_gain_ratio = (low_gain / high_gain).powi(2);
            let shape =
                (high_versine - squared_gain_ratio * low_versine) / (squared_gain_ratio - 1.0);
            // shape ≤ 0: unreachable ratio; pole 1 here, pulled back by the DC cap below.
            pole_with_shape(shape.max(0.0))
        };

        let dc_ceiling = low_gain
            .powf(1.0 / MAXIMUM_LOW_FREQUENCY_DECAY_RATIO)
            .min(MAXIMUM_LOOP_GAIN);
        if dc_ceiling > low_gain {
            // Largest pole whose DC gain, with the 500 Hz gain held, is the ceiling:
            // ceiling²·(1 − p)² = g_low²·((1 − p)² + 2p·v_low).
            let low_gain_squared = low_gain * low_gain;
            let dc_shape =
                low_gain_squared * low_versine / (dc_ceiling * dc_ceiling - low_gain_squared);
            pole = pole.min(pole_with_shape(dc_shape));
        } else {
            pole = 0.0;
        }

        let gain = low_gain * ((1.0 - pole).powi(2) + 2.0 * pole * low_versine).sqrt();
        Self {
            k: gain as f32,
            p: pole as f32,
            state: 0.0,
        }
    }

    /// Steady-state energy build-up of a loop closed through this filter, for a
    /// white input: the mean over frequency of `1 / (1 − |H(e^{jω})|²)`. With
    /// `(1/π)∫₀^π dω / (a − b·cos ω) = 1/√(a² − b²)` this is exactly
    /// `1 + k² / √(((1 − p)² − k²)·((1 + p)² − k²))`, finite because the DC
    /// gain `k / (1 − p)` is below 1. Reduces to `1 / (1 − k²)` when `p = 0`.
    pub(super) fn white_noise_energy_buildup(&self) -> f32 {
        let gain_squared = f64::from(self.k).powi(2);
        let pole = f64::from(self.p);
        let below_dc = (1.0 - pole).powi(2) - gain_squared;
        let below_nyquist = (1.0 + pole).powi(2) - gain_squared;
        if below_dc <= 0.0 {
            return f32::INFINITY;
        }
        (1.0 + gain_squared / (below_dc * below_nyquist).sqrt()) as f32
    }

    #[inline(always)]
    pub(super) fn process(&mut self, input: f32) -> f32 {
        let output = self.k * input + self.p * self.state;
        self.state = output;
        output
    }
}

/// The pole `0 < p ≤ 1` with `(1 − p)² / (2p) = shape`: the root inside the
/// unit circle of `p² − 2(1 + shape)·p + 1 = 0` (the roots multiply to 1).
/// Written as `1 / (1 + shape + √(shape·(2 + shape)))` so a large shape (a
/// pole near 0) does not cancel. `shape = 0` gives 1, `shape → ∞` gives 0.
fn pole_with_shape(shape: f64) -> f64 {
    1.0 / (1.0 + shape + (shape * (2.0 + shape)).sqrt())
}
