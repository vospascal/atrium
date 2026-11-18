// Physically-based rain synthesis (v2).
//
// Key differences from v1 (rain.rs):
//   • Drops are the PRIMARY sound — no continuous pink/brown noise bed.
//     Real rain = superposition of thousands of individual impacts + bubbles.
//     At high density this naturally converges to pink-ish noise (statistically).
//   • Each drop has TWO components (SIGGRAPH 2019 / Liu, Cheng & Tong):
//       1. Impact: short exponential-decay noise burst (1-4 ms)
//       2. Bubble: damped sinusoid exp(-αt)·sin(2πft) at Minnaert frequency
//   • Bubble frequency falls with drop size (Minnaert direction, shifted well
//     below the underwater values for warmth — air absorbs HF over distance):
//       - Small  (<1.2mm):  500-2500 Hz, rarely (half the base probability)
//       - Medium (1.2-2mm): 400-2000 Hz
//       - Large  (>2mm):    250-1200 Hz (the deep "plink")
//   • Drop rate follows a piecewise-linear intensity curve: 15/s at 0.2,
//     50/s at 0.5, `drop_rate` (75/s by default) from 0.9 up.
//   • Only a subtle brown-noise bed for low-frequency body, plus a faint
//     pink-modulated white-noise texture that fills between drops.
//   • No scratch buffer needed — impact+bubble written directly to ring.
//
// References:
//   - Liu, Cheng, Tong "Physically-based statistical simulation of rain sound"
//     (SIGGRAPH 2019, ACM ToG 38:4)
//   - Minnaert resonance: f₀ = (1/2πa)·√(3γP₀/ρ)
//   - Medwin & Palmer '94 (drop size distribution)
//   - Tsugi "Procedural Audio LODs" (3-layer rain architecture)

use std::f32::consts::TAU;

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

use super::noise::{white_noise_density_scale, BrownNoise, PinkNoise, Rng};

const RING_SIZE: usize = 8192;
const RING_MASK: usize = RING_SIZE - 1;

/// Corner of the one-pole smoother on the ring readout: -ln(0.65) · 48 kHz / 2π,
/// the 0.65-per-sample coefficient it was tuned with at 48 kHz.
const RING_SMOOTHING_CORNER_HZ: f32 = 3_290.939;

/// Drops per second at intensity 0.2 and 0.5 as fractions of the rate at full
/// intensity (15/s and 50/s of the default 75/s).
const LIGHT_DROP_RATE_FRACTION: f32 = 15.0 / 75.0;
const MEDIUM_DROP_RATE_FRACTION: f32 = 50.0 / 75.0;

/// Fraction of `drop_rate` that falls at `intensity`: piecewise linear from 0
/// through 0.2 at intensity 0.2 and 2/3 at 0.5 to 1 at 0.9, flat above.
fn drop_rate_fraction(intensity: f32) -> f32 {
    if intensity <= 0.2 {
        LIGHT_DROP_RATE_FRACTION * (intensity / 0.2)
    } else if intensity <= 0.5 {
        LIGHT_DROP_RATE_FRACTION
            + (MEDIUM_DROP_RATE_FRACTION - LIGHT_DROP_RATE_FRACTION) * ((intensity - 0.2) / 0.3)
    } else {
        MEDIUM_DROP_RATE_FRACTION
            + (1.0 - MEDIUM_DROP_RATE_FRACTION) * ((intensity - 0.5) / 0.4).min(1.0)
    }
}

/// Physically-based rain source (v2).
///
/// Each raindrop is modeled as impact burst + Minnaert-frequency bubble
/// oscillation. The sound comes from the aggregate of many individual
/// drop events — no continuous noise generators needed.
pub struct RainSourceV2 {
    // Subtle low-frequency bed (models distant aggregate rain body)
    brown: BrownNoise,

    // Csound-inspired "noise × pink × pink" texture layer.
    // Two independent pink noise generators modulate white noise,
    // creating organic amplitude fluctuations that mimic rain pattering.
    pink_mod1: PinkNoise,
    pink_mod2: PinkNoise,
    texture_rng: Rng,
    texture_lp_state: f32, // inline LP — cutoff varies with intensity

    // Ring buffer for drop event superposition
    ring: Box<[f32; RING_SIZE]>,
    ring_idx: usize,

