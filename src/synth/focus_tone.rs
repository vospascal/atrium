//! Focus tone: a sustained tone at a set pitch, with soft overtones and a slow
//! shimmer that wanders.
//!
//! It gives its sound as two layers, so a mixer can set them apart:
//!
//! - **fundamental**: the pitch itself, plus the shimmer partial that beats
//!   against it;
//! - **overtones**: harmonics 2, 3, 4 … at whole multiples of the pitch.
//!   Harmonic `n` has amplitude `overtone_level · brightness^(n − 1)`, so each
//!   is `brightness` times the one below it (0.35: −9.1 dB per harmonic).
//!
//! No partial is made at or above the ceiling, `min(0.95 · Nyquist, 20 kHz)`,
//! so nothing folds back from above Nyquist; the last tenth below the ceiling
//! fades a partial out with a raised cosine, so one that a pitch change pushes
//! across it fades instead of switching off.
//!
//! The shimmer is a second partial a fraction of a hertz above the pitch, at
//! `shimmer_depth` times the fundamental's amplitude. The two beat: the
//! fundamental layer swells and fades once every `1 / beat` seconds, between
//! `1 − depth` and `1 + depth` of its level. The beat rate glides from one
//! seeded value to the next inside `[shimmer_rate_min, shimmer_rate_max]`,
//! one glide every 6 to 14 s, so it never settles into a fixed pulse.
//!
//! Nothing steps: the tone fades in over half a second, every setting moves to
//! a new value with a 50 ms time constant, and the oscillators keep their
//! phase when the pitch changes. The same seed gives the same samples.

use std::f32::consts::{PI, TAU};

use atrium_core::commands::SynthParam;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

use super::noise::Rng;

/// Lowest and highest pitch it plays (Hz).
const PITCH_RANGE_HZ: (f32, f32) = (20.0, 5_000.0);
/// Highest harmonic it makes. At the default brightness (0.35) harmonic 14
/// is already 120 dB below the overtone level.
const HIGHEST_HARMONIC: u32 = 32;
/// Harmonics fainter than this (linear, before the layer level) are not
/// computed: 120 dB down, far below anything heard.
const FAINTEST_HARMONIC: f32 = 1e-6;
/// Highest brightness: each harmonic at most 0.9 of the one below it.
const BRIGHTEST: f32 = 0.9;
/// Fastest shimmer beat (Hz). Above a few hertz a beat is heard as a
/// flutter, not a shimmer.
const FASTEST_SHIMMER_HZ: f32 = 5.0;
/// Highest frequency any partial reaches, whatever the sample rate (Hz).
const HIGHEST_PARTIAL_HZ: f32 = 20_000.0;
/// Share of the ceiling where a partial starts to fade out.
const CEILING_FADE_START: f32 = 0.9;
/// Time constant every setting moves to a new value with (s).
const SETTING_GLIDE_SECONDS: f32 = 0.05;
/// Length of the raised-cosine fade-in when it starts (s).
const ONSET_SECONDS: f64 = 0.5;
/// Shortest and longest glide from one shimmer rate to the next (s).
const SHIMMER_GLIDE_SECONDS: (f32, f32) = (6.0, 14.0);

/// What a focus tone is set to. Every value is kept in range by
/// [`FocusToneSource`]; out-of-range values are clamped, not refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusToneSettings {
    /// The pitch (Hz), 20 to 5000.
    pub pitch_hz: f32,
    /// Each harmonic's amplitude as a share of the one below it, 0 to 0.9.
    pub brightness: f32,
    /// Level of the fundamental layer, linear, 0 to 2.
    pub fundamental_level: f32,
    /// Level of the overtone layer, linear, 0 to 2: harmonic 2's amplitude
    /// is `overtone_level · brightness`.
    pub overtone_level: f32,
    /// Slowest and fastest the shimmer beats (Hz), 0 to 5.
    pub shimmer_rate_min_hz: f32,
    pub shimmer_rate_max_hz: f32,
    /// The shimmer partial's amplitude as a share of the fundamental's, 0 to 1.
    pub shimmer_depth: f32,
    /// Output trim, 0 to 2.
    pub master_gain: f32,
}

impl Default for FocusToneSettings {
    fn default() -> Self {
        Self {
            pitch_hz: 333.0,
            brightness: 0.35,
            fundamental_level: 0.2,
            overtone_level: 0.2,
            shimmer_rate_min_hz: 0.1,
            shimmer_rate_max_hz: 0.4,
            shimmer_depth: 0.3,
            master_gain: 1.0,
        }
    }
}

