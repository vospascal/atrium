//! The delay lines: their lengths, the LFO that modulates each read, and the
//! fractional-delay interpolator that reads between samples.

/// 16 delay lines (was 8). Modal density is `fs / Σdelays`; doubling the line
/// count roughly halves the mode spacing (to ~1.3 Hz here), which crosses
/// Schroeder's overlap threshold for a ~1.6 s tail so the reverb reads as a
/// smooth wash rather than a ringing/metallic one. Must stay a power of two
/// (the Hadamard mixing matrix is built by the fast Walsh–Hadamard transform).
pub(super) const NUM_LINES: usize = 16;
/// Initial delay buffer size (floor). Grown at init to fit `BASE_DELAYS` for the
/// actual sample rate (see `required_buffer_size` / `compute_delays`).
pub(super) const MIN_BUF_SIZE: usize = 512;
/// Delay-line lengths (samples @ 48 kHz): 16 mutually-prime values spread over
/// ~26–70 ms. HALL-scale, not box-scale: modal density is `fs / Σdelays`, so the
/// ~36.8k-sample total gives ~1.3 Hz mode spacing (vs ~15.5 Hz for the old box
/// delays, which rang metallically). Coprime lengths keep modes from coinciding;
/// longer delays raise density without changing RT60 (the damping formula is
/// per-sample normalized). Longest (3347, plus modulation depth and the
/// interpolator's taps) fits the 4096 buffer.
pub(super) const BASE_DELAYS: [usize; NUM_LINES] = [
    1249, 1399, 1531, 1669, 1811, 1949, 2089, 2237, 2371, 2503, 2657, 2791, 2939, 3067, 3209, 3347,
];
pub(super) const BASE_SAMPLE_RATE: f32 = 48000.0;

/// Per-line delay modulation parameters.
/// Irrational rate ratios prevent modulation patterns from repeating.
/// Rates chosen from golden-ratio-spaced values between 0.5–1.7 Hz
/// (slow enough to be inaudible, fast enough to smear modes within 1–2s).
pub(super) const MOD_RATES_HZ: [f32; NUM_LINES] = [
    0.51, 0.73, 0.97, 1.13, 1.31, 1.53, 0.67, 1.07, 0.59, 0.83, 1.03, 1.21, 1.43, 1.61, 0.79, 1.19,
];
/// Modulation depth in samples at 48 kHz. Scaled by sample_rate/48000 at init.
/// ±10 samples ≈ ±208 µs smears residual modes across the ~1.3 Hz spacing so the
/// tail stays smooth rather than metallic. On the hall-scale delay lines this is
/// only ±0.3–0.8 %, and the slow rates keep pitch deviation at ~2 cents (inaudible).
pub(super) const MOD_DEPTH_SAMPLES_48K: f32 = 10.0;

/// Per-line LFO state for delay modulation.
#[derive(Clone, Copy)]
pub(super) struct DelayModulator {
    phase: f32,
    phase_increment: f32,
    pub(super) depth: f32,
}

impl DelayModulator {
    pub(super) fn new() -> Self {
        Self {
            phase: 0.0,
            phase_increment: 0.0,
            depth: 0.0,
        }
    }

    /// Initialize the modulator for a given rate and sample rate.
    pub(super) fn init(&mut self, rate_hz: f32, sample_rate: f32, starting_phase: f32) {
        self.phase_increment = rate_hz / sample_rate;
        self.depth = MOD_DEPTH_SAMPLES_48K * (sample_rate / BASE_SAMPLE_RATE);
        self.phase = starting_phase;
    }

    /// Advance phase and return current modulation offset in fractional samples.
    #[inline(always)]
    pub(super) fn next(&mut self) -> f32 {
        let offset = (self.phase * std::f32::consts::TAU).sin() * self.depth;
        self.phase += self.phase_increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        offset
    }
}

/// Cubic Hermite (Catmull-Rom) fractional-delay coefficients for the
/// modulated delay-line read. A read at `integer + fraction` samples, with
/// `fraction` in [0, 1), weights the samples at delays `integer − 1`,
/// `integer`, `integer + 1` and `integer + 2` — the fraction always lies
/// between the two middle taps. `fraction = 0` is exactly `[0, 1, 0, 0]`, so a
/// frozen LFO reads an integer delay bit-exactly.
///
/// Why this interpolator. The LFO sweeps the fraction through [0, 1) every
/// few tens of milliseconds, so what the loop sees per pass is |H|² averaged
/// over the fraction (figures below are that average):
/// - Linear (the previous read): 2/3 + cos(ω)/3, i.e. −0.79 dB at 8 kHz and
///   −1.76 dB at 12 kHz on every ~48 ms trip round the loop. A flat 1.5 s
///   tail measured 1.14 s at 8 kHz and 0.92 s at 12 kHz.
/// - Catmull-Rom: −0.12 / −0.53 / −1.39 dB at 8 / 12 / 16 kHz; the flat
///   1.5 s tail measures 1.41 s at 8 kHz, 1.25 s at 12 kHz. Its magnitude
///   never exceeds 1 at any frequency/fraction pair: |H|² meets 1 at DC with
///   zero slope, zero curvature and a negative fourth derivative, and falls
///   from there (`hermite_interpolator_is_passive_at_every_fraction` measures
///   the worst case on a dense grid). The loop gain is therefore at most the
///   damping filter's, which `DampingFilter::design` keeps below 1.
/// - 4-point Lagrange is equally passive and as cheap but loses more
///   (−0.14 / −0.65 dB at 8 / 12 kHz; measured 1.39 s / 1.22 s).
/// - A first-order allpass (Thiran) is exactly flat in magnitude, but it is
///   recursive: its state carries the previous fraction, so a moving fraction
///   leaves transients, and when the fraction wraps (the integer delay steps
///   by one) its coefficient jumps across its whole range. Avoiding that
///   needs crossfaded dual reads, which cost more than four taps.
/// - 6-point Lagrange measures 1.47 s / 1.34 s for 50 % more taps. Even a
///   12-point one leaves the tail at 1.51 s / 1.46 s against 1.55 s frozen and
///   recovers only 0.6 dB of the ~2.7 dB by which the modulated network's
///   white-noise wet level sits below the frozen-LFO network's (−15.4 vs
///   −12.7 dB at 1.5 s); the other ~2.1 dB does not yield to interpolation.
#[inline(always)]
pub(super) fn hermite_coefficients(fraction: f32) -> [f32; 4] {
    let fraction_squared = fraction * fraction;
    [
        fraction * (-0.5 + fraction * (1.0 - 0.5 * fraction)),
        1.0 + fraction_squared * (-2.5 + 1.5 * fraction),
        fraction * (0.5 + fraction * (2.0 - 1.5 * fraction)),
        fraction_squared * (-0.5 + 0.5 * fraction),
    ]
}
