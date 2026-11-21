//! The wavering itself: one path's level and treble coming and going as the
//! theory says they do.
//!
//! The paper this follows renders the amplitude fluctuations with a
//! time-variant high-shelving filter; so does this. The sound is split at
//! [`SPLIT_HZ`] into a low and a high part with first-order filters that sum
//! back to the input exactly, and each part gets its own slowly wandering
//! gain. Both follow one wander: the same eddies move the whole spectrum, the
//! treble further. The gain is log-normal, exp(σ n − σ²) for a unit Gaussian
//! n, whose mean square is 1: the air moves the level about and takes no
//! energy away.
//!
//! The theory's deviation grows steadily with frequency, and two parts can
//! only follow it so far. The parts' own deviations are solved so that a
//! tone at [`LOW_HZ`] and one at [`HIGH_HZ`] get exactly the theory's values;
//! with the split at 2.4 kHz the mix between is then within an eighth of the
//! theory. Measured on tones, the filter is within 15 % for a sound 40 m off
//! in a breeze, and up to a fifth short in a gale at 60 m, where the level
//! swings by 4 dB and more and two parts are no longer enough.

use atrium_core::types::Vec3;

use super::{log_amplitude_variance, surface_layer, wavering_seconds, AirMotion};

/// The two frequencies the deviation is worked out at (Hz), and where the
/// sound is split between them: the split that keeps the frequencies between
/// closest to the theory.
pub(super) const LOW_HZ: f32 = 500.0;
pub(super) const HIGH_HZ: f32 = 6000.0;
const SPLIT_HZ: f32 = 2400.0;
/// The wander moves on once every this many samples, and the gains glide
/// between: it is far slower than this, and an exponential per sample for
/// every path would be wasted work.
const CONTROL_SAMPLES: u32 = 32;
/// A gain this close to 1 has come home (a thousandth of a dB).
const HOME: f32 = 1e-4;
/// Celsius to kelvin.
const KELVIN: f32 = 273.15;

/// One path through the air, from a sound to where it is heard.
#[derive(Clone, Copy, Debug)]
pub struct AirPath<'a> {
    pub air: &'a AirMotion,
    /// Whether the path lies under the open sky; indoors nothing wavers.
    pub open_air: bool,
    pub from: Vec3,
    pub to: Vec3,
    /// Height of the ground (m, the same axis as the positions' z).
    pub ground: f32,
    /// Length of the path (m); longer than from–to for a reflection.
    pub distance: f32,
    pub temperature_c: f32,
    pub speed_of_sound: f32,
}

/// How a path wavers: the standard deviation of its log-amplitude (nepers)
/// at 500 Hz and at 6 kHz, and how long the level holds (s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wavering {
    pub low: f32,
    pub high: f32,
    pub seconds: f32,
}

impl Wavering {
    pub const NONE: Self = Self {
        low: 0.0,
        high: 0.0,
        seconds: 1.0,
    };

    /// What the theory gives for `path`.
    pub fn along(path: &AirPath) -> Self {
        if !path.open_air || path.air.is_still() || path.distance <= 0.0 {
            return Self::NONE;
        }
        let temperature = path.temperature_c + KELVIN;
        let height = (path.from.z + path.to.z) / 2.0 - path.ground;
        let layer = surface_layer(path.air, height, temperature);
        let deviation = |frequency_hz: f32| {
            log_amplitude_variance(
                &layer,
                frequency_hz,
                path.distance,
                path.speed_of_sound,
                temperature,
            )
            .sqrt()
        };
        Self {
            low: deviation(LOW_HZ),
            high: deviation(HIGH_HZ),
            seconds: wavering_seconds(&layer, path.distance, path.speed_of_sound, crosswind(path)),
        }
    }
}

/// The share of the wind that blows across `path`, 0 to 1: the sine of the
/// angle between them over the ground. A path straight up or down has all
/// of it.
fn crosswind(path: &AirPath) -> f32 {
    let (east, north) = (path.to.x - path.from.x, path.to.y - path.from.y);
    let length = east.hypot(north);
    if length < 1e-3 {
        return 1.0;
    }
    let [wind_east, wind_north] = path.air.wind_toward;
    ((east * wind_north - north * wind_east) / length)
        .abs()
        .min(1.0)
}

/// The filter that makes one path waver.
pub struct AirWaver {
    random: u64,
    /// Two smoothing stages of the wander, so it has no corners.
    first: f32,
    second: f32,
    /// Their coefficient per control step, and what brings the result back
    /// to unit variance.
    pole: f32,
    scale: f32,
    wavering: Wavering,
    low_gain: f32,
    high_gain: f32,
    low_step: f32,
    high_step: f32,
    until_control: u32,
    /// The parts' own deviations, solved from the two the theory gives.
    low_part: f32,
    high_part: f32,
    split: f32,
    lowpass: f32,
    /// False while nothing wavers: the sound then passes untouched.
    active: bool,
}