impl FocusToneSettings {
    fn clamped(self) -> Self {
        let shimmer_rate_min_hz = self.shimmer_rate_min_hz.clamp(0.0, FASTEST_SHIMMER_HZ);
        Self {
            pitch_hz: self.pitch_hz.clamp(PITCH_RANGE_HZ.0, PITCH_RANGE_HZ.1),
            brightness: self.brightness.clamp(0.0, BRIGHTEST),
            fundamental_level: self.fundamental_level.clamp(0.0, 2.0),
            overtone_level: self.overtone_level.clamp(0.0, 2.0),
            shimmer_rate_min_hz,
            shimmer_rate_max_hz: self
                .shimmer_rate_max_hz
                .clamp(shimmer_rate_min_hz, FASTEST_SHIMMER_HZ),
            shimmer_depth: self.shimmer_depth.clamp(0.0, 1.0),
            master_gain: self.master_gain.clamp(0.0, 2.0),
        }
    }
}

/// The shimmer's beat rate: a raised-cosine glide from one seeded rate to the
/// next, each drawn uniformly inside the range. A convex blend of two rates in
/// the range stays in it.
struct ShimmerDrift {
    rng: Rng,
    from_hz: f32,
    to_hz: f32,
    elapsed_seconds: f64,
    glide_seconds: f64,
}

impl ShimmerDrift {
    fn new(seed: u64, minimum_hz: f32, maximum_hz: f32) -> Self {
        let mut rng = Rng::new(seed);
        let from_hz = minimum_hz + (maximum_hz - minimum_hz) * rng.next_f32();
        let to_hz = minimum_hz + (maximum_hz - minimum_hz) * rng.next_f32();
        let glide_seconds = Self::draw_glide_seconds(&mut rng);
        Self {
            rng,
            from_hz,
            to_hz,
            elapsed_seconds: 0.0,
            glide_seconds,
        }
    }

    fn draw_glide_seconds(rng: &mut Rng) -> f64 {
        let (shortest, longest) = SHIMMER_GLIDE_SECONDS;
        (shortest + (longest - shortest) * rng.next_f32()) as f64
    }

    /// The rate now (Hz).
    fn rate_hz(&self) -> f32 {
        let progress = (self.elapsed_seconds / self.glide_seconds) as f32;
        let blend = 0.5 - 0.5 * (PI * progress).cos();
        self.from_hz + (self.to_hz - self.from_hz) * blend
    }

    fn advance(&mut self, seconds: f64, minimum_hz: f32, maximum_hz: f32) {
        self.elapsed_seconds += seconds;
        if self.elapsed_seconds >= self.glide_seconds {
            self.elapsed_seconds -= self.glide_seconds;
            self.from_hz = self.to_hz;
            self.to_hz = minimum_hz + (maximum_hz - minimum_hz) * self.rng.next_f32();
            self.glide_seconds = Self::draw_glide_seconds(&mut self.rng);
        }
    }

    /// Keep both ends of the glide inside a new range, so the rate is in it
    /// from the next sample on.
    fn confine(&mut self, minimum_hz: f32, maximum_hz: f32) {
        self.from_hz = self.from_hz.clamp(minimum_hz, maximum_hz);
        self.to_hz = self.to_hz.clamp(minimum_hz, maximum_hz);
    }
}

/// A sustained tone with overtones and a wandering shimmer; see the module
/// documentation for what it makes.
pub struct FocusToneSource {
    /// Where each setting is heading.
    target: FocusToneSettings,
    /// Where each setting is now, one 50 ms glide behind the target.
    current: FocusToneSettings,
    /// Fundamental and shimmer phase, in cycles, in [0, 1).
    fundamental_phase: f64,
    shimmer_phase: f64,
    drift: ShimmerDrift,
    onset_elapsed_seconds: f64,
    position: Vec3,
    sample_rate_cached: f32,
    /// One-pole step per sample for the setting glide at the cached rate.
    glide_coefficient: f32,
    /// Ceiling no partial reaches at the cached rate (Hz).
    ceiling_hz: f32,
}

impl FocusToneSource {
    /// A tone at `position`, set to `settings` from its first sample. `seed`
    /// sets the shimmer's starting phase and its wandering.
    pub fn new(position: Vec3, settings: FocusToneSettings, seed: u64) -> Self {
        let settings = settings.clamped();
        let mut phase_rng = Rng::new(seed);
        let shimmer_phase = phase_rng.next_f32() as f64;
        Self {
            target: settings,
            current: settings,
            fundamental_phase: 0.0,
            shimmer_phase,
            drift: ShimmerDrift::new(
                seed.wrapping_add(1),
                settings.shimmer_rate_min_hz,
                settings.shimmer_rate_max_hz,
            ),
            onset_elapsed_seconds: 0.0,
            position,
            sample_rate_cached: 0.0,
            glide_coefficient: 1.0,
            ceiling_hz: HIGHEST_PARTIAL_HZ,
        }
    }

    fn retune(&mut self, sample_rate: f32) {
        self.glide_coefficient = 1.0 - (-1.0 / (SETTING_GLIDE_SECONDS * sample_rate)).exp();
        self.ceiling_hz = (0.95 * 0.5 * sample_rate).min(HIGHEST_PARTIAL_HZ);
    }

