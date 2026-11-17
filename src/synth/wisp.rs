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

use crate::world::types::Vec3;
use atrium_core::source::SoundSource;

use super::noise::{OnePoleHP, OnePoleLP, Rng};

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
            wing_highpass: OnePoleHP::new(700.0, 48_000.0),
            wing_lowpass: [
                OnePoleLP::new(1800.0, 48_000.0),
                OnePoleLP::new(1800.0, 48_000.0),
            ],
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
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
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
        let mut band = self.wing_highpass.process(self.rng.next_bipolar());
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
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;
    /// One audio buffer of 480 frames at 48 kHz.
    const BUFFER_SECONDS: f32 = 0.01;

    fn wisp() -> WispSource {
        WispSource::new(Vec3::new(10.0, 20.0, 0.0), 7)
    }

    /// Fly for `seconds`, calling `visit` after every buffer.
    fn fly(source: &mut WispSource, seconds: f32, mut visit: impl FnMut(&WispSource)) {
        let buffers = (seconds / BUFFER_SECONDS) as usize;
        for _ in 0..buffers {
            source.tick(BUFFER_SECONDS);
            visit(source);
        }
    }

    /// Dominant amplitude-modulation rate of a signal: the frequency between
    /// 30 and 120 Hz (in 0.1 Hz steps) with the most energy in its rectified
    /// envelope. A direct Fourier projection; an autocorrelation peak wandered
    /// by ±1.5 Hz on the noise.
    fn modulation_rate_hz(samples: &[f32]) -> f32 {
        let envelope: Vec<f32> = samples.iter().map(|x| x.abs()).collect();
        let mean = envelope.iter().sum::<f32>() / envelope.len() as f32;
        let mut best_rate = 0.0f32;
        let mut best_power = f32::MIN;
        for step in 300..=1200 {
            let rate = step as f32 * 0.1;
            let (mut real, mut imaginary) = (0.0f64, 0.0f64);
            for (index, value) in envelope.iter().enumerate() {
                let angle = (TAU * rate * index as f32 / SAMPLE_RATE) as f64;
                real += (value - mean) as f64 * angle.cos();
                imaginary += (value - mean) as f64 * angle.sin();
            }
            let power = (real * real + imaginary * imaginary) as f32;
            if power > best_power {
                best_power = power;
                best_rate = rate;
            }
        }
        best_rate
    }

    fn render(source: &mut WispSource, seconds: f32) -> Vec<f32> {
        (0..(seconds * SAMPLE_RATE) as usize)
            .map(|_| source.next_sample(SAMPLE_RATE))
            .collect()
    }

    #[test]
    fn wing_rate_runs_from_hover_to_dart() {
        assert_eq!(wing_rate_hz(0.0), 48.0);
        assert_eq!(wing_rate_hz(1.0), 82.0);
        assert_eq!(wing_rate_hz(0.5), 65.0);
        // Out-of-range speeds are clamped, not extrapolated.
        assert_eq!(wing_rate_hz(-1.0), 48.0);
        assert_eq!(wing_rate_hz(3.0), 82.0);
    }

    /// The audible wing pulse really is at the wingbeat rate: 48 Hz hovering,
    /// 82 Hz at full speed. Measured on the rendered signal, with the hum
    /// off so only the wings remain.
    #[test]
    fn wing_pulse_matches_wingbeat_rate_at_hover_and_dart() {
        for (speed_fraction, expected_hz) in [(0.0, 48.0), (1.0, 82.0)] {
            let mut source = wisp();
            source.hum_gain = 0.0;
            source.speed_fraction = speed_fraction;
            let samples = render(&mut source, 2.0);
            let measured = modulation_rate_hz(&samples);
            assert!(
                (measured - expected_hz).abs() < 1.0,
                "speed {speed_fraction}: measured {measured:.1} Hz, expected {expected_hz} Hz"
            );
        }
    }

    /// A dart is brighter than a hover. Zero-crossing rate is a simple proxy
    /// for where the energy sits: more crossings per second = brighter.
    #[test]
    fn wings_are_brighter_in_a_dart() {
        let crossings_per_second = |speed_fraction: f32| -> f32 {
            let mut source = wisp();
            source.hum_gain = 0.0;
            source.speed_fraction = speed_fraction;
            let samples = render(&mut source, 2.0);
            let crossings = samples
                .windows(2)
                .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
                .count();
            crossings as f32 / 2.0
        };
        let hover = crossings_per_second(0.0);
        let dart = crossings_per_second(1.0);
        eprintln!("wing brightness: hover {hover:.0} crossings/s, dart {dart:.0} crossings/s");
        assert!(
            dart > hover * 1.25,
            "dart ({dart:.0}/s) should be clearly brighter than hover ({hover:.0}/s)"
        );
    }

    /// Ten minutes of flight stays inside the territory: within `range`
    /// horizontally (plus overshoot and hover drift) and between the height
    /// limits.
    #[test]
    fn flight_stays_inside_its_territory() {
        let mut source = wisp();
        let anchor = source.position;
        let margin = 0.6;
        let (range, min_height, max_height) = (source.range, source.min_height, source.max_height);
        fly(&mut source, 600.0, |wisp| {
            let offset = wisp.position() - anchor;
            let horizontal = (offset.x * offset.x + offset.y * offset.y).sqrt();
            assert!(horizontal <= range + margin, "strayed to {horizontal:.2} m");
            assert!(
                offset.z >= min_height - margin && offset.z <= max_height + margin,
                "height {:.2} m outside [{min_height}, {max_height}]",
                offset.z
            );
        });
    }

    /// The wisp never exceeds its top speed and never jumps: each buffer moves
    /// it at most max_speed × dt.
    #[test]
    fn flight_is_continuous_and_speed_limited() {
        let mut source = wisp();
        let max_speed = source.max_speed;
        let mut previous = source.position();
        fly(&mut source, 300.0, |wisp| {
            let step = wisp.position().distance_to(previous);
            assert!(
                step <= max_speed * BUFFER_SECONDS + 1e-4,
                "moved {step:.4} m in one buffer"
            );
            assert!(wisp.speed() <= max_speed + 1e-4);
            previous = wisp.position();
        });
    }

    /// It both hovers and darts, and it covers its territory instead of
    /// loitering in one corner.
    #[test]
    fn flight_alternates_hovering_and_darting_across_the_territory() {
        let mut source = wisp();
        let anchor = source.position;
        let mut hovering_buffers = 0usize;
        let mut darting_buffers = 0usize;
        let mut quadrants = [false; 4];
        let mut total = 0usize;
        let max_speed = source.max_speed;
        fly(&mut source, 300.0, |wisp| {
            total += 1;
            if wisp.speed() < 0.5 {
                hovering_buffers += 1;
            }
            if wisp.speed() > 0.5 * max_speed {
                darting_buffers += 1;
            }
            let offset = wisp.position() - anchor;
            let quadrant = (offset.x >= 0.0) as usize + 2 * (offset.y >= 0.0) as usize;
            quadrants[quadrant] = true;
        });
        let hovering = hovering_buffers as f32 / total as f32;
        let darting = darting_buffers as f32 / total as f32;
        assert!(
            hovering > 0.2,
            "hovered only {:.0}% of the time",
            hovering * 100.0
        );
        assert!(
            darting > 0.05,
            "darted only {:.0}% of the time",
            darting * 100.0
        );
        assert!(
            quadrants.iter().all(|seen| *seen),
            "quadrants visited: {quadrants:?}"
        );
    }

    /// The sound follows the flight: the speed the synth hears tracks the real
    /// speed, reaching most of the way up in a dart and falling back in a hover.
    #[test]
    fn sound_speed_follows_flight_speed() {
        let mut source = wisp();
        let mut highest = 0.0f32;
        let mut lowest = 1.0f32;
        fly(&mut source, 120.0, |wisp| {
            highest = highest.max(wisp.speed_fraction);
            lowest = lowest.min(wisp.speed_fraction);
            assert!((0.0..=1.0).contains(&wisp.speed_fraction));
        });
        assert!(highest > 0.7, "never sounded like a dart: {highest:.2}");
        assert!(lowest < 0.1, "never sounded like a hover: {lowest:.2}");
    }

    /// Output stays inside ±1 and finite through a minute of real flight.
    #[test]
    fn output_is_bounded_while_flying() {
        let mut source = wisp();
        let mut peak = 0.0f32;
        let mut energy = 0.0f32;
        for _ in 0..6000 {
            source.tick(BUFFER_SECONDS);
            for _ in 0..480 {
                let sample = source.next_sample(SAMPLE_RATE);
                assert!(sample.is_finite());
                peak = peak.max(sample.abs());
                energy += sample * sample;
            }
        }
        assert!(peak <= 1.0, "peak {peak}");
        assert!(energy > 0.0, "wisp rendered silence");
    }

    #[test]
    fn same_seed_gives_the_same_flight_and_sound() {
        let run = |seed: u64| -> (Vec3, f32) {
            let mut source = WispSource::new(Vec3::ZERO, seed);
            let mut sum = 0.0f32;
            for _ in 0..500 {
                source.tick(BUFFER_SECONDS);
                for _ in 0..480 {
                    sum += source.next_sample(SAMPLE_RATE);
                }
            }
            (source.position(), sum)
        };
        let (position_a, sum_a) = run(11);
        let (position_b, sum_b) = run(11);
        let (position_c, _) = run(12);
        assert_eq!(position_a.x, position_b.x);
        assert_eq!(sum_a, sum_b);
        assert!(
            position_a.distance_to(position_c) > 1e-3,
            "seeds should differ"
        );
    }
}
