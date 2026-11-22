//! Generative open-field wind texture.
//!
//! The synthesis model follows three constraints:
//! - environmental wind behaves as a stochastic texture on multiple time scales;
//! - airflow speed changes both temporal variance and spectral shape;
//! - the signal is inharmonic, so time-varying filtered noise is the primitive.
//!
//! All clocks below are expressed in seconds and advanced per audio sample.
//! Nothing depends on the graphics frame rate, callback size, or `tick()` rate.

mod driver;

use atrium_core::commands::SynthParam;
use atrium_core::source::{EmitterKind, SoundSource};

use atrium_core::types::Vec3;

use super::noise::{white_noise_density_scale, OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};
use super::wind_behaviour::WindBehaviour;
pub(crate) use driver::WindDriver;

/// Open-field wind signal. Spatial diffuseness is supplied by the pipeline
/// (`pipeline::renderers::field::FieldRenderer`, and on headphones the
/// ears' diffuse field); this struct only creates the mono stochastic
/// texture it spreads around the listener.
pub struct FieldWindSource {
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
    mid_lp: OnePoleLP,
    mid_lp2: OnePoleLP,

    presence_noise: PinkNoiseFull,
    presence_hp: OnePoleHP,
    presence_hp2: OnePoleHP,
    presence_lp: OnePoleLP,
    presence_lp2: OnePoleLP,

    air_rng: Rng,
    air_hp: OnePoleHP,
    air_lp: OnePoleLP,

    /// Hard bounds for the instantaneous airflow driver, in m/s.
    pub min_speed: f32,
    pub max_speed: f32,
    /// Fraction of the speed range reserved for the short gust trajectory.
    pub gust_strength: f32,
    /// Positive values make rises tend shorter and falls tend longer.
    pub rise_bias: f32,
    pub low_gain: f32,
    pub body_gain: f32,
    pub mid_gain: f32,
    pub presence_gain: f32,
    pub air_gain: f32,
    /// Depth of sub-second eddy modulation.
    pub turbulence_depth: f32,
    /// Additional high-band opening at positive gust peaks.
    pub gust_brightness: f32,
    /// Signed high-band response to the fast eddy trajectory.
    pub turbulence_brightness: f32,
    pub master_gain: f32,

    /// Keeps the white air layer at its 48 kHz density per Hz.
    white_noise_scale: f32,
    position: Vec3,
    sample_rate_cached: f32,
}

impl FieldWindSource {
    pub fn new(position: Vec3, min_speed: f32, max_speed: f32, seed: u64) -> Self {
        let min_speed = min_speed.clamp(0.0, 25.0);
        let max_speed = max_speed.clamp(min_speed, 25.0);
        Self {
            driver: WindDriver::new(seed),
            low_noise: PinkNoiseFull::new(seed.wrapping_add(1)),
            low_hp: OnePoleHP::new(110.0, 48_000.0),
            low_hp2: OnePoleHP::new(110.0, 48_000.0),
            low_lp: OnePoleLP::new(250.0, 48_000.0),
            body_noise: PinkNoiseFull::new(seed.wrapping_add(3)),
            body_hp: OnePoleHP::new(175.0, 48_000.0),
            body_hp2: OnePoleHP::new(175.0, 48_000.0),
            body_lp: OnePoleLP::new(950.0, 48_000.0),
            body_lp2: OnePoleLP::new(950.0, 48_000.0),
            mid_noise: PinkNoiseFull::new(seed.wrapping_add(5)),
            mid_hp: OnePoleHP::new(450.0, 48_000.0),
            mid_lp: OnePoleLP::new(1_600.0, 48_000.0),
            mid_lp2: OnePoleLP::new(1_600.0, 48_000.0),
            presence_noise: PinkNoiseFull::new(seed.wrapping_add(7)),
            presence_hp: OnePoleHP::new(1_700.0, 48_000.0),
            presence_hp2: OnePoleHP::new(1_700.0, 48_000.0),
            presence_lp: OnePoleLP::new(6_500.0, 48_000.0),
            presence_lp2: OnePoleLP::new(6_500.0, 48_000.0),
            air_rng: Rng::new(seed.wrapping_add(11)),
            air_hp: OnePoleHP::new(3_800.0, 48_000.0),
            air_lp: OnePoleLP::new(10_000.0, 48_000.0),
            min_speed,
            max_speed,
            gust_strength: 0.30,
            rise_bias: 0.20,
            low_gain: 0.12,
            body_gain: 1.0,
            mid_gain: 0.40,
            presence_gain: 1.10,
            air_gain: 0.08,
            turbulence_depth: 0.45,
            gust_brightness: 0.0,
            turbulence_brightness: 0.0,
            master_gain: 1.0,
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

    pub fn set_change_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.driver.set_change_time_range(min_seconds, max_seconds);
    }

    /// Let the wind follow `behaviour`: steady, with a gust, swell or lull
    /// now and then, instead of gusting about half the time.
    pub fn set_behaviour(&mut self, behaviour: WindBehaviour) {
        self.driver.set_behaviour(behaviour);
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
        self.low_hp.set_cutoff(110.0, sample_rate);
        self.low_hp2.set_cutoff(110.0, sample_rate);
        self.low_lp.set_cutoff(250.0, sample_rate);
        self.body_hp.set_cutoff(175.0, sample_rate);
        self.body_hp2.set_cutoff(175.0, sample_rate);
        self.body_lp.set_cutoff(950.0, sample_rate);
        self.body_lp2.set_cutoff(950.0, sample_rate);
        self.mid_hp.set_cutoff(450.0, sample_rate);
        self.mid_lp.set_cutoff(1_600.0, sample_rate);
        self.mid_lp2.set_cutoff(1_600.0, sample_rate);
        self.presence_hp.set_cutoff(1_700.0, sample_rate);
        self.presence_hp2.set_cutoff(1_700.0, sample_rate);
        self.presence_lp.set_cutoff(6_500.0, sample_rate);
        self.presence_lp2.set_cutoff(6_500.0, sample_rate);
        self.air_hp.set_cutoff(3_800.0, sample_rate);
        self.air_lp
            .set_cutoff(10_000.0_f32.min(sample_rate * 0.45), sample_rate);
        self.white_noise_scale = white_noise_density_scale(sample_rate);
    }
}

impl SoundSource for FieldWindSource {
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
        let speed_norm = (state.speed / 25.0).clamp(0.0, 1.0);

