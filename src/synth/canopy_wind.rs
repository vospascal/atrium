//! Generative wind moving through a leafy tree canopy.
//!
//! This is deliberately a different instrument from open-field wind. Its
//! layers model the things in the canopy that radiate sound:
//! - a broad, stable 100-1000 Hz foliage body;
//! - continuous leaf wash whose brightness follows wind and leaf dryness;
//! - statistically superposed leaf-contact grains;
//! - sparse, low-level branch creaks at stronger speeds.
//!
//! The weather driver is shared with [`super::field_wind`], but every sonic
//! layer here is independent. All clocks advance per audio sample, so neither
//! graphics FPS nor host callback size can alter the result.

use std::f32::consts::TAU;

use atrium_core::commands::SynthParam;
use atrium_core::source::{EmitterKind, SoundSource};

use crate::world::types::Vec3;

use super::field_wind::WindDriver;
use super::noise::{OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};

const CONTACT_RING_SIZE: usize = 4096;
const CONTACT_RING_MASK: usize = CONTACT_RING_SIZE - 1;

/// A diffuse vegetation field driven by bounded wind speed in metres/second.
pub struct CanopyWindSource {
    driver: WindDriver,

    body_noise: PinkNoiseFull,
    body_hp: OnePoleHP,
    body_hp2: OnePoleHP,
    body_lp: OnePoleLP,
    body_lp2: OnePoleLP,

    leaf_noise: PinkNoiseFull,
    leaf_hp: OnePoleHP,
    leaf_hp2: OnePoleHP,
    leaf_lp: OnePoleLP,

    dry_rng: Rng,
    dry_hp: OnePoleHP,
    dry_hp2: OnePoleHP,
    dry_lp: OnePoleLP,

    contact_rng: Rng,
    contact_ring: Box<[f32; CONTACT_RING_SIZE]>,
    contact_ring_index: usize,

    branch_rng: Rng,
    branch_phase: f32,
    branch_frequency: f32,
    branch_envelope: f32,
    branch_decay: f32,
    branch_noise_lp: OnePoleLP,

    /// Hard bounds for instantaneous wind speed, in m/s.
    pub min_speed: f32,
    pub max_speed: f32,
    /// Fraction of the speed range reserved for shorter positive gusts.
    pub gust_strength: f32,
    /// Positive values tend toward faster rises and slower releases.
    pub rise_bias: f32,
    /// Sub-second eddy modulation depth.
    pub turbulence_depth: f32,
    pub gust_brightness: f32,
    pub turbulence_brightness: f32,

    /// Amount of foliage available to rustle and collide, 0-1.
    pub foliage_density: f32,
    /// Leaf stiffness/dryness, 0 = soft green leaves, 1 = dry bright leaves.
    pub leaf_dryness: f32,
    /// Probability and level of low branch movement, 0-1.
    pub branch_level: f32,

    pub body_gain: f32,
    pub rustle_gain: f32,
    pub contact_gain: f32,
    pub master_gain: f32,

    position: Vec3,
    sample_rate_cached: f32,
}

