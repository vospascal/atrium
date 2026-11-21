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

    use crate::synth::test_support::{
        averaged_power_spectrum, band_density_db, mean, render as render_at, root_mean_square,
    };

    /// Amplitude of the sinusoidal component at `frequency_hz`, by direct
    /// Fourier projection over a whole number of its periods.
    fn amplitude_at(samples: &[f32], sample_rate: f32, frequency_hz: f32) -> f32 {
        let (mut real, mut imaginary) = (0.0_f64, 0.0_f64);
        for (index, &value) in samples.iter().enumerate() {
            let angle = (TAU * frequency_hz * index as f32 / sample_rate) as f64;
            real += value as f64 * angle.cos();
            imaginary += value as f64 * angle.sin();
        }
        (2.0 * (real * real + imaginary * imaginary).sqrt() / samples.len() as f64) as f32
    }

    /// Hum = sum over h = 2..5 of sin(h phase) / h, scaled by
    /// hum_gain (0.4 + 0.6 speed): at rest each harmonic has amplitude
    /// 0.18 * 0.4 / h and the fundamental is absent. Frequencies are exact
    /// multiples of the 48 Hz wingbeat.
    #[test]
    fn hum_is_harmonics_two_to_five_of_the_wingbeat_with_one_over_h_amplitude() {
        for sample_rate in [44_100.0_f32, 48_000.0, 96_000.0] {
            let mut source = wisp();
            source.wing_gain = 0.0;
            let samples = render_at(2.0, sample_rate, |rate| source.next_sample(rate));
            for harmonic in 2..=5 {
                let expected = 0.18 * 0.4 / harmonic as f32;
                let measured = amplitude_at(&samples, sample_rate, 48.0 * harmonic as f32);
                assert!(
                    (measured - expected).abs() < 0.0015,
                    "{sample_rate} Hz, harmonic {harmonic}: {measured:.4}, expected {expected:.4}"
                );
            }
            assert!(
                amplitude_at(&samples, sample_rate, 48.0) < 0.001,
                "fundamental present"
            );
            assert!(
                amplitude_at(&samples, sample_rate, 72.0) < 0.001,
                "inharmonic energy"
            );
        }
    }

    /// Phase advances 2 pi f / sample_rate per sample and wraps into
    /// [0, 2 pi) without drifting: 4800 samples at 48 Hz / 48 kHz is exactly
    /// 4.8 radians x ... = 48 full cycles ... check the residual phase.
    #[test]
    fn wing_phase_advances_by_two_pi_f_over_rate_and_wraps() {
        for sample_rate in [44_100.0_f32, 48_000.0, 96_000.0] {
            let mut source = wisp();
            source.speed_fraction = 0.0;
            let samples = 10_000usize;
            for _ in 0..samples {
                source.next_sample(sample_rate);
                assert!((0.0..TAU).contains(&source.wing_phase));
            }
            let expected = (TAU as f64 * 48.0 * samples as f64 / sample_rate as f64) % TAU as f64;
            let difference = (source.wing_phase as f64 - expected).abs();
            let difference = difference.min(TAU as f64 - difference);
            assert!(
                difference < 0.02,
                "{sample_rate} Hz: phase {} vs {expected}",
                source.wing_phase
            );
        }
    }

    /// Power of the rectified envelope at `rate_hz` (Fourier projection).
    fn envelope_power(samples: &[f32], sample_rate: f32, rate_hz: f32) -> f64 {
        let envelope: Vec<f32> = samples.iter().map(|value| value.abs()).collect();
        let level = envelope.iter().sum::<f32>() / envelope.len() as f32;
        let centred: Vec<f32> = envelope.iter().map(|value| value - level).collect();
        let amplitude = amplitude_at(&centred, sample_rate, rate_hz) as f64;
        amplitude * amplitude
    }

    /// The wing pulse sits at the wingbeat rate at every host rate: 48 Hz
    /// hovering, 82 Hz in a dart, and not at neighbouring frequencies.
    #[test]
    fn wing_pulse_rate_is_independent_of_sample_rate() {
        for sample_rate in [44_100.0_f32, 96_000.0] {
            for (speed_fraction, expected_hz) in [(0.0, 48.0), (1.0, 82.0)] {
                let mut source = wisp();
                source.hum_gain = 0.0;
                source.speed_fraction = speed_fraction;
                let samples = render_at(1.0, sample_rate, |rate| source.next_sample(rate));
                let at_rate = envelope_power(&samples, sample_rate, expected_hz);
                for neighbour in [expected_hz - 9.0, expected_hz + 9.0] {
                    assert!(
                        at_rate > 8.0 * envelope_power(&samples, sample_rate, neighbour),
                        "{sample_rate} Hz, speed {speed_fraction}: pulse not at {expected_hz} Hz"
                    );
                }
            }
        }
    }

    /// The whirr is a band-passed noise: highpass at 700 Hz, two lowpass
    /// stages at 1.8 kHz (hover). Below 700 Hz it is a 6 dB/oct skirt. Were
    /// the highpass left at its 48 kHz construction, at 96 kHz its corner
    /// would silently become 1.4 kHz (measured before the fix: the 300-600 Hz
    /// vs 2-3 kHz balance moved by 3.6 dB at a fixed wing speed).
    #[test]
    fn whirr_low_cut_is_sample_rate_independent() {
        let low_to_mid_db = |sample_rate: f32| {
            let mut source = wisp();
            source.hum_gain = 0.0;
            source.speed_fraction = 0.5;
            // Hold the wings steady: no pulse, just the band-passed noise.
            let mut samples = Vec::new();
            for _ in 0..(6.0 * sample_rate) as usize {
                source.wing_phase = std::f32::consts::PI; // cos = -1: pulse = 1
                samples.push(source.next_sample(sample_rate));
            }
            let spectrum = averaged_power_spectrum(&samples);
            band_density_db(&spectrum, sample_rate, 300.0, 600.0)
                - band_density_db(&spectrum, sample_rate, 2_000.0, 3_000.0)
        };
        let difference = (low_to_mid_db(96_000.0) - low_to_mid_db(48_000.0)).abs();
        assert!(
            difference < 1.5,
            "low-cut moved the 300-600 Hz balance by {difference:.1} dB"
        );
    }

    /// Same measurement at 48 kHz: the one-pole 700 Hz highpass and the
    /// two-pole 1.8 kHz lowpass shape a band: strongest around 1-2.5 kHz,
    /// about 7 dB down at 100-200 Hz and 20+ dB down at 8-12 kHz.
    #[test]
    fn whirr_is_a_band_peaking_in_the_low_kilohertz() {
        let mut source = wisp();
        source.hum_gain = 0.0;
        source.speed_fraction = 0.0;
        let mut samples = Vec::new();
        for _ in 0..(6.0 * SAMPLE_RATE) as usize {
            source.wing_phase = std::f32::consts::PI;
            samples.push(source.next_sample(SAMPLE_RATE));
        }
        let spectrum = averaged_power_spectrum(&samples);
        let low = band_density_db(&spectrum, SAMPLE_RATE, 100.0, 200.0);
        let peak_band = band_density_db(&spectrum, SAMPLE_RATE, 1_000.0, 2_500.0);
        let high = band_density_db(&spectrum, SAMPLE_RATE, 8_000.0, 12_000.0);
        assert!(peak_band > low + 5.0, "peak {peak_band:.1} vs low {low:.1}");
        assert!(
            peak_band > high + 15.0,
            "peak {peak_band:.1} vs high {high:.1}"
        );
    }

    /// Targets are uniform over the disc's area (radius = range sqrt(u)) and
    /// uniform in height: E[r^2] = range^2 / 2, E[height] = mid height.
    #[test]
    fn new_targets_are_uniform_over_the_territory() {
        let mut source = wisp();
        let draws = 40_000;
        let (mut radius_squared, mut height_sum, mut x_sum, mut y_sum) =
            (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
        for _ in 0..draws {
            source.choose_target();
            let target = source.target;
            let horizontal = (target.x * target.x + target.y * target.y) as f64;
            assert!(horizontal.sqrt() <= source.range as f64 + 1e-4);
            assert!(target.z >= source.min_height && target.z <= source.max_height);
            radius_squared += horizontal;
            height_sum += target.z as f64;
            x_sum += target.x as f64;
            y_sum += target.y as f64;
        }
        let draws = draws as f64;
        let expected_radius_squared = (source.range as f64).powi(2) / 2.0;
        assert!((radius_squared / draws / expected_radius_squared - 1.0).abs() < 0.03);
        assert!(
            (height_sum / draws - 1.8).abs() < 0.03,
            "mean height {}",
            height_sum / draws
        );
        assert!(
            (x_sum / draws).abs() < 0.08 && (y_sum / draws).abs() < 0.08,
            "targets lean to one side"
        );
    }

    /// Semi-implicit Euler: velocity is updated first, then the offset moves
    /// by the new velocity x dt, exactly.
    #[test]
    fn position_integrates_velocity_exactly_each_tick() {
        let mut source = wisp();
        for _ in 0..2_000 {
            let before = source.offset;
            source.tick(BUFFER_SECONDS);
            let moved = source.offset - before;
            let expected = source.velocity * BUFFER_SECONDS;
            assert!(moved.distance_to(expected) < 1e-5);
        }
    }

    /// From rest a far target pulls with k d = 14 x 6 = 84 m/s^2 against
    /// damping 6.5 v: the uncapped terminal speed is 12.9 m/s, so the 7 m/s cap
    /// is what limits a dart, and the dart saturates exactly at the cap.
    #[test]
    fn dart_speed_saturates_at_max_speed() {
        let mut source = wisp();
        source.offset = Vec3::new(0.0, 0.0, 1.2);
        source.target = Vec3::new(6.0, 0.0, 1.2);
        source.hover_remaining = 1.0e9;
        let mut fastest = 0.0_f32;
        for _ in 0..200 {
            source.tick(BUFFER_SECONDS);
            fastest = fastest.max(source.speed());
        }
        assert!(
            (fastest - source.max_speed).abs() < 1e-3,
            "top speed {fastest}"
        );
    }

    /// Arrival: damping ratio zeta = 6.5 / (2 sqrt 14) = 0.87, so the spring
    /// is slightly under-damped and overshoots by only exp(-pi zeta /
    /// sqrt(1 - zeta^2)) = 0.4 % of the move (no visible bounce).
    #[test]
    fn spring_is_slightly_underdamped() {
        let zeta = SPRING_DAMPING / (2.0 * SPRING_STIFFNESS.sqrt());
        assert!((zeta - 0.868).abs() < 0.001);
        assert!(zeta < 1.0 && zeta > 0.8);
        let overshoot = (-std::f32::consts::PI * zeta / (1.0 - zeta * zeta).sqrt()).exp();
        assert!((overshoot - 0.0041).abs() < 0.0005, "overshoot {overshoot}");
    }

    /// After a settling move the wisp ends up within the hover drift
    /// (0.12 m x sqrt 3) of its target whatever the control rate.
    #[test]
    fn settled_position_does_not_depend_on_the_tick_rate() {
        let settle = |dt: f32| {
            let mut source = wisp();
            source.offset = Vec3::new(0.0, 0.0, 1.2);
            source.target = Vec3::new(3.0, -2.0, 2.0);
            source.hover_remaining = 1.0e9;
            for _ in 0..(4.0 / dt) as usize {
                source.tick(dt);
            }
            source.offset
        };
        let reference = settle(0.005);
        for dt in [0.01, 0.02, 0.05] {
            let offset = settle(dt);
            assert!(
                offset.distance_to(reference) < 0.25,
                "dt {dt}: {offset:?} vs {reference:?}"
            );
            assert!(offset.distance_to(Vec3::new(3.0, -2.0, 2.0)) < 0.4);
        }
    }

    /// Every hover lasts hover_time_min..max seconds (the countdown only runs
    /// while within the arrival distance), so the interval between new targets
    /// is at least the shortest hover.
    #[test]
    fn hovers_last_at_least_the_minimum_hover_time() {
        let mut source = wisp();
        let mut last_target = source.target;
        let mut last_change = 0.0_f32;
        let mut clock = 0.0_f32;
        let mut intervals = Vec::new();
        for _ in 0..(300.0 / BUFFER_SECONDS) as usize {
            source.tick(BUFFER_SECONDS);
            clock += BUFFER_SECONDS;
            if source.target.distance_to(last_target) > 1e-6 {
                intervals.push(clock - last_change);
                last_change = clock;
                last_target = source.target;
            }
        }
        assert!(
            intervals.len() > 30,
            "only {} darts in 300 s",
            intervals.len()
        );
        let shortest = intervals.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            shortest >= source.hover_time_min - 0.02,
            "shortest {shortest}"
        );
        let mean_interval = intervals.iter().sum::<f32>() / intervals.len() as f32;
        // 1.9 s mean hover plus travel.
        assert!((2.0..6.0).contains(&mean_interval), "mean {mean_interval}");
    }

    #[test]
    fn tick_ignores_non_positive_time_steps_and_survives_huge_ones() {
        let mut source = wisp();
        let before = (source.offset, source.velocity);
        source.tick(0.0);
        source.tick(-1.0);
        assert_eq!((source.offset, source.velocity), before);
        for _ in 0..400 {
            source.tick(0.25);
            assert!(source.offset.x.is_finite() && source.speed() <= source.max_speed + 1e-3);
        }
        let anchor = source.position;
        assert!((source.position() - anchor - source.motion_offset()).length() < 1e-6);
    }

    #[test]
    fn rms_is_roughly_stable_across_sample_rates() {
        let rms = |sample_rate: f32| {
            let mut source = wisp();
            source.speed_fraction = 0.3;
            let samples = render_at(4.0, sample_rate, |rate| source.next_sample(rate));
            (root_mean_square(&samples), mean(&samples))
        };
        let (low, _) = rms(48_000.0);
        let (high, dc) = rms(96_000.0);
        // Measured 1.00 (it read 0.86 before the wing noise kept its density
        // per Hz and the highpass was retuned).
        assert!(
            (0.7..1.3).contains(&(high / low)),
            "RMS ratio {}",
            high / low
        );
        assert!(dc.abs() < 0.01);
    }

    /// What changes while it flies reaches it: its top speed and how long it
    /// hovers, each kept in range, and master gain 0 silences it.
    #[test]
    fn its_speed_hovering_and_gain_change_while_it_flies() {
        use atrium_core::commands::SynthParam;
        use atrium_core::source::SoundSource;
        let mut wisp = WispSource::new(Vec3::ZERO, 5);
        wisp.set_synth_param(SynthParam::MaxSpeed, 2.0);
        wisp.set_synth_param(SynthParam::HoverTimeMin, 0.2);
        wisp.set_synth_param(SynthParam::HoverTimeMax, 0.1);
        assert_eq!(
            (wisp.max_speed, wisp.hover_time_min, wisp.hover_time_max),
            (2.0, 0.2, 0.2),
            "the longest hover is never shorter than the shortest"
        );
        for _ in 0..240_000 {
            wisp.next_sample(48_000.0);
            wisp.tick(1.0 / 48_000.0);
            assert!(
                wisp.speed() <= 2.0 + 1e-3,
                "it flies faster than it was told"
            );
        }
        wisp.set_synth_param(SynthParam::MasterGain, 0.0);
        let energy: f32 = (0..9_600).map(|_| wisp.next_sample(48_000.0).powi(2)).sum();
        assert_eq!(energy, 0.0);
    }
}
