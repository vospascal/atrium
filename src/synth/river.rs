//! Procedural flowing-river synthesis.
//!
//! A river is not rendered by slowing one noise carrier. Its identity comes
//! from several scales acting together: a continuous turbulent body, faster
//! surface flow, and many short bubble resonances. Slow eddy envelopes bind
//! those layers into one moving current. Optional splash and spray layers add
//! obstacle detail without becoming the foundation of the sound.
//!
//! Every ramp and event advances on the audio sample clock. [`SoundSource::tick`]
//! is intentionally a no-op, so graphics FPS and callback size cannot alter
//! the river's rhythm.

use std::f32::consts::TAU;

use atrium_core::commands::SynthParam;
use atrium_core::source::{EmitterKind, SoundSource};

use crate::audio::filters::Biquad;
use atrium_core::types::Vec3;

use super::noise::{white_noise_density_scale, OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};

const RING_SIZE: usize = 8192;
const RING_MASK: usize = RING_SIZE - 1;

#[derive(Clone, Copy)]
struct RandomRamp {
    from: f32,
    to: f32,
    elapsed: f32,
    duration: f32,
}

impl RandomRamp {
    fn new(initial: f32) -> Self {
        Self {
            from: initial,
            to: initial,
            elapsed: 0.0,
            duration: 0.0,
        }
    }

    #[inline]
    fn smootherstep(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    }

    fn next(
        &mut self,
        rng: &mut Rng,
        dt: f32,
        target_min: f32,
        target_max: f32,
        time_min: f32,
        time_max: f32,
    ) -> f32 {
        if self.duration <= 0.0 || self.elapsed >= self.duration {
            self.from = self.to.clamp(target_min, target_max);
            self.to = target_min + (target_max - target_min) * rng.next_f32();
            self.duration = time_min + (time_max - time_min) * rng.next_f32();
            self.elapsed = 0.0;
        }
        let t = Self::smootherstep(self.elapsed / self.duration.max(dt));
        let value = self.from + (self.to - self.from) * t;
        self.elapsed += dt;
        value.clamp(target_min, target_max)
    }
}

#[derive(Clone, Copy)]
struct FlowState {
    speed_m_s: f32,
    slow_eddy: f32,
    fast_eddy: f32,
}

/// Long flow evolution plus two correlated-motion scales. The configured
/// eddy range controls the perceptually important 0.2-1 Hz motion; a derived
/// faster ramp adds surface detail without controlling the carrier rate.
struct RiverDriver {
    rng: Rng,
    speed: RandomRamp,
    slow_eddy: RandomRamp,
    fast_eddy: RandomRamp,
    change_time_min: f32,
    change_time_max: f32,
    eddy_time_min: f32,
    eddy_time_max: f32,
}

impl RiverDriver {
    fn new(seed: u64, initial_speed: f32) -> Self {
        Self {
            rng: Rng::new(seed),
            speed: RandomRamp::new(initial_speed),
            slow_eddy: RandomRamp::new(0.0),
            fast_eddy: RandomRamp::new(0.0),
            change_time_min: 15.0,
            change_time_max: 110.0,
            eddy_time_min: 0.35,
            eddy_time_max: 1.25,
        }
    }

    fn set_change_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        let min_seconds = min_seconds.max(0.1);
        self.change_time_min = min_seconds.min(max_seconds.max(0.1));
        self.change_time_max = min_seconds.max(max_seconds.max(0.1));
    }

    fn change_time_range(&self) -> (f32, f32) {
        (self.change_time_min, self.change_time_max)
    }

    fn set_eddy_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        let min_seconds = min_seconds.max(0.05);
        self.eddy_time_min = min_seconds.min(max_seconds.max(0.05));
        self.eddy_time_max = min_seconds.max(max_seconds.max(0.05));
    }

    fn eddy_time_range(&self) -> (f32, f32) {
        (self.eddy_time_min, self.eddy_time_max)
    }

    fn next(&mut self, sample_rate: f32, min_speed: f32, max_speed: f32) -> FlowState {
        let dt = 1.0 / sample_rate.max(1.0);
        let speed_m_s = self.speed.next(
            &mut self.rng,
            dt,
            min_speed,
            max_speed,
            self.change_time_min,
            self.change_time_max,
        );
        let slow_eddy = self.slow_eddy.next(
            &mut self.rng,
            dt,
            -1.0,
            1.0,
            self.eddy_time_min,
            self.eddy_time_max,
        );
        let fast_min = (self.eddy_time_min * 0.16).clamp(0.04, 0.40);
        let fast_max = (self.eddy_time_max * 0.16).clamp(fast_min, 0.60);
        let fast_eddy = self
            .fast_eddy
            .next(&mut self.rng, dt, -1.0, 1.0, fast_min, fast_max);
        FlowState {
            speed_m_s,
            slow_eddy,
            fast_eddy,
        }
    }
}

/// A located segment of moving water. The scene wrapper supplies spread,
/// directivity, SPL calibration, and live position changes.
pub struct RiverSource {
    driver: RiverDriver,

    body_noise: PinkNoiseFull,
    body_hp: OnePoleHP,
    body_lp: OnePoleLP,

    current_noise: PinkNoiseFull,
    current_hp: OnePoleHP,
    current_lp: OnePoleLP,

    spray_rng: Rng,
    spray_hp: OnePoleHP,
    spray_lp: OnePoleLP,

    event_rng: Rng,
    bubble_hazard: f32,
    bubble_threshold: f32,
    ring: Box<[f32; RING_SIZE]>,
    ring_idx: usize,

    output_hp: OnePoleHP,
    output_lp: OnePoleLP,
    body_ridge: Biquad,
    babble_ridge: Biquad,
    high_shelf: Biquad,

