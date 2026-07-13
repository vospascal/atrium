//! Generative gale and storm-force wind field.
//!
//! This instrument contains wind only: rain, thunder, waves, and vegetation
//! remain separate sources that can be composed in a scene. Its own layers are
//! pressure/rumble, broadband roar, turbulent shear, high-frequency tearing,
//! and optional sparse debris or structural-strain events.
//!
//! The bounded weather driver is shared with the other environmental winds,
//! but the storm DSP is independent. Every clock advances per audio sample;
//! graphics FPS and callback size therefore cannot alter the weather.

use std::f32::consts::TAU;

use atrium_core::commands::SynthParam;
use atrium_core::source::{EmitterKind, SoundSource};

use crate::world::types::Vec3;

use super::field_wind::WindDriver;
use super::noise::{OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};

const DEBRIS_RING_SIZE: usize = 8192;
const DEBRIS_RING_MASK: usize = DEBRIS_RING_SIZE - 1;

/// Diffuse strong-wind field with bounded instantaneous speed in m/s.
pub struct StormWindSource {
    driver: WindDriver,

    pressure_noise: PinkNoiseFull,
    pressure_hp: OnePoleHP,
    pressure_lp: OnePoleLP,
    pressure_lp2: OnePoleLP,

    roar_noise: PinkNoiseFull,
    roar_hp: OnePoleHP,
    roar_hp2: OnePoleHP,
    roar_lp: OnePoleLP,
    roar_lp2: OnePoleLP,

    shear_noise: PinkNoiseFull,
    shear_hp: OnePoleHP,
    shear_lp: OnePoleLP,

    tear_rng: Rng,
    tear_hp: OnePoleHP,
    tear_hp2: OnePoleHP,
    tear_lp: OnePoleLP,

    debris_rng: Rng,
    debris_ring: Box<[f32; DEBRIS_RING_SIZE]>,
    debris_ring_index: usize,

    structure_rng: Rng,
    structure_phase: f32,
    structure_frequency: f32,
    structure_envelope: f32,
    structure_decay: f32,
    structure_noise_lp: OnePoleLP,

    /// Hard bounds for instantaneous wind speed, in m/s.
    pub min_speed: f32,
    pub max_speed: f32,
    /// Fraction of the range reserved for short positive gusts.
    pub gust_strength: f32,
    /// Positive values tend toward quicker rises and slower releases.
    pub rise_bias: f32,
    /// Depth of smooth 120-800 ms turbulent buffeting.
    pub turbulence_depth: f32,
    pub gust_brightness: f32,
    pub turbulence_brightness: f32,

    /// Frequency and level of sparse airborne-object events, 0-1.
    pub debris_level: f32,
    /// Frequency and level of low structural strain events, 0-1.
    pub structure_level: f32,

    pub pressure_gain: f32,
    pub roar_gain: f32,
    pub shear_gain: f32,
    pub tear_gain: f32,
    pub master_gain: f32,

    position: Vec3,
    sample_rate_cached: f32,
}

