// Procedural wind synthesis.
//
// Port of WindNoiseProcessor.ts. Layers:
//   • Brown (1/f²) noise         → low-frequency body / rumble
//   • Pink (1/f) noise           → broadband hiss
//   • Envelope state machine     → rise / hold / fall / rest swell cycles
//   • Turbulence bursts          → short Gaussian-gated white-noise puffs
//   • Speed-dependent spectral mix → calm = mostly rumble, strong = brighter hiss

use crate::spatial::source::SoundSource;
use crate::world::types::Vec3;

use super::noise::{BrownNoise, PinkNoise, Rng};

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Rise,
    Hold,
    Fall,
    Rest,
}

/// Procedural wind sound source.
///
/// The `speed` parameter drives the spectral tilt: low speed produces a dark
/// rumble, high speed adds bright hiss. The envelope state machine creates
/// natural gusting cycles without any external modulation.
pub struct WindSource {
    pink: PinkNoise,
    brown: BrownNoise,

    // Envelope state machine
    phase: Phase,
    phase_pos: usize,
    len_rise: usize,
    len_hold: usize,
    len_fall: usize,
    len_rest: usize,

    // Turbulence burst (computed inline — no allocation)
    turb_pos: usize,
    turb_len: usize,

    // Tuning knobs
    /// Wind speed in m/s (0–25). Controls spectral tilt.
    pub speed: f32,
    /// Gustiness (1–10). Deeper modulation + more frequent turbulence puffs.
    pub gustiness: f32,
    /// Envelope floor — wind never fully dies below this.
    pub min_intensity: f32,
    /// Envelope ceiling.
    pub max_intensity: f32,
    /// Average swell cycle duration in seconds.
    pub mean_duration: f32,
    /// ± jitter on cycle duration in seconds.
    pub duration_jitter: f32,
    /// Pink hiss gain.
    pub hiss_gain: f32,
    /// Brown rumble gain.
    pub rumble_gain: f32,
    /// Turbulence puff gain.
    pub turb_gain: f32,
    /// Output trim.
    pub master_gain: f32,

    /// World-space position.
    pub position: Vec3,

    rng: Rng,
    sample_rate_cached: f32,
}

impl WindSource {
    /// Create a new wind source.
    ///
    /// - `position`: where the wind originates (e.g. a window or wall opening)
    /// - `speed`: 0–25 m/s (5 = gentle breeze, 15 = strong wind)
    /// - `gustiness`: 1–10 (3 = mild gusts, 8 = stormy)
    /// - `seed`: PRNG seed
    pub fn new(position: Vec3, speed: f32, gustiness: f32, seed: u64) -> Self {
        Self {
            pink: PinkNoise::new(seed.wrapping_add(1)),
            brown: BrownNoise::new(seed.wrapping_add(2)),
            phase: Phase::Rise,
            phase_pos: 0,
            len_rise: 0,
            len_hold: 0,
            len_fall: 0,
            len_rest: 0,
            turb_pos: 0,
            turb_len: 0,
            speed: speed.clamp(0.0, 25.0),
            gustiness: gustiness.clamp(1.0, 10.0),
            min_intensity: 0.2,
            max_intensity: 1.0,
            mean_duration: 8.0,
            duration_jitter: 3.0,
            hiss_gain: 0.3,
            rumble_gain: 0.8,
            turb_gain: 0.4,
            master_gain: 1.0,
            position,
            rng: Rng::new(seed),
            sample_rate_cached: 0.0,
        }
    }

    /// Draw a new random swell cycle and divide into four phases.
    /// Ratio: 35% rise, 20% hold, 25% fall, 20% rest.
    fn schedule_next_cycle(&mut self, sample_rate: f32) {
        let dur_sec =
            self.mean_duration + self.rng.next_bipolar() * self.duration_jitter;
        let total = (dur_sec.max(1.0) * sample_rate) as usize;
        self.len_rise = (total as f32 * 0.35) as usize;
        self.len_hold = (total as f32 * 0.20) as usize;
        self.len_fall = (total as f32 * 0.25) as usize;
        self.len_rest = (total as f32 * 0.20) as usize;
        self.phase = Phase::Rise;
        self.phase_pos = 0;
    }

