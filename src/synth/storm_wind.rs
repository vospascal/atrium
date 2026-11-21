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

use atrium_core::types::Vec3;

use super::field_wind::WindDriver;
use super::noise::{white_noise_density_scale, OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};

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

    /// Keeps the white tear and structure noise at their 48 kHz density per Hz.
    white_noise_scale: f32,
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
            white_noise_scale: 1.0,
            position,
            sample_rate_cached: 0.0,
        }
    }

    pub fn set_speed_range(&mut self, min_speed: f32, max_speed: f32) {
        let min_speed = min_speed.clamp(0.0, 25.0);
        self.min_speed = min_speed.min(max_speed.clamp(0.0, 25.0));
        self.max_speed = min_speed.max(max_speed.clamp(0.0, 25.0));
    }

    /// Let the wind follow `behaviour`: steady, with a gust, swell or lull
    /// now and then, instead of gusting about half the time.
    pub fn set_behaviour(&mut self, behaviour: crate::synth::wind_behaviour::WindBehaviour) {
        self.driver.set_behaviour(behaviour);
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
        self.white_noise_scale = white_noise_density_scale(sample_rate);
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
        let white_noise_scale = white_noise_density_scale(sample_rate);
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
            lowpass += lp_mix * (self.debris_rng.next_bipolar() * white_noise_scale - lowpass);
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
            .process(self.structure_rng.next_bipolar() * self.white_noise_scale);
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
            self.tear_hp2.process(
                self.tear_hp
                    .process(self.tear_rng.next_bipolar() * self.white_noise_scale),
            ),
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
        // `f32::clamp` passes NaN through, and one NaN gain or speed would
        // turn every later sample into NaN: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        match param {
            // Settings of rain, waves and flying sources; not this one's.
            SynthParam::Intensity
            | SynthParam::DropRate
            | SynthParam::DripRate
            | SynthParam::WavePeriod
            | SynthParam::CrashChance
            | SynthParam::HoverTimeMin
            | SynthParam::HoverTimeMax => {}
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
            SynthParam::MoveRest => self.driver.set_move_rest(value),
            SynthParam::MoveStrength => self.driver.set_move_strength(value),
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

    use crate::synth::test_support::{
        averaged_power_spectrum, band_density_db, mean, peak, render, root_mean_square,
    };

    // ── Sparse event layers ──────────────────────────────────────────────

    /// Debris events started over `seconds`: each begins at
    /// `debris_ring_index + 1` with an attack ramp, so slot `+ 2` changing
    /// during a call marks one.
    fn count_debris(wind: &mut StormWindSource, seconds: f32, sample_rate: f32) -> usize {
        let mut count = 0;
        for _ in 0..(seconds * sample_rate) as usize {
            let watched = (wind.debris_ring_index + 2) & DEBRIS_RING_MASK;
            let before = wind.debris_ring[watched];
            wind.next_sample(sample_rate);
            if wind.debris_ring[watched] != before {
                count += 1;
            }
        }
        count
    }

    /// rate = debris_level (0.03 + 2 storm_norm^2) per second above 10 m/s,
    /// zero at or below it; storm_norm = (speed - 8) / 17.
    #[test]
    fn debris_rate_follows_the_storm_law_and_is_gated_by_speed() {
        let mut gale = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 3);
        gale.debris_level = 1.0;
        gale.structure_level = 0.0;
        let seconds = 100.0;
        let expected = 2.03 * seconds;
        let count = count_debris(&mut gale, seconds, 8_000.0) as f32;
        assert!(
            (count - expected).abs() < 4.0 * expected.sqrt(),
            "{count} debris events in {seconds} s, expected {expected}"
        );

        let mut strong = StormWindSource::new(Vec3::ZERO, 15.0, 15.0, 3);
        strong.debris_level = 1.0;
        strong.structure_level = 0.0;
        // storm_norm = 7 / 17: rate = 0.03 + 2 (0.412)^2 = 0.37 per second.
        let expected = (0.03 + 2.0 * (7.0_f32 / 17.0).powi(2)) * seconds;
        let count = count_debris(&mut strong, seconds, 8_000.0) as f32;
        assert!(
            (count - expected).abs() < 4.0 * expected.sqrt(),
            "{count} debris events at 15 m/s, expected {expected}"
        );

        let mut breeze = StormWindSource::new(Vec3::ZERO, 9.0, 9.0, 3);
        breeze.debris_level = 1.0;
        breeze.structure_level = 0.0;
        assert_eq!(count_debris(&mut breeze, 60.0, 8_000.0), 0);
    }

    /// Drive `next_structure` alone at a low rate so thousands of seconds of
    /// events are cheap. Returns (events, lengths in samples, decays).
    fn run_structure(
        wind: &mut StormWindSource,
        speed: f32,
        storm_norm: f32,
        seconds: f32,
        sample_rate: f32,
    ) -> (usize, Vec<usize>, Vec<f32>) {
        let (mut events, mut lengths, mut decays) = (0, Vec::new(), Vec::new());
        let mut running = 0usize;
        for _ in 0..(seconds * sample_rate) as usize {
            let was_active = wind.structure_envelope > 0.0;
            wind.next_structure(speed, storm_norm, sample_rate);
            let is_active = wind.structure_envelope > 0.0;
            if !was_active && is_active {
                events += 1;
                running = 0;
                decays.push(wind.structure_decay);
                assert!((35.0..=135.0).contains(&wind.structure_frequency));
            }
            if is_active || was_active {
                running += 1;
            }
            if was_active && !is_active {
                lengths.push(running);
            }
        }
        (events, lengths, decays)
    }

    /// The strain event's envelope is exp(-6.9 n / (duration x rate)): it
    /// reaches -60 dB (the 0.001 cut-off) after exactly `duration` seconds.
    #[test]
    fn structure_events_decay_by_60_db_over_their_duration() {
        let sample_rate = 1_000.0;
        let mut wind = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 9);
        wind.structure_level = 1.0;
        let (events, lengths, decays) = run_structure(&mut wind, 25.0, 1.0, 400.0, sample_rate);
        assert!(events > 50 && lengths.len() >= events - 1);
        for (length, decay) in lengths.iter().zip(&decays) {
            let duration = -6.9 / (decay.ln() * sample_rate);
            assert!((0.40..=2.0).contains(&duration), "duration {duration}");
            // -6.9 nepers is 0.001; the loop stops once it falls below, so the
            // length is duration x rate to within the rounding of one sample.
            let expected = duration * sample_rate * (6.9078 / 6.9);
            assert!(
                (*length as f32 - expected).abs() <= 2.0,
                "{length} vs {expected}"
            );
        }
    }

    /// Events start only above 13 m/s; their rate is level (0.01 + 0.6
    /// storm_norm^2) and a new one cannot start until the last has died, so
    /// the observed renewal rate is 1 / (1 / rate + mean duration 1.2 s).
    #[test]
    fn structure_event_rate_follows_the_renewal_law_and_speed_gate() {
        let sample_rate = 1_000.0;
        let mut wind = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 10);
        wind.structure_level = 1.0;
        let seconds = 2_000.0;
        let (events, _, _) = run_structure(&mut wind, 25.0, 1.0, seconds, sample_rate);
        let rate = 0.01 + 0.6;
        let expected = seconds / (1.0 / rate + 1.2 * 1.0011);
        assert!(
            (events as f32 - expected).abs() < 5.0 * expected.sqrt(),
            "{events} strain events in {seconds} s, expected {expected:.0}"
        );
        let mut calm = StormWindSource::new(Vec3::ZERO, 12.9, 12.9, 10);
        calm.structure_level = 1.0;
        assert_eq!(run_structure(&mut calm, 12.9, 0.3, 500.0, sample_rate).0, 0);
        let mut off = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 10);
        off.structure_level = 0.0;
        assert_eq!(run_structure(&mut off, 25.0, 1.0, 500.0, sample_rate).0, 0);
    }

    // ── Rendered audio ───────────────────────────────────────────────────

    /// Pressure law: (speed / 12)^1.8 capped at 3.8 (never reached below 25 m/s).
    #[test]
    fn level_follows_the_power_law_in_speed() {
        let level = |speed: f32| {
            let mut wind = StormWindSource::new(Vec3::ZERO, speed, speed, 71);
            wind.debris_level = 0.0;
            wind.structure_level = 0.0;
            root_mean_square(&render(15.0, 48_000.0, |rate| wind.next_sample(rate)))
        };
        let ratio = level(20.0) / level(10.0);
        // Gain ratio 2^1.8 = 3.5, times a stronger shear/tear mix as
        // storm_norm rises from 0.12 to 0.71: texture RMS grows about 2.1x,
        // so the ratio is about 7.5 (measured 7.4-7.7 over five seeds).
        // Rendered at the 48 kHz reference: at 16 kHz the tear band is cut
        // off at the 8 kHz Nyquist limit, and this read 12.4 there before
        // the white tear layer kept its density per Hz (the unscaled noise put
        // the whole 48 kHz variance below 8 kHz, +4.8 dB of tear).
        assert!(
            (6.0..10.0).contains(&ratio),
            "RMS ratio 20 vs 10 m/s is {ratio}"
        );
        assert!((25.0_f32 / 12.0).powf(1.8) < 3.8);
    }

    #[test]
    fn output_is_finite_zero_mean_and_inside_unity_for_a_strong_breeze() {
        let mut wind = StormWindSource::new(Vec3::ZERO, 8.0, 18.0, 7);
        let samples = render(20.0, 48_000.0, |rate| wind.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(peak(&samples) < 1.0, "peak {}", peak(&samples));
        assert!(mean(&samples).abs() < 0.01, "DC {}", mean(&samples));
    }

    /// The pressure layer (18 Hz highpass, two 180 Hz lowpass poles) is the
    /// rumble: removing it drops the 30-120 Hz density by several dB.
    #[test]
    fn pressure_layer_supplies_the_low_frequency_rumble() {
        let rumble = |pressure_gain: f32| {
            let mut wind = StormWindSource::new(Vec3::ZERO, 18.0, 18.0, 5);
            wind.debris_level = 0.0;
            wind.structure_level = 0.0;
            wind.pressure_gain = pressure_gain;
            let samples = render(12.0, 48_000.0, |rate| wind.next_sample(rate));
            band_density_db(&averaged_power_spectrum(&samples), 48_000.0, 30.0, 120.0)
        };
        assert!(rumble(1.03) > rumble(0.0) + 3.0);
    }

    #[test]
    fn low_and_mid_band_density_is_sample_rate_independent() {
        let density = |sample_rate: f32| {
            let mut wind = StormWindSource::new(Vec3::ZERO, 14.0, 14.0, 73);
            wind.debris_level = 0.0;
            wind.structure_level = 0.0;
            let samples = render(10.0, sample_rate, |rate| wind.next_sample(rate));
            let spectrum = averaged_power_spectrum(&samples);
            (
                band_density_db(&spectrum, sample_rate, 100.0, 200.0),
                band_density_db(&spectrum, sample_rate, 700.0, 1_000.0),
            )
        };
        let reference = density(48_000.0);
        for sample_rate in [44_100.0, 96_000.0] {
            let (low, mid) = density(sample_rate);
            assert!(
                (low - reference.0).abs() < 1.0,
                "{sample_rate}: low {low} vs {}",
                reference.0
            );
            assert!(
                (mid - reference.1).abs() < 1.0,
                "{sample_rate}: mid {mid} vs {}",
                reference.1
            );
        }
    }

    #[test]
    fn same_seed_is_bit_identical_and_other_seeds_differ() {
        let run = |seed: u64| {
            let mut wind = StormWindSource::new(Vec3::ZERO, 12.0, 25.0, seed);
            render(0.5, 48_000.0, |rate| wind.next_sample(rate))
        };
        assert_eq!(run(5), run(5));
        assert_ne!(run(5), run(6));
    }

    // ── Parameters ───────────────────────────────────────────────────────

    #[test]
    fn speed_range_and_levels_are_clamped_and_routed() {
        let mut wind = StormWindSource::new(Vec3::ZERO, -3.0, 99.0, 1);
        assert_eq!((wind.min_speed, wind.max_speed), (0.0, 25.0));
        wind.set_speed_range(20.0, 6.0);
        assert_eq!((wind.min_speed, wind.max_speed), (6.0, 20.0));
        wind.set_synth_param(SynthParam::DebrisLevel, 4.0);
        assert_eq!(wind.debris_level, 1.0);
        wind.set_synth_param(SynthParam::StructureLevel, -4.0);
        assert_eq!(wind.structure_level, 0.0);
        wind.set_synth_param(SynthParam::LowGain, 0.1);
        wind.set_synth_param(SynthParam::BodyGain, 0.2);
        wind.set_synth_param(SynthParam::MidGain, 0.3);
        wind.set_synth_param(SynthParam::AirGain, 0.4);
        assert_eq!(
            (
                wind.pressure_gain,
                wind.roar_gain,
                wind.shear_gain,
                wind.tear_gain
            ),
            (0.1, 0.2, 0.3, 0.4)
        );
        wind.set_synth_param(SynthParam::MasterGain, 3.0);
        assert_eq!(wind.master_gain, 2.0);
    }
}