    /// Weight of a partial at `frequency_hz`: 1 below the fade, 0 at and
    /// above the ceiling, a raised cosine between.
    fn ceiling_weight(&self, frequency_hz: f32) -> f32 {
        let fade_start = CEILING_FADE_START * self.ceiling_hz;
        if frequency_hz <= fade_start {
            1.0
        } else if frequency_hz >= self.ceiling_hz {
            0.0
        } else {
            0.5 + 0.5 * (PI * (frequency_hz - fade_start) / (self.ceiling_hz - fade_start)).cos()
        }
    }

    fn glide_settings(&mut self) {
        let step = self.glide_coefficient;
        let glide = |current: &mut f32, target: f32| *current += (target - *current) * step;
        glide(&mut self.current.pitch_hz, self.target.pitch_hz);
        glide(&mut self.current.brightness, self.target.brightness);
        glide(
            &mut self.current.fundamental_level,
            self.target.fundamental_level,
        );
        glide(&mut self.current.overtone_level, self.target.overtone_level);
        glide(&mut self.current.shimmer_depth, self.target.shimmer_depth);
        glide(&mut self.current.master_gain, self.target.master_gain);
    }

    /// The fundamental layer and the overtone layer, this sample, before the
    /// onset fade and the master gain.
    fn layers(&self) -> (f32, f32) {
        let pitch_hz = self.current.pitch_hz;
        let fundamental = self.ceiling_weight(pitch_hz)
            * (TAU * self.fundamental_phase as f32).sin()
            + self.current.shimmer_depth
                * self.ceiling_weight(pitch_hz + self.drift.rate_hz())
                * (TAU * self.shimmer_phase as f32).sin();

        let mut overtones = 0.0;
        let mut amplitude = self.current.brightness;
        for harmonic in 2..=HIGHEST_HARMONIC {
            let frequency_hz = harmonic as f32 * pitch_hz;
            if frequency_hz >= self.ceiling_hz || amplitude < FAINTEST_HARMONIC {
                break;
            }
            let phase = (harmonic as f64 * self.fundamental_phase).fract() as f32;
            overtones += amplitude * self.ceiling_weight(frequency_hz) * (TAU * phase).sin();
            amplitude *= self.current.brightness;
        }
        (
            self.current.fundamental_level * fundamental,
            self.current.overtone_level * overtones,
        )
    }

    /// The shimmer's beat rate now (Hz).
    #[cfg(test)]
    fn shimmer_rate_hz(&self) -> f32 {
        self.drift.rate_hz()
    }
}

impl SoundSource for FocusToneSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.retune(sample_rate);
        }
        self.glide_settings();

        let (fundamental, overtones) = self.layers();
        let onset = if self.onset_elapsed_seconds < ONSET_SECONDS {
            0.5 - 0.5 * (std::f64::consts::PI * self.onset_elapsed_seconds / ONSET_SECONDS).cos()
        } else {
            1.0
        } as f32;
        let sample = (fundamental + overtones) * onset * self.current.master_gain;

        let seconds = 1.0 / sample_rate as f64;
        self.onset_elapsed_seconds = (self.onset_elapsed_seconds + seconds).min(ONSET_SECONDS);
        let pitch_hz = self.current.pitch_hz as f64;
        self.fundamental_phase = (self.fundamental_phase + pitch_hz * seconds).fract();
        self.shimmer_phase =
            (self.shimmer_phase + (pitch_hz + self.drift.rate_hz() as f64) * seconds).fract();
        self.drift.advance(
            seconds,
            self.target.shimmer_rate_min_hz,
            self.target.shimmer_rate_max_hz,
        );
        sample
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    // All time-varying state advances on the audio sample clock above.
    fn tick(&mut self, _dt: f32) {}

    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through, and one NaN would turn every later
        // sample into NaN: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        let mut target = self.target;
        match param {
            SynthParam::Pitch => target.pitch_hz = value,
            SynthParam::Brightness => target.brightness = value,
            SynthParam::FundamentalLevel => target.fundamental_level = value,
            SynthParam::OvertoneLevel => target.overtone_level = value,
            SynthParam::ShimmerRateMin => {
                target.shimmer_rate_min_hz = value.min(target.shimmer_rate_max_hz)
            }
            SynthParam::ShimmerRateMax => {
                target.shimmer_rate_max_hz = value.max(target.shimmer_rate_min_hz)
            }
            SynthParam::ShimmerDepth => target.shimmer_depth = value,
            SynthParam::MasterGain => target.master_gain = value,
            _ => return,
        }
        self.target = target.clamped();
        self.drift.confine(
            self.target.shimmer_rate_min_hz,
            self.target.shimmer_rate_max_hz,
        );
    }
}

#[cfg(test)]
mod tests;
