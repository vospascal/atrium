// Shared noise generators for procedural audio synthesis.
//
// Port of NoiseColours.ts. Every generator is a small struct with a
// `next(&mut self) -> f32` method — no allocations, no FFTs, just f32 math.
//
// Uses an embedded xorshift64 PRNG so we don't pull in the `rand` crate
// and stay allocation-free on the audio thread.

use std::f32::consts::TAU;

// ---------------------------------------------------------------------------
// PRNG
// ---------------------------------------------------------------------------

/// Weyl increment of splitmix64 (2^64 / golden ratio, odd).
const SPLITMIX64_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// One splitmix64 output for `seed`: add the Weyl increment, then the
/// finaliser. A bijection on u64 with full avalanche, so seeds that differ in a
/// single low bit give unrelated 64-bit states.
fn splitmix64(seed: u64) -> u64 {
    let mut mixed = seed.wrapping_add(SPLITMIX64_GAMMA);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^ (mixed >> 31)
}

/// Marsaglia's xorshift64 step with the (13, 7, 17) shift triple.
#[inline(always)]
fn xorshift64_step(state: u64) -> u64 {
    let mut next = state;
    next ^= next << 13;
    next ^= next >> 7;
    next ^= next << 17;
    next
}

/// Fast xorshift64 PRNG. Not cryptographic — perfect for audio noise.
///
/// The seed is scrambled through [`splitmix64`] before it becomes the
/// xorshift state. xorshift64 has no output scrambler, so a raw small seed
/// (a few low bits set) needs several steps to reach the upper bits that
/// [`Rng::next_f32`] reads: unmixed, every seed below about 2^17 drew exactly
/// 0.0 first, and seeds 0 and 1 gave the same stream.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let state = splitmix64(seed);
        // Zero is xorshift's one fixed point; exactly one seed
        // (2^64 - SPLITMIX64_GAMMA) mixes to it.
        Self {
            state: if state == 0 { SPLITMIX64_GAMMA } else { state },
        }
    }

    #[inline(always)]
    fn next_u64(&mut self) -> u64 {
        self.state = xorshift64_step(self.state);
        self.state
    }

    /// Uniform f32 in [0, 1).
    #[inline(always)]
    pub fn next_f32(&mut self) -> f32 {
        // Upper 24 bits → [0, 2^24) → divide by 2^24
        (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32)
    }

    /// Uniform f32 in [-1, 1).
    #[inline(always)]
    pub fn next_bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }
}

// ---------------------------------------------------------------------------
// Sample-rate reference
// ---------------------------------------------------------------------------

/// Host rate at which every synth's levels and spectra were tuned. Anything
/// specified per sample below is derived from a rate-independent quantity
/// (corner frequency, time constant, density per Hz) so that it reproduces
/// its tuned value exactly at this rate and the same sound at any other.
pub const REFERENCE_SAMPLE_RATE: f32 = 48_000.0;

/// Gain for a white stream from [`Rng::next_bipolar`] that keeps its power
/// spectral density per Hz equal to its density at [`REFERENCE_SAMPLE_RATE`].
///
/// White noise of fixed per-sample variance spreads that variance over
/// `sample_rate / 2` Hz, so unscaled its density falls 3 dB per doubling of
/// the rate and every band-limited layer fed by it gets quieter. Exactly 1.0
/// at the reference rate.
#[inline]
pub fn white_noise_density_scale(sample_rate: f32) -> f32 {
    (sample_rate / REFERENCE_SAMPLE_RATE).sqrt()
}

/// Leaky integrator of white noise, `state = pole · state + gain · white`,
/// specified in continuous time: a corner frequency and the input gain it
/// had at [`REFERENCE_SAMPLE_RATE`]. Retuning keeps both the corner and the
/// output density per Hz, so the spectrum is the same at every host rate.
struct LeakyIntegrator {
    corner_hz: f32,
    reference_input_gain: f32,
    pole: f32,
    input_gain: f32,
    state: f32,
}

impl LeakyIntegrator {
    const fn new(corner_hz: f32, reference_input_gain: f32) -> Self {
        Self {
            corner_hz,
            reference_input_gain,
            pole: 0.0,
            input_gain: 0.0,
            state: 0.0,
        }
    }