impl AirWaver {
    /// A waver whose wander is its own: two with different seeds never move
    /// together.
    pub fn new(seed: u64) -> Self {
        Self {
            // Never zero, which a xorshift cannot leave.
            random: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            first: 0.0,
            second: 0.0,
            pole: 1.0,
            scale: 1.0,
            wavering: Wavering::NONE,
            low_gain: 1.0,
            high_gain: 1.0,
            low_step: 0.0,
            high_step: 0.0,
            until_control: 0,
            low_part: 0.0,
            high_part: 0.0,
            split: 1.0,
            lowpass: 0.0,
            active: false,
        }
    }

    /// Waver as `wavering` says from now on, at `sample_rate`. Called once a
    /// buffer.
    pub fn set(&mut self, wavering: Wavering, sample_rate: f32) {
        self.wavering = wavering;
        if wavering.low > 0.0 || wavering.high > 0.0 {
            self.active = true;
        }
        let control_seconds = CONTROL_SAMPLES as f32 / sample_rate;
        self.pole = 1.0 - (-control_seconds / wavering.seconds.max(control_seconds)).exp();
        // Two equal one-pole stages fed white noise of variance 1 give
        // a⁴ (1 + b²) / (1 − b²)³, with b = 1 − a.
        let keep = 1.0 - self.pole;
        let variance =
            self.pole.powi(4) * (1.0 + keep * keep) / (1.0 - keep * keep).powi(3).max(1e-12);
        self.scale = 1.0 / variance.sqrt();
        self.split = split_coefficient(sample_rate);
        // A tone at frequency f gets s·low_part + (1 − s)·high_part, with s
        // the share of it in the low part. Asking the theory's values at the
        // two frequencies gives two equations for the two parts.
        let at_low = low_share(LOW_HZ, sample_rate);
        let at_high = low_share(HIGH_HZ, sample_rate);
        self.high_part =
            ((wavering.high * at_low - wavering.low * at_high) / (at_low - at_high)).max(0.0);
        self.low_part = ((wavering.low - (1.0 - at_low) * self.high_part) / at_low).max(0.0);
    }

    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        if !self.active {
            return sample;
        }
        if self.until_control == 0 {
            self.wander();
            self.until_control = CONTROL_SAMPLES;
        }
        self.until_control -= 1;
        self.low_gain += self.low_step;
        self.high_gain += self.high_step;
        self.lowpass += self.split * (sample - self.lowpass);
        self.low_gain * self.lowpass + self.high_gain * (sample - self.lowpass)
    }

    /// Start over: still, and the wander back at its middle.
    pub fn reset(&mut self) {
        self.first = 0.0;
        self.second = 0.0;
        self.low_gain = 1.0;
        self.high_gain = 1.0;
        self.low_step = 0.0;
        self.high_step = 0.0;
        self.until_control = 0;
        self.lowpass = 0.0;
        self.active = false;
    }

    /// Move the wander on one control step and aim the gains at it.
    fn wander(&mut self) {
        let (low, high) = (self.low_part, self.high_part);
        if low <= 0.0 && high <= 0.0 {
            // The air has stilled: glide home, and once there switch off, so
            // the sound passes untouched again.
            let home = |gain: f32| (gain - 1.0).abs() < HOME;
            if home(self.low_gain) && home(self.high_gain) {
                self.low_gain = 1.0;
                self.high_gain = 1.0;
                self.low_step = 0.0;
                self.high_step = 0.0;
                self.active = false;
            } else {
                self.low_step = (1.0 - self.low_gain) / CONTROL_SAMPLES as f32;
                self.high_step = (1.0 - self.high_gain) / CONTROL_SAMPLES as f32;
            }
            return;
        }
        let noise = self.gaussian();
        self.first += self.pole * (noise - self.first);
        self.second += self.pole * (self.first - self.second);
        let wander = self.second * self.scale;
        // exp(σ n − σ²): log-normal, with a mean square of exactly 1.
        let low_target = (low * wander - low * low).exp();
        let high_target = (high * wander - high * high).exp();
        self.low_step = (low_target - self.low_gain) / CONTROL_SAMPLES as f32;
        self.high_step = (high_target - self.high_gain) / CONTROL_SAMPLES as f32;
    }

    /// Close to a unit Gaussian: four uniform draws, centred and scaled.
    /// The smoothing after it adds up many, which finishes the job.
    fn gaussian(&mut self) -> f32 {
        let mut sum = 0.0;
        for _ in 0..4 {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 7;
            self.random ^= self.random << 17;
            sum += (self.random >> 40) as f32 / (1u64 << 24) as f32;
        }
        (sum - 2.0) * 3f32.sqrt()
    }
}

/// The split's one-pole coefficient at `sample_rate`.
fn split_coefficient(sample_rate: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * SPLIT_HZ / sample_rate).exp()
}

/// The share of a tone at `frequency_hz` that lies in the low part: the real
/// part of the split's low-pass there, a / (1 − (1 − a) e^(−jω)).
pub(super) fn low_share(frequency_hz: f32, sample_rate: f32) -> f32 {
    let coefficient = split_coefficient(sample_rate);
    let keep = 1.0 - coefficient;
    let turn = std::f32::consts::TAU * frequency_hz / sample_rate;
    let (real, imaginary) = (1.0 - keep * turn.cos(), keep * turn.sin());
    coefficient * real / (real * real + imaginary * imaginary)
}