impl CanopyWindSource {
    pub fn new(position: Vec3, min_speed: f32, max_speed: f32, seed: u64) -> Self {
        let min_speed = min_speed.clamp(0.0, 25.0);
        let max_speed = max_speed.clamp(min_speed, 25.0);
        Self {
            driver: WindDriver::new(seed),
            body_noise: PinkNoiseFull::new(seed.wrapping_add(1)),
            body_hp: OnePoleHP::new(90.0, 48_000.0),
            body_hp2: OnePoleHP::new(90.0, 48_000.0),
            body_lp: OnePoleLP::new(1_050.0, 48_000.0),
            body_lp2: OnePoleLP::new(1_050.0, 48_000.0),
            leaf_noise: PinkNoiseFull::new(seed.wrapping_add(3)),
            leaf_hp: OnePoleHP::new(450.0, 48_000.0),
            leaf_hp2: OnePoleHP::new(450.0, 48_000.0),
            leaf_lp: OnePoleLP::new(5_500.0, 48_000.0),
            dry_rng: Rng::new(seed.wrapping_add(5)),
            dry_hp: OnePoleHP::new(2_400.0, 48_000.0),
            dry_hp2: OnePoleHP::new(2_400.0, 48_000.0),
            dry_lp: OnePoleLP::new(11_000.0, 48_000.0),
            contact_rng: Rng::new(seed.wrapping_add(7)),
            contact_ring: Box::new([0.0; CONTACT_RING_SIZE]),
            contact_ring_index: 0,
            branch_rng: Rng::new(seed.wrapping_add(11)),
            branch_phase: 0.0,
            branch_frequency: 120.0,
            branch_envelope: 0.0,
            branch_decay: 0.0,
            branch_noise_lp: OnePoleLP::new(350.0, 48_000.0),
            min_speed,
            max_speed,
            gust_strength: 0.40,
            rise_bias: 0.30,
            turbulence_depth: 0.30,
            gust_brightness: 0.0,
            turbulence_brightness: 0.0,
            foliage_density: 0.75,
            leaf_dryness: 0.25,
            branch_level: 0.12,
            body_gain: 0.80,
            rustle_gain: 0.90,
            contact_gain: 0.75,
            master_gain: 1.0,
            position,
            sample_rate_cached: 0.0,
        }
    }

    pub fn set_speed_range(&mut self, min_speed: f32, max_speed: f32) {
        let min_speed = min_speed.clamp(0.0, 25.0);
        self.min_speed = min_speed.min(max_speed.clamp(0.0, 25.0));
        self.max_speed = min_speed.max(max_speed.clamp(0.0, 25.0));
    }

