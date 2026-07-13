//! Bright, steady soft-wind texture derived from the local reference recording.
//!
//! This is intentionally distinct from [`super::field_wind::FieldWindSource`].
//! The reference has a high, stable noise floor, substantial 2-16 kHz energy,
//! and relatively modest level movement whose louder moments become brighter.
//! `soft_wind` therefore keeps a persistent mid/upper texture and lets gusts
//! open the high bands more than they change total gain.
//!
//! Every control trajectory advances per audio sample. Graphics FPS and audio
//! callback size cannot affect the generated signal.

use atrium_core::commands::SynthParam;
use atrium_core::source::{EmitterKind, SoundSource};

use crate::world::types::Vec3;

use super::field_wind::WindDriver;
use super::noise::{OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};

/// A diffuse, bright soft-wind field with restrained level modulation.
pub struct SoftWindSource {
    driver: WindDriver,

    low_noise: PinkNoiseFull,
    low_hp: OnePoleHP,
    low_hp2: OnePoleHP,
    low_lp: OnePoleLP,

    body_noise: PinkNoiseFull,
    body_hp: OnePoleHP,
    body_hp2: OnePoleHP,
    body_lp: OnePoleLP,
    body_lp2: OnePoleLP,

    mid_noise: PinkNoiseFull,
    mid_hp: OnePoleHP,
    mid_hp2: OnePoleHP,
    mid_lp: OnePoleLP,

    presence_noise: PinkNoiseFull,
    presence_hp: OnePoleHP,
    presence_hp2: OnePoleHP,
    presence_lp: OnePoleLP,
    presence_lp2: OnePoleLP,

    air_rng: Rng,
    air_hp: OnePoleHP,
    air_lp: OnePoleLP,
    air_lp2: OnePoleLP,

    /// Hard instantaneous airflow bounds in metres/second.
    pub min_speed: f32,
    pub max_speed: f32,
    pub gust_strength: f32,
    pub rise_bias: f32,
    pub turbulence_depth: f32,
    pub gust_brightness: f32,
    pub turbulence_brightness: f32,

    pub low_gain: f32,
    pub body_gain: f32,
    pub mid_gain: f32,
    pub presence_gain: f32,
    pub air_gain: f32,
    pub master_gain: f32,

    position: Vec3,
    sample_rate_cached: f32,
}