impl StormWindSource {
    pub fn new(position: Vec3, min_speed: f32, max_speed: f32, seed: u64) -> Self {
        let min_speed = min_speed.clamp(0.0, 25.0);
        let max_speed = max_speed.clamp(min_speed, 25.0);
        let mut driver = WindDriver::new(seed);
        driver.set_change_time_range(12.0, 40.0);
        driver.set_gust_duration_range(1.5, 14.0);
        Self {
            driver,
            pressure_noise: PinkNoiseFull::new(seed.wrapping_add(1)),
            pressure_hp: OnePoleHP::new(18.0, 48_000.0),
            pressure_lp: OnePoleLP::new(180.0, 48_000.0),
            pressure_lp2: OnePoleLP::new(180.0, 48_000.0),
            roar_noise: PinkNoiseFull::new(seed.wrapping_add(3)),
            roar_hp: OnePoleHP::new(55.0, 48_000.0),
            roar_hp2: OnePoleHP::new(55.0, 48_000.0),
            roar_lp: OnePoleLP::new(1_250.0, 48_000.0),
            roar_lp2: OnePoleLP::new(1_250.0, 48_000.0),
            shear_noise: PinkNoiseFull::new(seed.wrapping_add(5)),
            shear_hp: OnePoleHP::new(280.0, 48_000.0),
            shear_lp: OnePoleLP::new(4_800.0, 48_000.0),
            tear_rng: Rng::new(seed.wrapping_add(7)),
            tear_hp: OnePoleHP::new(1_800.0, 48_000.0),
            tear_hp2: OnePoleHP::new(1_800.0, 48_000.0),
            tear_lp: OnePoleLP::new(9_500.0, 48_000.0),
            debris_rng: Rng::new(seed.wrapping_add(11)),
            debris_ring: Box::new([0.0; DEBRIS_RING_SIZE]),
            debris_ring_index: 0,
            structure_rng: Rng::new(seed.wrapping_add(13)),
            structure_phase: 0.0,
            structure_frequency: 70.0,
            structure_envelope: 0.0,
            structure_decay: 0.0,
            structure_noise_lp: OnePoleLP::new(240.0, 48_000.0),
            min_speed,
            max_speed,
            gust_strength: 0.55,
            rise_bias: 0.45,
            turbulence_depth: 0.65,
            gust_brightness: 0.0,
            turbulence_brightness: 0.0,
            debris_level: 0.06,
            structure_level: 0.05,
            pressure_gain: 1.03,
            roar_gain: 1.50,
            shear_gain: 0.70,
            tear_gain: 0.35,
            master_gain: 0.85,
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
        self.pressure_hp.set_cutoff(18.0, sample_rate);
        self.pressure_lp.set_cutoff(180.0, sample_rate);
        self.pressure_lp2.set_cutoff(180.0, sample_rate);
        self.roar_hp.set_cutoff(55.0, sample_rate);
        self.roar_hp2.set_cutoff(55.0, sample_rate);
        self.roar_lp.set_cutoff(1_250.0, sample_rate);
        self.roar_lp2.set_cutoff(1_250.0, sample_rate);
        self.shear_hp.set_cutoff(280.0, sample_rate);
        self.shear_lp
            .set_cutoff(4_800.0_f32.min(sample_rate * 0.45), sample_rate);
        self.tear_hp.set_cutoff(1_800.0, sample_rate);
        self.tear_hp2.set_cutoff(1_800.0, sample_rate);
        self.tear_lp
            .set_cutoff(9_500.0_f32.min(sample_rate * 0.45), sample_rate);
        self.structure_noise_lp.set_cutoff(240.0, sample_rate);
    }

    /// Write one short snap or tumbling flutter into a fixed superposition
    /// ring. These events remain sparse so they do not turn into rain.
    fn write_debris_event(&mut self, sample_rate: f32, storm_norm: f32) {
        let flutter = self.debris_rng.next_f32() < 0.55;
        let seconds = if flutter {
            0.020 + 0.080 * self.debris_rng.next_f32()
        } else {
            0.003 + 0.018 * self.debris_rng.next_f32()
        };
        let length = ((seconds * sample_rate) as usize).clamp(8, DEBRIS_RING_SIZE - 1);
        let attack_samples = (if flutter { 0.003 } else { 0.0006 } * sample_rate).max(1.0);

        let highpass_hz = 180.0 + 1_300.0 * self.debris_rng.next_f32();
        let lowpass_hz = (1_800.0 + 5_800.0 * self.debris_rng.next_f32()).min(sample_rate * 0.45);
        let hp_a = (-TAU * highpass_hz / sample_rate).exp();
        let lp_mix = 1.0 - (-TAU * lowpass_hz / sample_rate).exp();
        let gain = 0.026 * (0.65 + 0.75 * storm_norm) * (0.60 + 0.40 * self.debris_rng.next_f32());
        let flutter_hz = 18.0 + 45.0 * self.debris_rng.next_f32();

        let mut lowpass = 0.0_f32;
        let mut hp_state = 0.0_f32;
        let mut hp_previous = 0.0_f32;
        for i in 0..length {
            let time = i as f32 / sample_rate;
            let attack = (i as f32 / attack_samples).min(1.0);
            let decay = (1.0 - i as f32 / length as f32).powi(2);
            let flutter_mod = if flutter {
                0.35 + 0.65 * (TAU * flutter_hz * time).sin().abs()
            } else {
                1.0
            };
            lowpass += lp_mix * (self.debris_rng.next_bipolar() - lowpass);
            hp_state = hp_a * (hp_state + lowpass - hp_previous);
            hp_previous = lowpass;
            let index = (self.debris_ring_index + 1 + i) & DEBRIS_RING_MASK;
            self.debris_ring[index] += hp_state * attack * decay * flutter_mod * gain;
        }
    }

    fn next_structure(&mut self, speed: f32, storm_norm: f32, sample_rate: f32) -> f32 {
        let level = self.structure_level.clamp(0.0, 1.0);
        if self.structure_envelope <= 0.0 && speed > 13.0 && level > 0.0 {
            let events_per_second = level * (0.01 + 0.60 * storm_norm * storm_norm);
            if self.structure_rng.next_f32() < events_per_second / sample_rate {
                self.structure_phase = self.structure_rng.next_f32() * TAU;
                self.structure_frequency = 35.0 + 100.0 * self.structure_rng.next_f32();
                let duration = 0.40 + 1.60 * self.structure_rng.next_f32();
                self.structure_decay = (-6.9 / (duration * sample_rate)).exp();
                self.structure_envelope = 1.0;
            }
        }

        if self.structure_envelope <= 0.0 {
            return 0.0;
        }

        let noise = self
            .structure_noise_lp
            .process(self.structure_rng.next_bipolar());
        let resonant = 0.70 * self.structure_phase.sin()
            + 0.20 * (2.0 * self.structure_phase + 0.4).sin()
            + 0.10 * noise;
        let output = resonant * self.structure_envelope * level * 0.18;
        let bent_frequency = self.structure_frequency * (0.68 + 0.32 * self.structure_envelope);
        self.structure_phase = (self.structure_phase + TAU * bent_frequency / sample_rate) % TAU;
        self.structure_envelope *= self.structure_decay;
        if self.structure_envelope < 0.001 {
            self.structure_envelope = 0.0;
        }
        output
    }
}

impl SoundSource for StormWindSource {
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
        let storm_norm = ((state.speed - 8.0) / 17.0).clamp(0.0, 1.0);

