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
use crate::world::types::Vec3;

use super::noise::{OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};

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
        let mut lp = 0.0_f32;
        let mut hp = 0.0_f32;
        let mut previous = 0.0_f32;

        for i in 0..len {
            let t = i as f32 * dt;
            let attack = (i as f32 / attack_samples).min(1.0);
            let envelope = attack * (-decay * t).exp();
            lp += lp_alpha * (self.event_rng.next_bipolar() - lp);
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
        let mut lp = 0.0_f32;
        let mut hp = 0.0_f32;
        let mut previous = 0.0_f32;

        for i in 0..len {
            let t = i as f32 * dt;
            let envelope = (-6.0 * t / duration).exp();
            lp += lp_alpha * (self.event_rng.next_bipolar() - lp);
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
        let spray = self
            .spray_lp
            .process(self.spray_hp.process(self.spray_rng.next_bipolar()));

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
        match param {
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
}