    pub fn set_change_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.driver.set_change_time_range(min_seconds, max_seconds);
    }

    pub fn change_time_range(&self) -> (f32, f32) {
        self.driver.change_time_range()
    }

    pub fn set_gust_duration_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.driver
            .set_gust_duration_range(min_seconds, max_seconds);
    }

    pub fn gust_duration_range(&self) -> (f32, f32) {
        self.driver.gust_duration_range()
    }

    pub fn set_turbulence_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.driver
            .set_turbulence_time_range(min_seconds, max_seconds);
    }

    pub fn turbulence_time_range(&self) -> (f32, f32) {
        self.driver.turbulence_time_range()
    }

    fn retune(&mut self, sample_rate: f32) {
        self.body_hp.set_cutoff(90.0, sample_rate);
        self.body_hp2.set_cutoff(90.0, sample_rate);
        self.body_lp.set_cutoff(1_050.0, sample_rate);
        self.body_lp2.set_cutoff(1_050.0, sample_rate);
        self.leaf_hp.set_cutoff(450.0, sample_rate);
        self.leaf_hp2.set_cutoff(450.0, sample_rate);
        self.leaf_lp
            .set_cutoff(5_500.0_f32.min(sample_rate * 0.45), sample_rate);
        self.dry_hp.set_cutoff(2_400.0, sample_rate);
        self.dry_hp2.set_cutoff(2_400.0, sample_rate);
        self.dry_lp
            .set_cutoff(11_000.0_f32.min(sample_rate * 0.45), sample_rate);
        self.branch_noise_lp.set_cutoff(350.0, sample_rate);
    }

    /// Add one short, filtered leaf-on-leaf contact to a fixed ring buffer.
    /// Event superposition naturally turns individual taps into dense rustle.
    fn write_leaf_contact(&mut self, sample_rate: f32) {
        let dryness = self.leaf_dryness.clamp(0.0, 1.0);
        let seconds = 0.003 + self.contact_rng.next_f32() * (0.006 + 0.007 * dryness);
        let length = ((seconds * sample_rate) as usize).clamp(8, CONTACT_RING_SIZE / 2);
        let attack_samples = (0.0007 * sample_rate).max(1.0);

        let highpass_hz = 550.0 + 1_050.0 * dryness;
        let lowpass_hz = (2_800.0 + 5_800.0 * dryness + 1_400.0 * self.contact_rng.next_f32())
            .min(sample_rate * 0.45);
        let hp_a = (-TAU * highpass_hz / sample_rate).exp();
        let lp_mix = 1.0 - (-TAU * lowpass_hz / sample_rate).exp();
        let event_gain =
            0.055 * (0.75 + 0.50 * dryness) * (0.65 + 0.35 * self.contact_rng.next_f32());

        let mut lowpass = 0.0_f32;
        let mut hp_state = 0.0_f32;
        let mut hp_previous = 0.0_f32;
        for i in 0..length {
            let attack = (i as f32 / attack_samples).min(1.0);
            let decay = (1.0 - i as f32 / length as f32).powi(2);
            lowpass += lp_mix * (self.contact_rng.next_bipolar() - lowpass);
            hp_state = hp_a * (hp_state + lowpass - hp_previous);
            hp_previous = lowpass;
            let index = (self.contact_ring_index + 1 + i) & CONTACT_RING_MASK;
            self.contact_ring[index] += hp_state * attack * decay * event_gain;
        }
    }

    fn next_branch(&mut self, speed: f32, sample_rate: f32) -> f32 {
        let branch_level = self.branch_level.clamp(0.0, 1.0);
        if self.branch_envelope <= 0.0 && speed > 3.0 && branch_level > 0.0 {
            let bend = ((speed - 3.0) / 9.0).clamp(0.0, 1.5);
            let events_per_second = branch_level * (0.02 + 1.20 * bend * bend);
            if self.branch_rng.next_f32() < events_per_second / sample_rate {
                self.branch_phase = self.branch_rng.next_f32() * TAU;
                self.branch_frequency = 75.0 + 155.0 * self.branch_rng.next_f32();
                let duration = 0.25 + 0.90 * self.branch_rng.next_f32();
                self.branch_decay = (-6.9 / (duration * sample_rate)).exp();
                self.branch_envelope = 1.0;
            }
        }

        if self.branch_envelope <= 0.0 {
            return 0.0;
        }

        let noise = self.branch_noise_lp.process(self.branch_rng.next_bipolar());
        let resonant = 0.72 * self.branch_phase.sin()
            + 0.20 * (2.0 * self.branch_phase + 0.6).sin()
            + 0.08 * noise;
        let output = resonant * self.branch_envelope * branch_level * 0.20;

        // A slight downward bend keeps the event from reading as a static tone.
        let bent_frequency = self.branch_frequency * (0.72 + 0.28 * self.branch_envelope);
        self.branch_phase = (self.branch_phase + TAU * bent_frequency / sample_rate) % TAU;
        self.branch_envelope *= self.branch_decay;
        if self.branch_envelope < 0.001 {
            self.branch_envelope = 0.0;
        }
        output
    }
}