        let debris_level = self.debris_level.clamp(0.0, 1.0);
        let debris_rate = if state.speed > 10.0 {
            debris_level * (0.03 + 2.0 * storm_norm * storm_norm)
        } else {
            0.0
        };
        if self.debris_rng.next_f32() < debris_rate / sample_rate {
            self.write_debris_event(sample_rate, storm_norm);
        }
        let debris = self.debris_ring[self.debris_ring_index] * debris_level;
        self.debris_ring[self.debris_ring_index] = 0.0;
        self.debris_ring_index = (self.debris_ring_index + 1) & DEBRIS_RING_MASK;

        let pressure = self.pressure_lp2.process(
            self.pressure_lp
                .process(self.pressure_hp.process(self.pressure_noise.next_sample())),
        );
        let roar = self.roar_lp2.process(
            self.roar_lp.process(
                self.roar_hp2
                    .process(self.roar_hp.process(self.roar_noise.next_sample())),
            ),
        );
        let shear = self
            .shear_lp
            .process(self.shear_hp.process(self.shear_noise.next_sample()));
        let tear = self.tear_lp.process(
            self.tear_hp2
                .process(self.tear_hp.process(self.tear_rng.next_bipolar())),
        );
        let structure = self.next_structure(state.speed, storm_norm, sample_rate);

        let positive_eddy = state.eddy.max(0.0).powi(2);
        let pressure_motion = 0.58 + 0.65 * state.activity + 0.55 * positive_eddy;
        let spectral_response =
            self.gust_brightness * state.gust + self.turbulence_brightness * state.eddy;
        let shear_motion = (0.30 + 1.35 * storm_norm.powf(1.3) + spectral_response).max(0.0);
        let tearing_motion =
            (0.05 + 1.85 * storm_norm * storm_norm + 1.5 * spectral_response).max(0.0);
        let texture = self.pressure_gain * pressure * pressure_motion
            + self.roar_gain * roar * (0.62 + 0.68 * state.activity)
            + self.shear_gain * shear * shear_motion
            + self.tear_gain * tear * tearing_motion
            + debris
            + structure;

