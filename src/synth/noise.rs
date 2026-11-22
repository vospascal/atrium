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
#[derive(Clone)]
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
#[derive(Clone)]
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
#[derive(Clone)]
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
#[derive(Clone)]
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
mod tests;