        // Short eddies modulate by only a few dB and become stronger at gust
        // peaks. Their 120–800 ms time scale is still far below any graphics FPS.
        let eddy_db = state.eddy * self.turbulence_depth * state.activity * 4.0;
        let eddy_gain = 10.0_f32.powf(eddy_db / 20.0);

        // Both airflow speed and the instantaneous gust brighten the spectrum.
        // This makes high bands breathe with, but not merely mirror, loudness.
        let brightness = (0.08
            + 0.58 * speed_norm
            + 0.34 * state.activity
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
        let mid = self.mid_lp2.process(
            self.mid_lp
                .process(self.mid_hp.process(self.mid_noise.next_sample())),
        );
        let presence = self.presence_lp2.process(
            self.presence_lp.process(
                self.presence_hp2
                    .process(self.presence_hp.process(self.presence_noise.next_sample())),
            ),
        );
        let air = self.air_lp.process(
            self.air_hp
                .process(self.air_rng.next_bipolar() * self.white_noise_scale),
        );

        let texture = self.low_gain * low * (0.90 + 0.10 * state.activity)
            + self.body_gain * body * (0.68 + 0.48 * state.activity)
            + self.mid_gain * mid * (0.25 + 1.00 * brightness)
            + self.presence_gain * presence * (0.10 + 1.35 * brightness.powf(1.4))
            + self.air_gain * air * (0.03 + 1.55 * brightness * brightness);

        // Airflow is the central driver: it controls both spectral character
        // above and pressure here. A quadratic pressure curve gives useful
        // acoustic contrast while the configured m/s bounds stay literal.
        let speed_gain = if state.speed <= 0.0 {
            0.0
        } else {
            (state.speed / 8.0).powi(2).min(10.0)
        };
        texture * eddy_gain * speed_gain * self.master_gain
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }

    // Deliberately a no-op: all temporal evolution is sample-clocked above.
    fn tick(&mut self, _dt: f32) {}

    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through, and one NaN gain or speed would
        // turn every later sample into NaN: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        match param {
            // Settings of rain, waves, flying sources and tones; not this one's.
            SynthParam::Intensity
            | SynthParam::DropRate
            | SynthParam::DripRate
            | SynthParam::WavePeriod
            | SynthParam::CrashChance
            | SynthParam::HoverTimeMin
            | SynthParam::HoverTimeMax
            | SynthParam::Pitch
            | SynthParam::Brightness
            | SynthParam::FundamentalLevel
            | SynthParam::OvertoneLevel
            | SynthParam::ShimmerRateMin
            | SynthParam::ShimmerRateMax
            | SynthParam::ShimmerDepth => {}
            SynthParam::FlowSpeedMin | SynthParam::FlowSpeedMax | SynthParam::RiverSplashRate => {}
            SynthParam::MinSpeed => {
                self.min_speed = value.clamp(0.0, self.max_speed);
            }
            SynthParam::MaxSpeed => {
                self.max_speed = value.clamp(self.min_speed, 25.0);
            }
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
mod tests;