impl SoundSource for CanopyWindSource {
    #[inline]
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.retune(sample_rate);
        }

        let state = self.driver.next(
            sample_rate,
            self.min_speed,
            self.max_speed,
            self.gust_strength,
            self.rise_bias,
        );
        let speed_at_canopy = (state.speed / 8.0).clamp(0.0, 3.125);
        let foliage = self.foliage_density.clamp(0.0, 1.0);
        let dryness = self.leaf_dryness.clamp(0.0, 1.0);

        // Collision density grows faster than linearly: stronger flow moves
        // more leaves far enough to touch, while green/light air remains soft.
        let contact_rate = if state.speed <= 0.0 {
            0.0
        } else {
            foliage * (2.0 + 180.0 * speed_at_canopy.powf(1.7)).min(900.0)
        };
        if self.contact_rng.next_f32() < contact_rate / sample_rate {
            self.write_leaf_contact(sample_rate);
        }
        let contact = self.contact_ring[self.contact_ring_index];
        self.contact_ring[self.contact_ring_index] = 0.0;
        self.contact_ring_index = (self.contact_ring_index + 1) & CONTACT_RING_MASK;

        let body = self.body_lp2.process(
            self.body_lp.process(
                self.body_hp2
                    .process(self.body_hp.process(self.body_noise.next_sample())),
            ),
        );
        let leaf = self.leaf_lp.process(
            self.leaf_hp2
                .process(self.leaf_hp.process(self.leaf_noise.next_sample())),
        );
        let dry = self.dry_lp.process(
            self.dry_hp2
                .process(self.dry_hp.process(self.dry_rng.next_bipolar())),
        );
        let branch = self.next_branch(state.speed, sample_rate);

        let spectral_response =
            self.gust_brightness * state.gust + self.turbulence_brightness * state.eddy;
        let leaf_motion = ((0.42 + 0.58 * state.activity)
            * (0.65 + 0.35 * speed_at_canopy.min(1.0))
            + spectral_response)
            .max(0.0);
        let texture = self.body_gain * body * (0.30 + 0.70 * foliage)
            + self.rustle_gain * foliage * leaf * leaf_motion
            + self.rustle_gain * foliage * dryness * dry * (0.10 + 0.32 * leaf_motion)
            + self.contact_gain * contact
            + branch;

        let speed_gain = if state.speed <= 0.0 {
            0.0
        } else {
            speed_at_canopy.powf(1.25).min(4.2)
        };
        let eddy_db = state.eddy * self.turbulence_depth * state.activity * 3.0;
        let eddy_gain = 10.0_f32.powf(eddy_db / 20.0);
        texture * speed_gain * eddy_gain * self.master_gain
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }

    // Deliberately empty: all temporal state advances on the audio clock.
    fn tick(&mut self, _dt: f32) {}

    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        match param {
            SynthParam::FlowSpeedMin | SynthParam::FlowSpeedMax | SynthParam::RiverSplashRate => {}
            SynthParam::MinSpeed => self.min_speed = value.clamp(0.0, self.max_speed),
            SynthParam::MaxSpeed => self.max_speed = value.clamp(self.min_speed, 25.0),
            SynthParam::ChangeTimeMin => {
                let (_, max) = self.change_time_range();
                self.set_change_time_range(value.clamp(0.05, max), max);
            }
            SynthParam::ChangeTimeMax => {
                let (min, _) = self.change_time_range();
                self.set_change_time_range(min, value.max(min));
            }
            SynthParam::GustDurationMin => {
                let (_, max) = self.gust_duration_range();
                self.set_gust_duration_range(value.clamp(0.05, max), max);
            }
            SynthParam::GustDurationMax => {
                let (min, _) = self.gust_duration_range();
                self.set_gust_duration_range(min, value.max(min));
            }
            SynthParam::TurbulenceTimeMin => {
                let (_, max) = self.turbulence_time_range();
                self.set_turbulence_time_range(value.clamp(0.02, max), max);
            }
            SynthParam::TurbulenceTimeMax => {
                let (min, _) = self.turbulence_time_range();
                self.set_turbulence_time_range(min, value.max(min));
            }
            SynthParam::GustStrength => self.gust_strength = value.clamp(0.0, 1.0),
            SynthParam::RiseBias => self.rise_bias = value.clamp(-1.0, 1.0),
            SynthParam::GustBrightness => self.gust_brightness = value.clamp(0.0, 1.0),
            SynthParam::TurbulenceBrightness => self.turbulence_brightness = value.clamp(0.0, 1.0),
            SynthParam::TurbulenceDepth => self.turbulence_depth = value.clamp(0.0, 1.0),
            SynthParam::FoliageDensity => self.foliage_density = value.clamp(0.0, 1.0),
            SynthParam::LeafDryness => self.leaf_dryness = value.clamp(0.0, 1.0),
            SynthParam::BranchLevel => self.branch_level = value.clamp(0.0, 1.0),
            SynthParam::BodyGain | SynthParam::LowGain => self.body_gain = value.max(0.0),
            SynthParam::PresenceGain | SynthParam::MidGain => self.rustle_gain = value.max(0.0),
            SynthParam::AirGain => self.contact_gain = value.max(0.0),
            SynthParam::MasterGain => self.master_gain = value.clamp(0.0, 2.0),
            SynthParam::DebrisLevel | SynthParam::StructureLevel => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mean_square(source: &mut CanopyWindSource, seconds: f32) -> f32 {
        let sample_rate = 48_000.0;
        let count = (seconds * sample_rate) as usize;
        (0..count)
            .map(|_| source.next_sample(sample_rate).powi(2))
            .sum::<f32>()
            / count as f32
    }

    #[test]
    fn canopy_wind_is_a_field_and_produces_audio() {
        let mut wind = CanopyWindSource::new(Vec3::ZERO, 2.0, 8.0, 42);
        assert_eq!(wind.emitter_kind(), EmitterKind::Field);
        assert!(mean_square(&mut wind, 1.0) > 0.0);
    }

    #[test]
    fn zero_speed_range_is_true_calm() {
        let mut wind = CanopyWindSource::new(Vec3::ZERO, 0.0, 0.0, 43);
        for _ in 0..48_000 {
            assert_eq!(wind.next_sample(48_000.0), 0.0);
        }
    }

    #[test]
    fn graphics_tick_and_callback_chunking_do_not_change_audio() {
        fn render(chunk_size: usize) -> Vec<f32> {
            let sample_rate = 48_000.0;
            let mut wind = CanopyWindSource::new(Vec3::ZERO, 2.0, 8.0, 99);
            let mut output = Vec::with_capacity(48_000);
            while output.len() < 48_000 {
                let frames = chunk_size.min(48_000 - output.len());
                wind.tick(frames as f32 / sample_rate);
                output.extend((0..frames).map(|_| wind.next_sample(sample_rate)));
            }
            output
        }

        assert_eq!(render(64), render(1024));
    }

    #[test]
    fn stronger_wind_has_more_acoustic_energy() {
        let mut light = CanopyWindSource::new(Vec3::ZERO, 2.0, 2.0, 71);
        let mut brisk = CanopyWindSource::new(Vec3::ZERO, 8.0, 8.0, 71);
        light.branch_level = 0.0;
        brisk.branch_level = 0.0;
        let light_energy = mean_square(&mut light, 2.0);
        let brisk_energy = mean_square(&mut brisk, 2.0);
        assert!(
            brisk_energy > light_energy * 4.0,
            "8 m/s should be clearly stronger: {brisk_energy} vs {light_energy}"
        );
    }

    #[test]
    fn dry_leaves_are_spectrally_brighter() {
        fn high_frequency_fraction(dryness: f32) -> f32 {
            let sample_rate = 48_000.0;
            let mut wind = CanopyWindSource::new(Vec3::ZERO, 8.0, 8.0, 83);
            wind.leaf_dryness = dryness;
            wind.branch_level = 0.0;
            let mut highpass = OnePoleHP::new(2_500.0, sample_rate);
            let mut total = 0.0;
            let mut high = 0.0;
            for _ in 0..96_000 {
                let sample = wind.next_sample(sample_rate);
                total += sample * sample;
                let bright = highpass.process(sample);
                high += bright * bright;
            }
            high / total.max(f32::EPSILON)
        }

        let green = high_frequency_fraction(0.0);
        let dry = high_frequency_fraction(1.0);
        assert!(
            dry > green * 1.10,
            "dry foliage should add high-frequency energy: {dry} vs {green}"
        );
    }
}
