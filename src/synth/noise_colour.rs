//! Noise colours: steady noise with a named spectrum, each made from its
//! definition, which its own file states: the published one where there is
//! one, and where there is none, Atrium's choice, labelled as such.
//!
//! Every colour is a power spectral density per hertz over the audible band,
//! 20 Hz to 20 kHz, and is made the same at every host rate: corners are in
//! hertz, and white noise is scaled so its density per hertz does not depend
//! on the rate (`noise::white_noise_density_scale`).
//!
//! | colour | density per Hz | made by |
//! |---|---|---|
//! | white | flat | white noise ([`white`]) |
//! | pink | 1/f, −3 dB/oct | Paul Kellet's seven-pole filter ([`pink`]) |
//! | red | 1/f², −6 dB/oct (Brownian) | integrated white, 20 Hz high-pass ([`red`]) |
//! | brown | 1/f² under a 285 Hz low-pass, after an example | white shaped in blocks ([`brown`]) |
//! | blue | rising, flat above 5 kHz, after an example | white shaped in blocks ([`blue`]) |
//! | violet | f², +6 dB/oct | differentiated white ([`violet`]) |
//! | grey | more bass, a dip near 2 kHz, after an example | white shaped in blocks ([`grey`]) |
//! | green | 1/f^1.45 under a 1640 Hz low-pass, after an example | white shaped in blocks ([`green`]) |
//! | orange | dark brown and red, after an example | two layers shaped in blocks ([`orange`]) |
//!
//! In physics red and Brownian noise are two names for 1/f²; that is red
//! here. Brown, blue and grey follow the example recordings Atrium was
//! given rather than their textbook definitions, fitted; each file gives
//! both and says how they differ. Green and orange have no agreed
//! definition; Atrium's follow the example recordings too. Each colour leaves
//! its generator at an RMS of about 0.1 at 48 kHz; a preset's
//! `reference_spl` sets how loud it is heard.
//!
//! Nothing moves: the noise is stationary, with no drift or variation. It
//! fades in over half a second, and a new master gain glides there with a
//! 50 ms time constant. Every colour is one layer except orange, whose deep
//! and bright parts are two. The same seed gives the same samples.

mod blue;
mod brown;
mod dispatch;
mod green;
mod grey;
mod orange;
mod pink;
mod red;
mod shaped;
mod violet;
mod white;

use atrium_core::commands::SynthParam;
use atrium_core::layers::LayerMix;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::types::Vec3;
use serde::Deserialize;

use dispatch::Generator;

/// Which spectrum the noise has. A preset names it as `colour:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoiseColour {
    White,
    Pink,
    /// Darker than Brownian noise, after an example recording.
    Brown,
    /// Brownian noise, 1/f².
    Red,
    Blue,
    Violet,
    Grey,
    Green,
    Orange,
}

/// The names of orange's two layers: deep (dark brown) then bright (red).
pub const ORANGE_LAYERS: [&str; 2] = ["deep", "bright"];

/// Time constant a new master gain glides to its value with (s).
const GAIN_GLIDE_SECONDS: f32 = 0.05;
/// Length of the raised-cosine fade-in when it starts (s).
const ONSET_SECONDS: f64 = 0.5;
/// Highest master gain, linear.
const LOUDEST_MASTER_GAIN: f32 = 2.0;
/// Closer than this to its target, the gliding gain takes the target
/// itself: in `f32` a one-pole glide stalls some 10⁻⁵ short of it, where a
/// step rounds to nothing. The last 10⁻⁴ (−80 dB) is taken in one step.
const GAIN_SNAP: f32 = 1e-4;

/// What a noise colour is set to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseColourSettings {
    pub colour: NoiseColour,
    /// Output trim, linear, 0 to 2; out-of-range values are clamped.
    pub master_gain: f32,
}

impl Default for NoiseColourSettings {
    fn default() -> Self {
        Self {
            colour: NoiseColour::White,
            master_gain: 1.0,
        }
    }
}

/// Steady noise of one colour; see the module documentation.
pub struct NoiseColourSource {
    generator: Generator,
    /// Where the gain is heading.
    master_gain: f32,
    /// The gain now, one 50 ms glide behind `master_gain`.
    gain: f32,
    onset_elapsed_seconds: f64,
    position: Vec3,
    sample_rate_cached: f32,
    /// One-pole step per sample of the gain glide at the cached rate.
    glide_coefficient: f32,
}

impl NoiseColourSource {
    /// Noise of `settings.colour` at `position`, its randomness from `seed`.
    pub fn new(position: Vec3, settings: NoiseColourSettings, seed: u64) -> Self {
        let master_gain = clamped_gain(settings.master_gain);
        Self {
            generator: Generator::new(settings.colour, seed),
            master_gain,
            gain: master_gain,
            onset_elapsed_seconds: 0.0,
            position,
            sample_rate_cached: 0.0,
            glide_coefficient: 1.0,
        }
    }
}

fn clamped_gain(gain: f32) -> f32 {
    gain.clamp(0.0, LOUDEST_MASTER_GAIN)
}

impl SoundSource for NoiseColourSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        self.next_mixed(sample_rate, &mut LayerMix::untouched())
    }

    /// One layer, or two for orange: its deep and its bright part.
    fn layer_count(&self) -> usize {
        self.generator.layer_count()
    }

    #[inline]
    fn next_mixed(&mut self, sample_rate: f32, mix: &mut LayerMix) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.glide_coefficient = 1.0 - (-1.0 / (GAIN_GLIDE_SECONDS * sample_rate)).exp();
        }
        self.gain += (self.master_gain - self.gain) * self.glide_coefficient;
        if (self.master_gain - self.gain).abs() < GAIN_SNAP {
            self.gain = self.master_gain;
        }

        let layers = self.generator.next_layers(sample_rate);
        let sample = mix.sum(0, &layers[..self.generator.layer_count()]);

        let onset = if self.onset_elapsed_seconds < ONSET_SECONDS {
            0.5 - 0.5 * (std::f64::consts::PI * self.onset_elapsed_seconds / ONSET_SECONDS).cos()
        } else {
            1.0
        } as f32;
        self.onset_elapsed_seconds =
            (self.onset_elapsed_seconds + 1.0 / sample_rate as f64).min(ONSET_SECONDS);
        sample * self.gain * onset
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    /// Heard all around, not from a point: noise to sit in. A scene's
    /// `shape.ambient: false` makes it a point.
    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }

    // Everything advances on the audio sample clock above.
    fn tick(&mut self, _dt: f32) {}

    /// The master gain only: a preset's colour is fixed.
    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through, and one NaN would turn every later
        // sample into NaN: keep the previous value instead.
        if param == SynthParam::MasterGain && value.is_finite() {
            self.master_gain = clamped_gain(value);
        }
    }
}

#[cfg(test)]
mod tests;
