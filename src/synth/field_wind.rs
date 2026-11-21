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

use atrium_core::types::Vec3;

use super::noise::{white_noise_density_scale, OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};

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
            // Settings of rain, waves and flying sources; not this one's.
            SynthParam::Intensity
            | SynthParam::DropRate
            | SynthParam::DripRate
            | SynthParam::WavePeriod
            | SynthParam::CrashChance
            | SynthParam::HoverTimeMin
            | SynthParam::HoverTimeMax => {}
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

    use crate::synth::test_support::{
        averaged_power_spectrum, band_density_db, mean, peak, render, root_mean_square,
    };

    const SAMPLE_RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

    // ── SmoothRandom ─────────────────────────────────────────────────────

    /// Cosine interpolation between uniform [-1, 1) targets: the value never
    /// leaves [-1, 1] and never steps by more than the steepest cosine slope.
    #[test]
    fn smooth_random_is_bounded_and_continuous() {
        let sample_rate = 200.0;
        let mut trajectory = SmoothRandom::new(5, 0.2, 0.6);
        let mut previous = trajectory.next(sample_rate);
        // Steepest slope: (pi / 2) x span (2) / shortest segment (0.2 s).
        let step_limit = std::f32::consts::FRAC_PI_2 * 2.0 / (0.2 * sample_rate) + 1e-4;
        for _ in 0..200_000 {
            let value = trajectory.next(sample_rate);
            assert!((-1.0..=1.0).contains(&value));
            assert!(
                (value - previous).abs() <= step_limit,
                "stepped {}",
                (value - previous).abs()
            );
            previous = value;
        }
    }

    /// Seconds-to-samples conversion: each segment lasts between min and max
    /// seconds at any rate, and the trajectory's own start is seed dependent
    /// (not a fixed fade from zero).
    #[test]
    fn smooth_random_segments_last_the_configured_seconds_at_any_rate() {
        for sample_rate in [100.0_f32, 1_000.0, 4_800.0] {
            let mut trajectory = SmoothRandom::new(8, 2.0, 5.0);
            let mut lengths = Vec::new();
            for _ in 0..(200.0 * sample_rate) as usize {
                trajectory.next(sample_rate);
                // `length` is set at a segment start and cleared at its end.
                if trajectory.position == 1 {
                    lengths.push(trajectory.length);
                }
            }
            assert!(lengths.len() > 30);
            for &length in &lengths {
                let seconds = length as f32 / sample_rate;
                assert!(
                    (2.0 - 0.01..=5.0 + 0.01).contains(&seconds),
                    "{seconds} s at {sample_rate} Hz"
                );
            }
        }
    }

    /// `SmoothRandom::new` takes its first value from the seeded generator so
    /// that "starting every source at zero" is avoided (see its comment).
    /// Without seed mixing in `Rng::new`, xorshift64's first draw was exactly
    /// -1.0 for any seed below about 2^17, so every driver built with an
    /// ordinary seed started with weather 0, gust at rest and eddy at -1: all
    /// winds began calm at their minimum speed, whatever the seed.
    #[test]
    fn wind_driver_start_state_depends_on_the_seed() {
        let first_weather = |seed: u64| {
            let mut driver = WindDriver::new(seed);
            driver.next(48_000.0, 0.0, 25.0, 0.3, 0.0).weather
        };
        let starts: Vec<f32> = [1u64, 2, 42, 99, 1_000]
            .iter()
            .map(|&seed| first_weather(seed))
            .collect();
        let spread = starts.iter().cloned().fold(f32::MIN, f32::max)
            - starts.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            spread > 0.2,
            "first-frame weather for five seeds: {starts:?}"
        );
    }

    /// Duration shaping: with bias +1 a rise draws raw^3 (mean 1/4 of the
    /// range) and a fall 1 - (1-raw)^3 (mean 3/4); bias 0 is symmetric and a
    /// negative bias swaps the two.
    #[test]
    fn rise_bias_shapes_segment_durations_as_documented() {
        let measure = |bias: f32| {
            let mut trajectory = SmoothRandom::new(77, 1.0, 11.0);
            let (mut rise_sum, mut rise_count, mut fall_sum, mut fall_count) =
                (0.0_f64, 0, 0.0_f64, 0);
            for _ in 0..30_000 {
                trajectory.length = 0;
                trajectory.next_shaped(100.0, bias);
                let seconds = trajectory.length as f64 / 100.0;
                if trajectory.to >= trajectory.from {
                    rise_sum += seconds;
                    rise_count += 1;
                } else {
                    fall_sum += seconds;
                    fall_count += 1;
                }
            }
            (rise_sum / rise_count as f64, fall_sum / fall_count as f64)
        };
        let (rise, fall) = measure(1.0);
        assert!(
            (rise - (1.0 + 10.0 * 0.25)).abs() < 0.15,
            "rise mean {rise}"
        );
        assert!(
            (fall - (1.0 + 10.0 * 0.75)).abs() < 0.15,
            "fall mean {fall}"
        );
        let (rise, fall) = measure(0.0);
        assert!(
            (rise - 6.0).abs() < 0.15 && (fall - 6.0).abs() < 0.15,
            "{rise} {fall}"
        );
        let (rise, fall) = measure(-1.0);
        assert!(
            (rise - 8.5).abs() < 0.15 && (fall - 3.5).abs() < 0.15,
            "{rise} {fall}"
        );
    }

    #[test]
    fn duration_ranges_are_ordered_and_floored() {
        let mut trajectory = SmoothRandom::new(1, 1.0, 2.0);
        trajectory.set_duration_range(9.0, 3.0);
        assert_eq!(trajectory.duration_range(), (3.0, 9.0));
        trajectory.set_duration_range(0.0, 0.0);
        assert_eq!(trajectory.duration_range(), (0.05, 0.05));
    }

    // ── WindDriver ───────────────────────────────────────────────────────

    /// weather = (trajectory + 1) / 2 is in [0, 1] with mean 0.5; negative
    /// gust excursions become true rests (pressure 0) about half the time;
    /// activity always stays in [0, 1].
    #[test]
    fn driver_frames_have_the_documented_ranges_and_statistics() {
        let mut driver = WindDriver::new(12);
        driver.set_change_time_range(0.5, 1.0);
        driver.set_gust_duration_range(0.2, 0.5);
        driver.set_turbulence_time_range(0.05, 0.1);
        let frames = 200_000;
        let (mut weather_sum, mut rests) = (0.0_f64, 0usize);
        for _ in 0..frames {
            let frame = driver.next(100.0, 0.0, 25.0, 1.0, 0.0);
            assert!((0.0..=1.0).contains(&frame.weather));
            assert!((0.0..=1.0).contains(&frame.gust));
            assert!((0.0..=1.0).contains(&frame.activity));
            assert!((-1.0..=1.0).contains(&frame.eddy));
            assert!((0.0..=25.0).contains(&frame.speed));
            // gust_strength = 1: activity is exactly the gust pressure.
            assert!((frame.activity - frame.gust).abs() < 1e-6);
            weather_sum += frame.weather as f64;
            if frame.gust == 0.0 {
                rests += 1;
            }
        }
        let weather_mean = weather_sum / frames as f64;
        assert!(
            (weather_mean - 0.5).abs() < 0.03,
            "weather mean {weather_mean}"
        );
        let rest_share = rests as f64 / frames as f64;
        assert!((rest_share - 0.5).abs() < 0.06, "rest share {rest_share}");
    }

    /// gust_strength mixes slow weather and fast gust pressure linearly:
    /// activity = weather (1 - s) + gust_pressure s, and speed maps
    /// activity linearly onto [min, max].
    #[test]
    fn activity_and_speed_follow_the_mixing_law() {
        let mut driver = WindDriver::new(3);
        for strength in [0.0_f32, 0.3, 1.0] {
            for _ in 0..5_000 {
                let frame = driver.next(500.0, 2.0, 12.0, strength, 0.2);
                let expected =
                    (frame.weather * (1.0 - strength) + frame.gust * strength).clamp(0.0, 1.0);
                assert!((frame.activity - expected).abs() < 1e-6);
                assert!((frame.speed - (2.0 + 10.0 * frame.activity)).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn driver_clamps_speed_limits_to_the_physical_range() {
        let mut driver = WindDriver::new(4);
        for _ in 0..2_000 {
            let frame = driver.next(500.0, -5.0, 99.0, 2.0, 0.0);
            assert!((0.0..=25.0).contains(&frame.speed));
        }
        // Reversed limits collapse to the lower one rather than panicking.
        let frame = driver.next(500.0, 10.0, 3.0, 0.3, 0.0);
        assert_eq!(frame.speed, 10.0);
    }

    // ── Rendered audio ───────────────────────────────────────────────────

    /// Pressure law: speed_gain = (speed / 8)^2, so 8 m/s against 4 m/s is
    /// +12 dB of gain, a little more with the extra brightness (4.3x RMS).
    #[test]
    fn level_follows_the_quadratic_speed_law() {
        let level = |speed: f32| {
            let mut wind = FieldWindSource::new(Vec3::ZERO, speed, speed, 71);
            root_mean_square(&render(20.0, 16_000.0, |rate| wind.next_sample(rate)))
        };
        let ratio = level(8.0) / level(4.0);
        assert!(
            (3.6..5.5).contains(&ratio),
            "RMS ratio 8 vs 4 m/s is {ratio}"
        );
        let ratio = level(4.0) / level(2.0);
        assert!(
            (3.4..5.5).contains(&ratio),
            "RMS ratio 4 vs 2 m/s is {ratio}"
        );
    }

    /// Both airflow and gust brighten the spectrum.
    #[test]
    fn faster_wind_is_brighter() {
        let high_share = |speed: f32| {
            let mut wind = FieldWindSource::new(Vec3::ZERO, speed, speed, 72);
            let samples = render(12.0, 48_000.0, |rate| wind.next_sample(rate));
            let spectrum = averaged_power_spectrum(&samples);
            band_density_db(&spectrum, 48_000.0, 3_000.0, 8_000.0)
                - band_density_db(&spectrum, 48_000.0, 150.0, 400.0)
        };
        assert!(high_share(10.0) > high_share(2.0) + 3.0);
    }

    #[test]
    fn output_is_finite_zero_mean_and_inside_unity_at_default_speeds() {
        let mut wind = FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 7);
        let samples = render(20.0, 48_000.0, |rate| wind.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(peak(&samples) < 1.0, "peak {}", peak(&samples));
        assert!(mean(&samples).abs() < 0.002, "DC {}", mean(&samples));
    }

    /// At 25 m/s the gain is (25 / 8)^2 = 9.8, just under its cap of 10, so
    /// the raw generator peaks well above 1 (2.1 here). That is by design:
    /// `SynthNode` rescales by an RMS calibration, so raw level is not a
    /// clipping risk, but it is not documented anywhere either.
    #[test]
    fn gale_force_raw_output_exceeds_unity_but_stays_finite() {
        let mut wind = FieldWindSource::new(Vec3::ZERO, 25.0, 25.0, 7);
        let samples = render(10.0, 48_000.0, |rate| wind.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(peak(&samples) > 1.0 && peak(&samples) < 4.0);
        assert!((25.0_f32 / 8.0).powi(2) < 10.0);
    }

    /// The pink-driven low and mid bands keep their density at a fixed
    /// frequency when the host rate changes.
    #[test]
    fn low_and_mid_band_density_is_sample_rate_independent() {
        let densities: Vec<(f64, f64)> = SAMPLE_RATES
            .iter()
            .map(|&sample_rate| {
                let mut wind = FieldWindSource::new(Vec3::ZERO, 6.0, 6.0, 73);
                let samples = render(10.0, sample_rate, |rate| wind.next_sample(rate));
                let spectrum = averaged_power_spectrum(&samples);
                (
                    band_density_db(&spectrum, sample_rate, 180.0, 260.0),
                    band_density_db(&spectrum, sample_rate, 700.0, 1_000.0),
                )
            })
            .collect();
        for (low, mid) in &densities {
            assert!((low - densities[1].0).abs() < 1.0, "{densities:?}");
            assert!((mid - densities[1].1).abs() < 1.0, "{densities:?}");
        }
    }

    /// Switching the host rate mid-stream retunes every filter: the output
    /// stays finite and keeps its level.
    #[test]
    fn changing_sample_rate_midstream_retunes_without_a_glitch() {
        let mut wind = FieldWindSource::new(Vec3::ZERO, 6.0, 6.0, 74);
        let before = render(3.0, 48_000.0, |rate| wind.next_sample(rate));
        let after = render(3.0, 96_000.0, |rate| wind.next_sample(rate));
        assert!(after.iter().all(|value| value.is_finite()));
        let ratio = root_mean_square(&after) / root_mean_square(&before);
        assert!((0.6..1.5).contains(&ratio), "level jumped by {ratio}");
        assert_eq!(wind.sample_rate_cached, 96_000.0);
    }

    #[test]
    fn same_seed_is_bit_identical_and_other_seeds_differ() {
        let run = |seed: u64| {
            let mut wind = FieldWindSource::new(Vec3::ZERO, 2.0, 9.0, seed);
            render(0.5, 48_000.0, |rate| wind.next_sample(rate))
        };
        assert_eq!(run(5), run(5));
        assert_ne!(run(5), run(6));
    }

    // ── Parameters ───────────────────────────────────────────────────────

    #[test]
    fn speed_range_is_clamped_ordered_and_routed_through_synth_params() {
        let mut wind = FieldWindSource::new(Vec3::ZERO, -3.0, 99.0, 1);
        assert_eq!((wind.min_speed, wind.max_speed), (0.0, 25.0));
        wind.set_speed_range(12.0, 4.0);
        assert_eq!((wind.min_speed, wind.max_speed), (4.0, 12.0));
        wind.set_synth_param(SynthParam::MinSpeed, 40.0);
        assert_eq!(wind.min_speed, 12.0);
        wind.set_synth_param(SynthParam::MaxSpeed, 3.0);
        assert_eq!(wind.max_speed, 12.0);
        wind.set_synth_param(SynthParam::GustStrength, 5.0);
        assert_eq!(wind.gust_strength, 1.0);
        wind.set_synth_param(SynthParam::RiseBias, -5.0);
        assert_eq!(wind.rise_bias, -1.0);
        wind.set_synth_param(SynthParam::MasterGain, 9.0);
        assert_eq!(wind.master_gain, 2.0);
        wind.set_synth_param(SynthParam::LowGain, -1.0);
        assert_eq!(wind.low_gain, 0.0);
        wind.set_synth_param(SynthParam::ChangeTimeMin, 1.0e6);
        assert_eq!(wind.change_time_range().0, wind.change_time_range().1);
    }

    /// A NaN from a UI or script would survive the `clamp`-based validation
    /// (`f32::clamp` returns NaN for NaN) and silence the source permanently:
    /// the master gain multiplies every output sample. Non-finite values are
    /// rejected before the match (see also the cross-generator check in
    /// `synth::test_support`).
    #[test]
    fn non_finite_parameters_are_rejected() {
        let mut wind = FieldWindSource::new(Vec3::ZERO, 4.0, 4.0, 1);
        wind.set_synth_param(SynthParam::MasterGain, f32::NAN);
        let samples = render(0.1, 48_000.0, |rate| wind.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
    }
}