    fn retune(&mut self, sample_rate: f32) {
        let reference_pole = (-TAU * self.corner_hz / REFERENCE_SAMPLE_RATE).exp();
        self.pole = (-TAU * self.corner_hz / sample_rate).exp();
        // DC gain is input_gain / (1 - pole): scaling the input gain with
        // (1 - pole) holds it, and the white-density scale holds the input
        // density per Hz. Both factors are exactly 1 at the reference rate.
        self.input_gain = self.reference_input_gain * (1.0 - self.pole) / (1.0 - reference_pole)
            * white_noise_density_scale(sample_rate);
    }

    #[inline(always)]
    fn process(&mut self, white: f32) -> f32 {
        self.state = self.pole * self.state + self.input_gain * white;
        self.state
    }
}

// ---------------------------------------------------------------------------
// Noise colours
// ---------------------------------------------------------------------------

/// Voss-McCartney 1/f (pink) noise — three leaky integrators.
///
/// The three corners (23, 115 and 392 Hz; poles 0.997, 0.985 and 0.95 at
/// 48 kHz) give a ~1/f rolloff up to a few hundred Hz. The corners are fixed
/// in Hz, not in samples, so the spectrum is the same at every host rate.
/// Output is normalized by [`PINK_NORM`] so its RMS is ≈0.30 (peaks ≈±1),
/// matching the level downstream gains assume. Without normalization the raw
/// sum runs ~6× hot (RMS ≈1.56), which over-weighted every hiss layer (wind,
/// waves, rain bed) and clipped rain v1 — the audited "too bright / sissing"
/// root cause. See docs/rain-synthesis-audit.md.
pub struct PinkNoise {
    sections: [LeakyIntegrator; 3],
    rng: Rng,
    sample_rate_cached: f32,
}

/// Output scale for [`PinkNoise`]: maps the raw filter-sum RMS (≈1.56) to ≈0.30.
const PINK_NORM: f32 = 0.19;

impl PinkNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            // Corners are -ln(pole) · 48 kHz / 2π for the tuned 48 kHz poles
            // 0.997, 0.985 and 0.95; the gains are the tuned 48 kHz gains.
            sections: [
                LeakyIntegrator::new(22.952_758, 0.021_092_38),
                LeakyIntegrator::new(115.459_69, 0.071_134_78),
                LeakyIntegrator::new(391.851_9, 0.688_735_6),
            ],
            rng: Rng::new(seed),
            sample_rate_cached: 0.0,
        }
    }

    #[inline(always)]
    pub fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            for section in &mut self.sections {
                section.retune(sample_rate);
            }
        }
        let white = self.rng.next_bipolar();
        let [first, second, third] = &mut self.sections;
        (first.process(white) + second.process(white) + third.process(white)) * PINK_NORM
    }
}

/// Full 7-pole Paul Kellet pink noise — holds a true ≈−3 dB/octave slope out to
/// ~20 kHz (unlike the economy 3-pole [`PinkNoise`], which steepens to −6 dB/oct
/// above a few hundred Hz and sounds dark). Use this where a natural broadband
/// "airy" tail matters — e.g. matching a field-recorded wind, whose brilliance
/// band sits only ~16 dB below its low-mid peak.
pub struct PinkNoiseFull {
    b: [f32; 7],
    rng: Rng,
}

/// Output scale for [`PinkNoiseFull`]: maps the raw Kellet sum to RMS ≈0.30.
const PINK_FULL_NORM: f32 = 0.062;

impl PinkNoiseFull {
    pub fn new(seed: u64) -> Self {
        Self {
            b: [0.0; 7],
            rng: Rng::new(seed),
        }
    }

    #[inline(always)]
    pub fn next_sample(&mut self) -> f32 {
        let w = self.rng.next_bipolar();
        self.b[0] = 0.99886 * self.b[0] + w * 0.0555179;
        self.b[1] = 0.99332 * self.b[1] + w * 0.0750759;
        self.b[2] = 0.96900 * self.b[2] + w * 0.1538520;
        self.b[3] = 0.86650 * self.b[3] + w * 0.3104856;
        self.b[4] = 0.55000 * self.b[4] + w * 0.5329522;
        self.b[5] = -0.7616 * self.b[5] - w * 0.0168980;
        let out = self.b[0]
            + self.b[1]
            + self.b[2]
            + self.b[3]
            + self.b[4]
            + self.b[5]
            + self.b[6]
            + w * 0.5362;
        self.b[6] = w * 0.115926;
        out * PINK_FULL_NORM
    }
}