    /// Advance the envelope state machine by one sample. Returns envelope value.
    fn advance_envelope(&mut self, sample_rate: f32) -> f32 {
        let min_i = self.min_intensity;
        let max_i = self.max_intensity;

        let env = match self.phase {
            Phase::Rise => {
                let t = if self.len_rise > 0 {
                    self.phase_pos as f32 / self.len_rise as f32
                } else {
                    1.0
                };
                min_i + (max_i - min_i) * t
            }
            Phase::Hold => max_i,
            Phase::Fall => {
                let t = if self.len_fall > 0 {
                    self.phase_pos as f32 / self.len_fall as f32
                } else {
                    1.0
                };
                max_i - (max_i - min_i) * t
            }
            Phase::Rest => min_i,
        };

        self.phase_pos += 1;

        match self.phase {
            Phase::Rise if self.phase_pos >= self.len_rise => {
                self.phase = Phase::Hold;
                self.phase_pos = 0;
            }
            Phase::Hold if self.phase_pos >= self.len_hold => {
                self.phase = Phase::Fall;
                self.phase_pos = 0;
            }
            Phase::Fall if self.phase_pos >= self.len_fall => {
                self.phase = Phase::Rest;
                self.phase_pos = 0;
            }
            Phase::Rest if self.phase_pos >= self.len_rest => {
                self.schedule_next_cycle(sample_rate);
            }
            _ => {}
        }

        env
    }

    /// Inline Gaussian window for turbulence puff at current position.
    fn turb_envelope(&self) -> f32 {
        if self.turb_len == 0 {
            return 0.0;
        }
        let mid = self.turb_len as f32 / 2.0;
        let sigma = self.turb_len as f32 / 6.0;
        let x = (self.turb_pos as f32 - mid) / sigma;
        (-0.5 * x * x).exp()
    }
}

impl SoundSource for WindSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        // Lazy init: schedule first cycle once we know the sample rate
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.schedule_next_cycle(sample_rate);
        }

        let env = self.advance_envelope(sample_rate);

        // Speed → spectral mix: calm wind is dark, strong wind is bright
        let hiss_mix = (self.speed / 25.0).clamp(0.0, 1.0);
        let rumble_level = self.rumble_gain * (1.0 - hiss_mix * 0.8);
        let hiss_level = self.hiss_gain * hiss_mix;

        // Trigger turbulence puff when envelope is high
        if self.turb_len == 0
            && env > 0.7
            && self.rng.next_f32() < self.gustiness / (400.0 * sample_rate)
        {
            self.turb_len = (0.05 * sample_rate) as usize; // 50 ms puff
            self.turb_pos = 0;
        }

        let mut turb = 0.0;
        if self.turb_len > 0 {
            turb = self.rng.next_bipolar() * self.turb_envelope();
            self.turb_pos += 1;
            if self.turb_pos >= self.turb_len {
                self.turb_len = 0;
            }
        }

        // Noise sources
        let brown = self.brown.next();
        let pink = self.pink.next();

        let sample =
            env * (rumble_level * brown + hiss_level * pink) + self.turb_gain * turb;

        sample * self.master_gain
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn tick(&mut self, _dt: f32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wind_produces_nonzero_output() {
        let mut wind = WindSource::new(Vec3::ZERO, 10.0, 5.0, 42);
        let sum: f32 = (0..48000).map(|_| wind.next_sample(48000.0).abs()).sum();
        assert!(sum > 0.0, "wind source is silent");
    }

    #[test]
    fn wind_louder_at_higher_speed() {
        let mut calm = WindSource::new(Vec3::ZERO, 2.0, 3.0, 42);
        let mut strong = WindSource::new(Vec3::ZERO, 20.0, 3.0, 42);

        let energy_calm: f32 = (0..48000).map(|_| calm.next_sample(48000.0).powi(2)).sum();
        let energy_strong: f32 =
            (0..48000).map(|_| strong.next_sample(48000.0).powi(2)).sum();

        assert!(
            energy_strong > energy_calm,
            "strong wind ({energy_strong}) should be louder than calm ({energy_calm})"
        );
    }
}
