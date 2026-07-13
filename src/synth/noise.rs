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

/// Fast xorshift64 PRNG. Not cryptographic — perfect for audio noise.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    #[inline(always)]
    fn next_u64(&mut self) -> u64 {
        let mut s = self.state;
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        self.state = s;
        s
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
// Noise colours
// ---------------------------------------------------------------------------

/// Voss-McCartney 1/f (pink) noise — three leaky integrators.
///
/// The three poles (0.997, 0.985, 0.95) give a ~1/f rolloff up to a few hundred
/// Hz. Output is normalized by [`PINK_NORM`] so its RMS is ≈0.30 (peaks ≈±1),
/// matching the level downstream gains assume. Without normalization the raw
/// sum runs ~6× hot (RMS ≈1.56), which over-weighted every hiss layer (wind,
/// waves, rain bed) and clipped rain v1 — the audited "too bright / sissing"
/// root cause. See docs/rain-synthesis-audit.md.
pub struct PinkNoise {
    b0: f32,
    b1: f32,
    b2: f32,
    rng: Rng,
}

/// Output scale for [`PinkNoise`]: maps the raw filter-sum RMS (≈1.56) to ≈0.30.
const PINK_NORM: f32 = 0.19;

impl PinkNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            rng: Rng::new(seed),
        }
    }

    #[inline(always)]
    pub fn next_sample(&mut self) -> f32 {
        let w = self.rng.next_bipolar();
        self.b0 = 0.997 * self.b0 + 0.02109238 * w;
        self.b1 = 0.985 * self.b1 + 0.07113478 * w;
        self.b2 = 0.95 * self.b2 + 0.688_735_6 * w;
        (self.b0 + self.b1 + self.b2) * PINK_NORM
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
/// Output range is roughly [-0.1, 0.1]. The small step size (0.01) and
/// leak factor (0.998) keep the random walk bounded without hard clamping.
pub struct BrownNoise {
    val: f32,
    rng: Rng,
}

impl BrownNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            val: 0.0,
            rng: Rng::new(seed),
        }
    }

    #[inline(always)]
    pub fn next_sample(&mut self) -> f32 {
        self.val += self.rng.next_bipolar() * 0.01;
        self.val *= 0.998; // leak prevents DC drift
        self.val
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

/// One-pole highpass filter (white minus lowpassed).
pub struct OnePoleHP {
    a: f32,
    y: f32,
    z: f32,
}

impl OnePoleHP {
    pub fn new(cut_hz: f32, sample_rate: f32) -> Self {
        Self {
            a: (-TAU * cut_hz / sample_rate).exp(),
            y: 0.0,
            z: 0.0,
        }
    }

    /// Retune the cutoff in place (control-rate; keeps filter state).
    #[inline(always)]
    pub fn set_cutoff(&mut self, cut_hz: f32, sample_rate: f32) {
        self.a = (-TAU * cut_hz / sample_rate).exp();
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        self.y = x - self.z + self.a * self.y;
        self.z = x;
        self.y
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
        let sum: f32 = (0..1000).map(|_| pink.next_sample().abs()).sum();
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
            let v = pink.next_sample();
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
            let v = brown.next_sample();
            assert!(v.abs() < 1.0, "brown noise unbounded: {v}");
        }
    }
}