/// Brownian (1/f²) noise — integrated white with DC leak.
///
/// RMS is about 0.09 (peaks about ±0.35). At 48 kHz this is the random walk
/// `value = 0.998 · (value + 0.01 · white)`: the small step and the leak keep
/// it bounded without hard clamping. The leak is a 15.3 Hz corner and the
/// step is a diffusion rate per second, so the walk has the same spectrum at
/// every host rate (a fixed per-sample step would rise 3 dB per doubling).
pub struct BrownNoise {
    walk: LeakyIntegrator,
    rng: Rng,
    sample_rate_cached: f32,
}

impl BrownNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            // Corner -ln(0.998) · 48 kHz / 2π; input gain is leak · step at
            // 48 kHz.
            walk: LeakyIntegrator::new(15.294_174, 0.998 * 0.01),
            rng: Rng::new(seed),
            sample_rate_cached: 0.0,
        }
    }

    #[inline(always)]
    pub fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.walk.retune(sample_rate);
        }
        self.walk.process(self.rng.next_bipolar())
    }
}

// ---------------------------------------------------------------------------
// One-pole filters
// ---------------------------------------------------------------------------

/// One-pole lowpass filter.  H(z) = (1-a) / (1 - a·z⁻¹)
pub struct OnePoleLP {
    a: f32,
    y: f32,
}

impl OnePoleLP {
    pub fn new(cut_hz: f32, sample_rate: f32) -> Self {
        Self {
            a: (-TAU * cut_hz / sample_rate).exp(),
            y: 0.0,
        }
    }

    /// Retune the cutoff in place (control-rate; keeps filter state).
    #[inline(always)]
    pub fn set_cutoff(&mut self, cut_hz: f32, sample_rate: f32) {
        self.a = (-TAU * cut_hz / sample_rate).exp();
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        self.y = self.a * self.y + (1.0 - self.a) * x;
        self.y
    }
}

/// One-pole highpass filter: the DC blocker `y[n] = x[n] - x[n-1] + a·y[n-1]`.
///
/// Its passband gain is 2 / (1 + a), above unity for cutoffs that are a
/// sizeable fraction of the rate (+2 dB for 4 kHz at 48 kHz), and that shelf
/// shrinks as the rate rises (+1 dB at 96 kHz). The synths were tuned with the
/// shelf of [`REFERENCE_SAMPLE_RATE`], so the output is scaled to keep the
/// passband gain the cutoff has at that rate: the response in Hz then matches
/// the 48 kHz response within 0.15 dB at 44.1 and 96 kHz for cutoffs up to
/// 4 kHz, and the scale is exactly 1 at 48 kHz.
pub struct OnePoleHP {
    a: f32,
    reference_gain: f32,
    y: f32,
    z: f32,
}

impl OnePoleHP {
    pub fn new(cut_hz: f32, sample_rate: f32) -> Self {
        let mut filter = Self {
            a: 0.0,
            reference_gain: 1.0,
            y: 0.0,
            z: 0.0,
        };
        filter.set_cutoff(cut_hz, sample_rate);
        filter
    }