    /// Instantaneous water-flow speed bounds in metres/second.
    pub min_flow_speed: f32,
    pub max_flow_speed: f32,
    /// Depth of the multi-scale eddy modulation, 0-1.
    pub eddy_depth: f32,
    /// Low-mid turbulent current.
    pub body_gain: f32,
    /// Faster continuous surface flow.
    pub current_gain: f32,
    /// Bubble population density and level.
    pub bubble_activity: f32,
    /// Mean obstacle splashes per second at nominal flow.
    pub splash_rate: f32,
    /// Obstacle-splash level.
    pub splash_gain: f32,
    /// Fine turbulent spray level.
    pub spray_gain: f32,
    pub master_gain: f32,

    /// Keeps the white spray layer at its 48 kHz density per Hz.
    white_noise_scale: f32,
    position: Vec3,
    sample_rate_cached: f32,
}

impl RiverSource {
    pub fn new(position: Vec3, min_flow_speed: f32, max_flow_speed: f32, seed: u64) -> Self {
        let min_flow_speed = min_flow_speed.clamp(0.0, 5.0);
        let max_flow_speed = max_flow_speed.clamp(min_flow_speed, 5.0);
        let initial_speed = (min_flow_speed + max_flow_speed) * 0.5;
        Self {
            driver: RiverDriver::new(seed.wrapping_add(1), initial_speed),
            body_noise: PinkNoiseFull::new(seed.wrapping_add(3)),
            body_hp: OnePoleHP::new(350.0, 48_000.0),
            body_lp: OnePoleLP::new(1_050.0, 48_000.0),
            current_noise: PinkNoiseFull::new(seed.wrapping_add(5)),
            current_hp: OnePoleHP::new(450.0, 48_000.0),
            current_lp: OnePoleLP::new(4_000.0, 48_000.0),
            spray_rng: Rng::new(seed.wrapping_add(7)),
            spray_hp: OnePoleHP::new(1_800.0, 48_000.0),
            spray_lp: OnePoleLP::new(6_500.0, 48_000.0),
            event_rng: Rng::new(seed.wrapping_add(11)),
            bubble_hazard: 0.0,
            bubble_threshold: 1.0,
            ring: Box::new([0.0; RING_SIZE]),
            ring_idx: 0,
            output_hp: OnePoleHP::new(90.0, 48_000.0),
            output_lp: OnePoleLP::new(12_000.0, 48_000.0),
            body_ridge: Biquad::unity(),
            babble_ridge: Biquad::unity(),
            high_shelf: Biquad::unity(),
            min_flow_speed,
            max_flow_speed,
            eddy_depth: 0.65,
            body_gain: 0.40,
            current_gain: 0.65,
            bubble_activity: 0.75,
            splash_rate: 0.45,
            splash_gain: 0.30,
            spray_gain: 0.050,
            master_gain: 1.0,
            white_noise_scale: 1.0,
            position,
            sample_rate_cached: 0.0,
        }
    }

    pub fn set_flow_speed_range(&mut self, min_m_s: f32, max_m_s: f32) {
        let min_m_s = min_m_s.clamp(0.0, 5.0);
        self.min_flow_speed = min_m_s.min(max_m_s.clamp(0.0, 5.0));
        self.max_flow_speed = min_m_s.max(max_m_s.clamp(0.0, 5.0));
    }

