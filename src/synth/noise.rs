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
/// Output range is roughly [-0.8, 0.8]. The three poles (0.997, 0.985, 0.95)
/// give a convincing -3 dB/octave rolloff without any FFT or FIR filtering.
pub struct PinkNoise {
    b0: f32,
    b1: f32,
    b2: f32,
    rng: Rng,
}

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
        self.b0 + self.b1 + self.b2
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

    #[test]
    fn brown_noise_bounded() {
        let mut brown = BrownNoise::new(42);
        for _ in 0..100_000 {
            let v = brown.next_sample();
            assert!(v.abs() < 1.0, "brown noise unbounded: {v}");
        }
    }
}
