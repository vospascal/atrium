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

mod driver;
mod events;

use atrium_core::commands::SynthParam;
use atrium_core::source::{EmitterKind, SoundSource};

use crate::audio::filters::Biquad;
use atrium_core::types::Vec3;

use super::noise::{white_noise_density_scale, OnePoleHP, OnePoleLP, PinkNoiseFull, Rng};
use driver::RiverDriver;

const RING_SIZE: usize = 8192;
const RING_MASK: usize = RING_SIZE - 1;

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
            | SynthParam::MoveRest
            | SynthParam::MoveStrength
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
mod tests;