    pub fn set_change_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.driver.set_change_time_range(min_seconds, max_seconds);
    }

    pub fn change_time_range(&self) -> (f32, f32) {
        self.driver.change_time_range()
    }

    pub fn set_eddy_time_range(&mut self, min_seconds: f32, max_seconds: f32) {
        self.driver.set_eddy_time_range(min_seconds, max_seconds);
    }

    pub fn eddy_time_range(&self) -> (f32, f32) {
        self.driver.eddy_time_range()
    }

    fn retune(&mut self, sample_rate: f32) {
        let nyquist_guard = sample_rate * 0.45;
        self.body_hp.set_cutoff(350.0, sample_rate);
        self.body_lp.set_cutoff(1_050.0, sample_rate);
        self.current_hp.set_cutoff(450.0, sample_rate);
        self.current_lp.set_cutoff(4_000.0, sample_rate);
        self.spray_hp.set_cutoff(1_800.0, sample_rate);
        self.spray_lp
            .set_cutoff(6_500.0_f32.min(nyquist_guard), sample_rate);
        self.output_hp.set_cutoff(90.0, sample_rate);
        self.output_lp
            .set_cutoff(12_000.0_f32.min(nyquist_guard), sample_rate);

        // The creek reference is concentrated from roughly 500-1600 Hz and
        // falls steeply above 4 kHz. The resonant ridges leave room for bubble
        // events to define the material instead of a flat broadband wash.
        self.body_ridge.set_peak(650.0, 1.0, 0.75, sample_rate);
        self.babble_ridge.set_peak(1_450.0, 4.0, 0.75, sample_rate);
        self.high_shelf.set_high_shelf(4_000.0, -5.5, sample_rate);
        self.white_noise_scale = white_noise_density_scale(sample_rate);
    }

    /// Add a short, low-Q bubble resonance. Bubble radius maps to frequency
    /// through the Minnaert relationship; the procedurally sampled 1.2-7 mm
    /// population yields approximately 450-2700 Hz airborne components.
    fn write_bubble(&mut self, freq: f32, decay: f32, gain: f32, sample_rate: f32) {
        let duration = (5.5 / decay).clamp(0.006, 0.050);
        let len = ((duration * sample_rate) as usize).clamp(16, RING_SIZE / 2);
        let attack_samples = (0.00045 * sample_rate).max(1.0);
        let phase = self.event_rng.next_f32() * TAU;
        let chirp = freq * (0.03 + 0.09 * self.event_rng.next_f32()) / duration;

        for i in 0..len {
            let t = i as f32 / sample_rate;
            let attack = (i as f32 / attack_samples).min(1.0);
            let envelope = attack * (-decay * t).exp();
            let oscillator = (phase + TAU * (freq * t + 0.5 * chirp * t * t)).sin();
            let index = (self.ring_idx + 1 + i) & RING_MASK;
            self.ring[index] += oscillator * envelope * gain;
        }
    }

    /// Add a short broadband collision with an obstacle or shallow stone.
    fn write_splash(&mut self, duration: f32, cutoff: f32, gain: f32, sample_rate: f32) {
        let len = ((duration * sample_rate) as usize).clamp(16, RING_SIZE / 2);
        let attack_samples = (0.0012 * sample_rate).max(1.0);
        let dt = 1.0 / sample_rate;
        let lp_alpha = dt / (1.0 / (TAU * cutoff) + dt);
        let hp_alpha = (1.0 / (TAU * 220.0)) / (1.0 / (TAU * 220.0) + dt);
        let decay = 5.0 / duration.max(0.005);
        let white_noise_scale = white_noise_density_scale(sample_rate);
        let mut lp = 0.0_f32;
        let mut hp = 0.0_f32;
        let mut previous = 0.0_f32;

        for i in 0..len {
            let t = i as f32 * dt;
            let attack = (i as f32 / attack_samples).min(1.0);
            let envelope = attack * (-decay * t).exp();
            lp += lp_alpha * (self.event_rng.next_bipolar() * white_noise_scale - lp);
            hp = hp_alpha * (hp + lp - previous);
            previous = lp;
            let index = (self.ring_idx + 1 + i) & RING_MASK;
            self.ring[index] += hp * envelope * gain;
        }
    }

    /// Very short bubble-split/pop transient. These rare broadband ticks give
    /// the brilliance band the heavy-tailed envelope seen in real creek audio
    /// without turning the continuous current into hiss.
    fn write_pop(&mut self, gain: f32, sample_rate: f32) {
        let duration = 0.0012 + 0.0028 * self.event_rng.next_f32();
        let len = ((duration * sample_rate) as usize).clamp(12, RING_SIZE / 4);
        let dt = 1.0 / sample_rate;
        let lp_alpha = dt / (1.0 / (TAU * 9_000.0) + dt);
        let hp_alpha = (1.0 / (TAU * 2_000.0)) / (1.0 / (TAU * 2_000.0) + dt);
        let white_noise_scale = white_noise_density_scale(sample_rate);
        let mut lp = 0.0_f32;
        let mut hp = 0.0_f32;
        let mut previous = 0.0_f32;

        for i in 0..len {
            let t = i as f32 * dt;
            let envelope = (-6.0 * t / duration).exp();
            lp += lp_alpha * (self.event_rng.next_bipolar() * white_noise_scale - lp);
            hp = hp_alpha * (hp + lp - previous);
            previous = lp;
            let index = (self.ring_idx + 1 + i) & RING_MASK;
            self.ring[index] += hp * envelope * gain;
        }
    }

    fn spawn_bubble(&mut self, flow_norm: f32, sample_rate: f32) {
        // A broad bubble-radius population: large bubbles provide the 450-900
        // Hz body, smaller bubbles fill the 0.9-2.7 kHz babble band.
        let region = self.event_rng.next_f32();
        let freq = if region < 0.45 {
            450.0 + 450.0 * self.event_rng.next_f32()
        } else if region < 0.85 {
            900.0 + 800.0 * self.event_rng.next_f32()
        } else {
            1_700.0 + 1_000.0 * self.event_rng.next_f32()
        } * (0.90 + 0.16 * flow_norm.min(1.5));
        let decay = 170.0 + 300.0 * self.event_rng.next_f32() + 0.08 * freq;
        let amplitude = self.event_rng.next_f32();
        let gain =
            self.bubble_activity * (0.018 + 0.180 * amplitude.powi(5)) * (0.75 + 0.25 * flow_norm);
        self.write_bubble(freq, decay, gain, sample_rate);

        // A quiet coupled mode adds the fuller low-frequency cloud response
        // observed when bubbles are not acoustically isolated.
        if self.event_rng.next_f32() < 0.35 {
            let coupled = freq * (0.48 + 0.18 * self.event_rng.next_f32());
            self.write_bubble(coupled, decay * 0.80, gain * 0.24, sample_rate);
        }
        if self.event_rng.next_f32() < 0.16 {
            let pop_gain = self.spray_gain * (0.18 + 0.65 * self.event_rng.next_f32());
            self.write_pop(pop_gain, sample_rate);
        }
    }
}