impl SoftWindSource {
    pub fn new(position: Vec3, min_speed: f32, max_speed: f32, seed: u64) -> Self {
        let min_speed = min_speed.clamp(0.0, 25.0);
        let max_speed = max_speed.clamp(min_speed, 25.0);
        let mut driver = WindDriver::new(seed);
        driver.set_change_time_range(10.0, 24.0);
        driver.set_gust_duration_range(0.8, 4.0);
        driver.set_turbulence_time_range(0.05, 0.40);
        Self {
            driver,
            low_noise: PinkNoiseFull::new(seed.wrapping_add(1)),
            low_hp: OnePoleHP::new(100.0, 48_000.0),
            low_hp2: OnePoleHP::new(100.0, 48_000.0),
            low_lp: OnePoleLP::new(280.0, 48_000.0),
            body_noise: PinkNoiseFull::new(seed.wrapping_add(3)),
            body_hp: OnePoleHP::new(260.0, 48_000.0),
            body_hp2: OnePoleHP::new(260.0, 48_000.0),
            body_lp: OnePoleLP::new(1_350.0, 48_000.0),
            body_lp2: OnePoleLP::new(1_350.0, 48_000.0),
            mid_noise: PinkNoiseFull::new(seed.wrapping_add(5)),
            mid_hp: OnePoleHP::new(500.0, 48_000.0),
            mid_hp2: OnePoleHP::new(500.0, 48_000.0),
            mid_lp: OnePoleLP::new(2_800.0, 48_000.0),
            presence_noise: PinkNoiseFull::new(seed.wrapping_add(7)),
            presence_hp: OnePoleHP::new(4_000.0, 48_000.0),
            presence_hp2: OnePoleHP::new(4_000.0, 48_000.0),
            presence_lp: OnePoleLP::new(7_500.0, 48_000.0),
            presence_lp2: OnePoleLP::new(12_000.0, 48_000.0),
            air_rng: Rng::new(seed.wrapping_add(11)),
            air_hp: OnePoleHP::new(4_000.0, 48_000.0),
            air_lp: OnePoleLP::new(10_000.0, 48_000.0),
            air_lp2: OnePoleLP::new(10_000.0, 48_000.0),
            min_speed,
            max_speed,
            gust_strength: 0.25,
            rise_bias: 0.10,
            turbulence_depth: 0.35,
            gust_brightness: 0.18,
            turbulence_brightness: 0.10,
            low_gain: 0.02,
            body_gain: 0.55,
            mid_gain: 1.05,
            presence_gain: 2.25,
            air_gain: 0.11,
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
        self.low_hp.set_cutoff(100.0, sample_rate);
        self.low_hp2.set_cutoff(100.0, sample_rate);
        self.low_lp.set_cutoff(280.0, sample_rate);
        self.body_hp.set_cutoff(260.0, sample_rate);
        self.body_hp2.set_cutoff(260.0, sample_rate);
        self.body_lp.set_cutoff(1_350.0, sample_rate);
        self.body_lp2.set_cutoff(1_350.0, sample_rate);
        self.mid_hp.set_cutoff(500.0, sample_rate);
        self.mid_hp2.set_cutoff(500.0, sample_rate);
        self.mid_lp.set_cutoff(2_800.0, sample_rate);
        self.presence_hp.set_cutoff(4_000.0, sample_rate);
        self.presence_hp2.set_cutoff(4_000.0, sample_rate);
        self.presence_lp
            .set_cutoff(7_500.0_f32.min(sample_rate * 0.45), sample_rate);
        self.presence_lp2
            .set_cutoff(12_000.0_f32.min(sample_rate * 0.45), sample_rate);
        self.air_hp.set_cutoff(4_000.0, sample_rate);
        self.air_lp
            .set_cutoff(10_000.0_f32.min(sample_rate * 0.45), sample_rate);
        self.air_lp2
            .set_cutoff(10_000.0_f32.min(sample_rate * 0.45), sample_rate);
    }
}

impl SoundSource for SoftWindSource {
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
        if state.speed <= 0.0 {
            return 0.0;
        }

        let speed_norm = (state.speed / 5.0).clamp(0.0, 1.5);
        // Soft wind does not have one monolithic envelope. Mean airflow sets
        // the persistent bed, gust pressure creates frequent swells, and the
        // faster eddy trajectory opens/closes the bright foliage-like sheen.
        let brightness = (0.52
            + 0.18 * speed_norm
            + self.gust_brightness * state.gust
            + self.turbulence_brightness * state.eddy)
            .clamp(0.0, 1.0);

        let low = self.low_lp.process(
            self.low_hp2
                .process(self.low_hp.process(self.low_noise.next_sample())),
        );
        let body = self.body_lp2.process(
            self.body_lp.process(
                self.body_hp2
                    .process(self.body_hp.process(self.body_noise.next_sample())),
            ),
        );
        let mid = self.mid_lp.process(
            self.mid_hp2
                .process(self.mid_hp.process(self.mid_noise.next_sample())),
        );
        let presence = self.presence_lp2.process(
            self.presence_lp.process(
                self.presence_hp2
                    .process(self.presence_hp.process(self.presence_noise.next_sample())),
            ),
        );
        let air = self.air_lp2.process(
            self.air_lp
                .process(self.air_hp.process(self.air_rng.next_bipolar())),
        );

        let texture = self.low_gain * low
            + self.body_gain * body * (0.94 + 0.06 * state.weather)
            + self.mid_gain * mid * (0.62 + 0.42 * brightness)
            + self.presence_gain * presence * (0.24 + 1.00 * brightness.powf(1.20))
            + self.air_gain * air * (0.16 + 0.90 * brightness.powf(1.45));