    /// Retune the cutoff in place (control-rate; keeps filter state).
    #[inline(always)]
    pub fn set_cutoff(&mut self, cut_hz: f32, sample_rate: f32) {
        self.a = (-TAU * cut_hz / sample_rate).exp();
        let reference_a = (-TAU * cut_hz / REFERENCE_SAMPLE_RATE).exp();
        // Passband gain 2 / (1 + a) at this rate, rescaled to 2 / (1 + a_ref).
        self.reference_gain = (1.0 + self.a) / (1.0 + reference_a);
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        self.y = x - self.z + self.a * self.y;
        self.z = x;
        self.y * self.reference_gain
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_produces_values_in_range() {
        let mut rng = Rng::new(42);
        for _ in 0..10_000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn rng_bipolar_in_range() {
        let mut rng = Rng::new(42);
        for _ in 0..10_000 {
            let v = rng.next_bipolar();
            assert!((-1.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn pink_noise_nonzero() {
        let mut pink = PinkNoise::new(42);
        let sum: f32 = (0..1000).map(|_| pink.next_sample(48_000.0).abs()).sum();
        assert!(sum > 0.0, "pink noise is silent");
    }

    /// Level regression guard: pink output must sit near unity, not ~6× hot.
    /// This is the warmth/clipping fix — if it drifts, every hiss layer and
    /// rain v1's headroom go with it.
    #[test]
    fn pink_noise_is_level_normalized() {
        let mut pink = PinkNoise::new(42);
        let n = 96_000;
        let mut sum_sq = 0.0_f64;
        let mut peak = 0.0_f32;
        for _ in 0..n {
            let v = pink.next_sample(48_000.0);
            sum_sq += (v as f64) * (v as f64);
            peak = peak.max(v.abs());
        }
        let rms = (sum_sq / n as f64).sqrt() as f32;
        assert!(
            (0.2..0.45).contains(&rms),
            "pink RMS should be ~0.3 (near unity), got {rms}"
        );
        assert!(peak < 1.5, "pink peak should not clip the mix, got {peak}");
    }

    #[test]
    fn brown_noise_bounded() {
        let mut brown = BrownNoise::new(42);
        for _ in 0..100_000 {
            let v = brown.next_sample(48_000.0);
            assert!(v.abs() < 1.0, "brown noise unbounded: {v}");
        }
    }

    // ── Statistical and spectral verification ────────────────────────────

    use crate::synth::test_support::{
        averaged_power_spectrum, band_density_db, mean, peak, render, root_mean_square,
        spectral_slope_db_per_octave, variance,
    };

    const SAMPLE_RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

    /// Marsaglia's xorshift64 (13, 7, 17) from his published seed. The first
    /// output, 8748534153485358512, is the reference value quoted in the
    /// literature, so this pins the shift triple and the update order. The
    /// generator iterates exactly this step from its mixed state.
    #[test]
    fn xorshift64_step_matches_marsaglia_reference_values() {
        let first = xorshift64_step(88172645463325252);
        let second = xorshift64_step(first);
        let third = xorshift64_step(second);
        assert_eq!(first, 8748534153485358512);
        assert_eq!(second, 3040900993826735515);
        assert_eq!(third, 3453997556048239312);

        let mut rng = Rng::new(42);
        let start = rng.state;
        assert_eq!(rng.next_u64(), xorshift64_step(start));
        assert_eq!(rng.next_u64(), xorshift64_step(xorshift64_step(start)));
    }

    /// The seed is mixed by splitmix64 (Steele, Lea and Flood 2014). Its
    /// published stream from state 0 starts 0xE220A8397B1DCDAF,
    /// 0x6E789E6AA1B965F4: the first is the mix of seed 0, the second the
    /// mix of seed GAMMA.
    #[test]
    fn seed_mixing_matches_the_splitmix64_reference_stream() {
        assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
        assert_eq!(splitmix64(SPLITMIX64_GAMMA), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(Rng::new(0).state, 0xE220_A839_7B1D_CDAF);
    }

    /// Zero is the one fixed point of xorshift. Seed 0 is an ordinary seed
    /// now (its own stream, distinct from seed 1), and the single seed that
    /// mixes to state 0 is remapped off the fixed point.
    #[test]
    fn rng_never_starts_on_the_zero_fixed_point() {
        let seed_mixing_to_zero = 0u64.wrapping_sub(SPLITMIX64_GAMMA);
        assert_eq!(splitmix64(seed_mixing_to_zero), 0);
        for seed in [0u64, 1, seed_mixing_to_zero] {
            let mut rng = Rng::new(seed);
            for _ in 0..8 {
                assert_ne!(rng.next_u64(), 0, "seed {seed} hit the fixed point");
            }
        }
        let mut zero = Rng::new(0);
        let mut one = Rng::new(1);
        let differing = (0..8).filter(|_| zero.next_u64() != one.next_u64()).count();
        assert_eq!(differing, 8, "seeds 0 and 1 share a stream");
    }

    /// Uniform on [-1, 1): zero mean, variance (b - a)^2 / 12 = 1/3.
    #[test]
    fn rng_bipolar_has_zero_mean_and_one_third_variance() {
        let mut rng = Rng::new(42);
        let samples: Vec<f32> = (0..400_000).map(|_| rng.next_bipolar()).collect();
        // Standard error of the mean is sqrt(1/3 / 4e5) = 0.0009.
        assert!(mean(&samples).abs() < 0.004, "mean {}", mean(&samples));
        let measured = variance(&samples);
        assert!(
            (measured - 1.0 / 3.0).abs() < 0.004,
            "variance {measured}, expected 1/3"
        );
    }

    /// Ten equal-width bins of next_f32 each hold 10 % of the draws.
    #[test]
    fn rng_unit_interval_is_uniform_across_bins() {
        let mut rng = Rng::new(7);
        let draws = 200_000;
        let mut bins = [0usize; 10];
        for _ in 0..draws {
            bins[(rng.next_f32() * 10.0) as usize] += 1;
        }
        for (index, &count) in bins.iter().enumerate() {
            let fraction = count as f64 / draws as f64;
            assert!(
                (fraction - 0.1).abs() < 0.005,
                "bin {index} holds {fraction:.4}"
            );
        }
    }

    #[test]
    fn rng_is_deterministic_per_seed_and_differs_between_seeds() {
        let draw = |seed: u64| -> Vec<u32> {
            let mut rng = Rng::new(seed);
            (0..64).map(|_| rng.next_f32().to_bits()).collect()
        };
        assert_eq!(draw(5), draw(5));
        assert_ne!(draw(5), draw(6));
    }

    /// xorshift64 has no output scrambler, so a raw small seed (a few bits
    /// set) would need several iterations to fill the upper bits that
    /// `next_f32` reads: unmixed, every seed below about 2^17 drew exactly 0.0
    /// first (bipolar -1.0), and callers that consume the first value
    /// (SmoothRandom's initial state) started at -1 for every seed. The
    /// splitmix64 seed mixing must spread first draws across [0, 1).
    #[test]
    fn rng_first_draw_depends_on_the_seed() {
        let firsts: Vec<f32> = [1u64, 2, 3, 42, 99, 1_000, 65_535]
            .iter()
            .map(|&seed| Rng::new(seed).next_f32())
            .collect();
        let spread = firsts.iter().cloned().fold(f32::MIN, f32::max)
            - firsts.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread > 0.2, "first draws {firsts:?}");
    }

    /// Synths seed their layers with `seed + 1`, `seed + 3`, `seed + 5`, ...
    /// Neighbouring seeds must give statistically independent streams, or the
    /// "independent" noise layers would be partly the same noise.
    #[test]
    fn rng_streams_from_neighbouring_seeds_are_uncorrelated() {
        for (first_seed, second_seed) in [(42u64, 43u64), (43, 45), (1, 2), (100, 101)] {
            let mut first = Rng::new(first_seed);
            let mut second = Rng::new(second_seed);
            let count = 200_000;
            let mut product_sum = 0.0_f64;
            for _ in 0..count {
                product_sum += first.next_bipolar() as f64 * second.next_bipolar() as f64;
            }
            // Correlation coefficient = E[xy] / variance(1/3); its standard
            // error is 1 / sqrt(count) = 0.0022.
            let correlation = product_sum / count as f64 * 3.0;
            assert!(
                correlation.abs() < 0.01,
                "seeds {first_seed} and {second_seed} correlate at {correlation:.4}"
            );
        }
    }

    /// Paul Kellet's seven-pole filter: -3 dB per octave from 50 Hz to 16 kHz.
    /// The measurement is a least-squares fit across third-octave bands.
    #[test]
    fn pink_full_slope_is_minus_three_db_per_octave_at_every_sample_rate() {
        for sample_rate in SAMPLE_RATES {
            let mut pink = PinkNoiseFull::new(42);
            let samples = render(10.0, sample_rate, |_| pink.next_sample());
            let spectrum = averaged_power_spectrum(&samples);
            let slope = spectral_slope_db_per_octave(&spectrum, sample_rate, 100.0, 16_000.0);
            assert!(
                (slope + 3.0).abs() < 0.3,
                "{sample_rate} Hz: pink slope {slope:.2} dB/oct, expected -3"
            );
        }
    }

    /// The Kellet coefficients are published for 44.1 kHz, yet because pink
    /// noise is self-similar the density at a fixed frequency is unchanged
    /// when the poles are reused at another rate (only the band edges move).
    #[test]
    fn pink_full_density_at_one_khz_is_sample_rate_independent() {
        let densities: Vec<f64> = SAMPLE_RATES
            .iter()
            .map(|&sample_rate| {
                let mut pink = PinkNoiseFull::new(42);
                let samples = render(10.0, sample_rate, |_| pink.next_sample());
                let spectrum = averaged_power_spectrum(&samples);
                band_density_db(&spectrum, sample_rate, 900.0, 1_100.0)
            })
            .collect();
        for density in &densities {
            assert!(
                (density - densities[1]).abs() < 0.5,
                "densities across rates differ: {densities:?}"
            );
        }
    }

    /// `PINK_FULL_NORM`'s doc comment says it yields RMS ≈ 0.30. It does not:
    /// the measured RMS is 0.108 (-8.9 dB below the documented value). The
    /// downstream gains were tuned against the real level, so this pins the
    /// real level; the comment, not the constant, is what is wrong.
    #[test]
    fn pink_full_rms_is_the_tuned_0_108_not_the_documented_0_30() {
        let mut pink = PinkNoiseFull::new(42);
        let samples = render(10.0, 48_000.0, |_| pink.next_sample());
        let rms = root_mean_square(&samples);
        assert!((rms - 0.108).abs() < 0.006, "RMS {rms}");
        assert!(peak(&samples) < 0.6, "peak {}", peak(&samples));
    }

    /// The economy three-pole generator is only -3 dB/oct through the middle
    /// (about 200-800 Hz), flat below ~200 Hz and -6 dB/oct above ~1.6 kHz.
    /// The `PinkNoise` doc claims 1/f "up to a few hundred Hz"; the measured
    /// shape is recorded here so the behaviour cannot drift silently.
    #[test]
    fn economy_pink_slope_is_minus_three_only_in_the_mid_band() {
        let mut pink = PinkNoise::new(42);
        let samples = render(10.0, 48_000.0, |rate| pink.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        let mid = spectral_slope_db_per_octave(&spectrum, 48_000.0, 200.0, 800.0);
        let high = spectral_slope_db_per_octave(&spectrum, 48_000.0, 1_600.0, 6_400.0);
        let low = spectral_slope_db_per_octave(&spectrum, 48_000.0, 50.0, 200.0);
        assert!((mid + 3.3).abs() < 0.7, "mid slope {mid:.2}");
        assert!((high + 5.9).abs() < 0.7, "high slope {high:.2}");
        assert!(low > -2.0, "low band should be nearly flat, slope {low:.2}");
    }

    #[test]
    fn economy_pink_rms_matches_its_documented_level() {
        let mut pink = PinkNoise::new(42);
        let samples = render(10.0, 48_000.0, |rate| pink.next_sample(rate));
        let rms = root_mean_square(&samples);
        assert!((rms - 0.30).abs() < 0.02, "RMS {rms}, documented ≈0.30");
    }

    /// The three corners (23, 115 and 392 Hz) are fixed in Hz. Were the poles
    /// fixed in samples instead, the corners would double at 96 kHz and the
    /// density at 1 kHz would move by +1.6 dB.
    #[test]
    fn economy_pink_density_is_sample_rate_independent() {
        let density = |sample_rate: f32| {
            let mut pink = PinkNoise::new(42);
            let samples = render(10.0, sample_rate, |rate| pink.next_sample(rate));
            let spectrum = averaged_power_spectrum(&samples);
            band_density_db(&spectrum, sample_rate, 900.0, 1_100.0)
        };
        let difference = (density(96_000.0) - density(48_000.0)).abs();
        assert!(difference < 0.5, "density moved {difference:.2} dB");
    }

    /// Brownian noise falls 6 dB per octave above the leak corner.
    #[test]
    fn brown_noise_slope_is_minus_six_db_per_octave() {
        let mut brown = BrownNoise::new(42);
        let samples = render(15.0, 48_000.0, |rate| brown.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        let slope = spectral_slope_db_per_octave(&spectrum, 48_000.0, 100.0, 10_000.0);
        assert!((slope + 6.0).abs() < 0.4, "brown slope {slope:.2} dB/oct");
    }

    /// Leaky random walk: each step has variance (0.01)^2 / 3, the leak is
    /// 0.998, so the stationary RMS is 0.01 / sqrt(3) / sqrt(1 - 0.998^2)
    /// = 0.0913 at 48 kHz; peaks reach about +-0.35.
    #[test]
    fn brown_noise_rms_matches_the_leaky_random_walk_theory() {
        let mut brown = BrownNoise::new(42);
        let samples = render(20.0, 48_000.0, |rate| brown.next_sample(rate));
        let theoretical = 0.01 / 3.0_f64.sqrt() / (1.0 - 0.998_f64 * 0.998).sqrt();
        let measured = root_mean_square(&samples);
        assert!(
            (measured / theoretical - 1.0).abs() < 0.10,
            "RMS {measured:.4} vs theory {theoretical:.4}"
        );
        assert!(mean(&samples).abs() < 0.03, "DC drift {}", mean(&samples));
        assert!(peak(&samples) < 0.6, "peak {}", peak(&samples));
    }

    /// A random walk with a fixed per-sample step would have a continuous-time
    /// step variance proportional to the sample rate, so its density above the
    /// leak corner would rise 3 dB when the rate doubles (measured +2.9 dB
    /// before the step became a per-second diffusion rate).
    #[test]
    fn brown_noise_density_is_sample_rate_independent() {
        let density = |sample_rate: f32| {
            let mut brown = BrownNoise::new(42);
            let samples = render(15.0, sample_rate, |rate| brown.next_sample(rate));
            let spectrum = averaged_power_spectrum(&samples);
            band_density_db(&spectrum, sample_rate, 900.0, 1_100.0)
        };
        let difference = (density(96_000.0) - density(48_000.0)).abs();
        assert!(difference < 0.5, "density moved {difference:.2} dB");
    }

    /// Scaled white noise has the same density per Hz at every rate, and the
    /// scale is exactly 1 at the reference rate (48 kHz output unchanged).
    #[test]
    fn scaled_white_noise_density_is_sample_rate_independent() {
        assert_eq!(white_noise_density_scale(REFERENCE_SAMPLE_RATE), 1.0);
        let density = |sample_rate: f32| {
            let mut rng = Rng::new(42);
            let scale = white_noise_density_scale(sample_rate);
            let samples = render(10.0, sample_rate, |_| rng.next_bipolar() * scale);
            let spectrum = averaged_power_spectrum(&samples);
            band_density_db(&spectrum, sample_rate, 1_000.0, 10_000.0)
        };
        let reference = density(48_000.0);
        for sample_rate in [44_100.0, 96_000.0] {
            let difference = density(sample_rate) - reference;
            assert!(
                difference.abs() < 0.2,
                "{sample_rate} Hz: {difference:+.2} dB"
            );
        }
    }

    // ── One-pole filters: measured frequency response ────────────────────

    /// Steady-state gain of a filter for a sine at `frequency_hz`.
    fn sine_gain(sample_rate: f32, frequency_hz: f32, mut process: impl FnMut(f32) -> f32) -> f32 {
        let total = (sample_rate * 0.3) as usize;
        let settle = total / 2;
        let (mut input_energy, mut output_energy) = (0.0_f64, 0.0_f64);
        for index in 0..total {
            let input = (TAU * frequency_hz * index as f32 / sample_rate).sin();
            let output = process(input);
            if index >= settle {
                input_energy += (input as f64).powi(2);
                output_energy += (output as f64).powi(2);
            }
        }
        (output_energy / input_energy).sqrt() as f32
    }

    fn to_db(gain: f32) -> f32 {
        20.0 * gain.log10()
    }

    /// H(z) = (1-a)/(1-a z^-1) with a = exp(-2 pi fc / fs): the -3 dB point
    /// sits at the nominal cutoff (to within the one-pole mapping error, which
    /// grows toward Nyquist) and the rolloff is 6 dB per octave.
    #[test]
    fn one_pole_lowpass_is_three_db_down_at_its_cutoff() {
        for sample_rate in SAMPLE_RATES {
            for cutoff_hz in [300.0_f32, 1_000.0, 4_000.0] {
                let mut filter = OnePoleLP::new(cutoff_hz, sample_rate);
                let at_cutoff = to_db(sine_gain(sample_rate, cutoff_hz, |x| filter.process(x)));
                assert!(
                    (at_cutoff + 3.0).abs() < 0.6,
                    "{sample_rate} Hz, fc {cutoff_hz}: {at_cutoff:.2} dB at cutoff"
                );
                let mut filter = OnePoleLP::new(cutoff_hz, sample_rate);
                let passband = to_db(sine_gain(sample_rate, cutoff_hz / 20.0, |x| {
                    filter.process(x)
                }));
                assert!(passband.abs() < 0.1, "passband {passband:.2} dB");
                if cutoff_hz * 8.0 < sample_rate * 0.25 {
                    let mut filter = OnePoleLP::new(cutoff_hz, sample_rate);
                    let two_above = to_db(sine_gain(sample_rate, cutoff_hz * 2.0, |x| {
                        filter.process(x)
                    }));
                    let mut filter = OnePoleLP::new(cutoff_hz, sample_rate);
                    let four_above = to_db(sine_gain(sample_rate, cutoff_hz * 4.0, |x| {
                        filter.process(x)
                    }));
                    let mut filter = OnePoleLP::new(cutoff_hz, sample_rate);
                    let eight_above = to_db(sine_gain(sample_rate, cutoff_hz * 8.0, |x| {
                        filter.process(x)
                    }));
                    assert!(
                        (four_above - eight_above - 6.0).abs() < 0.8
                            && (two_above - four_above - 5.5).abs() < 1.0,
                        "rolloff {:.2} then {:.2} dB/oct",
                        two_above - four_above,
                        four_above - eight_above
                    );
                }
            }
        }
    }

    /// The highpass is the DC blocker y[n] = x[n] - x[n-1] + a y[n-1], scaled
    /// to the passband gain 2/(1+a) the cutoff has at the 48 kHz reference
    /// (slightly above unity for high cutoffs), and it is 3 dB below that
    /// gain at the nominal cutoff.
    #[test]
    fn one_pole_highpass_is_three_db_down_at_its_cutoff() {
        for sample_rate in SAMPLE_RATES {
            for cutoff_hz in [100.0_f32, 700.0, 2_000.0] {
                let reference_alpha = (-TAU * cutoff_hz / REFERENCE_SAMPLE_RATE).exp();
                let high_frequency_gain = 2.0 / (1.0 + reference_alpha);
                let mut filter = OnePoleHP::new(cutoff_hz, sample_rate);
                let at_cutoff = sine_gain(sample_rate, cutoff_hz, |x| filter.process(x));
                let relative_db = to_db(at_cutoff / high_frequency_gain);
                assert!(
                    (relative_db + 3.0).abs() < 0.8,
                    "{sample_rate} Hz, fc {cutoff_hz}: {relative_db:.2} dB re passband"
                );
                let mut filter = OnePoleHP::new(cutoff_hz, sample_rate);
                let stopband = to_db(sine_gain(sample_rate, cutoff_hz / 20.0, |x| {
                    filter.process(x)
                }));
                assert!(stopband < -22.0, "stopband {stopband:.1} dB");
            }
        }
    }

    /// Even a 4 kHz highpass (a sizeable fraction of the rate) has the same
    /// response in Hz at 44.1, 48 and 96 kHz: the passband shelf is held at
    /// its 48 kHz value instead of shrinking from +2 dB to +1 dB at 96 kHz.
    #[test]
    fn one_pole_highpass_response_is_sample_rate_independent() {
        for frequency_hz in [1_000.0_f32, 4_000.0, 8_000.0] {
            let mut reference = OnePoleHP::new(4_000.0, 48_000.0);
            let reference_db = to_db(sine_gain(48_000.0, frequency_hz, |x| reference.process(x)));
            for sample_rate in [44_100.0_f32, 96_000.0] {
                let mut filter = OnePoleHP::new(4_000.0, sample_rate);
                let measured_db =
                    to_db(sine_gain(sample_rate, frequency_hz, |x| filter.process(x)));
                assert!(
                    (measured_db - reference_db).abs() < 0.2,
                    "{sample_rate} Hz at {frequency_hz} Hz: {measured_db:.2} dB vs {reference_db:.2} dB at 48 kHz"
                );
            }
        }
    }

    /// `set_cutoff` must be exactly equivalent to constructing at that cutoff
    /// (the synths rely on it to retune when the host sample rate changes).
    #[test]
    fn set_cutoff_retunes_to_the_same_response_as_a_fresh_filter() {
        let mut retuned_low = OnePoleLP::new(10_000.0, 48_000.0);
        retuned_low.set_cutoff(500.0, 96_000.0);
        let mut fresh_low = OnePoleLP::new(500.0, 96_000.0);
        let mut retuned_high = OnePoleHP::new(10_000.0, 48_000.0);
        retuned_high.set_cutoff(500.0, 96_000.0);
        let mut fresh_high = OnePoleHP::new(500.0, 96_000.0);
        let mut rng = Rng::new(3);
        for _ in 0..1_000 {
            let input = rng.next_bipolar();
            assert_eq!(retuned_low.process(input), fresh_low.process(input));
            assert_eq!(retuned_high.process(input), fresh_high.process(input));
        }
    }
}
