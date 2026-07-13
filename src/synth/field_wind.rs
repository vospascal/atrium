//! Generative open-field wind texture.
//!
//! The synthesis model follows three constraints:
//! - environmental wind behaves as a stochastic texture on multiple time scales;
//! - airflow speed changes both temporal variance and spectral shape;
//! - the signal is inharmonic, so time-varying filtered noise is the primitive.
//!
//! All clocks below are expressed in seconds and advanced per audio sample.
//! Nothing depends on the graphics frame rate, callback size, or `tick()` rate.

use std::f32::consts::PI;

use atrium_core::commands::SynthParam;
use atrium_core::source::{EmitterKind, SoundSource};

use crate::world::types::Vec3;

use super::noise::{OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};

/// Smooth random trajectory with segment durations specified in seconds.
/// Cosine interpolation gives zero slope at target changes and avoids audible
/// control discontinuities.
struct SmoothRandom {
    rng: Rng,
    from: f32,
    to: f32,
    position: usize,
    length: usize,
    min_seconds: f32,
    max_seconds: f32,
}

impl SmoothRandom {
    fn new(seed: u64, min_seconds: f32, max_seconds: f32) -> Self {
        let mut rng = Rng::new(seed);
        // Begin at an arbitrary point in the weather process. Starting every
        // source at zero would create a seed-independent artificial fade and
        // make short control-side loudness previews unrepresentative.
        let initial = rng.next_bipolar();
        Self {
            rng,
            from: initial,
            to: initial,
            position: 0,
            length: 0,
            min_seconds,
            max_seconds,
        }
    }

    fn set_duration_range(&mut self, min_seconds: f32, max_seconds: f32) {
        let min_seconds = min_seconds.max(0.05);
        self.min_seconds = min_seconds.min(max_seconds.max(0.05));
        self.max_seconds = min_seconds.max(max_seconds);
    }

    fn duration_range(&self) -> (f32, f32) {
        (self.min_seconds, self.max_seconds)
    }

    #[inline(always)]
    fn next(&mut self, sample_rate: f32) -> f32 {
        self.next_shaped(sample_rate, 0.0)
    }

    /// Advance the trajectory, optionally biasing rising segments toward the
    /// short end and falling segments toward the long end of the configured
    /// duration range. Negative bias reverses that relationship.
    #[inline(always)]
    fn next_shaped(&mut self, sample_rate: f32, rise_bias: f32) -> f32 {
        if self.length == 0 {
            self.from = self.to;
            self.to = self.rng.next_bipolar();
            let directional_bias = if self.to >= self.from {
                rise_bias
            } else {
                -rise_bias
            }
            .clamp(-1.0, 1.0);
            let raw = self.rng.next_f32();
            let shaped = if directional_bias >= 0.0 {
                raw.powf(1.0 + 2.0 * directional_bias)
            } else {
                1.0 - (1.0 - raw).powf(1.0 - 2.0 * directional_bias)
            };
            let seconds = self.min_seconds + shaped * (self.max_seconds - self.min_seconds);
            self.length = (seconds * sample_rate).round().max(1.0) as usize;
            self.position = 0;
        }
        let t = self.position as f32 / self.length as f32;
        let smooth = 0.5 - 0.5 * (PI * t).cos();
        let value = self.from + (self.to - self.from) * smooth;
        self.position += 1;
        if self.position >= self.length {
            self.length = 0;
        }
        value
    }
}

/// Hierarchical weather state: slow air-mass changes, individual gusts, and
/// small turbulent eddies. This replaces a repeating ADSR-like gust cycle.
pub(crate) struct WindDriver {
    weather: SmoothRandom,
    gust: SmoothRandom,
    eddy: SmoothRandom,
}

