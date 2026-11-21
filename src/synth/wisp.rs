// Wisp — a small magical flying creature: a soft wing whirr, somewhere
// between a hummingbird and a golden snitch.
//
// The wisp owns both its flight and its sound, because the sound follows the
// flight: wings beat faster and brighter in a dart, slower and darker in a
// hover. Dragging a fixed sample around would not do that.
//
// Flight (advanced once per buffer in `tick`):
//   • hover at a spot for a moment, drifting a few centimetres
//   • pick a new spot inside its territory and dart there
//   • a spring pulls it toward the target; speed is capped, so it accelerates
//     and brakes smoothly and never teleports
//
// Sound (per sample):
//   • wings — band-passed noise, pulsed at the wingbeat rate. A whirr, not a
//             buzz: a house fly is a harsh ~200 Hz sawtooth; this is noise
//             shaped by a 48–82 Hz beat.
//   • hum   — a few harmonics of the wingbeat, quiet. Gives the wings a pitch
//             for the Doppler shift to bend as it passes.
//
// Modulation is not decoration here: a steady tone is close to impossible to
// localise. The wing pulses and the noise band are the cues the ear uses to
// place the wisp in the room.
//
// Two chime layers were tried on top and removed after listening: continuous
// bell partials at 2–6 kHz were painful, and struck music-box notes were not
// wanted either. The wings alone are the sound.

use std::f32::consts::TAU;

use atrium_core::commands::SynthParam;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

use super::noise::{white_noise_density_scale, OnePoleHP, OnePoleLP, Rng};

/// Low cut of the wing-noise band (Hz).
const WING_HIGHPASS_HZ: f32 = 700.0;

/// Wingbeat rate while hovering (Hz).
const WING_RATE_HOVER_HZ: f32 = 48.0;
/// Wingbeat rate at full dart speed (Hz).
const WING_RATE_DART_HZ: f32 = 82.0;

/// How strongly the spring pulls the wisp toward its target (1/s²).
const SPRING_STIFFNESS: f32 = 14.0;
/// Velocity damping (1/s). Slightly under critical: a small overshoot reads as
/// alive, a perfectly damped stop reads as a robot.
const SPRING_DAMPING: f32 = 6.5;
/// A target counts as reached inside this distance (m).
const ARRIVAL_DISTANCE: f32 = 0.35;
/// Radius of the slow drift while hovering (m).
const HOVER_DRIFT_RADIUS: f32 = 0.12;

/// Wingbeat rate for a speed fraction in [0, 1] (0 = hovering, 1 = full dart).
pub fn wing_rate_hz(speed_fraction: f32) -> f32 {
    let speed_fraction = speed_fraction.clamp(0.0, 1.0);
    WING_RATE_HOVER_HZ + (WING_RATE_DART_HZ - WING_RATE_HOVER_HZ) * speed_fraction
}

/// Procedural wisp sound source with its own flight.
pub struct WispSource {
    // ── Flight ──────────────────────────────────────────────────────────
    /// Displacement from the anchor position (m).
    offset: Vec3,
    velocity: Vec3,
    target: Vec3,
    /// Seconds left to hover before choosing a new target.
    hover_remaining: f32,
    /// Running clock for the hover drift (s).
    flight_time: f32,
    /// Current speed as a fraction of `max_speed`, smoothed for the sound.
    speed_fraction: f32,

    // ── Sound ───────────────────────────────────────────────────────────
    wing_phase: f32,
    wing_highpass: OnePoleHP,
    /// Two one-pole stages in series (12 dB/octave). A single stage left the
    /// whirr almost white: 81 % of its energy above 3 kHz, a hiss, not wings.
    wing_lowpass: [OnePoleLP; 2],
    /// Keeps the wing white noise at its 48 kHz density per Hz.
    white_noise_scale: f32,
    sample_rate_cached: f32,

    // ── Tuning knobs ────────────────────────────────────────────────────
    /// Horizontal radius of the territory around the anchor (m).
    pub range: f32,
    /// Lowest flight height above the anchor (m).
    pub min_height: f32,
    /// Highest flight height above the anchor (m).
    pub max_height: f32,
    /// Top speed in a dart (m/s).
    pub max_speed: f32,
    /// Shortest hover between darts (s).
    pub hover_time_min: f32,
    /// Longest hover between darts (s).
    pub hover_time_max: f32,
    /// Wing whirr level.
    pub wing_gain: f32,
    /// Wingbeat hum level.
    pub hum_gain: f32,
    /// Output trim.
    pub master_gain: f32,

    /// World-space anchor: the centre of the territory at ground level.
    pub position: Vec3,

    rng: Rng,
}

impl WispSource {
    /// Create a wisp whose territory is centred on `position`.
    pub fn new(position: Vec3, seed: u64) -> Self {
        let rng = Rng::new(seed);
        let min_height = 0.6;
        let start = Vec3::new(0.0, 0.0, min_height + 0.6);
        Self {
            offset: start,
            velocity: Vec3::ZERO,
            target: start,
            hover_remaining: 1.0,
            flight_time: 0.0,
            speed_fraction: 0.0,
            wing_phase: 0.0,
            wing_highpass: OnePoleHP::new(WING_HIGHPASS_HZ, 48_000.0),
            wing_lowpass: [
                OnePoleLP::new(1800.0, 48_000.0),
                OnePoleLP::new(1800.0, 48_000.0),
            ],
            white_noise_scale: 1.0,
            sample_rate_cached: 0.0,
            range: 6.0,
            min_height,
            max_height: 3.0,
            max_speed: 7.0,
            hover_time_min: 0.8,
            hover_time_max: 3.0,
            wing_gain: 0.9,
            hum_gain: 0.18,
            master_gain: 1.0,
            position,
            rng,
        }
    }