        let speed_gain = if state.speed <= 0.0 {
            0.0
        } else {
            (state.speed / 12.0).powf(1.8).min(3.8)
        };
        let buffet_db = state.eddy * self.turbulence_depth * state.activity * 7.0
            + positive_eddy * self.turbulence_depth * 2.0;
        let buffet_gain = 10.0_f32.powf(buffet_db / 20.0);
        texture * speed_gain * buffet_gain * self.master_gain
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
            SynthParam::DebrisLevel => self.debris_level = value.clamp(0.0, 1.0),
            SynthParam::StructureLevel => self.structure_level = value.clamp(0.0, 1.0),
            SynthParam::LowGain => self.pressure_gain = value.max(0.0),
            SynthParam::BodyGain => self.roar_gain = value.max(0.0),
            SynthParam::MidGain => self.shear_gain = value.max(0.0),
            SynthParam::PresenceGain | SynthParam::AirGain => self.tear_gain = value.max(0.0),
            SynthParam::MasterGain => self.master_gain = value.clamp(0.0, 2.0),
            SynthParam::FoliageDensity | SynthParam::LeafDryness | SynthParam::BranchLevel => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mean_square(source: &mut StormWindSource, seconds: f32) -> f32 {
        let sample_rate = 48_000.0;
        let count = (seconds * sample_rate) as usize;
        (0..count)
            .map(|_| source.next_sample(sample_rate).powi(2))
            .sum::<f32>()
            / count as f32
    }

    #[test]
    fn storm_wind_is_a_field_and_produces_audio() {
        let mut wind = StormWindSource::new(Vec3::ZERO, 8.0, 25.0, 42);
        assert_eq!(wind.emitter_kind(), EmitterKind::Field);
        assert!(mean_square(&mut wind, 1.0) > 0.0);
    }

    #[test]
    fn tuned_defaults_remain_stable() {
        let wind = StormWindSource::new(Vec3::ZERO, 8.0, 18.0, 42);
        assert_eq!((wind.min_speed, wind.max_speed), (8.0, 18.0));
        assert_eq!(wind.change_time_range(), (12.0, 40.0));
        assert_eq!(wind.gust_duration_range(), (1.5, 14.0));
        assert_eq!(wind.turbulence_time_range(), (0.12, 0.80));
        assert_eq!(wind.gust_strength, 0.55);
        assert_eq!(wind.rise_bias, 0.45);
        assert_eq!(wind.turbulence_depth, 0.65);
        assert_eq!(wind.gust_brightness, 0.0);
        assert_eq!(wind.turbulence_brightness, 0.0);
        assert_eq!(wind.debris_level, 0.06);
        assert_eq!(wind.structure_level, 0.05);
        assert_eq!(wind.pressure_gain, 1.03);
        assert_eq!(wind.roar_gain, 1.50);
        assert_eq!(wind.shear_gain, 0.70);
        assert_eq!(wind.tear_gain, 0.35);
        assert_eq!(wind.master_gain, 0.85);
    }

    #[test]
    fn zero_speed_range_is_true_calm() {
        let mut wind = StormWindSource::new(Vec3::ZERO, 0.0, 0.0, 43);
        for _ in 0..48_000 {
            assert_eq!(wind.next_sample(48_000.0), 0.0);
        }
    }

    #[test]
    fn graphics_tick_and_callback_chunking_do_not_change_audio() {
        fn render(chunk_size: usize) -> Vec<f32> {
            let sample_rate = 48_000.0;
            let mut wind = StormWindSource::new(Vec3::ZERO, 8.0, 25.0, 99);
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
    fn storm_force_wind_has_much_more_energy_than_strong_breeze() {
        let mut strong = StormWindSource::new(Vec3::ZERO, 10.0, 10.0, 71);
        let mut storm = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 71);
        strong.debris_level = 0.0;
        strong.structure_level = 0.0;
        storm.debris_level = 0.0;
        storm.structure_level = 0.0;
        let strong_energy = mean_square(&mut strong, 2.0);
        let storm_energy = mean_square(&mut storm, 2.0);
        assert!(
            storm_energy > strong_energy * 10.0,
            "25 m/s should be much stronger: {storm_energy} vs {strong_energy}"
        );
    }

    #[test]
    fn stronger_wind_adds_turbulent_high_frequency_tearing() {
        fn high_frequency_fraction(speed: f32) -> f32 {
            let sample_rate = 48_000.0;
            let mut wind = StormWindSource::new(Vec3::ZERO, speed, speed, 83);
            wind.debris_level = 0.0;
            wind.structure_level = 0.0;
            let mut highpass = OnePoleHP::new(1_800.0, sample_rate);
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

        let strong = high_frequency_fraction(10.0);
        let storm = high_frequency_fraction(25.0);
        assert!(
            storm > strong * 1.5,
            "storm force should tear brighter: {storm} vs {strong}"
        );
    }
}