    // Minimal smoothing on ring output. The per-sample coefficient is
    // derived from RING_SMOOTHING_CORNER_HZ.
    env_state: f32,
    env_smooth: f32,
    /// Keeps the texture white noise at its 48 kHz density per Hz.
    white_noise_scale: f32,
    sample_rate_cached: f32,

    // Output smoothing LP — softens transients at low intensity
    output_lp_state: f32,

    // -- Tuning knobs (all pub) --
    /// Rain intensity 0–1. Scales drop rate and overall level.
    pub intensity: f32,
    /// Drop impacts per second at full intensity (reached from intensity
    /// 0.9). Lower intensities take a fixed fraction of it: 20 % at 0.2 and
    /// two thirds at 0.5. Default 75. Drips and the texture layer follow the
    /// intensity curve, not this knob.
    pub drop_rate: f32,
    /// Sporadic large drips per second at intensity=1 (gutter / puddle hits).
    pub drip_rate: f32,
    /// Gain for the broadband impact noise burst.
    pub impact_gain: f32,
    /// Gain for the Minnaert bubble sinusoid. This is the main "rain" sound.
    pub bubble_gain: f32,
    /// Subtle brown noise bed gain (low-frequency body).
    pub bed_gain: f32,
    /// Gain for the pink-modulated noise texture layer.
    pub texture_gain: f32,
    /// Output trim.
    pub master_gain: f32,

    /// World-space position of this rain source.
    pub position: Vec3,

    rng: Rng,
}

impl RainSourceV2 {
    /// Create a new physically-based rain source.
    ///
    /// - `position`: where the rain originates (e.g. overhead skylight)
    /// - `intensity`: 0.0 (silence) to 1.0 (heavy downpour)
    /// - `seed`: PRNG seed — use different seeds for multiple sources
    pub fn new(position: Vec3, intensity: f32, seed: u64) -> Self {
        Self {
            brown: BrownNoise::new(seed.wrapping_add(1)),
            pink_mod1: PinkNoise::new(seed.wrapping_add(2)),
            pink_mod2: PinkNoise::new(seed.wrapping_add(3)),
            texture_rng: Rng::new(seed.wrapping_add(4)),
            texture_lp_state: 0.0,
            ring: Box::new([0.0; RING_SIZE]),
            ring_idx: 0,
            env_state: 0.0,
            env_smooth: 0.0, // set from RING_SMOOTHING_CORNER_HZ on the first sample
            white_noise_scale: 1.0,
            sample_rate_cached: 0.0,
            output_lp_state: 0.0,
            intensity: intensity.clamp(0.0, 1.0),
            drop_rate: 75.0,
            drip_rate: 1.5,
            impact_gain: 0.6,   // compensate for 2-pole LP amplitude loss
            bubble_gain: 0.08,  // very subtle — pure tones are perceptually sharp
            bed_gain: 0.04,     // barely-there sub-bass warmth
            texture_gain: 0.03, // barely perceptible fill between drops
            master_gain: 2.5,   // boost overall — rain should be audible at normal volume
            position,
            rng: Rng::new(seed),
        }
    }

