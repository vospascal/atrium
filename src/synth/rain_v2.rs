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

use atrium_core::commands::SynthParam;
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
    /// What changes while it plays; any other setting is not one it has.
    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through, and one NaN would turn every later
        // sample into NaN: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        match param {
            SynthParam::Intensity => self.intensity = value.clamp(0.0, 1.0),
            SynthParam::DropRate => self.drop_rate = value.max(0.0),
            SynthParam::DripRate => self.drip_rate = value.max(0.0),
            SynthParam::MasterGain => self.master_gain = value.clamp(0.0, 2.0),
            _ => {}
        }
    }

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
mod tests;
