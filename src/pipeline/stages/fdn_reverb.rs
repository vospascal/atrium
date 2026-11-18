//! FDN (Feedback Delay Network) late reverb.
//!
//! Based on Jot & Chaigne (1991). 16 parallel delay lines coupled through a
//! 16×16 Hadamard mixing matrix, with one-pole lowpass damping per line for
//! frequency-dependent decay (highs fade faster than lows).
//!
//! Chain order: Direct (0–3ms) → EarlyReflections (3–50ms) → FdnReverb (50ms+)

use crate::audio::filters::Biquad;
use crate::pipeline::mix_stage::{MixContext, MixStage};
use crate::pipeline::room_acoustics;
use crate::pipeline::stages::sanitize_finite;

/// 16 delay lines (was 8). Modal density is `fs / Σdelays`; doubling the line
/// count roughly halves the mode spacing (to ~1.3 Hz here), which crosses
/// Schroeder's overlap threshold for a ~1.6 s tail so the reverb reads as a
/// smooth wash rather than a ringing/metallic one. Must stay a power of two
/// (the Hadamard mixing matrix is built by the fast Walsh–Hadamard transform).
const NUM_LINES: usize = 16;
const MAX_OUT: usize = 8;
/// Initial delay buffer size (floor). Grown at init to fit `BASE_DELAYS` for the
/// actual sample rate (see `required_buffer_size` / `compute_delays`).
const MIN_BUF_SIZE: usize = 512;
/// Fallback pre-delay when room geometry is degenerate (volume ≈ 0 or surface area ≈ 0).
const PRE_DELAY_FALLBACK_SECONDS: f32 = 0.020;
/// Shortest pre-delay the mean-free-path value is given (tiny rooms).
const PRE_DELAY_MINIMUM_SECONDS: f32 = 0.005;
/// Longest pre-delay, geometry-derived or explicit. The pre-delay ring is sized
/// at init to the pre-delay actually needed, so this only bounds the
/// allocation: the mean-free-path time reaches it at a mean free path of
/// ~86 m, far beyond any room (the 40×25×20 m cathedral is 50.6 ms).
const PRE_DELAY_MAXIMUM_SECONDS: f32 = 0.250;
/// Delay-line lengths (samples @ 48 kHz): 16 mutually-prime values spread over
/// ~26–70 ms. HALL-scale, not box-scale: modal density is `fs / Σdelays`, so the
/// ~36.8k-sample total gives ~1.3 Hz mode spacing (vs ~15.5 Hz for the old box
/// delays, which rang metallically). Coprime lengths keep modes from coinciding;
/// longer delays raise density without changing RT60 (the damping formula is
/// per-sample normalized). Longest (3347, plus modulation depth and the
/// interpolator's taps) fits the 4096 buffer.
const BASE_DELAYS: [usize; NUM_LINES] = [
    1249, 1399, 1531, 1669, 1811, 1949, 2089, 2237, 2371, 2503, 2657, 2791, 2939, 3067, 3209, 3347,
];
const BASE_SAMPLE_RATE: f32 = 48000.0;

/// Per-line delay modulation parameters.
/// Irrational rate ratios prevent modulation patterns from repeating.
/// Rates chosen from golden-ratio-spaced values between 0.5–1.7 Hz
/// (slow enough to be inaudible, fast enough to smear modes within 1–2s).
const MOD_RATES_HZ: [f32; NUM_LINES] = [
    0.51, 0.73, 0.97, 1.13, 1.31, 1.53, 0.67, 1.07, 0.59, 0.83, 1.03, 1.21, 1.43, 1.61, 0.79, 1.19,
];
/// Modulation depth in samples at 48 kHz. Scaled by sample_rate/48000 at init.
/// ±10 samples ≈ ±208 µs smears residual modes across the ~1.3 Hz spacing so the
/// tail stays smooth rather than metallic. On the hall-scale delay lines this is
/// only ±0.3–0.8 %, and the slow rates keep pitch deviation at ~2 cents (inaudible).
const MOD_DEPTH_SAMPLES_48K: f32 = 10.0;

/// Per-line LFO state for delay modulation.
#[derive(Clone, Copy)]
struct DelayModulator {
    phase: f32,
    phase_increment: f32,
    depth: f32,
}

impl DelayModulator {
    fn new() -> Self {
        Self {
            phase: 0.0,
            phase_increment: 0.0,
            depth: 0.0,
        }
    }

    /// Initialize the modulator for a given rate and sample rate.
    fn init(&mut self, rate_hz: f32, sample_rate: f32, starting_phase: f32) {
        self.phase_increment = rate_hz / sample_rate;
        self.depth = MOD_DEPTH_SAMPLES_48K * (sample_rate / BASE_SAMPLE_RATE);
        self.phase = starting_phase;
    }