impl SoundSource for RiverSource {
    #[inline]
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.retune(sample_rate);
        }

        let state = self
            .driver
            .next(sample_rate, self.min_flow_speed, self.max_flow_speed);
        if state.speed_m_s <= 0.0 && self.max_flow_speed <= 0.0 {
            return 0.0;
        }

        let flow_norm = (state.speed_m_s / 1.2).sqrt().clamp(0.0, 2.2);
        let activity = (state.speed_m_s / 0.15).clamp(0.0, 1.0);
        let eddy = self.eddy_depth.clamp(0.0, 1.0);

        // Shared eddy energy gives the river a 2-4 dB moving envelope. Layer
        // weighting is not identical, so the spectrum moves rather than one
        // fixed noise color passing through a volume LFO.
        let shared_db = eddy * (4.0 * state.slow_eddy + 0.35 * state.fast_eddy);
        let shared = 10.0_f32.powf(shared_db / 20.0);
        let body_envelope =
            (1.0 + eddy * (0.28 * state.slow_eddy + 0.05 * state.fast_eddy)).max(0.2);
        let current_motion =
            (1.0 + eddy * (-0.14 * state.slow_eddy + 0.02 * state.fast_eddy)).max(0.2);
        let spray_motion =
            (1.0 + eddy * (-0.28 * state.slow_eddy + 0.20 * state.fast_eddy)).max(0.1);

        let body = self
            .body_lp
            .process(self.body_hp.process(self.body_noise.next_sample()));
        let current = self
            .current_lp
            .process(self.current_hp.process(self.current_noise.next_sample()));
        let spray = self.spray_lp.process(
            self.spray_hp
                .process(self.spray_rng.next_bipolar() * self.white_noise_scale),
        );

        let continuous = (self.body_gain * body * body_envelope
            + self.current_gain * current * current_motion
            + self.spray_gain * spray * spray_motion)
            * shared
            * activity
            * (0.72 + 0.28 * flow_norm);

        let bubble_rate = self.bubble_activity.max(0.0)
            * (85.0 + 155.0 * flow_norm)
            * activity
            * (1.0 + eddy * (-0.12 * state.slow_eddy + 0.08 * state.fast_eddy)).max(0.2);
        self.bubble_hazard += bubble_rate / sample_rate.max(1.0);
        while self.bubble_hazard >= self.bubble_threshold {
            self.bubble_hazard -= self.bubble_threshold;
            let draw = self.event_rng.next_f32().clamp(1e-6, 1.0 - 1e-6);
            self.bubble_threshold = -(1.0 - draw).ln();
            self.spawn_bubble(flow_norm, sample_rate);
        }

        let splash_rate = self.splash_rate.max(0.0)
            * (0.30 + 0.70 * flow_norm)
            * activity
            * (1.0 + 0.45 * state.fast_eddy).max(0.2);
        let splash_probability = 1.0 - (-splash_rate / sample_rate.max(1.0)).exp();
        if self.event_rng.next_f32() < splash_probability {
            let size = self.event_rng.next_f32();
            let duration = 0.018 + 0.050 * size;
            let cutoff = 1_500.0 + 2_800.0 * (1.0 - size);
            let gain = self.splash_gain * (0.14 + 0.34 * self.event_rng.next_f32().powi(2));
            self.write_splash(duration, cutoff, gain, sample_rate);
        }

        let events = self.ring[self.ring_idx];
        self.ring[self.ring_idx] = 0.0;
        self.ring_idx = (self.ring_idx + 1) & RING_MASK;

        let output = self.output_hp.process(continuous + events);
        let output = self.body_ridge.process(output);
        let output = self.babble_ridge.process(output);
        let output = self.high_shelf.process(output);
        self.output_lp.process(output) * self.master_gain
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Object
    }

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
            SynthParam::FlowSpeedMin => self.min_flow_speed = value.clamp(0.0, self.max_flow_speed),
            SynthParam::FlowSpeedMax => self.max_flow_speed = value.clamp(self.min_flow_speed, 5.0),
            SynthParam::RiverSplashRate => self.splash_rate = value.max(0.0),
            SynthParam::ChangeTimeMin => {
                let (_, max) = self.change_time_range();
                self.set_change_time_range(value.clamp(0.1, max), max);
            }
            SynthParam::ChangeTimeMax => {
                let (min, _) = self.change_time_range();
                self.set_change_time_range(min, value.max(min));
            }
            SynthParam::TurbulenceTimeMin => {
                let (_, max) = self.eddy_time_range();
                self.set_eddy_time_range(value.clamp(0.05, max), max);
            }
            SynthParam::TurbulenceTimeMax => {
                let (min, _) = self.eddy_time_range();
                self.set_eddy_time_range(min, value.max(min));
            }
            SynthParam::TurbulenceDepth => self.eddy_depth = value.clamp(0.0, 1.0),
            SynthParam::LowGain => self.body_gain = value.max(0.0),
            SynthParam::BodyGain => self.current_gain = value.max(0.0),
            SynthParam::MidGain => self.bubble_activity = value.max(0.0),
            SynthParam::PresenceGain => self.splash_gain = value.max(0.0),
            SynthParam::AirGain => self.spray_gain = value.max(0.0),
            SynthParam::MasterGain => self.master_gain = value.clamp(0.0, 2.0),
            SynthParam::MinSpeed
            | SynthParam::MaxSpeed
            | SynthParam::GustDurationMin
            | SynthParam::GustDurationMax
            | SynthParam::GustStrength
            | SynthParam::RiseBias
            | SynthParam::GustBrightness
            | SynthParam::TurbulenceBrightness
            | SynthParam::FoliageDensity
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
    fn river_is_a_located_object_and_produces_audio() {
        let mut river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 42);
        assert_eq!(river.emitter_kind(), EmitterKind::Object);
        let energy: f32 = (0..48_000)
            .map(|_| river.next_sample(48_000.0).powi(2))
            .sum();
        assert!(energy > 0.0);
    }

    #[test]
    fn zero_flow_is_silent() {
        let mut river = RiverSource::new(Vec3::ZERO, 0.0, 0.0, 42);
        let energy: f32 = (0..48_000)
            .map(|_| river.next_sample(48_000.0).powi(2))
            .sum();
        assert_eq!(energy, 0.0);
    }

    #[test]
    fn graphics_ticks_do_not_advance_or_change_the_river() {
        let mut untouched = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 99);
        let mut ticked = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 99);
        for sample in 0..96_000 {
            if sample % 800 == 0 {
                ticked.tick(1.0 / 60.0);
            }
            assert_eq!(
                untouched.next_sample(48_000.0).to_bits(),
                ticked.next_sample(48_000.0).to_bits()
            );
        }
    }

    #[test]
    fn level_is_stable_across_sample_rates() {
        let rms = |sample_rate: f32| {
            let mut river = RiverSource::new(Vec3::ZERO, 0.75, 0.75, 73);
            let samples = (sample_rate * 3.0) as usize;
            let energy: f64 = (0..samples)
                .map(|_| river.next_sample(sample_rate) as f64)
                .map(|sample| sample * sample)
                .sum();
            (energy / samples as f64).sqrt() as f32
        };
        let ratio = rms(96_000.0) / rms(48_000.0).max(1e-12);
        assert!(
            (0.65..1.45).contains(&ratio),
            "river level changed too much with sample rate: {ratio}"
        );
    }

    #[test]
    fn defaults_define_a_multi_scale_flow() {
        let river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 42);
        assert_eq!(river.change_time_range(), (15.0, 110.0));
        assert_eq!(river.eddy_time_range(), (0.35, 1.25));
        assert_eq!(river.eddy_depth, 0.65);
        assert!(river.body_gain > 0.0);
        assert!(river.current_gain > 0.0);
        assert!(river.bubble_activity > 0.0);
        assert!(river.splash_rate > 0.0);
    }

    use crate::synth::test_support::{
        averaged_power_spectrum, band_density_db, band_power, mean, peak, render, root_mean_square,
    };

    const SAMPLE_RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

    // ── RandomRamp ───────────────────────────────────────────────────────

    #[test]
    fn smootherstep_has_zero_slope_at_both_ends_and_is_monotonic() {
        assert_eq!(RandomRamp::smootherstep(0.0), 0.0);
        assert_eq!(RandomRamp::smootherstep(1.0), 1.0);
        assert!((RandomRamp::smootherstep(0.5) - 0.5).abs() < 1e-6);
        // Out-of-range input is clamped.
        assert_eq!(RandomRamp::smootherstep(-3.0), 0.0);
        assert_eq!(RandomRamp::smootherstep(9.0), 1.0);
        // 6t^5 - 15t^4 + 10t^3: first derivative 30 t^2 (1 - t)^2 vanishes at 0 and 1.
        let slope_at =
            |t: f32| (RandomRamp::smootherstep(t + 1e-3) - RandomRamp::smootherstep(t)) / 1e-3;
        assert!(slope_at(0.0) < 1e-4);
        assert!(slope_at(0.999) < 1e-3);
        let mut previous = 0.0;
        for step in 1..=100 {
            let value = RandomRamp::smootherstep(step as f32 / 100.0);
            assert!(value >= previous);
            previous = value;
        }
    }

    /// The ramp never leaves its target range, never jumps, and each segment
    /// lasts between `time_min` and `time_max`.
    #[test]
    fn random_ramp_is_bounded_continuous_and_honours_segment_durations() {
        let mut rng = Rng::new(5);
        let mut ramp = RandomRamp::new(0.0);
        let sample_rate = 1_000.0;
        let dt = 1.0 / sample_rate;
        let mut previous = 0.0_f32;
        let mut segment_starts = vec![0usize];
        let mut last_duration = ramp.duration;
        for index in 0..60_000 {
            let value = ramp.next(&mut rng, dt, -1.0, 1.0, 0.5, 2.0);
            assert!((-1.0..=1.0).contains(&value));
            // Largest slope of a smootherstep is 1.875 x (span / duration).
            assert!(
                (value - previous).abs() < 2.0 * 1.875 / (0.5 * sample_rate) + 1e-4,
                "jump at {index}"
            );
            previous = value;
            if ramp.duration != last_duration {
                segment_starts.push(index);
                last_duration = ramp.duration;
            }
        }
        assert!(segment_starts.len() > 20);
        for pair in segment_starts.windows(2).skip(1) {
            let seconds = (pair[1] - pair[0]) as f32 * dt;
            assert!(
                (0.49..=2.01).contains(&seconds),
                "segment lasted {seconds} s"
            );
        }
    }

    #[test]
    fn random_ramp_with_equal_targets_is_constant() {
        let mut rng = Rng::new(6);
        let mut ramp = RandomRamp::new(0.75);
        for _ in 0..5_000 {
            assert_eq!(ramp.next(&mut rng, 0.001, 0.75, 0.75, 1.0, 5.0), 0.75);
        }
    }

    // ── Driver ───────────────────────────────────────────────────────────

    #[test]
    fn driver_time_ranges_are_ordered_and_floored() {
        let mut driver = RiverDriver::new(1, 0.5);
        assert_eq!(driver.change_time_range(), (15.0, 110.0));
        assert_eq!(driver.eddy_time_range(), (0.35, 1.25));
        driver.set_change_time_range(40.0, 20.0);
        assert_eq!(driver.change_time_range(), (20.0, 40.0));
        driver.set_change_time_range(0.0, 0.0);
        assert_eq!(driver.change_time_range(), (0.1, 0.1));
        driver.set_eddy_time_range(2.0, 0.5);
        assert_eq!(driver.eddy_time_range(), (0.5, 2.0));
        driver.set_eddy_time_range(0.0, 0.0);
        assert_eq!(driver.eddy_time_range(), (0.05, 0.05));
    }

    #[test]
    fn flow_state_stays_inside_its_bounds() {
        let mut driver = RiverDriver::new(9, 1.0);
        driver.set_change_time_range(0.2, 0.5);
        driver.set_eddy_time_range(0.05, 0.2);
        let (mut lowest, mut highest) = (f32::MAX, f32::MIN);
        for _ in 0..200_000 {
            let state = driver.next(2_000.0, 0.4, 1.6);
            assert!((0.4..=1.6).contains(&state.speed_m_s));
            assert!((-1.0..=1.0).contains(&state.slow_eddy));
            assert!((-1.0..=1.0).contains(&state.fast_eddy));
            lowest = lowest.min(state.speed_m_s);
            highest = highest.max(state.speed_m_s);
        }
        // Over 100 s of 0.2-0.5 s segments the speed explores most of the range.
        assert!(
            lowest < 0.6 && highest > 1.4,
            "explored {lowest}..{highest}"
        );
    }

    // ── Event processes ──────────────────────────────────────────────────

    /// Bubbles spawned over `seconds`: `bubble_threshold` is redrawn from an
    /// Exp(1) variate on every spawn.
    fn count_bubbles(river: &mut RiverSource, seconds: f32, sample_rate: f32) -> usize {
        let mut count = 0;
        for _ in 0..(seconds * sample_rate) as usize {
            let before = river.bubble_threshold;
            river.next_sample(sample_rate);
            if river.bubble_threshold != before {
                count += 1;
            }
        }
        count
    }

    fn steady_river(speed: f32, seed: u64) -> RiverSource {
        let mut river = RiverSource::new(Vec3::ZERO, speed, speed, seed);
        river.eddy_depth = 0.0;
        river
    }

    /// Bubble rate = activity * (85 + 155 flow_norm) * min(speed / 0.15, 1) per
    /// second with flow_norm = sqrt(speed / 1.2): a hazard integral against
    /// Exp(1) thresholds, i.e. a Poisson process.
    #[test]
    fn bubble_rate_follows_the_flow_speed_law() {
        for speed in [0.075_f32, 0.3, 1.2, 3.0] {
            let flow_norm = (speed / 1.2).sqrt().clamp(0.0, 2.2);
            let flow_activity = (speed / 0.15).clamp(0.0, 1.0);
            let rate = 0.75 * (85.0 + 155.0 * flow_norm) * flow_activity;
            let seconds = 20.0;
            let expected = rate * seconds;
            let count = count_bubbles(&mut steady_river(speed, 3), seconds, 16_000.0) as f32;
            assert!(
                (count - expected).abs() < 4.0 * expected.sqrt() + 2.0,
                "speed {speed}: {count} bubbles in {seconds} s, expected {expected:.0}"
            );
        }
    }

    #[test]
    fn bubble_rate_is_the_same_at_every_sample_rate() {
        let expected = 0.75 * 240.0 * 8.0; // speed 1.2: flow_norm 1, activity 1
        for sample_rate in [16_000.0_f32, 44_100.0, 96_000.0] {
            let seconds = if sample_rate > 40_000.0 { 4.0 } else { 8.0 };
            let count = count_bubbles(&mut steady_river(1.2, 4), seconds, sample_rate) as f32;
            let scaled = count * 8.0 / seconds;
            assert!(
                (scaled - expected).abs() < 5.0 * expected.sqrt(),
                "{sample_rate} Hz: {scaled:.0} per 8 s, expected {expected:.0}"
            );
        }
    }

    /// Splash starts: with bubbles and pops off, the only ring writes are
    /// splashes, which begin at `ring_idx + 1` with a 1.2 ms attack ramp.
    fn count_splashes(river: &mut RiverSource, seconds: f32, sample_rate: f32) -> usize {
        let mut count = 0;
        for _ in 0..(seconds * sample_rate) as usize {
            let watched = (river.ring_idx + 2) & RING_MASK;
            let before = river.ring[watched];
            river.next_sample(sample_rate);
            if river.ring[watched] != before {
                count += 1;
            }
        }
        count
    }

    /// splash probability = 1 - exp(-rate / sample_rate), rate =
    /// splash_rate (0.3 + 0.7 flow_norm) activity (1 + 0.45 fast_eddy), so
    /// at 1.2 m/s the mean rate is `splash_rate`.
    #[test]
    fn splash_rate_matches_the_configured_rate() {
        let mut river = steady_river(1.2, 5);
        river.bubble_activity = 0.0;
        river.splash_rate = 10.0;
        let count = count_splashes(&mut river, 30.0, 16_000.0) as f32;
        assert!(
            (count - 300.0).abs() < 4.0 * 300.0_f32.sqrt() + 15.0,
            "{count} splashes in 30 s"
        );
    }

    // ── Event shapes ─────────────────────────────────────────────────────

    /// A bubble is a chirped decaying sine: cycles = f T + chirp T / 2 with
    /// chirp / f / (1/T) between 3 % and 12 %, so 1.015 to 1.06 f T cycles,
    /// over a duration 5.5 / decay clamped to 6-50 ms.
    #[test]
    fn written_bubble_is_a_slightly_rising_decaying_sine() {
        let sample_rate = 48_000.0;
        for (frequency, decay) in [(450.0_f32, 200.0_f32), (1_000.0, 400.0), (2_500.0, 800.0)] {
            let mut river = RiverSource::new(Vec3::ZERO, 0.5, 0.5, 1);
            river.write_bubble(frequency, decay, 1.0, sample_rate);
            let duration = (5.5 / decay).clamp(0.006, 0.050);
            let length = (duration * sample_rate) as usize;
            let bubble: Vec<f32> = (0..length + 4)
                .map(|offset| river.ring[1 + offset])
                .collect();
            assert!(bubble[length..].iter().all(|&value| value == 0.0));
            let cycles = bubble[..length]
                .windows(2)
                .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
                .count() as f32;
            let base = frequency * duration;
            assert!(
                cycles >= base * 1.01 - 1.0 && cycles <= base * 1.07 + 1.0,
                "{frequency} Hz: {cycles} cycles, base {base}"
            );
            // Amplitude at the end of the burst is e^-5.5 of the peak (0.4 %),
            // unless the 50 ms ceiling cuts a slow decay short.
            let early = bubble[..bubble.len() / 4]
                .iter()
                .fold(0.0_f32, |a, v| a.max(v.abs()));
            let late = bubble[length * 9 / 10..length]
                .iter()
                .fold(0.0_f32, |a, v| a.max(v.abs()));
            assert!(
                late < 0.05 * early,
                "{frequency} Hz: tail {late} vs peak {early}"
            );
        }
    }

    /// The comment on `write_bubble` says 1.2-7 mm bubbles map to 450-2700 Hz
    /// airborne components through Minnaert's f = 3.26 / r (kHz, mm).
    #[test]
    fn spawned_bubble_frequencies_cover_the_documented_minnaert_span() {
        let minnaert = |radius_mm: f32| 3_260.0 / radius_mm;
        assert!((minnaert(7.0) - 466.0).abs() < 2.0 && (minnaert(1.2) - 2_717.0).abs() < 2.0);
        // spawn_bubble draws 450-900 (45 %), 900-1700 (40 %), 1700-2700 (15 %),
        // all scaled by 0.9 + 0.16 flow_norm: at flow_norm = 0 the span is
        // 405-2430 Hz, at flow_norm = 1.5 it is 1.14x higher: 513-3078 Hz.
        let low = 450.0_f32 * 0.90;
        let high = 2_700.0_f32 * (0.90 + 0.16 * 1.5);
        assert!(low > 380.0 && high < 3_200.0);
    }

    #[test]
    fn splash_and_pop_last_as_long_as_requested() {
        let sample_rate = 48_000.0;
        let mut river = RiverSource::new(Vec3::ZERO, 0.5, 0.5, 2);
        river.write_splash(0.040, 2_500.0, 1.0, sample_rate);
        let last = (1..RING_SIZE / 2)
            .rev()
            .find(|&index| river.ring[index] != 0.0)
            .unwrap();
        assert!((last as i32 - 1_920).abs() <= 3, "splash ends at {last}");
        river.ring.fill(0.0);
        river.write_pop(1.0, sample_rate);
        let last = (1..RING_SIZE / 2)
            .rev()
            .find(|&index| river.ring[index] != 0.0)
            .unwrap();
        // 1.2-4.0 ms: 57-192 samples.
        assert!((55..=195).contains(&last), "pop ends at {last}");
    }

    // ── Output filters ───────────────────────────────────────────────────

    fn sine_gain_db(
        sample_rate: f32,
        frequency_hz: f32,
        mut process: impl FnMut(f32) -> f32,
    ) -> f32 {
        let total = (sample_rate * 0.5) as usize;
        let (mut input_energy, mut output_energy) = (0.0_f64, 0.0_f64);
        for index in 0..total {
            let input = (TAU * frequency_hz * index as f32 / sample_rate).sin();
            let output = process(input);
            if index >= total / 2 {
                input_energy += (input as f64).powi(2);
                output_energy += (output as f64).powi(2);
            }
        }
        10.0 * ((output_energy / input_energy).log10() as f32)
    }

    /// `set_peak(frequency, GAIN_DB, Q, rate)`: the call sites pass
    /// (650, 1.0, 0.75) and (1450, 4.0, 0.75), i.e. +1 dB and +4 dB, both
    /// Q = 0.75 and therefore broad and gentle, despite the comment calling
    /// them "resonant ridges". If the author meant (frequency, Q, gain) the
    /// arguments are swapped; the measured gains below are what ships today.
    #[test]
    fn output_ridges_and_shelf_have_the_gains_the_call_sites_pass() {
        for sample_rate in SAMPLE_RATES {
            let mut river = RiverSource::new(Vec3::ZERO, 0.5, 0.5, 1);
            river.retune(sample_rate);
            let body = sine_gain_db(sample_rate, 650.0, |x| river.body_ridge.process(x));
            let babble = sine_gain_db(sample_rate, 1_450.0, |x| river.babble_ridge.process(x));
            let shelf = sine_gain_db(sample_rate, 12_000.0, |x| river.high_shelf.process(x));
            let well_below = sine_gain_db(sample_rate, 60.0, |x| river.high_shelf.process(x));
            assert!(
                (body - 1.0).abs() < 0.15,
                "{sample_rate}: body ridge {body:.2} dB"
            );
            assert!(
                (babble - 4.0).abs() < 0.15,
                "{sample_rate}: babble ridge {babble:.2} dB"
            );
            assert!(
                (shelf + 5.5).abs() < 0.5,
                "{sample_rate}: shelf {shelf:.2} dB at 12 kHz"
            );
            assert!(
                well_below.abs() < 0.2,
                "shelf touches 60 Hz: {well_below:.2} dB"
            );
        }
    }

    /// `retune` guards the 6.5 kHz and 12 kHz lowpasses at 0.45 Nyquist.
    #[test]
    fn retune_keeps_lowpass_corners_below_nyquist_at_low_sample_rates() {
        let mut river = RiverSource::new(Vec3::ZERO, 0.5, 0.5, 1);
        river.next_sample(8_000.0);
        let samples = render(2.0, 8_000.0, |rate| river.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(peak(&samples) < 1.5);
    }

    // ── Output statistics ────────────────────────────────────────────────

    /// The reference creek concentrates at 500-1600 Hz and falls steeply
    /// above 4 kHz (comment in `retune`).
    #[test]
    fn spectrum_is_concentrated_in_the_creek_band() {
        let mut river = RiverSource::new(Vec3::ZERO, 0.75, 0.75, 42);
        let samples = render(12.0, 48_000.0, |rate| river.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        let total = band_power(&spectrum, 48_000.0, 90.0, 24_000.0);
        let core = band_power(&spectrum, 48_000.0, 500.0, 1_600.0) / total;
        let brilliance = band_power(&spectrum, 48_000.0, 4_000.0, 24_000.0) / total;
        assert!(core > 0.5, "500-1600 Hz holds only {core:.2} of the power");
        assert!(brilliance < 0.07, "above 4 kHz holds {brilliance:.3}");
    }

    #[test]
    fn output_is_finite_zero_mean_and_inside_unity_at_every_speed() {
        for speed in [0.1_f32, 1.2, 5.0] {
            let mut river = RiverSource::new(Vec3::ZERO, speed, speed, 8);
            let samples = render(10.0, 48_000.0, |rate| river.next_sample(rate));
            assert!(samples.iter().all(|value| value.is_finite()));
            assert!(peak(&samples) < 1.0, "{speed} m/s: peak {}", peak(&samples));
            assert!(mean(&samples).abs() < 0.01, "DC {}", mean(&samples));
        }
    }

    /// Faster water is louder and brighter.
    #[test]
    fn level_rises_with_flow_speed() {
        let level = |speed: f32| {
            let mut river = RiverSource::new(Vec3::ZERO, speed, speed, 11);
            let samples = render(6.0, 24_000.0, |rate| river.next_sample(rate));
            root_mean_square(&samples)
        };
        let (trickle, stream, torrent) = (level(0.05), level(0.8), level(4.0));
        assert!(
            trickle < stream && stream < torrent,
            "{trickle} {stream} {torrent}"
        );
    }

    /// The pink-driven body of the river holds its density at a fixed
    /// frequency when the host rate changes. (Above ~4 kHz the white-noise
    /// spray layer loses ~2 dB at 96 kHz; see the cross-synth test.)
    #[test]
    fn density_at_one_kilohertz_is_sample_rate_independent() {
        let density = |sample_rate: f32| {
            let mut river = RiverSource::new(Vec3::ZERO, 0.75, 0.75, 73);
            let samples = render(8.0, sample_rate, |rate| river.next_sample(rate));
            band_density_db(
                &averaged_power_spectrum(&samples),
                sample_rate,
                900.0,
                1_100.0,
            )
        };
        let reference = density(48_000.0);
        for sample_rate in [44_100.0, 96_000.0] {
            let difference = (density(sample_rate) - reference).abs();
            assert!(
                difference < 1.0,
                "{sample_rate} Hz: {difference:.2} dB from 48 kHz"
            );
        }
    }

    // ── Parameters ───────────────────────────────────────────────────────

    #[test]
    fn constructor_and_setters_clamp_flow_speed() {
        let river = RiverSource::new(Vec3::ZERO, -2.0, 9.0, 1);
        assert_eq!((river.min_flow_speed, river.max_flow_speed), (0.0, 5.0));
        let river = RiverSource::new(Vec3::ZERO, 4.0, 1.0, 1);
        assert_eq!((river.min_flow_speed, river.max_flow_speed), (4.0, 4.0));
        let mut river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 1);
        river.set_flow_speed_range(3.0, 0.5);
        assert_eq!((river.min_flow_speed, river.max_flow_speed), (0.5, 3.0));
        river.set_flow_speed_range(-1.0, 10.0);
        assert_eq!((river.min_flow_speed, river.max_flow_speed), (0.0, 5.0));
    }

    #[test]
    fn synth_params_are_clamped_and_routed() {
        let mut river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 1);
        river.set_synth_param(SynthParam::FlowSpeedMin, 9.0);
        assert_eq!(river.min_flow_speed, 1.2);
        river.set_synth_param(SynthParam::FlowSpeedMax, 99.0);
        assert_eq!(river.max_flow_speed, 5.0);
        river.set_synth_param(SynthParam::TurbulenceDepth, 3.0);
        assert_eq!(river.eddy_depth, 1.0);
        river.set_synth_param(SynthParam::MasterGain, 7.0);
        assert_eq!(river.master_gain, 2.0);
        river.set_synth_param(SynthParam::RiverSplashRate, -1.0);
        assert_eq!(river.splash_rate, 0.0);
        river.set_synth_param(SynthParam::ChangeTimeMin, 500.0);
        assert_eq!(river.change_time_range(), (110.0, 110.0));
        river.set_synth_param(SynthParam::ChangeTimeMax, 5.0);
        assert_eq!(river.change_time_range(), (110.0, 110.0));
        river.set_synth_param(SynthParam::LowGain, 0.9);
        river.set_synth_param(SynthParam::BodyGain, 0.8);
        river.set_synth_param(SynthParam::MidGain, 0.7);
        river.set_synth_param(SynthParam::PresenceGain, 0.6);
        river.set_synth_param(SynthParam::AirGain, 0.5);
        assert_eq!(
            (
                river.body_gain,
                river.current_gain,
                river.bubble_activity,
                river.splash_gain,
                river.spray_gain
            ),
            (0.9, 0.8, 0.7, 0.6, 0.5)
        );
    }

    #[test]
    fn different_seeds_give_different_rivers() {
        let render_seed = |seed: u64| {
            let mut river = RiverSource::new(Vec3::ZERO, 0.8, 0.8, seed);
            render(0.5, 48_000.0, |rate| river.next_sample(rate))
        };
        assert_eq!(render_seed(1), render_seed(1));
        assert_ne!(render_seed(1), render_seed(2));
    }
}