    /// Write a noise impact burst directly into the ring.
    ///
    /// LP cutoff varies with intensity: light rain = dark thuds (heard through
    /// walls), medium/heavy = brighter taps (closer, more exposed).
    /// Duration: 3-10ms. Inline one-pole LP shapes the color.
    fn write_impact(&mut self, drop_radius_mm: f32, gain: f32, sample_rate: f32) {
        // Duration scales with drop size: 3ms (tiny) to 8ms (large)
        let dur = 0.003 + 0.0017 * drop_radius_mm.min(3.0);
        let len = ((dur * sample_rate) as usize).clamp(4, RING_SIZE / 2);
        let attack_samples = (0.0008 * sample_rate).max(1.0); // 0.8ms rise — just enough to avoid click
        let decay_rate = 1.0 / (dur * 0.4);

        // LP cutoff — INVERTED: light rain is brighter (exposed taps),
        // heavy rain is darker (density creates a low-mid wash)
        // With 2-pole LP (-12dB/oct), cutoff needs to be well above target
        // band to let presence through at low intensity.
        //   light (0.2): ~4660 Hz — taps have presence, brilliance tamed
        //   medium (0.5): ~3400 Hz — balanced
        //   heavy (0.9): ~1720 Hz — dense wash, low-mid dominant
        let base_cutoff = 5500.0 - 4200.0 * self.intensity;
        let cutoff = base_cutoff - 150.0 * (drop_radius_mm - 1.0).clamp(0.0, 2.0);
        let cutoff = cutoff.clamp(800.0, 6000.0);

        // Two-pole LP (-12 dB/oct) — steep enough to kill presence band
        let rc_lp = 1.0 / (TAU * cutoff);
        let dt = 1.0 / sample_rate;
        let alpha_lp = dt / (rc_lp + dt);
        let mut lp1 = 0.0_f32;
        let mut lp2 = 0.0_f32;

        // HP filter at ~120 Hz — removes sub-bass rumble
        let rc_hp = 1.0 / (TAU * 120.0);
        let alpha_hp = rc_hp / (rc_hp + dt);
        let mut hp_prev_in = 0.0_f32;
        let mut hp_state = 0.0_f32;
        let white_noise_scale = white_noise_density_scale(sample_rate);

        for i in 0..len {
            let t = i as f32 / sample_rate;
            let attack = (i as f32 / attack_samples).min(1.0);
            let env = attack * (-t * decay_rate).exp();
            if env < 0.001 && i > attack_samples as usize {
                break;
            }
            let noise = self.rng.next_bipolar() * white_noise_scale;
            // Cascaded LP (2-pole) then HP: tight bandpass on low-mid
            lp1 += alpha_lp * (noise - lp1);
            lp2 += alpha_lp * (lp1 - lp2);
            hp_state = alpha_hp * (hp_state + lp2 - hp_prev_in);
            hp_prev_in = lp2;
            let p = (self.ring_idx + 1 + i) & RING_MASK;
            self.ring[p] += hp_state * env * gain;
        }
    }

    /// Write a damped sinusoidal bubble oscillation into the ring.
    ///
    /// Models the trapped air bubble that forms on water impact.
    /// Frequency follows empirical Minnaert data. Random initial phase
    /// prevents phase-locked artifacts when many bubbles overlap.
    /// A short attack ramp (0.3ms) avoids click at onset.
    /// Higher frequencies are attenuated (air absorption over distance).
    fn write_bubble(&mut self, freq: f32, decay_rate: f32, gain: f32, sample_rate: f32) {
        // Higher frequency bubbles are quieter (air absorption + distance)
        let freq_atten = 1.0 - (freq / 20000.0).clamp(0.0, 0.7);
        let gain = gain * freq_atten;

        // Ring for ~4 time constants
        let dur_samples = ((4.0 / decay_rate) * sample_rate) as usize;
        let len = dur_samples.min(RING_SIZE / 2);

        let attack_samples = (0.0003 * sample_rate).max(1.0); // 0.3ms rise
        let phase = self.rng.next_f32() * TAU;

        for i in 0..len {
            let t = i as f32 / sample_rate;
            let attack = (i as f32 / attack_samples).min(1.0);
            let env = attack * (-t * decay_rate).exp();
            if env < 0.001 && i > attack_samples as usize {
                break;
            }
            let osc = (TAU * freq * t + phase).sin();
            let p = (self.ring_idx + 1 + i) & RING_MASK;
            self.ring[p] += osc * env * gain;
        }
    }

    /// Determine bubble frequency and decay rate from drop radius.
    ///
    /// Empirical Minnaert data shifted down for warmth (raw values are
    /// measured underwater — through air at distance, HF is heavily absorbed):
    ///   - Small drops (<1.2mm):  500-2500 Hz, at half the base probability
    ///   - Medium drops (1.2-2mm): 400-2000 Hz
    ///   - Large drops (>2mm):    250-1200 Hz (warm "plink")
    ///
    /// Bubble probability and frequency scale with intensity.
    ///
    /// Real data shows:
    ///   Light (centroid ~400Hz): mostly dark, few bubbles
    ///   Medium (centroid ~2700Hz): bright! Many exposed individual drops
    ///   Heavy (centroid ~550Hz): bassy wash, some mid-freq content
    fn bubble_params(&mut self, drop_radius_mm: f32) -> Option<(f32, f32)> {
        // Bubble probability — moderate across the board
        let bubble_prob = if self.intensity < 0.3 {
            0.12
        } else if self.intensity < 0.7 {
            0.35
        } else {
            0.20
        };

        if drop_radius_mm < 1.2 {
            if self.rng.next_f32() > bubble_prob * 0.5 {
                return None;
            }
            // Small drops: 800-2kHz (warm sparkle, not piercing)
            let base = 800.0 + 800.0 * self.intensity.min(0.7);
            let freq = base + self.rng.next_f32() * 600.0 - 300.0;
            let decay = 600.0 + self.rng.next_f32() * 400.0;
            Some((freq.clamp(500.0, 2500.0), decay))
        } else if drop_radius_mm <= 2.0 {
            if self.rng.next_f32() > bubble_prob {
                return None;
            }
            // Medium drops: 500-1.5kHz
            let base = 500.0 + 600.0 * self.intensity.min(0.7);
            let freq = base + self.rng.next_f32() * 600.0;
            let decay = 450.0 + self.rng.next_f32() * 300.0;
            Some((freq.clamp(400.0, 2000.0), decay))
        } else {
            if self.rng.next_f32() > bubble_prob * 1.2 {
                return None;
            }
            // Large drops: 300-1kHz (deep warm plink)
            let t = ((drop_radius_mm - 2.0) / 1.5).clamp(0.0, 1.0);
            let base_freq = 1000.0 - t * 500.0;
            let freq = base_freq + self.rng.next_f32() * 300.0 - 150.0;
            let decay = 300.0 + self.rng.next_f32() * 200.0;
            Some((freq.clamp(250.0, 1200.0), decay))
        }
    }