        // The level floor stays high. These independent contributions are in
        // decibels so their depths remain perceptually meaningful.
        let level_db = (state.weather - 0.5) * 1.8
            + state.gust * 3.4
            + state.eddy * self.turbulence_depth * 1.65;
        let level_gain = 10.0_f32.powf(level_db / 20.0);
        texture * level_gain * self.master_gain
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }

    // All time-varying state advances on the audio sample clock above.
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
            SynthParam::LowGain => self.low_gain = value.max(0.0),
            SynthParam::BodyGain => self.body_gain = value.max(0.0),
            SynthParam::MidGain => self.mid_gain = value.max(0.0),
            SynthParam::PresenceGain => self.presence_gain = value.max(0.0),
            SynthParam::AirGain => self.air_gain = value.max(0.0),
            SynthParam::TurbulenceDepth => self.turbulence_depth = value.clamp(0.0, 1.0),
            SynthParam::MasterGain => self.master_gain = value.clamp(0.0, 2.0),
            SynthParam::FoliageDensity
            | SynthParam::LeafDryness
            | SynthParam::BranchLevel
            | SynthParam::DebrisLevel
            | SynthParam::StructureLevel => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_wind_is_a_field_and_produces_audio() {
        let mut wind = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 42);
        assert_eq!(wind.emitter_kind(), EmitterKind::Field);
        let energy: f32 = (0..48_000)
            .map(|_| wind.next_sample(48_000.0).powi(2))
            .sum();
        assert!(energy > 0.0);
    }

    #[test]
    fn tuned_defaults_preserve_the_reference_derived_driver_layers() {
        let wind = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 42);
        assert_eq!(wind.change_time_range(), (10.0, 24.0));
        assert_eq!(wind.gust_duration_range(), (0.8, 4.0));
        assert_eq!(wind.turbulence_time_range(), (0.05, 0.40));
        assert_eq!(wind.gust_strength, 0.25);
        assert_eq!(wind.turbulence_depth, 0.35);
        assert_eq!(wind.gust_brightness, 0.18);
        assert_eq!(wind.turbulence_brightness, 0.10);
    }

    #[test]
    fn soft_wind_is_substantially_brighter_than_field_wind() {
        fn high_frequency_fraction(source: &mut dyn SoundSource) -> f32 {
            let sample_rate = 48_000.0;
            let mut highpass = OnePoleHP::new(2_000.0, sample_rate);
            let (mut total, mut high) = (0.0_f32, 0.0_f32);
            for _ in 0..192_000 {
                let sample = source.next_sample(sample_rate);
                total += sample * sample;
                let bright = highpass.process(sample);
                high += bright * bright;
            }
            high / total.max(f32::EPSILON)
        }

        let mut soft = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 46);
        let mut field = super::super::field_wind::FieldWindSource::new(Vec3::ZERO, 1.0, 5.0, 46);
        let soft_bright = high_frequency_fraction(&mut soft);
        let field_bright = high_frequency_fraction(&mut field);
        assert!(
            soft_bright > field_bright * 4.0,
            "soft reference response should be much brighter: {soft_bright} vs {field_bright}"
        );
    }

    #[test]
    fn zero_speed_range_is_true_calm() {
        let mut wind = SoftWindSource::new(Vec3::ZERO, 0.0, 0.0, 43);
        for _ in 0..48_000 {
            assert_eq!(wind.next_sample(48_000.0), 0.0);
        }
    }

    #[test]
    fn graphics_tick_and_callback_chunking_do_not_change_audio() {
        fn render(chunk_size: usize) -> Vec<f32> {
            let sample_rate = 48_000.0;
            let mut wind = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 99);
            let mut output = Vec::with_capacity(96_000);
            while output.len() < 96_000 {
                let frames = chunk_size.min(96_000 - output.len());
                wind.tick(frames as f32 / sample_rate);
                output.extend((0..frames).map(|_| wind.next_sample(sample_rate)));
            }
            output
        }

        assert_eq!(render(64), render(1024));
    }
}