impl WindDriver {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            weather: SmoothRandom::new(seed.wrapping_add(101), 18.0, 55.0),
            gust: SmoothRandom::new(seed.wrapping_add(103), 2.5, 11.0),
            eddy: SmoothRandom::new(seed.wrapping_add(107), 0.12, 0.80),
        }
    }

    pub(crate) fn set_change_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.weather.set_duration_range(min_seconds, max_seconds);
    }

    pub(crate) fn change_time_range(&self) -> (f32, f32) {
        self.weather.duration_range()
    }

    pub(crate) fn set_gust_duration_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.gust.set_duration_range(min_seconds, max_seconds);
    }

    pub(crate) fn gust_duration_range(&self) -> (f32, f32) {
        self.gust.duration_range()
    }

    pub(crate) fn set_turbulence_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.eddy.set_duration_range(min_seconds, max_seconds);
    }

    pub(crate) fn turbulence_time_range(&self) -> (f32, f32) {
        self.eddy.duration_range()
    }

    #[inline(always)]
    pub(crate) fn next(
        &mut self,
        sample_rate: f32,
        min_speed: f32,
        max_speed: f32,
        gust_strength: f32,
        rise_bias: f32,
    ) -> WindFrame {
        let weather = 0.5 * (self.weather.next_shaped(sample_rate, rise_bias) + 1.0);
        let gust = self.gust.next_shaped(sample_rate, rise_bias);
        let eddy = self.eddy.next(sample_rate);

        // The slow weather trajectory occupies the range left after reserving
        // headroom for short positive gusts. Negative halves of the gust
        // trajectory become genuine rests rather than downward oscillations.
        let gust_strength = gust_strength.clamp(0.0, 1.0);
        let gust_pressure = gust.max(0.0).powi(2);
        let activity =
            (weather * (1.0 - gust_strength) + gust_pressure * gust_strength).clamp(0.0, 1.0);
        let min_speed = min_speed.clamp(0.0, 25.0);
        let max_speed = max_speed.clamp(min_speed, 25.0);
        let speed = min_speed + (max_speed - min_speed) * activity;
        WindFrame {
            speed,
            activity,
            weather,
            gust: gust_pressure,
            eddy,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct WindFrame {
    pub(crate) speed: f32,
    pub(crate) activity: f32,
    /// Slow normalized air-mass state before gusts are mixed in.
    pub(crate) weather: f32,
    /// Positive normalized gust pressure before it is mixed into `activity`.
    pub(crate) gust: f32,
    pub(crate) eddy: f32,
}

/// Open-field wind signal. Spatial diffuseness is supplied by
/// `pipeline::renderers::field::FieldRenderer`; this struct only creates the
/// mono stochastic texture that all decorrelated field voices share.
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
        let air = self
            .air_lp
            .process(self.air_hp.process(self.air_rng.next_bipolar()));

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
        match param {
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
    fn field_wind_is_a_field_and_produces_audio() {
        let mut wind = FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 42);
        assert_eq!(wind.emitter_kind(), EmitterKind::Field);
        let energy: f32 = (0..48_000)
            .map(|_| wind.next_sample(48_000.0).powi(2))
            .sum();
        assert!(energy > 0.0);
    }

    #[test]
    fn zero_speed_range_is_true_calm() {
        let mut wind = FieldWindSource::new(Vec3::ZERO, 0.0, 0.0, 43);
        for _ in 0..48_000 {
            assert_eq!(wind.next_sample(48_000.0), 0.0);
        }
    }

    #[test]
    fn graphics_tick_and_callback_chunking_do_not_change_audio() {
        fn render(chunk_size: usize) -> Vec<f32> {
            let sample_rate = 48_000.0;
            let mut wind = FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 99);
            let mut output = Vec::with_capacity(48_000);
            while output.len() < 48_000 {
                let frames = chunk_size.min(48_000 - output.len());
                // Simulate a host calling tick once per callback. Different
                // callback sizes imply radically different tick rates.
                wind.tick(frames as f32 / sample_rate);
                output.extend((0..frames).map(|_| wind.next_sample(sample_rate)));
            }
            output
        }

        assert_eq!(render(64), render(1024));
    }

    #[test]
    fn control_trajectories_are_audio_rate_smooth() {
        let mut driver = WindDriver::new(7);
        let mut previous = driver.next(48_000.0, 2.0, 5.0, 0.3, 0.2).speed;
        let mut max_step = 0.0_f32;
        for _ in 0..48_000 {
            let current = driver.next(48_000.0, 2.0, 5.0, 0.3, 0.2).speed;
            max_step = max_step.max((current - previous).abs());
            previous = current;
        }
        assert!(max_step < 0.001, "control jumped by {max_step}");
    }

    #[test]
    fn instantaneous_speed_stays_inside_configured_bounds() {
        let mut driver = WindDriver::new(17);
        driver.set_change_time_range(0.2, 0.5);
        driver.set_gust_duration_range(0.05, 0.15);
        for _ in 0..(100 * 120) {
            let frame = driver.next(100.0, 2.0, 5.0, 0.45, 0.35);
            assert!(
                (2.0..=5.0).contains(&frame.speed),
                "speed escaped bounds: {}",
                frame.speed
            );
        }
    }

    #[test]
    fn configured_transition_lengths_are_expressed_in_seconds() {
        let mut trajectory = SmoothRandom::new(23, 20.0, 50.0);
        trajectory.next_shaped(100.0, 0.4);
        assert!((2_000..=5_000).contains(&trajectory.length));

        trajectory.length = 0;
        trajectory.next_shaped(1_000.0, 0.4);
        assert!((20_000..=50_000).contains(&trajectory.length));
    }

    #[test]
    fn positive_rise_bias_makes_rises_shorter_than_falls() {
        let mut trajectory = SmoothRandom::new(31, 1.0, 10.0);
        let mut rise_total = 0usize;
        let mut rise_count = 0usize;
        let mut fall_total = 0usize;
        let mut fall_count = 0usize;

        for _ in 0..2_000 {
            trajectory.length = 0;
            trajectory.next_shaped(100.0, 0.8);
            if trajectory.to >= trajectory.from {
                rise_total += trajectory.length;
                rise_count += 1;
            } else {
                fall_total += trajectory.length;
                fall_count += 1;
            }
        }

        let rise_mean = rise_total as f32 / rise_count as f32;
        let fall_mean = fall_total as f32 / fall_count as f32;
        assert!(
            rise_mean < fall_mean,
            "positive bias should shorten rises: {rise_mean} vs {fall_mean} samples"
        );
    }
}