    /// Current speed in m/s.
    pub fn speed(&self) -> f32 {
        self.velocity.length()
    }

    /// Pick a new spot inside the territory: uniform over the disc, uniform in
    /// height.
    fn choose_target(&mut self) {
        let angle = self.rng.next_f32() * TAU;
        // √ of a uniform value spreads targets evenly over the disc's area
        // instead of crowding the centre.
        let radius = self.range * self.rng.next_f32().sqrt();
        let height = self.min_height + (self.max_height - self.min_height) * self.rng.next_f32();
        self.target = Vec3::new(radius * angle.cos(), radius * angle.sin(), height);
    }
}

impl SoundSource for WispSource {
    /// What changes while it plays; any other setting is not one it has.
    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through, and one NaN would turn every later
        // sample into NaN: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        match param {
            SynthParam::MaxSpeed => self.max_speed = value.clamp(0.1, 30.0),
            SynthParam::HoverTimeMin => self.hover_time_min = value.clamp(0.0, self.hover_time_max),
            SynthParam::HoverTimeMax => self.hover_time_max = value.max(self.hover_time_min),
            SynthParam::MasterGain => self.master_gain = value.clamp(0.0, 2.0),
            _ => {}
        }
    }

    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            // The lowpasses follow the speed and are retuned every sample
            // below; the fixed low cut and the noise density follow the rate.
            self.sample_rate_cached = sample_rate;
            self.wing_highpass.set_cutoff(WING_HIGHPASS_HZ, sample_rate);
            self.white_noise_scale = white_noise_density_scale(sample_rate);
        }
        let speed_fraction = self.speed_fraction;

        // ── Wings ───────────────────────────────────────────────────────
        self.wing_phase += TAU * wing_rate_hz(speed_fraction) / sample_rate;
        if self.wing_phase >= TAU {
            self.wing_phase -= TAU;
        }
        // Raised-cosine pulse, squared: one soft "whup" per beat with a clear
        // gap between beats.
        let beat = 0.5 * (1.0 - self.wing_phase.cos());
        let wing_pulse = beat * beat;

        // Noise band: opens up (brighter) with speed.
        let cutoff = 1800.0 + 2600.0 * speed_fraction;
        let mut band = self
            .wing_highpass
            .process(self.rng.next_bipolar() * self.white_noise_scale);
        for stage in &mut self.wing_lowpass {
            stage.set_cutoff(cutoff, sample_rate);
            band = stage.process(band);
        }
        let wings = band * wing_pulse * (0.55 + 0.45 * speed_fraction);

        // ── Hum: harmonics 2–5 of the wingbeat ──────────────────────────
        let mut hum = 0.0;
        for harmonic in 2..=5 {
            hum += (self.wing_phase * harmonic as f32).sin() / harmonic as f32;
        }

        self.master_gain
            * (self.wing_gain * wings + self.hum_gain * hum * (0.4 + 0.6 * speed_fraction))
    }

    fn position(&self) -> Vec3 {
        self.position + self.offset
    }

    fn motion_offset(&self) -> Vec3 {
        self.offset
    }

    fn tick(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        self.flight_time += dt;

        // Hovering: count down, then choose somewhere new to go.
        let to_target = self.target - self.offset;
        if to_target.length() < ARRIVAL_DISTANCE {
            self.hover_remaining -= dt;
            if self.hover_remaining <= 0.0 {
                self.choose_target();
                self.hover_remaining = self.hover_time_min
                    + (self.hover_time_max - self.hover_time_min) * self.rng.next_f32();
            }
        }

        // A slow drift around the target keeps a hovering wisp from ever
        // sitting perfectly still.
        let drift = Vec3::new(
            (self.flight_time * 1.7).sin(),
            (self.flight_time * 2.3).cos(),
            (self.flight_time * 1.1).sin() * 0.5,
        ) * HOVER_DRIFT_RADIUS;
        let pull = (self.target + drift - self.offset) * SPRING_STIFFNESS;

        // Spring toward the target, damped, with the speed capped.
        self.velocity = self.velocity + (pull - self.velocity * SPRING_DAMPING) * dt;
        let speed = self.velocity.length();
        if speed > self.max_speed {
            self.velocity = self.velocity * (self.max_speed / speed);
        }
        self.offset = self.offset + self.velocity * dt;

        // Smooth the speed the sound follows (~80 ms), so the wingbeat glides
        // instead of stepping once per buffer.
        let target_fraction = (self.velocity.length() / self.max_speed.max(1e-3)).clamp(0.0, 1.0);
        let smoothing = 1.0 - (-dt / 0.08).exp();
        self.speed_fraction += (target_fraction - self.speed_fraction) * smoothing;
    }
}

#[cfg(test)]
mod tests;