    /// Advance phase and return current modulation offset in fractional samples.
    #[inline(always)]
    fn next(&mut self) -> f32 {
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
fn hermite_coefficients(fraction: f32) -> [f32; 4] {
    let fraction_squared = fraction * fraction;
    [
        fraction * (-0.5 + fraction * (1.0 - 0.5 * fraction)),
        1.0 + fraction_squared * (-2.5 + 1.5 * fraction),
        fraction * (0.5 + fraction * (2.0 - 1.5 * fraction)),
        fraction_squared * (-0.5 + 0.5 * fraction),
    ]
}

/// Frequency (Hz) at which each line's damping filter meets the low-band RT60:
/// Sabine band 2 (`RT60_LOW_BAND`), or the explicit RT60 override.
const LOW_DECAY_FREQUENCY_HZ: f64 = 500.0;
/// Frequency (Hz) at which each line's damping filter meets the high-band RT60:
/// Sabine band 5 (`RT60_HIGH_BAND`), or the override × `HF_DECAY_RATIO`.
const HIGH_DECAY_FREQUENCY_HZ: f64 = 4000.0;
/// Passivity ceiling on any line's loop gain at any frequency. An infinite RT60
/// would make the network lossless (gain exactly 1); 0.9999 per pass still
/// decays over minutes (≥ 3 min on the shortest line) but can never build up.
const MAXIMUM_LOOP_GAIN: f64 = 0.9999;
/// Longest decay the damping filter may give DC, as a multiple of the 500 Hz
/// RT60. A one-pole steep enough to reach a short 4 kHz decay lifts its DC gain
/// above the 500 Hz gain; unchecked, a strongly HF-absorbing room would turn
/// into a sub-bass drone. Typical rooms stay well inside it (carpeted
/// 6×4×3 m: DC decays at 1.1–2.0× the 500 Hz RT60, only the longest line binds).
const MAXIMUM_LOW_FREQUENCY_DECAY_RATIO: f64 = 2.0;

/// Jot's one-pole absorptive filter, `y[n] = k·x[n] + p·y[n−1]`, with
/// `|H(e^{jω})| = k / √(1 + p² − 2p·cos ω)`.
#[derive(Clone, Copy)]
struct DampingFilter {
    k: f32,
    p: f32,
    state: f32,
}

impl DampingFilter {
    fn new() -> Self {
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
    fn design(delay_samples: usize, sample_rate: f32, rt60_low: f32, rt60_high: f32) -> Self {
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
    fn white_noise_energy_buildup(&self) -> f32 {
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
    fn process(&mut self, input: f32) -> f32 {
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

/// Soft clamp for FDN feedback loop stability. Generous ±16.0 ceiling
/// (~24 dB headroom) with smooth knee starting at ±14.0. Uses the same
/// rational saturation curve as `soft_clip()` but at a much wider range.
///
/// Hard clamping at ±4.0 introduced discontinuities that compound through
/// the feedback loop, producing audible distortion. This soft version
/// asymptotes smoothly toward ±16.0, so fed-back signal remains smooth
/// even when the clamp activates.
#[inline(always)]
fn feedback_soft_clamp(x: f32) -> f32 {
    const CEILING: f32 = 16.0;
    const KNEE: f32 = 14.0;
    const WIDTH: f32 = CEILING - KNEE; // 2.0
    if x > KNEE {
        let excess = (x - KNEE) / WIDTH;
        KNEE + WIDTH * excess / (1.0 + excess)
    } else if x < -KNEE {
        let excess = (-x - KNEE) / WIDTH;
        -(KNEE + WIDTH * excess / (1.0 + excess))
    } else {
        x
    }
}

/// When RT60 is overridden explicitly, the 4 kHz decay is set to this fraction
/// of the mid-band (500 Hz) RT60 — highs ring out somewhat faster than lows for
/// a naturally bright-but-controlled "glass atrium" tail without going sizzly.
const HF_DECAY_RATIO: f32 = 0.6;

/// Voicing/tuning for the FDN reverb, resolved once at pipeline-build time from
/// the environment config. Separates *what the room does* (RT60, pre-delay from
/// geometry) from *how the designer wants the tail to sit* (per-channel balance,
/// low-end control).
#[derive(Clone)]
pub struct FdnConfig {
    /// Explicit mid-band (500 Hz) RT60 in seconds. `Some` bypasses the
    /// geometry+material Sabine RT60; the 4 kHz decay is derived via
    /// `HF_DECAY_RATIO`. `None` keeps the physically-derived decay.
    pub rt60_seconds: Option<f32>,
    /// Explicit pre-delay in milliseconds. `Some` bypasses the mean-free-path
    /// pre-delay (bounded to `PRE_DELAY_MAXIMUM_SECONDS`, 250 ms; the
    /// pre-delay ring is sized to it at init). `None` keeps it.
    pub pre_delay_ms: Option<f32>,
    /// Per-output-channel wet trim, indexed by the canonical 5.1 hardware layout
    /// (0=L, 1=R, 2=C, 3=LFE, 4=Ls, 5=Rs, 6/7=back surrounds). Lets surrounds run
    /// wetter than fronts and keeps the centre near-dry for dialogue clarity.
    pub channel_gains: [f32; MAX_OUT],
    /// High-pass cutoff (Hz) applied to the reverb send. Keeps low-mid mud out of
    /// the tail — and, since the tail then has little sub-bass, keeps reverb bass
    /// out of the LFE too. `0.0` disables it.
    pub hpf_cutoff_hz: f32,
    /// Overall wet level multiplier applied to the tail. Lets the reverb sit under
    /// the dry signal (the send law + normalization set the D/R balance; this trims
    /// the absolute prominence). `1.0` = full.
    pub wet: f32,
}

impl Default for FdnConfig {
    /// Neutral FDN: physically-derived decay/pre-delay, uniform channel level,
    /// full wet, no send high-pass. Matches the reverb's historical behaviour.
    fn default() -> Self {
        Self {
            rt60_seconds: None,
            pre_delay_ms: None,
            channel_gains: [1.0; MAX_OUT],
            hpf_cutoff_hz: 0.0,
            wet: 1.0,
        }
    }
}

impl FdnConfig {
    /// Voicing for the physical speaker array: surrounds wettest, centre near-dry,
    /// fronts moderate, LFE unused; reverb send high-passed to control the low end.
    /// This is the idea.md atrium recipe (surrounds > fronts > centre, LFE dry).
    pub fn speaker(rt60_seconds: Option<f32>, pre_delay_ms: Option<f32>) -> Self {
        Self {
            rt60_seconds,
            pre_delay_ms,
            //             L    R    C    LFE  Ls   Rs   (7.1 back ×2)
            channel_gains: [0.6, 0.6, 0.15, 0.0, 1.0, 1.0, 1.0, 1.0],
            hpf_cutoff_hz: 160.0,
            // Sit the tail clearly under the dry signal — a living-room atrium,
            // not a wash. Keeps the wet from dominating in energy-concentrating
            // modes (VBAP/WorldLocked/Ambisonics).
            wet: 0.5,
        }
    }

    /// Voicing for headphone (HRTF) output: full, uniform stereo reverb with only
    /// a gentle rumble filter — no speaker-array channel balancing.
    pub fn headphone(rt60_seconds: Option<f32>, pre_delay_ms: Option<f32>) -> Self {
        Self {
            rt60_seconds,
            pre_delay_ms,
            channel_gains: [1.0; MAX_OUT],
            hpf_cutoff_hz: 100.0,
            wet: 0.7,
        }
    }
}

/// Post-mix FDN late reverb stage.
///
/// Fully physics-based gain staging:
/// - **Send**: d²/(d²+d_c²) per source (bounded reverberant energy fraction)
/// - **Decay**: Sabine RT60 → Jot per-line damping gains, met at 500 Hz and 4 kHz
/// - **Output normalization**: derived from steady-state energy buildup of the
///   16-line feedback network (1/RMS over lines of the per-line white-noise
///   build-up), so the d/d_c send law directly controls perceived wet level
///   without an arbitrary wet knob.
///
/// High-frequency decay is derived from per-wall material absorption
/// coefficients via Sabine's equation at 500 Hz (low) and 4 kHz (high), and
/// each line's damping filter is solved to hit both at those frequencies, so
/// rooms with absorptive surfaces (carpet, acoustic tile) naturally produce
/// faster HF decay than hard-walled rooms.
pub struct FdnReverbStage {
    /// Delay line buffers — dynamically sized to accommodate delays at any
    /// sample rate. At 48 kHz the buffer is 4096 samples; at 96 kHz it grows
    /// to 8192 to preserve the coprime delay ratios.
    delay_buffers: Vec<Vec<f32>>,
    /// Current buffer size (always a power of 2).
    buffer_size: usize,
    /// Bitmask for wrapping write/read positions: `buffer_size - 1`.
    buffer_mask: usize,
    write_pos: usize,
    delays: [usize; NUM_LINES],
    damping: [DampingFilter; NUM_LINES],
    /// Pre-delay ring, sized at init to the next power of two above
    /// `pre_delay_samples` (see `set_pre_delay`).
    pre_delay_buffer: Vec<f32>,
    /// Bitmask for wrapping the pre-delay ring: `pre_delay_buffer.len() - 1`.
    pre_delay_mask: usize,
    pre_delay_write_pos: usize,
    pre_delay_samples: usize,
    /// Structural output normalization derived from the damping filters'
    /// steady-state energy build-up.
    /// Compensates for the FDN's steady-state energy accumulation so that
    /// the d/d_c send law directly controls the perceived wet level.
    output_normalization: f32,
    /// Per-line delay modulators. Slowly vary each line's read position
    /// to smear resonant modes, preventing metallic ringing.
    modulators: [DelayModulator; NUM_LINES],
    initialized: bool,
    /// False when the environment cannot sustain a reverberant field (no pair
    /// of facing reflective surfaces) and no RT60 was set explicitly. The
    /// stage then leaves the buffer untouched: an open field has no tail.
    reverberant: bool,
    measurement_mode: bool,
    // ── Configuration (resolved from the environment at build time) ──
    /// Explicit mid-band RT60 override (seconds); `None` = geometry-derived.
    rt60_override: Option<f32>,
    /// Explicit pre-delay override (milliseconds); `None` = mean-free-path.
    pre_delay_override_ms: Option<f32>,
    /// Per-channel wet trim (canonical 5.1 role order).
    channel_gains: [f32; MAX_OUT],
    /// Reverb-send high-pass cutoff (Hz); `0.0` disables it.
    hpf_cutoff_hz: f32,
    /// Reverb-send high-pass filter, built at init once the sample rate is known.
    send_hpf: Option<Biquad>,
    /// Overall wet-level multiplier for the tail.
    wet: f32,
}

/// Sabine RT60 band indices for FDN damping.
/// Band 2 = 500 Hz (low-frequency reference), band 5 = 4 kHz (HF decay).
const RT60_LOW_BAND: usize = 2;
const RT60_HIGH_BAND: usize = 5;

impl Default for FdnReverbStage {
    fn default() -> Self {
        Self::new()
    }
}

impl FdnReverbStage {
    /// Neutral reverb: physically-derived decay/pre-delay, uniform channel level,
    /// no send high-pass. Equivalent to `with_config(FdnConfig::default())`.
    pub fn new() -> Self {
        Self::with_config(FdnConfig::default())
    }

    /// Reverb configured from the environment (RT60/pre-delay overrides,
    /// per-channel voicing, reverb-send high-pass).
    pub fn with_config(config: FdnConfig) -> Self {
        Self {
            delay_buffers: vec![vec![0.0; MIN_BUF_SIZE]; NUM_LINES],
            buffer_size: MIN_BUF_SIZE,
            buffer_mask: MIN_BUF_SIZE - 1,
            write_pos: 0,
            delays: [0; NUM_LINES],
            damping: [DampingFilter::new(); NUM_LINES],
            pre_delay_buffer: vec![0.0; 1],
            pre_delay_mask: 0,
            pre_delay_write_pos: 0,
            pre_delay_samples: 0,
            output_normalization: 1.0,
            modulators: [DelayModulator::new(); NUM_LINES],
            initialized: false,
            reverberant: true,
            measurement_mode: false,
            rt60_override: config.rt60_seconds,
            pre_delay_override_ms: config.pre_delay_ms,
            channel_gains: config.channel_gains,
            hpf_cutoff_hz: config.hpf_cutoff_hz,
            send_hpf: None,
            wet: config.wet,
        }
    }

    /// Compute the minimum power-of-2 buffer size needed for the given sample rate.
    ///
    /// The oldest sample the modulated read touches is the longest line plus
    /// the full LFO depth plus the interpolator's two taps past the integer
    /// delay (see `hermite_coefficients`); the slot being written this sample
    /// must not alias it.
    fn required_buffer_size(sample_rate: f32) -> usize {
        let scale = sample_rate / BASE_SAMPLE_RATE;
        let longest_modulated_delay =
            ((BASE_DELAYS[NUM_LINES - 1] as f32 + MOD_DEPTH_SAMPLES_48K) * scale).ceil() as usize;
        let oldest_read = longest_modulated_delay + 2;
        (oldest_read + 1).next_power_of_two().max(MIN_BUF_SIZE)
    }

    fn compute_delays(&mut self, sample_rate: f32) {
        let needed = Self::required_buffer_size(sample_rate);
        if needed != self.buffer_size {
            self.buffer_size = needed;
            self.buffer_mask = needed - 1;
            self.delay_buffers = vec![vec![0.0; needed]; NUM_LINES];
            self.write_pos = 0;
        }
        let scale = sample_rate / BASE_SAMPLE_RATE;
        for (i, &base_delay) in BASE_DELAYS.iter().enumerate() {
            let scaled = ((base_delay as f32) * scale) as usize;
            self.delays[i] = scaled.clamp(1, self.buffer_size - 1);
        }

        // Initialize per-line delay modulators with different starting phases
        // so they don't all begin at the same point in their cycle.
        for (i, (modulator, &rate)) in self
            .modulators
            .iter_mut()
            .zip(MOD_RATES_HZ.iter())
            .enumerate()
        {
            let starting_phase = i as f32 / NUM_LINES as f32;
            modulator.init(rate, sample_rate, starting_phase);
        }
    }

    /// Compute pre-delay from mean free path time (average time between wall
    /// bounces in a diffuse field). Falls back to 20ms for degenerate rooms,
    /// and is held to at least `PRE_DELAY_MINIMUM_SECONDS`.
    ///
    /// Reference: Kuttruff, "Room Acoustics" (5th ed., 2009).
    fn compute_pre_delay(
        &mut self,
        sample_rate: f32,
        volume: f32,
        surface_area: f32,
        speed_of_sound: f32,
    ) {
        let pre_delay_seconds = if volume < 1e-6 || surface_area < 1e-6 {
            PRE_DELAY_FALLBACK_SECONDS
        } else {
            room_acoustics::mean_free_path_time(volume, surface_area, speed_of_sound)
        };
        self.set_pre_delay(
            pre_delay_seconds.max(PRE_DELAY_MINIMUM_SECONDS),
            sample_rate,
        );
    }

    /// Set the pre-delay, bounded to [0, `PRE_DELAY_MAXIMUM_SECONDS`], and size
    /// the ring to hold it at this sample rate (a power of two above the delay,
    /// so the read never aliases the write). Allocates when the size changes;
    /// called from `init` only.
    fn set_pre_delay(&mut self, seconds: f32, sample_rate: f32) {
        let seconds = seconds.clamp(0.0, PRE_DELAY_MAXIMUM_SECONDS);
        self.pre_delay_samples = (seconds * sample_rate) as usize;
        let required_size = (self.pre_delay_samples + 1).next_power_of_two();
        if required_size != self.pre_delay_buffer.len() {
            self.pre_delay_buffer = vec![0.0; required_size];
            self.pre_delay_mask = required_size - 1;
            self.pre_delay_write_pos = 0;
        }
    }

    /// Compute per-line one-pole damping filters for frequency-dependent decay,
    /// and derive structural output normalization from the loop gains.
    ///
    /// Each line's filter meets the Jot per-pass gain of `rt60_low` at 500 Hz
    /// and of `rt60_high` at 4 kHz, the frequencies the two RT60s describe
    /// (Sabine bands 2 and 5, or override and override × `HF_DECAY_RATIO`);
    /// see `DampingFilter::design` for the closed form and its edge cases.
    ///
    /// Output normalization divides out the network's steady-state energy
    /// build-up: per line the white-noise mean of 1/(1 − |H(e^{jω})|²) over
    /// frequency (`DampingFilter::white_noise_energy_buildup`, exact for the
    /// one-pole), RMS-averaged across lines (Hadamard is orthonormal, so lines
    /// don't add coherently).
    ///
    /// Reference: Jot, "Digital Delay Networks for Designing Artificial Reverberators",
    /// AES 90th Convention (1991), §3.2; extended in Jot's PhD thesis (1992), Ch. 4.
    fn compute_damping(&mut self, sample_rate: f32, rt60_low: f32, rt60_high: f32) {
        let mut sum_energy_buildup = 0.0f32;

        for (filter, &delay_samples) in self.damping.iter_mut().zip(self.delays.iter()) {
            *filter = DampingFilter::design(delay_samples, sample_rate, rt60_low, rt60_high);
            sum_energy_buildup += filter.white_noise_energy_buildup();
        }

        // Network-level gain: RMS across lines (Hadamard is orthonormal).
        let rms_gain = (sum_energy_buildup / NUM_LINES as f32).sqrt();
        let raw_norm = 1.0 / rms_gain;
        self.output_normalization = if self.measurement_mode {
            // Measurement mode: preserve linear gain, only prevent NaN/Inf.
            if raw_norm.is_finite() {
                raw_norm
            } else {
                1.0
            }
        } else {
            raw_norm.clamp(0.01, 1.0)
        };
    }

    /// In-place fast Walsh–Hadamard transform, normalized to be orthonormal.
    /// This is the FDN's lossless mixing matrix; it works for any power-of-two
    /// `NUM_LINES` (the old code hardcoded the 8-point butterfly).
    #[inline(always)]
    fn hadamard(v: &mut [f32; NUM_LINES]) {
        let mut h = 1;
        while h < NUM_LINES {
            let mut i = 0;
            while i < NUM_LINES {
                for j in i..i + h {
                    let a = v[j];
                    let b = v[j + h];
                    v[j] = a + b;
                    v[j + h] = a - b;
                }
                i += h * 2;
            }
            h *= 2;
        }
        let scale = 1.0 / (NUM_LINES as f32).sqrt();
        for x in v.iter_mut() {
            *x *= scale;
        }
    }

    #[allow(clippy::needless_range_loop)]
    #[inline(always)]
    fn process_fdn_sample(&mut self, mono_in: f32, out_channels: usize) -> [f32; MAX_OUT] {
        let mut output = [0.0f32; MAX_OUT];

        // Read from delay lines with modulated positions, smearing resonant
        // peaks. Cubic Hermite interpolation keeps the swept fractional read
        // nearly flat and never above unity gain (see `hermite_coefficients`).
        let mut taps = [0.0_f32; NUM_LINES];
        for i in 0..NUM_LINES {
            let mod_offset = self.modulators[i].next();
            let modulated_delay = self.delays[i] as f32 + mod_offset;
            // Taps span delays integer − 1 … integer + 2: the newest must be at
            // least 1 (the current slot is written below), the oldest at most
            // buffer_size − 1. `required_buffer_size` makes this a no-op guard.
            let clamped_delay = modulated_delay.clamp(2.0, (self.buffer_size - 3) as f32);
            let delay_integer = clamped_delay as usize;
            let coefficients = hermite_coefficients(clamped_delay - delay_integer as f32);

            let line = &self.delay_buffers[i];
            let newest = self.write_pos + self.buffer_size + 1 - delay_integer;
            taps[i] = coefficients[0] * line[newest & self.buffer_mask]
                + coefficients[1] * line[(newest - 1) & self.buffer_mask]
                + coefficients[2] * line[(newest - 2) & self.buffer_mask]
                + coefficients[3] * line[(newest - 3) & self.buffer_mask];
        }

        for i in 0..NUM_LINES {
            taps[i] = self.damping[i].process(taps[i]);
        }

        // Distribute the 16 delay line taps across output channels for a diffuse field.
        // Each channel accumulates taps via modular assignment (line i → channel i%N).
        // Normalization: divide each channel by √(taps_in_ch × ch_count).
        // This simultaneously:
        //  - Equalizes per-channel level (channels with more taps aren't louder)
        //  - Preserves total energy (total power across all channels = single-tap power)
        // Result: the reverb send level directly controls the perceived wet level.
        let ch_count = out_channels.clamp(1, MAX_OUT);
        let remainder = NUM_LINES % ch_count;
        for i in 0..NUM_LINES {
            output[i % ch_count] += taps[i];
        }
        for ch in 0..ch_count {
            let taps_in_ch = if ch < remainder {
                (NUM_LINES / ch_count + 1) as f32
            } else {
                (NUM_LINES / ch_count) as f32
            };
            output[ch] /= (taps_in_ch * ch_count as f32).sqrt();
        }

        Self::hadamard(&mut taps);

        let input_scale = 1.0 / (NUM_LINES as f32).sqrt();
        let scaled_input = mono_in * input_scale;
        for i in 0..NUM_LINES {
            let v = taps[i] + scaled_input;
            self.delay_buffers[i][self.write_pos] = if self.measurement_mode {
                // Stability ceiling only: prevent runaway without soft clipping.
                v.clamp(-100.0, 100.0)
            } else {
                // Soft clamp: prevents runaway without introducing hard-clip
                // discontinuities that compound through the feedback loop.
                feedback_soft_clamp(v)
            };
        }

        self.write_pos = (self.write_pos + 1) & self.buffer_mask;
        output
    }
}

impl MixStage for FdnReverbStage {
    fn init(&mut self, ctx: &MixContext) {
        self.measurement_mode = ctx.measurement_mode;
        self.compute_delays(ctx.sample_rate);

        // An explicit RT60 is a designer's decision and always wins. Otherwise
        // the geometry decides: without facing reflective surfaces there is no
        // diffuse field for Sabine to describe.
        self.reverberant = self.rt60_override.is_some_and(|rt60| rt60 > 0.0)
            || room_acoustics::has_facing_reflective_surfaces(&ctx.environment.wall_materials);

        // Derive per-band RT60 from room geometry + wall materials via Sabine's equation.
        // RT60 at 500 Hz (band 2) sets the overall decay; RT60 at 4 kHz (band 5)
        // controls how much faster highs decay — driven by actual material absorption.
        let (volume, surface_area) =
            room_acoustics::room_geometry(ctx.environment.min, ctx.environment.max);
        let wall_areas =
            room_acoustics::wall_surface_areas(ctx.environment.min, ctx.environment.max);
        let rt60_low = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &ctx.environment.wall_materials,
            &ctx.environment.atmosphere,
            RT60_LOW_BAND,
        );
        let rt60_high = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &ctx.environment.wall_materials,
            &ctx.environment.atmosphere,
            RT60_HIGH_BAND,
        );

        // Explicit RT60 override (e.g. "atrium tuned to 3.5 s") bypasses the
        // geometry/material Sabine decay. HF decays a bit faster for a bright
        // but non-sizzly tail. Guard against non-positive values.
        let (rt60_low, rt60_high) = match self.rt60_override {
            Some(rt60) if rt60 > 0.0 => (rt60, rt60 * HF_DECAY_RATIO),
            _ => (rt60_low, rt60_high),
        };

        // Pre-delay: an explicit override (e.g. 20 ms) wins; otherwise the mean
        // free path, the average time between wall bounces. Either sizes the
        // pre-delay ring for this sample rate.
        match self.pre_delay_override_ms {
            Some(milliseconds) => self.set_pre_delay(milliseconds / 1000.0, ctx.sample_rate),
            None => {
                let speed_of_sound = ctx.environment.atmosphere.speed_of_sound();
                self.compute_pre_delay(ctx.sample_rate, volume, surface_area, speed_of_sound);
            }
        }

        // Build the reverb-send high-pass now that the sample rate is known.
        self.send_hpf = (self.hpf_cutoff_hz > 0.0)
            .then(|| Biquad::highpass(self.hpf_cutoff_hz, ctx.sample_rate));

        self.compute_damping(ctx.sample_rate, rt60_low, rt60_high);
        self.initialized = true;
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        if !self.initialized || !self.reverberant {
            return;
        }

        let channels = ctx.channels;
        let render_channels = ctx.render_channels;
        let num_frames = buffer.len() / channels;

        for frame in 0..num_frames {
            let base = frame * channels;

            // Read mono input from the reverb send buffer (pre-weighted by d/d_c)
            // if available, otherwise fall back to mono-summing the main buffer.
            // Reverb send is a mono bus on channel 0 for the generic FDN path.
            let mono_in = if let Some(reverb_input) = ctx.reverb_input {
                reverb_input[base]
            } else {
                let mut mono_sum = 0.0f32;
                let mut active_count = 0u32;
                for ch in 0..render_channels {
                    if ctx.layout.is_channel_active(ch) {
                        mono_sum += buffer[base + ch];
                        active_count += 1;
                    }
                }
                if active_count == 0 {
                    continue;
                }
                mono_sum / active_count as f32
            };

            // High-pass the reverb send so the tail stays clear of low-mid mud
            // (and, with little sub-bass left, keeps reverb out of the LFE).
            let mono_in = match self.send_hpf.as_mut() {
                Some(hpf) => hpf.process(mono_in),
                None => mono_in,
            };

            // Pre-delay
            self.pre_delay_buffer[self.pre_delay_write_pos] = mono_in;
            let read_pos = (self.pre_delay_write_pos + self.pre_delay_buffer.len()
                - self.pre_delay_samples)
                & self.pre_delay_mask;
            let delayed_in = self.pre_delay_buffer[read_pos];
            self.pre_delay_write_pos = (self.pre_delay_write_pos + 1) & self.pre_delay_mask;

            // FDN → only active channels within render range
            let wet = self.process_fdn_sample(delayed_in, render_channels);

            for ch in 0..render_channels {
                if ctx.layout.is_channel_active(ch) {
                    // Per-channel voicing: surrounds run wetter than fronts, the
                    // centre stays near-dry (canonical 5.1 role order); `wet` sets
                    // the overall tail prominence.
                    let wet_ch =
                        wet[ch] * self.output_normalization * self.channel_gains[ch] * self.wet;
                    let mixed = buffer[base + ch] + wet_ch;
                    // Linear output — MasterGainStage is the single point of
                    // output limiting. sanitize_finite prevents NaN/Inf propagation
                    // and enforces a ±100.0 stability ceiling for runaway FDN.
                    buffer[base + ch] = sanitize_finite(mixed);
                }
            }
        }
    }

    fn reset(&mut self) {
        for line in &mut self.delay_buffers {
            line.fill(0.0);
        }
        self.pre_delay_buffer.fill(0.0);
        self.write_pos = 0;
        self.pre_delay_write_pos = 0;
        for d in &mut self.damping {
            d.state = 0.0;
        }
        if let Some(hpf) = self.send_hpf.as_mut() {
            hpf.reset();
        }
    }

    fn name(&self) -> &str {
        "fdn_reverb"
    }
}

// ── Test-only diagnostic accessors ──────────────────────────────────────────
#[cfg(test)]
impl FdnReverbStage {
    /// Diagnostic snapshot of FDN internal state for pipeline analysis.
    pub fn diagnostics(&self) -> FdnDiagnostics {
        let mut loop_gains = [0.0f32; NUM_LINES];
        for i in 0..NUM_LINES {
            // Effective loop gain per line: k / (1 - p) approximation at DC
            let k = self.damping[i].k;
            let p = self.damping[i].p;
            loop_gains[i] = if (1.0 - p).abs() > f32::EPSILON {
                k / (1.0 - p)
            } else {
                k
            };
        }
        FdnDiagnostics {
            output_normalization: self.output_normalization,
            pre_delay_samples: self.pre_delay_samples,
            delays: self.delays,
            loop_gains,
            buffer_size: self.buffer_size,
            initialized: self.initialized,
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
pub struct FdnDiagnostics {
    pub output_normalization: f32,
    pub pre_delay_samples: usize,
    pub delays: [usize; NUM_LINES],
    pub loop_gains: [f32; NUM_LINES],
    pub buffer_size: usize,
    pub initialized: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::SpeakerLayout;
    use atrium_core::types::Vec3;

    fn make_ctx(channels: usize, _render_channels: usize) -> (SpeakerLayout, Listener) {
        let layout = if channels >= 6 {
            SpeakerLayout::surround_5_1(
                Vec3::new(0.0, 4.0, 0.0),
                Vec3::new(6.0, 4.0, 0.0),
                Vec3::new(3.0, 4.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(6.0, 0.0, 0.0),
            )
        } else {
            SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0))
        };
        let listener = Listener::new(Vec3::ZERO, 0.0);
        (layout, listener)
    }

    use crate::pipeline::environment::Environment;
    use crate::pipeline::wall_material::WallMaterial;
    use std::sync::LazyLock;

    /// A 10 m cube centred on the origin with hard walls and 20 °C air.
    static TEST_ENVIRONMENT: LazyLock<Environment> = LazyLock::new(|| {
        Environment::uniform(
            Vec3::new(-5.0, -5.0, -5.0),
            Vec3::new(5.0, 5.0, 5.0),
            WallMaterial::HARD_WALL,
        )
    });

    fn mix_ctx<'a>(
        layout: &'a SpeakerLayout,
        listener: &'a Listener,
        channels: usize,
        render_channels: usize,
    ) -> MixContext<'a> {
        MixContext {
            listener,
            layout,
            sample_rate: 48000.0,
            channels,
            environment: &TEST_ENVIRONMENT,
            master_gain: 1.0,
            render_channels,
            reverb_input: None,
            wall_reflectivity: 0.9,
            measurement_mode: false,
        }
    }

    /// FDN with render_channels < channels must not touch channels beyond render_channels.
    #[test]
    fn render_channels_limits_wet_output() {
        let channels = 6;
        let render_channels = 2; // HRTF on 5.1 device
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        // Put signal only in channels 0 and 1 (stereo HRTF output)
        for frame in 0..frames {
            buffer[frame * channels] = 0.5;
            buffer[frame * channels + 1] = 0.5;
        }

        fdn.process(&mut buffer, &ctx);

        // Channels 2-5 must remain exactly zero
        for frame in 0..frames {
            for ch in 2..channels {
                assert_eq!(
                    buffer[frame * channels + ch], 0.0,
                    "channel {ch} at frame {frame} should be zero when render_channels={render_channels}"
                );
            }
        }

        // Channels 0-1 should have been modified (dry + wet)
        let energy_ch0: f32 = buffer.iter().step_by(channels).map(|s| s * s).sum();
        let energy_ch1: f32 = buffer.iter().skip(1).step_by(channels).map(|s| s * s).sum();
        assert!(energy_ch0 > 0.0, "channel 0 should have signal");
        assert!(energy_ch1 > 0.0, "channel 1 should have signal");
    }

    /// When render_channels == channels, all channels get wet signal (normal behavior).
    #[test]
    fn full_channels_all_get_wet() {
        let channels = 6;
        let render_channels = 6;
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        // Put signal in all channels
        for sample in buffer.iter_mut() {
            *sample = 0.5;
        }

        fdn.process(&mut buffer, &ctx);

        // All channels should have been processed (not just passthrough)
        for ch in 0..channels {
            let max_abs = (0..frames)
                .map(|f| buffer[f * channels + ch].abs())
                .fold(0.0f32, f32::max);
            assert!(
                max_abs > 0.0,
                "channel {ch} should have signal with render_channels={render_channels}"
            );
        }
    }

    /// FDN only sums mono from render_channels, ignoring higher channels.
    #[test]
    fn mono_sum_only_from_render_channels() {
        let channels = 6;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn_a = FdnReverbStage::new();
        let mut fdn_b = FdnReverbStage::new();
        fdn_a.init(&ctx);
        fdn_b.init(&ctx);

        let frames = 1024;

        // Buffer A: signal in channels 0-1, noise in channels 2-5
        let mut buffer_a = vec![0.0f32; frames * channels];
        for frame in 0..frames {
            buffer_a[frame * channels] = 0.5;
            buffer_a[frame * channels + 1] = 0.5;
            for ch in 2..channels {
                buffer_a[frame * channels + ch] = 99.0; // should be ignored
            }
        }

        // Buffer B: same signal in channels 0-1, zeros in 2-5
        let mut buffer_b = vec![0.0f32; frames * channels];
        for frame in 0..frames {
            buffer_b[frame * channels] = 0.5;
            buffer_b[frame * channels + 1] = 0.5;
        }

        fdn_a.process(&mut buffer_a, &ctx);
        fdn_b.process(&mut buffer_b, &ctx);

        // Channels 0-1 should be identical regardless of what's in channels 2-5
        for frame in 0..frames {
            for ch in 0..2 {
                let a = buffer_a[frame * channels + ch];
                let b = buffer_b[frame * channels + ch];
                assert!(
                    (a - b).abs() < 1e-6,
                    "ch{ch} frame {frame}: content beyond render_channels should not affect output ({a} vs {b})"
                );
            }
        }
    }

    /// Quad channel mode on 5.1 hardware: FDN only writes to active channels [0,1,4,5].
    /// Channels 2 (C) and 3 (LFE) must stay silent despite render_channels=6.
    #[test]
    fn active_mask_respects_quad_on_5_1() {
        let channels = 6;
        let render_channels = 6;
        let (mut layout, listener) = make_ctx(channels, render_channels);
        // Quad channel mode: only FL(0), FR(1), RL(4), RR(5) active
        layout.set_active_channels(&[0, 1, 4, 5]);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        // Signal in active channels only
        for frame in 0..frames {
            buffer[frame * channels] = 0.5; // FL
            buffer[frame * channels + 1] = 0.5; // FR
            buffer[frame * channels + 4] = 0.5; // RL
            buffer[frame * channels + 5] = 0.5; // RR
        }

        fdn.process(&mut buffer, &ctx);

        // Channels 2 (C) and 3 (LFE) must remain zero
        for frame in 0..frames {
            for ch in [2, 3] {
                assert_eq!(
                    buffer[frame * channels + ch],
                    0.0,
                    "channel {ch} at frame {frame} should be zero in quad mode"
                );
            }
        }

        // Active channels should have signal
        for ch in [0, 1, 4, 5] {
            let has_signal = (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10);
            assert!(has_signal, "active channel {ch} should have signal");
        }
    }

    /// Silent input produces silent output (no self-oscillation).
    #[test]
    fn silent_input_silent_output() {
        let channels = 6;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let frames = 512;
        let mut buffer = vec![0.0f32; frames * channels];
        fdn.process(&mut buffer, &ctx);

        let max = buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max < 1e-10, "silent input should produce silent output");
    }

    /// When reverb_input is provided (DBAP/VBAP/HRTF mono bus on ch0),
    /// FDN reads ch0 directly and produces wet output on active channels.
    #[test]
    fn reverb_input_mono_bus_produces_wet_on_active_channels() {
        let channels = 6;
        let render_channels = 6;
        let (mut layout, listener) = make_ctx(channels, render_channels);
        // Quad mask: FL(0), FR(1), RL(4), RR(5) active; C(2), LFE(3) masked.
        layout.set_active_channels(&[0, 1, 4, 5]);

        // Long enough to span the late-reverb onset: the hall-scale delay lines
        // start emitting only after pre-delay (~0.9k samples in the test room) plus
        // the shortest delay (~1.3k), so a short window would see no wet yet.
        let frames = 8192;
        let mut reverb_input = vec![0.0f32; frames * channels];
        // Write mono signal to ch0 only (mimics renderer mono bus).
        for frame in 0..frames {
            reverb_input[frame * channels] = 0.5;
        }

        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels,
            environment: &TEST_ENVIRONMENT,
            master_gain: 1.0,
            render_channels,
            reverb_input: Some(&reverb_input),
            wall_reflectivity: 0.9,
            measurement_mode: false,
        };

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let mut buffer = vec![0.0f32; frames * channels];
        fdn.process(&mut buffer, &ctx);

        // Active channels (0, 1, 4, 5) should have wet signal.
        let active_energy: f32 = (0..frames)
            .map(|f| {
                let base = f * channels;
                [0, 1, 4, 5]
                    .iter()
                    .map(|&ch| buffer[base + ch].powi(2))
                    .sum::<f32>()
            })
            .sum();
        assert!(
            active_energy > 0.01,
            "active channels should have wet signal"
        );

        // Masked channels (2, 3) should remain silent.
        let masked_energy: f32 = (0..frames)
            .map(|f| {
                let base = f * channels;
                buffer[base + 2].powi(2) + buffer[base + 3].powi(2)
            })
            .sum();
        assert!(
            masked_energy < 1e-10,
            "masked channels should stay silent, got energy {masked_energy}"
        );
    }

    /// Output normalization adapts to wall materials: absorptive rooms get
    /// less attenuation (higher output_normalization) because the Jot feedback
    /// gains are lower, causing less steady-state energy buildup in the network.
    #[test]
    fn output_normalization_scales_with_materials() {
        let channels = 2;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);

        // Dead room: acoustic tile on all walls → high absorption → short RT60
        let dead_environment = Environment::uniform(
            TEST_ENVIRONMENT.min,
            TEST_ENVIRONMENT.max,
            WallMaterial::ceiling_tile(),
        );
        let ctx_dead = MixContext {
            environment: &dead_environment,
            ..mix_ctx(&layout, &listener, channels, render_channels)
        };
        let mut fdn_dead = FdnReverbStage::new();
        fdn_dead.init(&ctx_dead);

        // Live room: hard walls → low absorption → long RT60
        let ctx_live = mix_ctx(&layout, &listener, channels, render_channels);
        let mut fdn_live = FdnReverbStage::new();
        fdn_live.init(&ctx_live);

        assert!(
            fdn_dead.output_normalization > fdn_live.output_normalization,
            "dead room norm ({}) should exceed live room norm ({})",
            fdn_dead.output_normalization,
            fdn_live.output_normalization
        );
    }

    /// Verify the actual output normalization for the default atrium room (6×4×3m, hard walls).
    /// This is the generic FDN used by VBAP, HRTF, and DBAP modes.
    #[test]
    fn output_normalization_atrium_room() {
        let channels = 6;
        let render_channels = 6;
        let (layout, listener) = make_ctx(channels, render_channels);

        // Actual atrium: 6×4×3m room, hard walls
        let atrium_environment = Environment::uniform(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 4.0, 3.0),
            WallMaterial::HARD_WALL,
        );
        let ctx = MixContext {
            environment: &atrium_environment,
            ..mix_ctx(&layout, &listener, channels, render_channels)
        };

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        // Hard walls produce long RT60 → high feedback gains → low output normalization.
        // The exact value depends on material-derived RT60 at 500 Hz (3.39 s) and
        // 4 kHz (1.57 s). Measured 0.691. It was 0.49 while the 1.57 s decay sat
        // at Nyquist: the damping filters now reach it at 4 kHz and damp 4–24 kHz
        // harder, so the network builds up less energy (its measured wet/send
        // level moved only +0.5 dB, −15.5 → −15.0 dB, so the estimate tracks it).
        eprintln!(
            "Generic FDN output_normalization = {:.4} (atrium 6x4x3, hard walls)",
            fdn.output_normalization
        );
        assert!(
            fdn.output_normalization > 0.05 && fdn.output_normalization < 0.8,
            "atrium room norm should be between 0.05 and 0.8, got {}",
            fdn.output_normalization
        );
    }

    /// Sweep wall_reflectivity and RT60 high-frequency ratio to show how each
    /// affects normalization and actual wet tail energy.
    ///
    /// Feeds a single impulse through the FDN and measures:
    /// - output_normalization (structural gain compensation)
    /// - RT60 low (from Sabine)
    /// - wet tail RMS (actual energy in the reverb tail after the impulse)
    #[test]
    fn parameter_sweep_reflectivity_and_hf_ratio() {
        use crate::pipeline::room_acoustics;

        let channels = 2;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);
        let sample_rate = 48000.0;

        let environment_min = Vec3::new(0.0, 0.0, 0.0);
        let environment_max = Vec3::new(6.0, 4.0, 3.0);
        let (volume, surface_area) =
            room_acoustics::room_geometry(environment_min, environment_max);
        let environment =
            Environment::uniform(environment_min, environment_max, WallMaterial::HARD_WALL);

        let reflectivities = [0.70, 0.75, 0.80, 0.85, 0.90, 0.95];
        let hf_ratios = [0.3, 0.4, 0.5, 0.6];

        eprintln!();
        eprintln!("═══ Parameter sweep: 6×4×3m atrium ═══");
        eprintln!(
            "{:>6}  {:>8}  {:>8}  {:>8}  {:>12}",
            "refl", "hf_ratio", "RT60_lo", "norm", "tail_rms_dB"
        );
        eprintln!("{}", "─".repeat(50));

        for &refl in &reflectivities {
            let rt60_low = room_acoustics::sabine_rt60(volume, surface_area, refl);

            for &hf_ratio in &hf_ratios {
                let rt60_high = rt60_low * hf_ratio;

                let mut fdn = FdnReverbStage::new();
                fdn.compute_delays(sample_rate);
                fdn.compute_pre_delay(sample_rate, volume, surface_area, 343.42);
                fdn.compute_damping(sample_rate, rt60_low, rt60_high);
                fdn.initialized = true;

                // Feed a single impulse and measure the tail energy.
                // Use reverb_input to control exactly what goes in (mono bus on ch0).
                let frames = 48000; // 1 second
                let mut buffer = vec![0.0f32; frames * channels];
                let mut reverb_input = vec![0.0f32; frames * channels];
                reverb_input[0] = 1.0; // single impulse on ch0

                let ctx = MixContext {
                    environment: &environment,
                    wall_reflectivity: refl,
                    reverb_input: Some(&reverb_input),
                    ..mix_ctx(&layout, &listener, channels, render_channels)
                };

                fdn.process(&mut buffer, &ctx);

                // Measure wet tail RMS (skip first 50ms to avoid the direct impulse).
                let skip_samples = (0.05 * sample_rate) as usize;
                let tail_energy: f32 = buffer[skip_samples * channels..]
                    .iter()
                    .step_by(channels) // ch0 only
                    .map(|s| s * s)
                    .sum();
                let tail_samples = (frames - skip_samples) as f32;
                let tail_rms = (tail_energy / tail_samples).sqrt();
                let tail_rms_db = if tail_rms > 1e-10 {
                    20.0 * tail_rms.log10()
                } else {
                    -100.0
                };

                eprintln!(
                    "{:>6.2}  {:>8.1}  {:>8.3}  {:>8.4}  {:>12.1}",
                    refl, hf_ratio, rt60_low, fdn.output_normalization, tail_rms_db
                );
            }
        }
        eprintln!();
    }