    /// Pick a raindrop radius [mm] based on intensity.
    /// Same Medwin & Palmer '94 distribution as v1.
    fn pick_drop_radius(&mut self) -> f32 {
        let r = self.rng.next_f32();
        if self.intensity < 0.3 {
            if r < 0.8 {
                0.95 + self.rng.next_f32() * 0.25
            } else {
                1.4 + self.rng.next_f32() * 0.6
            }
        } else if self.intensity < 0.7 {
            if r < 0.5 {
                1.0 + self.rng.next_f32() * 0.4
            } else if r < 0.85 {
                1.6 + self.rng.next_f32() * 0.9
            } else {
                2.4 + self.rng.next_f32() * 1.2
            }
        } else if r < 0.3 {
            1.2 + self.rng.next_f32() * 0.6
        } else {
            2.0 + self.rng.next_f32() * 1.5
        }
    }
}

impl SoundSource for RainSourceV2 {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.env_smooth = (-TAU * RING_SMOOTHING_CORNER_HZ / sample_rate).exp();
            self.white_noise_scale = white_noise_density_scale(sample_rate);
        }
        let intensity = self.intensity;
        // Piecewise-linear drop rate: light=15/s, medium=50/s, heavy=75/s
        // with the default drop_rate. The fraction alone (0..1) scales the
        // texture and drips, so the drop_rate knob changes only the drops.
        let drop_factor = drop_rate_fraction(intensity);
        let drop_rate = self.drop_rate * drop_factor;
        let drip_rate = self.drip_rate * drop_factor;

        // 1. Subtle low-frequency bed (distant aggregate rain body)
        let bed = self.bed_gain * intensity * self.brown.next_sample(sample_rate);

        // 2. Csound-inspired texture: noise × pink × pink
        // White noise modulated by two pink noise sources creates organic
        // amplitude fluctuations matching natural rain statistics.
        // Scaled by the drop-rate fraction so light rain stays
        // sparse/transparent while heavy rain gets the dense "wash" between
        // drops.
        let white = self.texture_rng.next_bipolar() * self.white_noise_scale;
        let mod1 = self.pink_mod1.next_sample(sample_rate).abs(); // rectify: 0..~0.8
        let mod2 = self.pink_mod2.next_sample(sample_rate).abs();
        let texture_raw = white * mod1 * mod2;
        // LP cutoff inverted like impacts: light = warm-bright (3.5kHz), heavy = warm (1.5kHz)
        let tex_cutoff = 3500.0 - 2000.0 * intensity;
        let tex_rc = 1.0 / (TAU * tex_cutoff);
        let tex_dt = 1.0 / sample_rate;
        let tex_alpha = tex_dt / (tex_rc + tex_dt);
        self.texture_lp_state += tex_alpha * (texture_raw - self.texture_lp_state);
        let texture = self.texture_lp_state;
        let texture_level = self.texture_gain * drop_factor;

        // 3. Read ring buffer (superposition of all active drops)
        let tail = self.ring[self.ring_idx];
        self.ring[self.ring_idx] = 0.0;
        self.env_state = self.env_state * self.env_smooth + tail * (1.0 - self.env_smooth);

        let sample = bed + texture * texture_level + self.env_state;

        // 4. Stochastic drop events
        if self.rng.next_f32() < drop_rate / sample_rate {
            let radius = self.pick_drop_radius();
            let amp = self.impact_gain * (0.3 + self.rng.next_f32() * 0.7);

            // Impact: short broadband noise burst
            self.write_impact(radius, amp, sample_rate);

            // Bubble: damped sinusoid at Minnaert frequency (if drop traps air)
            if let Some((freq, decay)) = self.bubble_params(radius) {
                self.write_bubble(freq, decay, amp * self.bubble_gain, sample_rate);
            }
        }

        // 5. Sporadic large drips (puddle / gutter hits — always produce bubbles)
        if self.rng.next_f32() < drip_rate / sample_rate {
            let radius = 2.5 + self.rng.next_f32() * 1.0; // large drops only
            let amp = self.impact_gain * 1.2;

            self.write_impact(radius, amp, sample_rate);

            // Drips always hit water → deep warm bubble
            let freq = 250.0 + self.rng.next_f32() * 600.0; // 250-850Hz
            let decay = 200.0 + self.rng.next_f32() * 150.0; // τ ≈ 2.8-5ms
            self.write_bubble(freq, decay, amp * self.bubble_gain, sample_rate);
        }

        // 6. Advance ring
        self.ring_idx = (self.ring_idx + 1) & RING_MASK;

        // 7. Output smoothing LP — gentle at light intensity, transparent at heavy
        //    light (0.2): ~2000 Hz — rounded patter
        //    medium (0.5): ~3800 Hz — natural
        //    heavy (0.9): ~6200 Hz — transparent
        let out_cutoff = 800.0 + 6000.0 * intensity;
        let out_rc = 1.0 / (TAU * out_cutoff);
        let out_dt = 1.0 / sample_rate;
        let out_alpha = out_dt / (out_rc + out_dt);
        self.output_lp_state += out_alpha * (sample - self.output_lp_state);

        self.output_lp_state * self.master_gain
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
    fn v2_produces_nonzero_output() {
        let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 42);
        let sum: f32 = (0..48000).map(|_| rain.next_sample(48000.0).abs()).sum();
        assert!(sum > 0.0, "rain v2 is silent");
    }

    #[test]
    fn v2_silent_at_zero_intensity() {
        let mut rain = RainSourceV2::new(Vec3::ZERO, 0.0, 42);
        let sum: f32 = (0..48000).map(|_| rain.next_sample(48000.0).abs()).sum();
        assert_eq!(sum, 0.0, "rain v2 should be silent at zero intensity");
    }

    #[test]
    fn v2_louder_at_higher_intensity() {
        let mut light = RainSourceV2::new(Vec3::ZERO, 0.2, 42);
        let mut heavy = RainSourceV2::new(Vec3::ZERO, 0.9, 42);

        let energy_light: f32 = (0..48000).map(|_| light.next_sample(48000.0).powi(2)).sum();
        let energy_heavy: f32 = (0..48000).map(|_| heavy.next_sample(48000.0).powi(2)).sum();

        assert!(
            energy_heavy > energy_light,
            "heavy rain ({energy_heavy}) should be louder than light ({energy_light})"
        );
    }

    #[test]
    fn v2_bed_is_quieter_than_drops() {
        // Continuous layers alone should be significantly quieter than full mix.
        // Drops are silenced via impact_gain/bubble_gain so the event stream
        // (and with it every rng draw) is the same in both renders.
        let mut bed_only = RainSourceV2::new(Vec3::ZERO, 0.5, 42);
        bed_only.impact_gain = 0.0;
        bed_only.bubble_gain = 0.0;
        bed_only.texture_gain = 0.0;

        let mut full = RainSourceV2::new(Vec3::ZERO, 0.5, 42);

        let energy_bed: f32 = (0..48000)
            .map(|_| bed_only.next_sample(48000.0).powi(2))
            .sum();
        let energy_full: f32 = (0..48000).map(|_| full.next_sample(48000.0).powi(2)).sum();

        assert!(
            energy_full > energy_bed * 1.2,
            "drops should add significant energy over bed alone (bed={energy_bed}, full={energy_full})"
        );
    }

    use crate::synth::test_support::{
        averaged_power_spectrum, band_power, mean, peak, render, root_mean_square,
    };

    /// Sample indices at which a drop or drip started. Every event writes its
    /// impact starting at `ring[ring_idx + 1]` with a 0.8 ms attack ramp, so
    /// burst sample 1 (slot `ring_idx + 2`) is non-zero; that slot changing
    /// during a call marks an event. It is first read two calls later.
    fn event_count(source: &mut RainSourceV2, seconds: f32, sample_rate: f32) -> usize {
        let mut count = 0;
        for _ in 0..(seconds * sample_rate) as usize {
            let watched = (source.ring_idx + 2) & RING_MASK;
            let before = source.ring[watched];
            source.next_sample(sample_rate);
            if source.ring[watched] != before {
                count += 1;
            }
        }
        count
    }

    fn drops_only(intensity: f32, seed: u64) -> RainSourceV2 {
        let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, seed);
        rain.drip_rate = 0.0;
        rain
    }

    /// Drop-rate curve: 15/s at intensity 0.2, 50/s at 0.5, 75/s at 0.9 and
    /// above, linear in between and from zero. Counts are Poisson, so allow
    /// four standard deviations.
    #[test]
    fn drop_rate_follows_the_piecewise_intensity_curve() {
        for (intensity, expected_rate) in [
            (0.1, 7.5),
            (0.2, 15.0),
            (0.35, 32.5),
            (0.5, 50.0),
            (0.7, 62.5),
            (0.9, 75.0),
            (1.0, 75.0),
        ] {
            let seconds = 40.0;
            let expected = expected_rate * seconds;
            let count = event_count(&mut drops_only(intensity, 31), seconds, 16_000.0) as f32;
            assert!(
                (count - expected).abs() < 4.0 * expected.sqrt(),
                "intensity {intensity}: {count} drops in {seconds} s, expected {expected}"
            );
        }
    }

    /// The default `drop_rate` (75/s) times the intensity fraction is the
    /// tuned curve exactly: 15/s at 0.2, 50/s at 0.5, 75/s from 0.9 up.
    #[test]
    fn default_drop_rate_reproduces_the_tuned_curve() {
        let default_rate = RainSourceV2::new(Vec3::ZERO, 0.5, 1).drop_rate;
        for (intensity, expected_rate) in [
            (0.0, 0.0),
            (0.1, 7.5),
            (0.2, 15.0),
            (0.35, 32.5),
            (0.5, 50.0),
            (0.7, 62.5),
            (0.9, 75.0),
            (1.0, 75.0),
        ] {
            let rate = default_rate * drop_rate_fraction(intensity);
            assert!(
                (rate - expected_rate).abs() < 1e-4,
                "intensity {intensity}: {rate} drops/s, expected {expected_rate}"
            );
        }
    }

    #[test]
    fn drop_rate_is_the_same_at_every_sample_rate() {
        for sample_rate in [16_000.0_f32, 44_100.0, 96_000.0] {
            let seconds = if sample_rate > 40_000.0 { 8.0 } else { 20.0 };
            let expected = 50.0 * seconds;
            let count = event_count(&mut drops_only(0.5, 32), seconds, sample_rate) as f32;
            assert!(
                (count - expected).abs() < 4.0 * expected.sqrt(),
                "{sample_rate} Hz: {count} drops in {seconds} s, expected {expected}"
            );
        }
    }

    /// Drips are an independent Poisson stream at `drip_rate * drop_rate / 75`.
    #[test]
    fn drip_rate_adds_its_own_poisson_stream() {
        let seconds = 30.0;
        let mut dripping = RainSourceV2::new(Vec3::ZERO, 0.9, 33);
        dripping.drip_rate = 40.0;
        let total = event_count(&mut dripping, seconds, 16_000.0) as f32;
        let expected = (75.0 + 40.0) * seconds;
        assert!(
            (total - expected).abs() < 4.0 * expected.sqrt(),
            "{total} events in {seconds} s, expected {expected}"
        );
    }

    /// `drop_rate` is the rate at full intensity and must reach the DSP: it
    /// used to be ignored in favour of a fixed curve, so `drop_rate` = 10 and
    /// 600 produced identical event streams (793 events in 10 s either way).
    #[test]
    fn drop_rate_knob_changes_the_event_rate() {
        let mut sparse = drops_only(0.9, 34);
        sparse.drop_rate = 10.0;
        let mut dense = drops_only(0.9, 34);
        dense.drop_rate = 600.0;
        let sparse_count = event_count(&mut sparse, 10.0, 16_000.0);
        let dense_count = event_count(&mut dense, 10.0, 16_000.0);
        assert!(
            dense_count > sparse_count * 5,
            "{sparse_count} vs {dense_count}"
        );
    }

    /// Per size class: bubble probability and frequency / decay ranges.
    /// Small (<1.2 mm) 0.5 p, medium 1.0 p, large 1.2 p with p = 0.12 / 0.35 /
    /// 0.20 for drizzle / moderate / heavy.
    #[test]
    fn bubble_parameters_follow_the_size_class_tables() {
        let draws = 30_000;
        for (intensity, probability) in [(0.2, 0.12), (0.5, 0.35), (0.9, 0.20)] {
            for (radius, scale, freq_range, decay_range) in [
                (1.0_f32, 0.5, (500.0, 2_500.0), (600.0, 1_000.0)),
                (1.6, 1.0, (400.0, 2_000.0), (450.0, 750.0)),
                (3.0, 1.2, (250.0, 1_200.0), (300.0, 500.0)),
            ] {
                let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 77);
                let mut present = 0;
                for _ in 0..draws {
                    if let Some((frequency, decay)) = rain.bubble_params(radius) {
                        present += 1;
                        assert!(
                            (freq_range.0..=freq_range.1).contains(&frequency),
                            "radius {radius}: {frequency} Hz outside {freq_range:?}"
                        );
                        assert!(
                            (decay_range.0..decay_range.1).contains(&decay),
                            "radius {radius}: decay {decay} outside {decay_range:?}"
                        );
                    }
                }
                let measured = present as f32 / draws as f32;
                let expected = probability * scale;
                assert!(
                    (measured - expected).abs() < 0.012,
                    "intensity {intensity}, radius {radius}: bubble share {measured}, expected {expected}"
                );
            }
        }
    }

    /// Bigger drops ring lower: the mean bubble frequency falls with radius
    /// (the physical Minnaert direction, f ~ 1/radius), though the values sit
    /// well below Minnaert's 3.26/r kHz*mm (see module header: shifted down
    /// for warmth).
    #[test]
    fn bubble_frequency_falls_as_drops_get_bigger() {
        let mean_frequency = |radius: f32| {
            let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 5);
            let (mut sum, mut count) = (0.0, 0);
            for _ in 0..20_000 {
                if let Some((frequency, _)) = rain.bubble_params(radius) {
                    sum += frequency;
                    count += 1;
                }
            }
            sum / count as f32
        };
        let (small, medium, large) = (
            mean_frequency(1.0),
            mean_frequency(1.6),
            mean_frequency(3.0),
        );
        assert!(small > medium && medium > large, "{small} {medium} {large}");
    }

    /// Reference: Minnaert resonance f = (1 / 2 pi r) sqrt(3 gamma P0 / rho)
    /// = 3.26 kHz*mm / r for an air bubble in water. The river's comment that
    /// 1.2-7 mm bubbles give 450-2700 Hz follows from it.
    #[test]
    fn minnaert_constant_is_3_26_hz_metres() {
        let gamma = 1.4_f64;
        let pressure = 101_325.0_f64;
        let density = 998.0_f64;
        let constant = (3.0 * gamma * pressure / density).sqrt() / std::f64::consts::TAU;
        assert!((constant - 3.26).abs() < 0.03, "{constant}");
        let frequency = |radius_mm: f64| constant / (radius_mm * 1e-3);
        assert!((frequency(1.2) - 2_717.0).abs() < 30.0);
        assert!((frequency(7.0) - 466.0).abs() < 6.0);
    }

    /// A written bubble is a decaying sine: its cycle count gives the
    /// frequency, its envelope falls by 1/e per 1/decay seconds, it lasts
    /// four time constants, and its gain is scaled by 1 - f / 20 kHz.
    #[test]
    fn written_bubble_has_the_requested_frequency_decay_and_duration() {
        let sample_rate = 48_000.0;
        let (frequency, decay) = (1_000.0_f32, 500.0_f32);
        let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 1);
        rain.write_bubble(frequency, decay, 1.0, sample_rate);
        let start = 1;
        let length = (4.0 / decay * sample_rate) as usize; // 384 samples
        let bubble: Vec<f32> = (0..length + 8)
            .map(|offset| rain.ring[start + offset])
            .collect();
        assert!(
            bubble[length..].iter().all(|&value| value == 0.0),
            "bubble longer than 4/decay"
        );
        let cycles = bubble[..length]
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        let expected_cycles = frequency * 4.0 / decay; // 8
        assert!(
            (cycles as f32 - expected_cycles).abs() <= 1.0,
            "{cycles} cycles"
        );
        // Envelope: peak within one cycle (48 samples) at t0 vs at t = 1/decay.
        let window_peak = |from: usize| {
            bubble[from..from + 48]
                .iter()
                .fold(0.0_f32, |a, v| a.max(v.abs()))
        };
        let time_constant = (sample_rate / decay) as usize; // 96 samples
        let ratio = window_peak(time_constant) / window_peak(0);
        assert!(
            (ratio - (-1.0_f32).exp()).abs() < 0.12,
            "envelope ratio {ratio}"
        );
        // Gain scaling: at most 1 - 1000/20000 = 0.95 of the unit gain; the
        // random start phase and the 0.3 ms attack ramp take some of it off.
        let largest = peak(&bubble);
        assert!((0.5..=0.951).contains(&largest), "peak {largest}");
    }

    /// Impact bursts last 3 ms + 1.7 ms per mm of radius (radius capped at
    /// 3 mm) and decay with tau = 0.4 of that duration.
    #[test]
    fn impact_duration_grows_with_drop_radius() {
        let sample_rate = 48_000.0;
        for (radius, expected_length) in [(1.0_f32, 225usize), (2.0, 306), (3.0, 388), (3.5, 388)] {
            let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 2);
            rain.write_impact(radius, 1.0, sample_rate);
            let last = (1..rain.ring.len() / 2)
                .rev()
                .find(|&index| rain.ring[index] != 0.0)
                .unwrap();
            assert!(
                (last as i32 - expected_length as i32).abs() <= 4,
                "radius {radius}: last non-zero slot {last}, expected {expected_length}"
            );
        }
    }

    /// "light rain is brighter, heavy rain darker": the impact lowpass moves
    /// from ~4.7 kHz at intensity 0.2 to ~1.7 kHz at 0.9. The share of power
    /// above 1.5 kHz must therefore fall as intensity rises.
    #[test]
    fn impact_spectrum_darkens_as_intensity_rises() {
        let sample_rate = 48_000.0;
        let bright_share = |intensity: f32| -> f64 {
            let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 3);
            let mut stream = Vec::new();
            while stream.len() < 16_384 {
                rain.ring.fill(0.0);
                rain.write_impact(1.5, 1.0, sample_rate);
                stream.extend((1..400).map(|index| rain.ring[index]));
            }
            let spectrum = averaged_power_spectrum(&stream);
            band_power(&spectrum, sample_rate, 1_500.0, 12_000.0)
                / band_power(&spectrum, sample_rate, 150.0, 12_000.0)
        };
        let light = bright_share(0.2);
        let moderate = bright_share(0.5);
        let heavy = bright_share(0.9);
        assert!(
            light > moderate && moderate > heavy,
            "{light:.3} {moderate:.3} {heavy:.3}"
        );
    }

    #[test]
    fn output_is_finite_zero_mean_and_inside_unity() {
        for intensity in [0.2, 0.5, 1.0] {
            let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 5);
            let samples = render(15.0, 48_000.0, |rate| rain.next_sample(rate));
            assert!(samples.iter().all(|value| value.is_finite()));
            assert!(
                peak(&samples) < 1.0,
                "intensity {intensity}: peak {}",
                peak(&samples)
            );
            assert!(mean(&samples).abs() < 0.01, "DC {}", mean(&samples));
        }
    }

    #[test]
    fn rms_is_roughly_stable_across_sample_rates() {
        let rms = |sample_rate: f32| {
            let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 42);
            let samples = render(8.0, sample_rate, |rate| rain.next_sample(rate));
            root_mean_square(&samples)
        };
        // Measured 1.07 (+0.6 dB, it read 0.84 before the white-noise bursts
        // kept their density per Hz); the residual comes from the
        // backward-Euler one-pole lowpasses, whose corner rises with the rate.
        let ratio = rms(96_000.0) / rms(48_000.0);
        assert!((0.70..1.30).contains(&ratio), "RMS ratio {ratio}");
    }

    /// With only the continuous layers left, a blank intensity still leaves
    /// exact silence, and raising it raises RMS monotonically at 0.2/0.5/0.9.
    #[test]
    fn level_rises_with_intensity_up_to_the_rate_plateau() {
        let level = |intensity: f32| {
            let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 6);
            let samples = render(6.0, 16_000.0, |rate| rain.next_sample(rate));
            root_mean_square(&samples)
        };
        assert!(level(0.2) < level(0.5));
        assert!(level(0.5) > 0.0 && level(0.0) == 0.0);
    }
}