    /// Break down the energy budget between early reflections and FDN late tail
    /// for the default atrium room. Shows which stage dominates the perceived echo.
    #[test]
    fn energy_breakdown_early_vs_late() {
        use crate::pipeline::path::{PathResolver, PathSet, ResolveContext};
        use crate::pipeline::path_resolvers::ImageSourceResolver;
        use crate::pipeline::room_acoustics;

        let channels = 2;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);
        let sample_rate = 48000.0;

        let environment_min = Vec3::new(0.0, 0.0, 0.0);
        let environment_max = Vec3::new(6.0, 4.0, 3.0);
        let (volume, surface_area) =
            room_acoustics::room_geometry(environment_min, environment_max);
        let hard_environment =
            Environment::uniform(environment_min, environment_max, WallMaterial::HARD_WALL);

        let listener_pos = Vec3::new(3.0, 2.0, 0.0); // center of room
        let source_positions = [
            ("djembe (center, 1.5m orbit)", Vec3::new(4.5, 2.0, 0.0)),
            ("campfire (corner)", Vec3::new(1.0, 1.0, 0.0)),
            ("purring (front-right)", Vec3::new(5.0, 3.0, 0.0)),
        ];

        let reflectivities = [0.75, 0.90];

        for &refl in &reflectivities {
            let rt60 = room_acoustics::sabine_rt60(volume, surface_area, refl);
            let d_c = room_acoustics::critical_distance(volume, rt60, 1.0);

            eprintln!();
            eprintln!("═══ Energy breakdown: r={refl}, RT60={rt60:.3}s, d_c={d_c:.3}m ═══");
            eprintln!(
                "{:>25}  {:>6}  {:>10}  {:>12}  {:>12}  {:>10}",
                "source", "dist", "send (d/dc)", "refl_energy", "fdn_tail_dB", "ratio E/L"
            );
            eprintln!("{}", "─".repeat(85));

            let resolver = ImageSourceResolver;
            let resolver_environment = Environment::uniform(
                environment_min,
                environment_max,
                WallMaterial::with_reflectivity(refl),
            );

            for (name, source_pos) in &source_positions {
                let dist = source_pos.distance_to(listener_pos);
                let send = room_acoustics::reverb_send(dist, d_c);

                // ── Early reflections: path resolver gives us the gains ──
                let mut paths = PathSet::new();
                let resolve_ctx = ResolveContext {
                    source_pos: *source_pos,
                    target_pos: listener_pos,
                    environment: &resolver_environment,
                };
                resolver.resolve(&resolve_ctx, &mut paths);

                let mut early_energy = 0.0f32;
                for path in paths.as_slice() {
                    if path.kind == crate::pipeline::path::PathKind::Reflection {
                        // Each reflection has gain = reflectivity / image_dist
                        early_energy += path.gain * path.gain;
                    }
                }

                // ── FDN late tail: feed impulse × send, measure tail ──
                // Use band-specific RT60 from wall materials (hard walls for this test).
                let wall_areas =
                    room_acoustics::wall_surface_areas(environment_min, environment_max);
                let atmosphere = AtmosphericParams::default();
                let rt60_high = room_acoustics::sabine_rt60_at_band(
                    volume,
                    &wall_areas,
                    &TEST_ENVIRONMENT.wall_materials,
                    &atmosphere,
                    RT60_HIGH_BAND,
                );
                let mut fdn = FdnReverbStage::new();
                fdn.compute_delays(sample_rate);
                fdn.compute_pre_delay(sample_rate, volume, surface_area, 343.42);
                fdn.compute_damping(sample_rate, rt60, rt60_high);
                fdn.initialized = true;

                let frames = 48000; // 1 second
                let mut buffer = vec![0.0f32; frames * channels];
                let mut reverb_input = vec![0.0f32; frames * channels];
                reverb_input[0] = send; // impulse scaled by d/d_c send law

                let ctx = MixContext {
                    environment: &hard_environment,
                    wall_reflectivity: refl,
                    reverb_input: Some(&reverb_input),
                    ..mix_ctx(&layout, &listener, channels, render_channels)
                };

                fdn.process(&mut buffer, &ctx);

                // Measure FDN tail energy (skip first 50ms)
                let skip = (0.05 * sample_rate) as usize;
                let tail_energy: f32 = buffer[skip * channels..]
                    .iter()
                    .step_by(channels)
                    .map(|s| s * s)
                    .sum();
                let tail_rms = (tail_energy / (frames - skip) as f32).sqrt();
                let tail_db = if tail_rms > 1e-10 {
                    20.0 * tail_rms.log10()
                } else {
                    -100.0
                };

                let ratio = if tail_energy > 1e-20 {
                    early_energy / tail_energy
                } else {
                    f32::INFINITY
                };

                eprintln!(
                    "{:>25}  {:>6.2}  {:>10.4}  {:>12.6}  {:>12.1}  {:>10.1}",
                    name, dist, send, early_energy, tail_db, ratio
                );
            }
        }
        eprintln!();
    }

    // ── Pre-delay from room geometry (Phase 4A) ──────────────────────────

    #[test]
    fn pre_delay_10m_cube() {
        // 10m cube: V=1000, S=600 → MFP = 4×1000/600 = 6.667m
        // t = 6.667 / 343.42 ≈ 19.4ms → 932 samples at 48kHz
        let mut fdn = FdnReverbStage::new();
        fdn.compute_pre_delay(48000.0, 1000.0, 600.0, 343.42);
        let expected_ms = 19.4;
        let actual_ms = fdn.pre_delay_samples as f32 / 48.0;
        assert!(
            (actual_ms - expected_ms).abs() < 1.0,
            "10m cube pre-delay should be ~{expected_ms}ms, got {actual_ms:.1}ms ({} samples)",
            fdn.pre_delay_samples
        );
    }

    #[test]
    fn pre_delay_small_room() {
        // 6×4×3m atrium: V=72, S=108 → MFP = 4×72/108 = 2.667m
        // t = 2.667 / 343.42 ≈ 7.8ms → 373 samples at 48kHz
        let mut fdn = FdnReverbStage::new();
        let (volume, surface_area) =
            room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(6.0, 4.0, 3.0));
        fdn.compute_pre_delay(48000.0, volume, surface_area, 343.42);
        let expected_ms = 7.8;
        let actual_ms = fdn.pre_delay_samples as f32 / 48.0;
        assert!(
            (actual_ms - expected_ms).abs() < 1.0,
            "6×4×3m room pre-delay should be ~{expected_ms}ms, got {actual_ms:.1}ms ({} samples)",
            fdn.pre_delay_samples
        );
    }

    #[test]
    fn pre_delay_varies_with_room_size() {
        let mut fdn_small = FdnReverbStage::new();
        let mut fdn_large = FdnReverbStage::new();

        // Small room: 3×3×3m
        let (vol_s, sa_s) = room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(3.0, 3.0, 3.0));
        fdn_small.compute_pre_delay(48000.0, vol_s, sa_s, 343.42);

        // Large room: 20×15×10m
        let (vol_l, sa_l) = room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(20.0, 15.0, 10.0));
        fdn_large.compute_pre_delay(48000.0, vol_l, sa_l, 343.42);

        assert!(
            fdn_large.pre_delay_samples > fdn_small.pre_delay_samples,
            "larger room should have longer pre-delay: {} vs {} samples",
            fdn_large.pre_delay_samples,
            fdn_small.pre_delay_samples
        );
    }

    #[test]
    fn pre_delay_degenerate_room_uses_fallback() {
        let mut fdn = FdnReverbStage::new();
        // Zero volume room → should use fallback (20ms)
        fdn.compute_pre_delay(48000.0, 0.0, 0.0, 343.42);
        let expected_samples = (PRE_DELAY_FALLBACK_SECONDS * 48000.0) as usize;
        assert_eq!(
            fdn.pre_delay_samples, expected_samples,
            "degenerate room should use fallback pre-delay"
        );
    }

    // ── HF decay from materials (Phase 4B) ────────────────────────────

    #[test]
    fn hard_walls_rt60_ratio_near_unity() {
        // Hard walls have nearly uniform absorption across bands (0.02–0.05),
        // so RT60 at 500 Hz and 4 kHz should be similar (ratio near 1.0).
        let environment_min = Vec3::ZERO;
        let environment_max = Vec3::new(6.0, 4.0, 3.0);
        let (volume, _) = room_acoustics::room_geometry(environment_min, environment_max);
        let wall_areas = room_acoustics::wall_surface_areas(environment_min, environment_max);
        let atmosphere = AtmosphericParams::default();

        let rt60_low = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &TEST_ENVIRONMENT.wall_materials,
            &atmosphere,
            RT60_LOW_BAND,
        );
        let rt60_high = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &TEST_ENVIRONMENT.wall_materials,
            &atmosphere,
            RT60_HIGH_BAND,
        );

        let ratio = rt60_high / rt60_low;
        // Hard walls: alpha at 500 Hz = 0.03, at 4 kHz = 0.05 + air absorption.
        // Ratio should be between 0.5 and 1.0 (close to 1 but air absorption pulls HF down).
        assert!(
            ratio > 0.4 && ratio < 1.0,
            "hard walls RT60 ratio should be near 1.0, got {ratio:.3} (low={rt60_low:.3}s, high={rt60_high:.3}s)"
        );
    }

    #[test]
    fn carpet_walls_have_lower_hf_ratio() {
        // Carpet has much higher absorption at 4 kHz (0.40) vs 500 Hz (0.08),
        // so RT60_high / RT60_low should be significantly lower than hard walls.
        let environment_min = Vec3::ZERO;
        let environment_max = Vec3::new(6.0, 4.0, 3.0);
        let (volume, _) = room_acoustics::room_geometry(environment_min, environment_max);
        let wall_areas = room_acoustics::wall_surface_areas(environment_min, environment_max);
        let atmosphere = AtmosphericParams::default();

        let carpet_materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::carpet());

        let rt60_low = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &carpet_materials,
            &atmosphere,
            RT60_LOW_BAND,
        );
        let rt60_high = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &carpet_materials,
            &atmosphere,
            RT60_HIGH_BAND,
        );

        let ratio = rt60_high / rt60_low;
        assert!(
            ratio < 0.5,
            "carpet walls should have RT60_high/RT60_low < 0.5, got {ratio:.3} (low={rt60_low:.3}s, high={rt60_high:.3}s)"
        );
    }

    #[test]
    fn fdn_init_uses_material_derived_damping() {
        // Verify that FDN init produces different damping for different materials.
        let environment_min = Vec3::ZERO;
        let environment_max = Vec3::new(6.0, 4.0, 3.0);

        let hard_environment =
            Environment::uniform(environment_min, environment_max, WallMaterial::HARD_WALL);
        let carpet_environment =
            Environment::uniform(environment_min, environment_max, WallMaterial::carpet());

        let layout = SpeakerLayout::new(&[], None, 2);
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        let mut fdn_hard = FdnReverbStage::new();
        let ctx_hard = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            environment: &hard_environment,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            measurement_mode: false,
        };
        fdn_hard.init(&ctx_hard);

        let mut fdn_carpet = FdnReverbStage::new();
        let ctx_carpet = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            environment: &carpet_environment,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            measurement_mode: false,
        };
        fdn_carpet.init(&ctx_carpet);

        // Hard walls should have higher output normalization (less damping, more buildup)
        // than carpet walls (more damping, less buildup).
        assert!(
            fdn_hard.output_normalization < fdn_carpet.output_normalization,
            "hard walls should have lower output_norm (more energy buildup) than carpet: hard={}, carpet={}",
            fdn_hard.output_normalization, fdn_carpet.output_normalization
        );
    }

    #[test]
    fn pre_delay_clamped_minimum() {
        let mut fdn = FdnReverbStage::new();
        // Very tiny room: 0.1×0.1×0.1m → MFP ≈ 0.067m → t ≈ 0.19ms
        // Should clamp to 5ms minimum
        let (volume, surface_area) =
            room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(0.1, 0.1, 0.1));
        fdn.compute_pre_delay(48000.0, volume, surface_area, 343.42);
        let min_samples = (0.005 * 48000.0) as usize;
        assert!(
            fdn.pre_delay_samples >= min_samples,
            "pre-delay should be at least 5ms ({min_samples} samples), got {}",
            fdn.pre_delay_samples
        );
    }

    /// Values below the knee (±14.0) pass through unchanged.
    #[test]
    fn feedback_soft_clamp_transparent_below_knee() {
        for &v in &[0.0, 1.0, -1.0, 5.0, -10.0, 13.99, -13.99] {
            let result = feedback_soft_clamp(v);
            assert_eq!(result, v, "value {v} should pass through unchanged");
        }
    }

    /// Extreme values are smoothly limited below ±16.0.
    #[test]
    fn feedback_soft_clamp_limits_extreme() {
        let result_pos = feedback_soft_clamp(100.0);
        assert!(
            result_pos > 14.0 && result_pos < 16.0,
            "100.0 should clamp below 16.0, got {result_pos}"
        );
        let result_neg = feedback_soft_clamp(-100.0);
        assert!(
            result_neg < -14.0 && result_neg > -16.0,
            "-100.0 should clamp above -16.0, got {result_neg}"
        );
        // Exact knee value passes through
        assert_eq!(feedback_soft_clamp(14.0), 14.0);
        assert_eq!(feedback_soft_clamp(-14.0), -14.0);
    }

    /// FDN output is linear — no soft clipping applied. MasterGainStage is
    /// the single point of output limiting. Verify that a buffer with values
    /// above the soft-clip knee (0.9) passes through without compression.
    #[test]
    fn fdn_output_is_linear() {
        let (layout, listener) = make_ctx(2, 2);
        let ctx = mix_ctx(&layout, &listener, 2, 2);
        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        // Set every sample to 1.5 (well above the 0.9 soft-clip knee).
        // With no reverb input, the FDN fallback mono-sums the buffer,
        // but output_normalization is small — the key test is that the
        // dry signal at 1.5 is NOT compressed by soft_clip.
        let frames = 64;
        let channels = 2;
        let mut buffer = vec![1.5f32; frames * channels];

        fdn.process(&mut buffer, &ctx);

        // If soft_clip were active, no sample could exceed 1.0.
        // With linear output, dry (1.5) + wet should stay above 1.0.
        let max_sample = buffer.iter().copied().fold(0.0f32, f32::max);
        assert!(
            max_sample > 1.0,
            "FDN output should be linear (no soft-clip at 0.9 knee). \
             Max sample = {max_sample}, expected > 1.0"
        );
        // Stability ceiling still enforced
        assert!(
            max_sample <= 100.0,
            "FDN output should respect ±100.0 stability ceiling. Max = {max_sample}"
        );
    }

    /// At 96 kHz, all 16 delay lines must remain distinct (not clamped to the
    /// same value). The buffer should grow to accommodate scaled delays.
    #[test]
    fn delays_preserved_at_96khz() {
        let (layout, listener) = make_ctx(2, 2);
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 96000.0,
            channels: 2,
            environment: &TEST_ENVIRONMENT,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            measurement_mode: false,
        };
        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        // All 16 delays should be distinct (coprime ratios preserved)
        let mut unique_delays: Vec<usize> = fdn.delays.to_vec();
        unique_delays.sort();
        unique_delays.dedup();
        assert_eq!(
            unique_delays.len(),
            NUM_LINES,
            "all 16 delays should be distinct at 96 kHz, got {:?}",
            fdn.delays
        );

        // Longest delay should be 3347 × 2 = 6694 at 96 kHz.
        let max_delay = *fdn.delays.iter().max().unwrap();
        assert!(
            max_delay > 5000,
            "longest delay at 96 kHz should be ~6694, got {max_delay}"
        );

        // Buffer should have grown beyond the 48 kHz minimum of 512
        assert!(
            fdn.buffer_size >= 1024,
            "buffer should be >= 1024 at 96 kHz, got {}",
            fdn.buffer_size
        );
    }

    /// required_buffer_size scales correctly with sample rate.
    #[test]
    fn buffer_size_scales_with_sample_rate() {
        // Longest delay 3347 + 10 modulation + 2 interpolation taps @ 48 kHz
        // → next_pow2(3360) = 4096.
        assert_eq!(FdnReverbStage::required_buffer_size(48000.0), 4096);
        assert!(FdnReverbStage::required_buffer_size(96000.0) >= 8192);
        assert!(FdnReverbStage::required_buffer_size(44100.0) >= 4096);
        // Very high sample rate
        assert!(FdnReverbStage::required_buffer_size(192000.0) >= 16384);
    }

    /// Modal-density guard — the fix for tinny/metallic reverb. Mode spacing is
    /// Δf = fs / Σ(delays); sparse modes (large Δf) ring metallically on a long
    /// tail. The old box-scale delays gave ~15.5 Hz; hall-scale delays must give
    /// well under 4 Hz so modes pack densely enough for a smooth wash.
    #[test]
    fn delay_lines_dense_enough_for_smooth_tail() {
        let fs = 48_000.0;
        let mut fdn = FdnReverbStage::new();
        fdn.compute_delays(fs);

        let sum: usize = fdn.delays.iter().sum();
        let mode_spacing = fs / sum as f32;
        // 16 hall-scale lines target ~1.3 Hz — below Schroeder's overlap threshold
        // (~1.4 Hz for a 1.6 s tail), so modes overlap and the tail is smooth.
        assert!(
            mode_spacing < 2.0,
            "FDN mode spacing should be < 2 Hz for a smooth (non-metallic) tail, \
             got {mode_spacing:.1} Hz (Σdelays={sum})"
        );

        // Delays must stay mutually distinct (coprime spread preserved).
        let mut uniq = fdn.delays.to_vec();
        uniq.sort();
        uniq.dedup();
        assert_eq!(
            uniq.len(),
            NUM_LINES,
            "all delay lengths should be distinct: {:?}",
            fdn.delays
        );
    }

    /// Benchmark: measure FDN processing cost per second of audio.
    /// Run with `cargo test fdn_benchmark -- --nocapture` to see timing.
    #[test]
    fn fdn_benchmark() {
        let channels = 6;
        let render_channels = 6;
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let sample_rate = 48000.0;
        let seconds = 10.0;
        let total_frames = (sample_rate * seconds) as usize;
        let frames_per_buffer = 1024;
        let num_buffers = total_frames / frames_per_buffer;

        // Prepare a buffer with signal and a reverb input
        let buf_len = frames_per_buffer * channels;
        let mut buffer = vec![0.3f32; buf_len];
        let reverb_input = vec![0.2f32; buf_len];

        let ctx_with_reverb = MixContext {
            reverb_input: Some(&reverb_input),
            ..mix_ctx(&layout, &listener, channels, render_channels)
        };

        // Warm up
        for _ in 0..10 {
            buffer.fill(0.3);
            fdn.process(&mut buffer, &ctx_with_reverb);
        }

        let start = std::time::Instant::now();
        for _ in 0..num_buffers {
            buffer.fill(0.3);
            fdn.process(&mut buffer, &ctx_with_reverb);
        }
        let elapsed = start.elapsed();

        let ms_per_second_of_audio = elapsed.as_secs_f64() / seconds * 1000.0;
        let cpu_percent = elapsed.as_secs_f64() / seconds * 100.0;

        eprintln!();
        eprintln!("═══ FDN Benchmark (6ch, 48kHz) ═══");
        eprintln!("  Audio processed: {seconds}s ({total_frames} frames)");
        eprintln!("  Wall time: {:.2}ms", elapsed.as_secs_f64() * 1000.0);
        eprintln!("  Per second of audio: {ms_per_second_of_audio:.2}ms");
        eprintln!("  CPU load: {cpu_percent:.2}%");
        eprintln!(
            "  Per sample: {:.0}ns",
            elapsed.as_nanos() as f64 / total_frames as f64
        );
        eprintln!();

        // Sanity: FDN should use a small fraction of a core for 6ch at 48kHz.
        // 16 delay lines roughly double the 8-line cost (~10% of a core, measured
        // in isolation); the bound is loose to stay reliable under parallel test
        // load while still catching gross regressions.
        assert!(
            cpu_percent < 20.0,
            "FDN should use < 20% CPU, got {cpu_percent:.1}%"
        );
    }

    // ── Configurable reverb: RT60 / pre-delay overrides, voicing, HPF ─────────

    /// Total wet energy the FDN adds to a buffer (output − input), isolating the
    /// tail from any dry passthrough already in the buffer.
    fn added_wet_energy(fdn: &mut FdnReverbStage, ctx: &MixContext, buffer: &mut [f32]) -> f32 {
        let input = buffer.to_vec();
        fdn.process(buffer, ctx);
        buffer
            .iter()
            .zip(&input)
            .map(|(out, dry)| {
                let wet = out - dry;
                wet * wet
            })
            .sum()
    }

    /// An explicit RT60 override must lengthen the decay: a 3.5 s tail retains far
    /// more late energy than a 1.0 s tail, independent of room geometry.
    #[test]
    fn rt60_override_slows_decay() {
        let channels = 6;
        let (layout, listener) = make_ctx(channels, channels);
        let ctx = mix_ctx(&layout, &listener, channels, channels);

        let tail_energy = |rt60: f32| -> f32 {
            let mut fdn = FdnReverbStage::with_config(FdnConfig {
                rt60_seconds: Some(rt60),
                ..FdnConfig::default()
            });
            fdn.init(&ctx);
            let frames = 48_000; // 1 s
            let mut buffer = vec![0.0f32; frames * channels];
            buffer[0] = 1.0; // impulse into channel 0 (mono-summed as FDN input)
            fdn.process(&mut buffer, &ctx);
            // Energy in the last 0.5 s — pure tail, dry impulse long gone.
            let start = (frames / 2) * channels;
            buffer[start..].iter().map(|s| s * s).sum()
        };

        let long = tail_energy(3.5);
        let short = tail_energy(1.0);
        assert!(
            long > short * 2.0,
            "RT60=3.5s tail should retain much more energy than RT60=1.0s: {long} vs {short}"
        );
    }

    /// A pre-delay override sets the delay directly (20 ms @ 48 kHz = 960 samples),
    /// bypassing the mean-free-path value the test room would otherwise produce.
    #[test]
    fn pre_delay_override_sets_samples() {
        let channels = 6;
        let (layout, listener) = make_ctx(channels, channels);
        let ctx = mix_ctx(&layout, &listener, channels, channels);

        let mut fdn = FdnReverbStage::with_config(FdnConfig {
            pre_delay_ms: Some(20.0),
            ..FdnConfig::default()
        });
        fdn.init(&ctx);
        assert_eq!(
            fdn.diagnostics().pre_delay_samples,
            960,
            "20 ms @ 48 kHz should be exactly 960 samples of pre-delay"
        );
    }

    /// Speaker voicing must run the surrounds much wetter than the centre
    /// (idea.md: surrounds > fronts > centre), so an impulse leaves far more
    /// late energy in Ls/Rs (ch 4/5, gain 1.0) than in C (ch 2, gain 0.15).
    #[test]
    fn speaker_voicing_surrounds_wetter_than_center() {
        let channels = 6;
        let (layout, listener) = make_ctx(channels, channels);
        let ctx = mix_ctx(&layout, &listener, channels, channels);

        let mut fdn = FdnReverbStage::with_config(FdnConfig::speaker(Some(2.0), None));
        fdn.init(&ctx);

        let frames = 24_000;
        let mut buffer = vec![0.0f32; frames * channels];
        buffer[0] = 1.0; // impulse into channel 0
        fdn.process(&mut buffer, &ctx);

        let ch_energy = |ch: usize| -> f32 {
            (0..frames)
                .map(|f| {
                    let v = buffer[f * channels + ch];
                    v * v
                })
                .sum::<f32>()
        };
        let center = ch_energy(2); // gain 0.15
        let ls = ch_energy(4); // gain 1.0
        let rs = ch_energy(5); // gain 1.0
        assert!(
            ls > center * 3.0 && rs > center * 3.0,
            "surrounds should be much wetter than centre: Ls={ls}, Rs={rs}, C={center}"
        );
    }

    /// The reverb-send HPF must strip sub-bass out of the tail: a 40 Hz send with
    /// a 160 Hz high-pass produces far less wet energy than with the HPF disabled.
    #[test]
    fn reverb_hpf_attenuates_low_frequency_send() {
        let channels = 6;
        let (layout, listener) = make_ctx(channels, channels);
        let ctx = mix_ctx(&layout, &listener, channels, channels);

        let wet_energy = |hpf: f32| -> f32 {
            let mut fdn = FdnReverbStage::with_config(FdnConfig {
                hpf_cutoff_hz: hpf,
                ..FdnConfig::default()
            });
            fdn.init(&ctx);
            let frames = 48_000;
            let mut buffer = vec![0.0f32; frames * channels];
            for frame in 0..frames {
                let s = (std::f32::consts::TAU * 40.0 * frame as f32 / 48_000.0).sin() * 0.5;
                for ch in 0..channels {
                    buffer[frame * channels + ch] = s;
                }
            }
            added_wet_energy(&mut fdn, &ctx, &mut buffer)
        };

        let with_hpf = wet_energy(160.0);
        let without_hpf = wet_energy(0.0);
        assert!(
            with_hpf < without_hpf * 0.3,
            "160 Hz send HPF should strongly attenuate a 40 Hz tail: with={with_hpf}, without={without_hpf}"
        );
    }

    // ── End-to-end physics: mixing matrix, damping, measured decay ──────────

    const SAMPLE_RATE: f32 = 48_000.0;

    /// Deterministic white noise in [-1, 1) (64-bit LCG, top 24 bits).
    fn white_noise(length: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..length)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ((state >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
            })
            .collect()
    }

    /// A stereo stage initialised in the 10 m hard-walled test cube, then
    /// optionally given an explicit (low, high) RT60 pair directly.
    fn stereo_stage(config: FdnConfig, rt60_low_high: Option<(f32, f32)>) -> FdnReverbStage {
        let (layout, listener) = make_ctx(2, 2);
        let ctx = mix_ctx(&layout, &listener, 2, 2);
        let mut fdn = FdnReverbStage::with_config(config);
        fdn.init(&ctx);
        if let Some((rt60_low, rt60_high)) = rt60_low_high {
            fdn.compute_damping(SAMPLE_RATE, rt60_low, rt60_high);
        }
        fdn
    }

    /// Run `send` through the stage as its mono reverb bus and return the
    /// per-frame wet output of both stereo channels.
    fn stereo_wet_output(fdn: &mut FdnReverbStage, send: &[f32]) -> Vec<[f32; 2]> {
        let channels = 2;
        let (layout, listener) = make_ctx(channels, channels);
        let mut reverb_input = vec![0.0f32; send.len() * channels];
        for (frame, &sample) in send.iter().enumerate() {
            reverb_input[frame * channels] = sample;
        }
        let ctx = MixContext {
            reverb_input: Some(&reverb_input),
            ..mix_ctx(&layout, &listener, channels, channels)
        };
        let mut buffer = vec![0.0f32; send.len() * channels];
        fdn.process(&mut buffer, &ctx);
        buffer
            .chunks_exact(channels)
            .map(|frame| [frame[0], frame[1]])
            .collect()
    }

    /// Mono (L+R) impulse response of the tail for a unit impulse on the send.
    fn impulse_response(fdn: &mut FdnReverbStage, frames: usize) -> Vec<f32> {
        let mut send = vec![0.0f32; frames];
        send[0] = 1.0;
        stereo_wet_output(fdn, &send)
            .iter()
            .map(|frame| frame[0] + frame[1])
            .collect()
    }

    /// Octave band around `center_hz`: two cascaded Butterworth high-passes at
    /// center/√2 and two low-passes at center·√2 (24 dB/oct skirts).
    fn octave_band(signal: &[f32], center_hz: f32) -> Vec<f32> {
        let lower_edge = center_hz / std::f32::consts::SQRT_2;
        let upper_edge = center_hz * std::f32::consts::SQRT_2;
        let mut filters = [
            Biquad::highpass(lower_edge, SAMPLE_RATE),
            Biquad::highpass(lower_edge, SAMPLE_RATE),
            Biquad::lowpass(upper_edge, SAMPLE_RATE),
            Biquad::lowpass(upper_edge, SAMPLE_RATE),
        ];
        signal
            .iter()
            .map(|&sample| {
                filters
                    .iter_mut()
                    .fold(sample, |x, filter| filter.process(x))
            })
            .collect()
    }

    /// Schroeder backward integration: energy remaining after each sample, in
    /// dB relative to the total energy (ISO 3382-1 energy decay curve).
    fn schroeder_decay_db(impulse_response: &[f32]) -> Vec<f64> {
        let mut remaining_energy = vec![0.0f64; impulse_response.len()];
        let mut running_sum = 0.0f64;
        for (index, &sample) in impulse_response.iter().enumerate().rev() {
            running_sum += f64::from(sample) * f64::from(sample);
            remaining_energy[index] = running_sum;
        }
        let total_energy = running_sum.max(f64::MIN_POSITIVE);
        remaining_energy
            .iter()
            .map(|&energy| 10.0 * (energy / total_energy).max(1e-30).log10())
            .collect()
    }

    /// RT60 extrapolated from a least-squares line through the decay curve
    /// between -5 dB and -25 dB (the ISO 3382 T20 evaluation range).
    fn rt60_from_decay(decay_db: &[f64]) -> f64 {
        let points: Vec<(f64, f64)> = decay_db
            .iter()
            .enumerate()
            .filter(|(_, &level)| (-25.0..=-5.0).contains(&level))
            .map(|(index, &level)| (index as f64 / f64::from(SAMPLE_RATE), level))
            .collect();
        assert!(points.len() > 100, "decay curve never spans -5..-25 dB");
        let count = points.len() as f64;
        let mean_time = points.iter().map(|point| point.0).sum::<f64>() / count;
        let mean_level = points.iter().map(|point| point.1).sum::<f64>() / count;
        let covariance: f64 = points
            .iter()
            .map(|(time, level)| (time - mean_time) * (level - mean_level))
            .sum();
        let variance: f64 = points
            .iter()
            .map(|(time, _)| (time - mean_time).powi(2))
            .sum();
        let slope_db_per_second = covariance / variance;
        -60.0 / slope_db_per_second
    }

    /// The Walsh–Hadamard mixing matrix must be orthonormal (lossless): every
    /// entry ±1/√16 = ±0.25, H·Hᵀ = I, energy preserved for any input, and —
    /// since the Sylvester Hadamard matrix is symmetric — its own inverse.
    #[test]
    fn hadamard_is_orthonormal_energy_preserving_and_self_inverse() {
        let mut matrix = [[0.0f32; NUM_LINES]; NUM_LINES];
        for (column, column_values) in matrix.iter_mut().enumerate() {
            let mut basis = [0.0f32; NUM_LINES];
            basis[column] = 1.0;
            FdnReverbStage::hadamard(&mut basis);
            *column_values = basis;
        }
        let entry_magnitude = 1.0 / (NUM_LINES as f32).sqrt();
        for row in 0..NUM_LINES {
            for column in 0..NUM_LINES {
                assert!(
                    (matrix[column][row].abs() - entry_magnitude).abs() < 1e-6,
                    "entry ({row},{column}) = {} should be ±{entry_magnitude}",
                    matrix[column][row]
                );
                let dot: f32 = (0..NUM_LINES)
                    .map(|k| matrix[row][k] * matrix[column][k])
                    .sum();
                let identity = if row == column { 1.0 } else { 0.0 };
                assert!(
                    (dot - identity).abs() < 1e-6,
                    "H·Hᵀ[{row}][{column}] = {dot}, expected {identity}"
                );
            }
        }

        let noise = white_noise(NUM_LINES * 64, 7);
        for chunk in noise.chunks_exact(NUM_LINES) {
            let original: [f32; NUM_LINES] = chunk.try_into().unwrap();
            let mut transformed = original;
            FdnReverbStage::hadamard(&mut transformed);
            let energy_in: f32 = original.iter().map(|x| x * x).sum();
            let energy_out: f32 = transformed.iter().map(|x| x * x).sum();
            assert!(
                (energy_out / energy_in - 1.0).abs() < 1e-5,
                "Hadamard must preserve energy: in={energy_in}, out={energy_out}"
            );
            FdnReverbStage::hadamard(&mut transformed);
            for (back, forward) in transformed.iter().zip(&original) {
                assert!(
                    (back - forward).abs() < 1e-5,
                    "H(H(v)) must equal v: {back} vs {forward}"
                );
            }
        }
    }

    /// Analytic one-pole gain |H(e^{jω})| = k / √(1 + p² − 2p·cos ω).
    fn damping_gain(filter: &DampingFilter, frequency_hz: f64, sample_rate: f32) -> f64 {
        let angle = std::f64::consts::TAU * frequency_hz / f64::from(sample_rate);
        let pole = f64::from(filter.p);
        f64::from(filter.k) / (1.0 + pole * pole - 2.0 * pole * angle.cos()).sqrt()
    }

    /// Steady-state amplitude of the filter's response to a unit sine at
    /// `frequency_hz` (48 kHz), measured by quadrature correlation over the
    /// last 960 samples — a whole number of periods at both 500 Hz and 4 kHz.
    fn measured_sine_gain(filter: DampingFilter, frequency_hz: f64) -> f64 {
        let mut filter = filter;
        let angle_per_sample = std::f64::consts::TAU * frequency_hz / f64::from(SAMPLE_RATE);
        let (settle, window) = (4800, 960);
        let (mut in_phase, mut quadrature) = (0.0f64, 0.0f64);
        for sample in 0..settle + window {
            let angle = angle_per_sample * sample as f64;
            let output = f64::from(filter.process(angle.sin() as f32));
            if sample >= settle {
                in_phase += output * angle.sin();
                quadrature += output * angle.cos();
            }
        }
        2.0 * in_phase.hypot(quadrature) / window as f64
    }

    /// The damping filter must hit the Jot per-pass gain at the frequencies the
    /// two RT60s are defined at: 10^(−3m/(RT60_low·fs)) at 500 Hz and
    /// 10^(−3m/(RT60_high·fs)) at 4 kHz.
    ///
    /// Hand derivation, 2237-sample line at 48 kHz, RT60 2.0 s / 1.2 s:
    ///   g_low  = 10^(−3·2237/96000) = 10^(−0.069906) = 0.85132
    ///   g_high = 10^(−3·2237/57600) = 10^(−0.116510) = 0.76470
    ///   v_low  = 1 − cos(2π·500/48000) = 0.0021411, v_high = 1 − cos(π/6) = 0.13397
    ///   r = (g_low/g_high)² = 1.23939
    ///   s = (v_high − r·v_low)/(r − 1) = (0.13397 − 0.00265)/0.23939 = 0.54856
    ///   p = 1 + s − √(s·(2 + s)) = 1.54856 − 1.18240 = 0.36617
    ///   k = g_low·√((1 − p)² + 2p·v_low) = 0.85132·√(0.40174 + 0.00157) = 0.54064
    ///   DC gain k/(1 − p) = 0.85298 (the peak, < 1); Nyquist k/(1 + p) = 0.39574.
    /// The gains are measured by driving the filter with 500 Hz and 4 kHz sines.
    /// Every line at 44.1, 48 and 96 kHz must meet both targets analytically,
    /// with its peak (DC) gain below 1.
    #[test]
    fn damping_filter_hits_jot_gain_at_500_hz_and_4_khz() {
        let mut fdn = FdnReverbStage::new();
        fdn.compute_delays(SAMPLE_RATE);
        fdn.compute_damping(SAMPLE_RATE, 2.0, 1.2);

        let line = BASE_DELAYS.iter().position(|&delay| delay == 2237).unwrap();
        let filter = fdn.damping[line];
        assert!((filter.p - 0.36617).abs() < 1e-4, "pole {}", filter.p);
        assert!((filter.k - 0.54064).abs() < 1e-4, "gain {}", filter.k);
        let dc_gain = filter.k / (1.0 - filter.p);
        let nyquist_gain = filter.k / (1.0 + filter.p);
        assert!((dc_gain - 0.85298).abs() < 1e-4, "DC gain {dc_gain}");
        assert!(
            (nyquist_gain - 0.39574).abs() < 1e-4,
            "Nyquist gain {nyquist_gain}"
        );
        let low_output = measured_sine_gain(filter, 500.0);
        let high_output = measured_sine_gain(filter, 4000.0);
        assert!(
            (low_output - 0.85132).abs() < 2e-4,
            "500 Hz loop gain should be 0.85132, got {low_output}"
        );
        assert!(
            (high_output - 0.76470).abs() < 2e-4,
            "4 kHz loop gain should be 0.76470, got {high_output}"
        );

        // Every line and sample rate: the design points equal
        // room_acoustics::jot_feedback_gain, the documented home of the formula.
        for sample_rate in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut fdn = FdnReverbStage::new();
            fdn.compute_delays(sample_rate);
            fdn.compute_damping(sample_rate, 2.0, 1.2);
            for (index, filter) in fdn.damping.iter().enumerate() {
                let delay_seconds = fdn.delays[index] as f32 / sample_rate;
                let low_gain = damping_gain(filter, 500.0, sample_rate);
                let high_gain = damping_gain(filter, 4000.0, sample_rate);
                let expected_low = f64::from(room_acoustics::jot_feedback_gain(delay_seconds, 2.0));
                let expected_high =
                    f64::from(room_acoustics::jot_feedback_gain(delay_seconds, 1.2));
                assert!(
                    (low_gain - expected_low).abs() < 1e-5,
                    "{sample_rate} Hz line {index}: 500 Hz gain {low_gain} vs {expected_low}"
                );
                assert!(
                    (high_gain - expected_high).abs() < 1e-5,
                    "{sample_rate} Hz line {index}: 4 kHz gain {high_gain} vs {expected_high}"
                );
                let peak_gain = filter.k / (1.0 - filter.p);
                assert!(
                    (0.0..1.0).contains(&filter.p) && peak_gain < 1.0,
                    "{sample_rate} Hz line {index}: pole {} peak gain {peak_gain}",
                    filter.p
                );
            }
        }
    }

    /// Targets a one-pole cannot meet, and the no-damping and lossless limits,
    /// must still give a passive loop with the 500 Hz gain exact.
    /// - 10 s / 0.1 s asks for up to 48 dB per pass between 500 Hz and 4 kHz;
    ///   a one-pole spans at most ≈ 18 dB. The pole stops where DC decays at
    ///   2 × the 500 Hz RT60 (g_dc = √g_low), so 4 kHz is under-damped, never over.
    /// - RT60_high ≥ RT60_low: no damping, p = 0 and k = g_low (flat).
    /// - Infinite or near-infinite RT60: loop gain capped at 0.9999, not 1.
    #[test]
    fn damping_filter_stays_passive_when_targets_are_unreachable() {
        for sample_rate in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut fdn = FdnReverbStage::new();
            fdn.compute_delays(sample_rate);

            fdn.compute_damping(sample_rate, 10.0, 0.1);
            for (index, filter) in fdn.damping.iter().enumerate() {
                let delay_seconds = fdn.delays[index] as f32 / sample_rate;
                let expected_low =
                    f64::from(room_acoustics::jot_feedback_gain(delay_seconds, 10.0));
                let target_high = f64::from(room_acoustics::jot_feedback_gain(delay_seconds, 0.1));
                let low_gain = damping_gain(filter, 500.0, sample_rate);
                let high_gain = damping_gain(filter, 4000.0, sample_rate);
                let peak_gain = f64::from(filter.k) / (1.0 - f64::from(filter.p));
                assert!(
                    (low_gain - expected_low).abs() < 1e-5,
                    "{sample_rate} Hz line {index}: 500 Hz gain {low_gain} vs {expected_low}"
                );
                assert!(
                    (peak_gain - expected_low.sqrt()).abs() < 1e-4,
                    "{sample_rate} Hz line {index}: DC gain {peak_gain} should sit at the cap {}",
                    expected_low.sqrt()
                );
                assert!(
                    high_gain >= target_high && high_gain < low_gain,
                    "{sample_rate} Hz line {index}: 4 kHz gain {high_gain} (target {target_high})"
                );
                assert!(filter.p < 1.0 && peak_gain < 1.0);
            }

            fdn.compute_damping(sample_rate, 1.0, 2.0);
            for (index, filter) in fdn.damping.iter().enumerate() {
                let delay_seconds = fdn.delays[index] as f32 / sample_rate;
                assert_eq!(filter.p, 0.0, "line {index}: no HF damping requested");
                assert!(
                    (filter.k - room_acoustics::jot_feedback_gain(delay_seconds, 1.0)).abs() < 1e-6
                );
            }

            for (rt60_low, rt60_high) in [(f32::INFINITY, f32::INFINITY), (1.0e6, 0.6e6)] {
                fdn.compute_damping(sample_rate, rt60_low, rt60_high);
                for filter in &fdn.damping {
                    let peak_gain = filter.k / (1.0 - filter.p);
                    assert!(
                        peak_gain < 1.0 && peak_gain > 0.9998,
                        "RT60 {rt60_low}: peak gain {peak_gain}"
                    );
                }
                assert!(fdn.output_normalization.is_finite() && fdn.output_normalization > 0.0);
            }
        }
    }

    /// Freeze the delay-modulation LFOs so every read is an integer delay.
    fn without_modulation(mut fdn: FdnReverbStage) -> FdnReverbStage {
        for modulator in &mut fdn.modulators {
            modulator.depth = 0.0;
        }
        fdn
    }

    /// Measured RT60 of the octave band at `center_hz` (or broadband if `None`).
    fn measured_rt60(response: &[f32], center_hz: Option<f32>) -> f64 {
        match center_hz {
            Some(center) => rt60_from_decay(&schroeder_decay_db(&octave_band(response, center))),
            None => rt60_from_decay(&schroeder_decay_db(response)),
        }
    }

    /// The Jot + Hadamard core is exact: with a frequency-independent RT60 of
    /// 1.5 s and the LFOs frozen, the Schroeder decay of the real impulse
    /// response (16 lines, Hadamard feedback, damping filters, output combiner)
    /// extrapolates to 1.5 s ± 10 % broadband and in every octave 500 Hz–8 kHz.
    /// (Measured: 1.54 broadband; 1.52 / 1.57 / 1.51 / 1.57 / 1.55 s per band.)
    #[test]
    fn measured_rt60_matches_configured_decay_in_every_band() {
        let mut fdn = without_modulation(stereo_stage(FdnConfig::default(), Some((1.5, 1.5))));
        let response = impulse_response(&mut fdn, (1.6 * SAMPLE_RATE) as usize);
        for band in [
            None,
            Some(500.0),
            Some(1000.0),
            Some(2000.0),
            Some(4000.0),
            Some(8000.0),
        ] {
            let measured = measured_rt60(&response, band);
            assert!(
                (measured - 1.5).abs() < 0.15,
                "flat 1.5 s FDN, band {band:?}: measured RT60 {measured:.3} s"
            );
        }
    }

    /// End-to-end through `init` with the production (modulated) network: an
    /// RT60 override of 2.0 s sets the 500 Hz decay. The measured 500 Hz
    /// octave-band RT60 must be 2.0 s ± 10 % (measured 1.97 s).
    #[test]
    fn measured_mid_band_rt60_matches_override() {
        let mut fdn = stereo_stage(
            FdnConfig {
                rt60_seconds: Some(2.0),
                ..FdnConfig::default()
            },
            None,
        );
        let response = impulse_response(&mut fdn, (1.8 * SAMPLE_RATE) as usize);
        let measured = measured_rt60(&response, Some(500.0));
        assert!(
            (measured - 2.0).abs() < 0.2,
            "500 Hz band should decay with the 2.0 s override, got {measured:.3} s"
        );
    }

    /// `FdnConfig::rt60_seconds` documents that "the 4 kHz decay is derived via
    /// HF_DECAY_RATIO" (0.6 → 1.2 s for a 2.0 s override). The damping filters
    /// meet exactly that per-pass gain at 4 kHz, so the 4 kHz octave band must
    /// measure 1.2 s ± 15 %. LFOs are frozen to isolate the filter mapping.
    ///
    /// Measured 1.37 s, close to the +15 % edge, and not a filter error (the
    /// gain at 4 kHz is pinned by `damping_filter_hits_jot_gain_at_500_hz_and_4_khz`):
    /// a one-pole's RT60 falls across the octave, 1.48 s at 2.83 kHz to 0.91 s
    /// at 5.66 kHz, and a T20 fit follows the slow edge (1.25 s for an ideal
    /// band); the 24 dB/oct band filter's skirts admit slower 2 kHz energy, and
    /// the method itself reads a flat 1.5 s as 1.57 s here. Before the fix,
    /// with the 1.2 s gain at Nyquist, this measured 1.97 s. The production
    /// modulated network measures 1.31 s (1.25 s with the old linear
    /// interpolation; see `modulated_interpolation_preserves_high_band_decay`).
    #[test]
    fn measured_high_band_rt60_matches_hf_decay_ratio() {
        let mut fdn = without_modulation(stereo_stage(
            FdnConfig {
                rt60_seconds: Some(2.0),
                ..FdnConfig::default()
            },
            None,
        ));
        let response = impulse_response(&mut fdn, (1.8 * SAMPLE_RATE) as usize);
        let measured = measured_rt60(&response, Some(4000.0));
        let expected = f64::from(2.0 * HF_DECAY_RATIO);
        assert!(
            (measured - expected).abs() < 0.15 * expected,
            "4 kHz band should decay in {expected:.2} s (HF_DECAY_RATIO), got {measured:.3} s"
        );
    }

    /// The modulated read's interpolation loss is not in the Jot gains, so it
    /// shortens the decay wherever it bites. Linear interpolation averaged
    /// (over the swept fraction f) |H|² = 2/3 + cos(ω)/3 per pass, −0.79 dB at
    /// 8 kHz on every ~48 ms trip round the loop: a flat 1.5 s configuration
    /// measured 1.14 s at 8 kHz and 0.92 s at 12 kHz (frozen LFOs: 1.55 s).
    /// Catmull-Rom loses −0.12 dB at 8 kHz (see `hermite_coefficients`), and
    /// the 8 kHz octave must decay in 1.5 s ± 10 %. Measured 1.41 s (12 kHz
    /// 1.25 s, broadband 1.40 s; white-noise wet/send −15.4 dB, was −16.7 dB,
    /// −12.7 dB frozen).
    #[test]
    fn modulated_interpolation_preserves_high_band_decay() {
        let mut fdn = stereo_stage(FdnConfig::default(), Some((1.5, 1.5)));
        let response = impulse_response(&mut fdn, (1.6 * SAMPLE_RATE) as usize);
        let measured = measured_rt60(&response, Some(8000.0));
        assert!(
            (measured - 1.5).abs() < 0.15,
            "flat 1.5 s FDN should decay in 1.5 s at 8 kHz, got {measured:.3} s"
        );
    }

    /// The modulated read must never add gain, or a lossless-limit loop
    /// (damping capped at `MAXIMUM_LOOP_GAIN` = 0.9999) could reach 1.
    /// |H(e^{jω})| = |Σ c_k e^{−jωk}| over 1025 fractions × 4097 frequencies
    /// in [0, π] must stay ≤ 1 up to f32 coefficient rounding (measured worst
    /// case 1 + 6e-8, at DC, where the coefficients sum to 1), and its product
    /// with the loop-gain cap must stay below 1. A frozen LFO (fraction 0) must
    /// read the integer sample alone. The fraction-averaged power at 8 kHz and
    /// 12 kHz pins the flatness: −0.118 dB and −0.527 dB per pass (hand-derived
    /// by integrating |H|² over the fraction; linear interpolation gives −0.79
    /// and −1.76 dB).
    #[test]
    fn hermite_interpolator_is_passive_at_every_fraction() {
        assert_eq!(hermite_coefficients(0.0), [0.0, 1.0, 0.0, 0.0]);

        let magnitude_squared = |coefficients: &[f32; 4], angle: f64| -> f64 {
            let (real, imaginary) = coefficients.iter().enumerate().fold(
                (0.0f64, 0.0f64),
                |(real, imaginary), (tap, &coefficient)| {
                    let phase = angle * tap as f64;
                    (
                        real + f64::from(coefficient) * phase.cos(),
                        imaginary - f64::from(coefficient) * phase.sin(),
                    )
                },
            );
            real * real + imaginary * imaginary
        };

        let fraction_steps = 1024;
        let frequency_steps = 4096;
        let mut worst_magnitude = 0.0f64;
        let mut worst_at = (0.0f32, 0.0f64);
        for fraction_step in 0..=fraction_steps {
            let fraction = fraction_step as f32 / fraction_steps as f32;
            let coefficients = hermite_coefficients(fraction);
            for frequency_step in 0..=frequency_steps {
                let angle = std::f64::consts::PI * frequency_step as f64 / frequency_steps as f64;
                let magnitude = magnitude_squared(&coefficients, angle).sqrt();
                if magnitude > worst_magnitude {
                    worst_magnitude = magnitude;
                    worst_at = (
                        fraction,
                        angle * f64::from(SAMPLE_RATE) / std::f64::consts::TAU,
                    );
                }
            }
        }
        eprintln!(
            "Hermite worst |H| = 1 + {:.2e} at fraction {}, {:.0} Hz",
            worst_magnitude - 1.0,
            worst_at.0,
            worst_at.1
        );
        assert!(
            worst_magnitude <= 1.0 + 1e-6,
            "interpolator gain {worst_magnitude} at fraction {}, {:.0} Hz",
            worst_at.0,
            worst_at.1
        );
        assert!(worst_magnitude * MAXIMUM_LOOP_GAIN < 1.0);

        for (frequency_hz, expected_db) in [(8000.0, -0.118), (12000.0, -0.527)] {
            let angle = std::f64::consts::TAU * frequency_hz / f64::from(SAMPLE_RATE);
            let mean_power = (0..fraction_steps)
                .map(|fraction_step| {
                    let fraction = (fraction_step as f32 + 0.5) / fraction_steps as f32;
                    magnitude_squared(&hermite_coefficients(fraction), angle)
                })
                .sum::<f64>()
                / fraction_steps as f64;
            let mean_db = 10.0 * mean_power.log10();
            assert!(
                (mean_db - expected_db).abs() < 0.005,
                "{frequency_hz} Hz: fraction-averaged loss {mean_db:.4} dB, expected {expected_db}"
            );
        }
    }

    /// Steady-state wet/send power ratio (total over both channels) for white
    /// noise on the send, through the production (modulated) network.
    fn steady_state_wet_to_send_db(rt60: f32) -> f64 {
        let mut fdn = stereo_stage(FdnConfig::default(), Some((rt60, rt60)));
        let frames = (2.5 * SAMPLE_RATE) as usize;
        let send = white_noise(frames, 11);
        let wet = stereo_wet_output(&mut fdn, &send);
        // Skip 1.5 s: the stored energy is within 0.03 dB of steady state even
        // for RT60 = 4 s (1 − 10^(−6·1.5/4) = 0.994).
        let start = (1.5 * SAMPLE_RATE) as usize;
        let send_power: f64 = send[start..]
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum();
        let wet_power: f64 = wet[start..]
            .iter()
            .map(|frame| f64::from(frame[0]).powi(2) + f64::from(frame[1]).powi(2))
            .sum();
        10.0 * (wet_power / send_power).log10()
    }

    /// Gain staging of the send → wet path, pinned so it cannot drift silently.
    /// `output_normalization` divides out the network's energy build-up
    /// (1/(1−g²)) so the wet level should not depend on RT60. Measured
    /// wet/send power: −18.5 / −16.1 / −15.2 / −15.1 dB at RT60 0.5 / 1 / 2 / 4 s
    /// — held within 3.5 dB over an 8× RT60 range (−19.4 / −17.2 / −16.6 /
    /// −16.8 dB with the old linear interpolation). The absolute level sits
    /// ~16 dB below the send: −12 dB is the output combiner extracting the
    /// per-line average (1/N, N = 16), the rest is loop loss g² and the
    /// modulated read (interpolation loss above ~10 kHz plus a ~2 dB deficit
    /// against frozen LFOs that better interpolation does not remove). Hence
    /// at d = d_c (send 0.5, −6 dB) the tail is ~22 dB under the direct path
    /// before `wet`, not the 0 dB D/R that the critical distance defines.
    #[test]
    fn steady_state_wet_level_is_nearly_independent_of_rt60() {
        let rt60_values = [0.5f32, 1.0, 2.0, 4.0];
        let levels: Vec<f64> = rt60_values
            .iter()
            .map(|&rt60| steady_state_wet_to_send_db(rt60))
            .collect();
        for (rt60, level) in rt60_values.iter().zip(&levels) {
            assert!(
                (level - -17.5).abs() < 2.5,
                "RT60 {rt60} s: wet/send power should be ≈ −17.5 dB, got {level:.1} dB"
            );
        }
        let loudest = levels.iter().copied().fold(f64::MIN, f64::max);
        let quietest = levels.iter().copied().fold(f64::MAX, f64::min);
        assert!(
            loudest - quietest < 4.0,
            "normalisation should hold the wet level within 4 dB over RT60 0.5–4 s: {levels:?}"
        );
    }

    /// Stability at the extremes. A near-infinite RT60 (loop gain → 1) driven by
    /// full-scale noise must stay bounded by the ±16 soft clamp inside the loop
    /// and the ±100 output ceiling, and remain finite; a near-zero RT60 must die
    /// out immediately. Covers both the normal and measurement-mode clamps.
    #[test]
    fn impulse_response_bounded_for_extreme_rt60() {
        for measurement_mode in [false, true] {
            for rt60 in [1.0e6_f32, f32::INFINITY] {
                let (layout, listener) = make_ctx(2, 2);
                let ctx = MixContext {
                    measurement_mode,
                    ..mix_ctx(&layout, &listener, 2, 2)
                };
                let mut fdn = FdnReverbStage::with_config(FdnConfig {
                    rt60_seconds: Some(rt60),
                    ..FdnConfig::default()
                });
                fdn.init(&ctx);
                let mut send = white_noise((0.5 * SAMPLE_RATE) as usize, 3);
                send.extend(std::iter::repeat_n(0.0, (0.5 * SAMPLE_RATE) as usize));
                let wet = stereo_wet_output(&mut fdn, &send);
                let peak = wet
                    .iter()
                    .flat_map(|frame| frame.iter())
                    .fold(0.0f32, |peak, &x| {
                        assert!(x.is_finite(), "non-finite output at RT60={rt60}");
                        peak.max(x.abs())
                    });
                assert!(peak <= 100.0, "output exceeded ±100 ceiling: {peak}");
                let line_ceiling = if measurement_mode { 100.0 } else { 16.0 };
                let stored_peak = fdn
                    .delay_buffers
                    .iter()
                    .flat_map(|line| line.iter())
                    .fold(0.0f32, |peak, &x| peak.max(x.abs()));
                assert!(
                    stored_peak <= line_ceiling,
                    "delay-line content {stored_peak} exceeded ±{line_ceiling} \
                     (RT60={rt60}, measurement_mode={measurement_mode})"
                );
            }
        }

        // Near-zero RT60: per-pass gain 10^(-3·1249/(1e-3·48000)) underflows to 0,
        // so nothing survives the first pass through the loop.
        let mut fdn = stereo_stage(FdnConfig::default(), Some((1.0e-3, 1.0e-3)));
        let response = impulse_response(&mut fdn, (0.2 * SAMPLE_RATE) as usize);
        let late_peak = response[(0.1 * SAMPLE_RATE) as usize..]
            .iter()
            .fold(0.0f32, |peak, &x| peak.max(x.abs()));
        assert!(
            late_peak < 1e-6,
            "1 ms RT60 should leave no tail, got {late_peak}"
        );
    }

    /// The in-loop soft clamp may only ever remove energy: identity below the
    /// knee, odd-symmetric, monotonic, |f(x)| ≤ |x|, C¹ at the knee (slope 1),
    /// and asymptotic to ±16. Together with |H_damping| < 1 this keeps the loop
    /// gain below 1 at every frequency and amplitude.
    #[test]
    fn feedback_soft_clamp_is_passive_monotonic_and_smooth() {
        let mut previous = feedback_soft_clamp(-1000.0);
        let mut value = -1000.0f32;
        while value <= 1000.0 {
            let clamped = feedback_soft_clamp(value);
            assert!(
                clamped.abs() <= value.abs() + 1e-6,
                "f({value}) gained energy"
            );
            assert!(
                clamped.abs() < 16.0,
                "f({value}) = {clamped} reached the ceiling"
            );
            assert!(clamped >= previous, "f not monotonic at {value}");
            assert_eq!(
                feedback_soft_clamp(-value),
                -clamped,
                "f not odd at {value}"
            );
            previous = clamped;
            value += 0.25;
        }
        let step = 1e-3f32;
        let slope_above_knee = (feedback_soft_clamp(14.0 + step) - 14.0) / step;
        assert!(
            (slope_above_knee - 1.0).abs() < 2e-3,
            "slope just above the knee should be 1 (C¹), got {slope_above_knee}"
        );
    }

    /// The default pre-delay is the mean-free-path time 4V/(S·c). For the
    /// shipped cathedral (40×25×20 m, V = 20000 m³, S = 4600 m²) that is
    /// 17.39 m / 343.42 m/s = 50.6 ms. A fixed 2048-sample pre-delay buffer
    /// used to cap it at 2047/48000 = 42.6 ms (21.3 ms at 96 kHz); the ring is
    /// now sized from the pre-delay and the sample rate.
    #[test]
    fn pre_delay_cathedral_not_truncated_by_buffer() {
        let (volume, surface_area) =
            room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(40.0, 25.0, 20.0));
        for sample_rate in [48_000.0f32, 96_000.0] {
            let mut fdn = FdnReverbStage::new();
            fdn.compute_pre_delay(sample_rate, volume, surface_area, 343.42);
            let actual_ms = fdn.pre_delay_samples as f32 / sample_rate * 1000.0;
            assert!(
                (actual_ms - 50.6).abs() < 0.5,
                "cathedral pre-delay at {sample_rate} Hz should be 50.6 ms, got {actual_ms:.1} ms"
            );
        }
    }

    /// An explicit pre-delay beyond the old 2048-sample cap is honoured end to
    /// end: 100 ms at 96 kHz = 9600 samples, so a unit impulse on the send
    /// cannot reach the output before frame 9600 + 2498 − 22 (the shortest
    /// line, 1249 × 2, shortened by at most the LFO's 20-sample depth at
    /// 96 kHz, the interpolator's newest tap and the integer rounding — any
    /// earlier wet means the read wrapped round an undersized ring). Overrides
    /// above 250 ms are bounded to 250 ms (24000 samples at 96 kHz).
    #[test]
    fn pre_delay_override_beyond_old_cap_reaches_the_output() {
        let (layout, listener) = make_ctx(2, 2);
        let sample_rate = 96_000.0;
        let frames = 16_384;
        let mut reverb_input = vec![0.0f32; frames * 2];
        reverb_input[0] = 1.0;
        let ctx = MixContext {
            sample_rate,
            reverb_input: Some(&reverb_input),
            ..mix_ctx(&layout, &listener, 2, 2)
        };
        let mut fdn = FdnReverbStage::with_config(FdnConfig {
            pre_delay_ms: Some(100.0),
            ..FdnConfig::default()
        });
        fdn.init(&ctx);
        assert_eq!(fdn.pre_delay_samples, 9600);
        assert!(fdn.pre_delay_buffer.len() > 9600);

        let mut buffer = vec![0.0f32; frames * 2];
        fdn.process(&mut buffer, &ctx);
        let first_wet_frame = buffer
            .chunks_exact(2)
            .position(|frame| frame[0] != 0.0 || frame[1] != 0.0)
            .expect("the tail must reach the output within the window");
        assert!(
            first_wet_frame >= 9600 + 2498 - 22,
            "wet arrived at frame {first_wet_frame}, before pre-delay + shortest line"
        );

        let mut fdn = FdnReverbStage::with_config(FdnConfig {
            pre_delay_ms: Some(1000.0),
            ..FdnConfig::default()
        });
        fdn.init(&ctx);
        assert_eq!(fdn.pre_delay_samples, 24_000);
    }
}
