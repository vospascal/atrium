//! Focus tone: an octave stack, in the way of harmonance.com's "Harmonics"
//! sound (`docs/focus-tones.md`, "How harmonance.com builds its Harmonics
//! sound").
//!
//! - **Lines.** The named pitch, then up to three octaves below it, none
//!   below the floor (60 Hz by default), as harmonance.com's Harmonics page
//!   describes it: 528 Hz plays 528, 264, 132 and 66 Hz; 333 Hz plays 333,
//!   166.5 and 83.25 Hz; 111 Hz plays alone. At most [`MOST_LINES`] lines. The
//!   named pitch always plays, even below the floor.
//! - **A level per frequency.** Each line's level follows its own frequency,
//!   whatever its place in the stack, on a curve read from harmonance.com's
//!   own page data (see [`line_level_db`]): the lower the line, the higher its
//!   level. The site says each line is set to sound as loud as the others;
//!   this curve is how its pages do it. It is their choice, measured from
//!   their pages, not a loudness standard, and puts more bass in than equal
//!   loudness contours (ISO 226) would.
//! - **The emphasis rotates.** Each line's gain swings log-sinusoidally,
//!   `gain_dB = (depth / 2) · cos(2π t / period − 2π · index / lines)`, index
//!   0 the named pitch, the lines spread evenly round one cycle (24 s and
//!   8 dB by default): the weight passes from the named pitch down through
//!   each octave, then back to the pitch.
//!
//! Nothing steps: it fades in over half a second, the pitch and every line's
//! gain move to a new value with a 50 ms time constant (a line's level for
//! its frequency follows the gliding frequency), and each line's oscillator
//! keeps its phase. A line that a change adds or removes fades in or
//! out over that glide. No line is made at or above the ceiling,
//! `min(0.95 · Nyquist, 20 kHz)`; the last tenth below it fades a line out
//! with a raised cosine. There is no randomness: the same settings give the
//! same samples.

use std::f32::consts::PI;

use atrium_core::commands::SynthParam;
use atrium_core::layers::LayerMix;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::types::Vec3;

/// Most lines a stack has: the named pitch and three octaves below it.
pub const MOST_LINES: usize = 4;
/// Lowest and highest pitch it plays (Hz). Below 40 Hz the level curve,
/// extended, asks for more than 14 dB over the 83.25 Hz line, and most
/// speakers barely play it.
const PITCH_RANGE_HZ: (f32, f32) = (40.0, 5_000.0);
/// Lowest and highest floor (Hz), for the same reason.
const FLOOR_RANGE_HZ: (f32, f32) = (40.0, 5_000.0);
/// Shortest and longest rotation (s).
const ROTATION_PERIOD_RANGE_SECONDS: (f32, f32) = (1.0, 600.0);
/// Deepest rotation, low to high (dB).
const DEEPEST_ROTATION_DB: f32 = 24.0;
/// Amplitude of a line at the curve's reference, 83.25 Hz, before the
/// rotation and the master gain.
const REFERENCE_AMPLITUDE: f32 = 0.1;
/// The level curve: a line's level (dB) against the 83.25 Hz line, at the
/// frequencies harmonance.com's 333 Hz and 666 Hz pages play. Read 2026-11-22
/// from both pages' data (`docs/focus-tones.md`): the 166.5 Hz and 333 Hz
/// lines are the mean of the two pages (−13.2 and −13.4; −23.6 and −24.0 dB),
/// the 666 Hz line is on the 666 Hz page alone.
const LEVEL_CURVE: [(f32, f32); 4] = [(83.25, 0.0), (166.5, -13.3), (333.0, -23.8), (666.0, -30.8)];
/// Highest frequency any line reaches, whatever the sample rate (Hz).
const HIGHEST_LINE_HZ: f32 = 20_000.0;
/// Share of the ceiling where a line starts to fade out.
const CEILING_FADE_START: f32 = 0.9;
/// Time constant the pitch and every line's amplitude move to a new value
/// with (s).
const SETTING_GLIDE_SECONDS: f32 = 0.05;
/// Length of the raised-cosine fade-in when it starts (s).
const ONSET_SECONDS: f64 = 0.5;
/// A line whose gain is below this (linear) and that is not in the stack is
/// not computed: it has finished fading out.
const SILENT_GAIN: f32 = 1e-9;

/// A line's level (dB) against a line at 83.25 Hz, from harmonance.com's
/// measured page data ([`LEVEL_CURVE`]): linear in log2(frequency) between the
/// measured lines, the end segments' slopes extended below 83.25 Hz
/// (−13.3 dB per octave, so a 66 Hz line is 4.5 dB over the 83.25 Hz one)
/// and above 666 Hz (−7.0 dB per octave).
pub fn line_level_db(frequency_hz: f32) -> f32 {
    let octave = |frequency: f32| (frequency / LEVEL_CURVE[0].0).log2();
    let position = octave(frequency_hz);
    let last = LEVEL_CURVE.len() - 1;
    // The segment it falls in, or the end segment it extends.
    let segment = (1..last)
        .find(|&index| position < octave(LEVEL_CURVE[index].0))
        .unwrap_or(last)
        - 1;
    let (low_hz, low_db) = LEVEL_CURVE[segment];
    let (high_hz, high_db) = LEVEL_CURVE[segment + 1];
    let (low, high) = (octave(low_hz), octave(high_hz));
    low_db + (high_db - low_db) * (position - low) / (high - low)
}

/// How many lines a stack has: the named pitch, then each octave below it
/// that is at or above the floor, at most [`MOST_LINES`] in all.
pub fn line_count(pitch_hz: f32, floor_hz: f32) -> usize {
    let mut lines = 1;
    while lines < MOST_LINES && pitch_hz / (1u32 << lines) as f32 >= floor_hz {
        lines += 1;
    }
    lines
}

/// What a focus tone is set to. Every value is kept in range by
/// [`FocusToneSource`]; out-of-range values are clamped, not refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusToneSettings {
    /// The named pitch, the stack's top line (Hz), 40 to 5000.
    pub pitch_hz: f32,
    /// The lowest a line below the pitch may go (Hz), 40 to 5000.
    pub floor_hz: f32,
    /// How long the emphasis takes to go once round the lines (s), 1 to 600.
    pub rotation_period_seconds: f32,
    /// How far each line's gain swings, low to high (dB), 0 to 24.
    pub rotation_depth_db: f32,
    /// Output trim, linear, 0 to 2.
    pub master_gain: f32,
}

impl Default for FocusToneSettings {
    fn default() -> Self {
        Self {
            pitch_hz: 333.0,
            floor_hz: 60.0,
            rotation_period_seconds: 24.0,
            rotation_depth_db: 8.0,
            master_gain: 1.0,
        }
    }
}

impl FocusToneSettings {
    /// The settings kept in range, as the tone plays them.
    pub fn clamped(self) -> Self {
        Self {
            pitch_hz: self.pitch_hz.clamp(PITCH_RANGE_HZ.0, PITCH_RANGE_HZ.1),
            floor_hz: self.floor_hz.clamp(FLOOR_RANGE_HZ.0, FLOOR_RANGE_HZ.1),
            rotation_period_seconds: self.rotation_period_seconds.clamp(
                ROTATION_PERIOD_RANGE_SECONDS.0,
                ROTATION_PERIOD_RANGE_SECONDS.1,
            ),
            rotation_depth_db: self.rotation_depth_db.clamp(0.0, DEEPEST_ROTATION_DB),
            master_gain: self.master_gain.clamp(0.0, 2.0),
        }
    }
}

/// An octave stack with a rotating emphasis; see the module documentation for
/// what it makes.
pub struct FocusToneSource {
    /// What it is set to; the pitch and the line gains glide to it.
    settings: FocusToneSettings,
    /// The named pitch now, one 50 ms glide behind the setting (Hz).
    pitch_hz: f32,
    /// Each line's oscillator phase, in cycles, in [0, 1). Line `index` is
    /// at `pitch / 2^index`.
    line_phases: [f64; MOST_LINES],
    /// Each line's gain now (rotation, master gain, and 0 once it has left
    /// the stack), one 50 ms glide behind its target.
    line_gains: [f32; MOST_LINES],
    /// Where the rotation is, in cycles, in [0, 1).
    rotation_phase: f64,
    onset_elapsed_seconds: f64,
    position: Vec3,
    sample_rate_cached: f32,
    /// One-pole step per sample for the glide at the cached rate.
    glide_coefficient: f32,
    /// Ceiling no line reaches at the cached rate (Hz).
    ceiling_hz: f32,
}

impl FocusToneSource {
    /// A tone at `position`, set to `settings` from its first sample.
    pub fn new(position: Vec3, settings: FocusToneSettings) -> Self {
        let settings = settings.clamped();
        let mut source = Self {
            settings,
            pitch_hz: settings.pitch_hz,
            line_phases: [0.0; MOST_LINES],
            line_gains: [0.0; MOST_LINES],
            rotation_phase: 0.0,
            onset_elapsed_seconds: 0.0,
            position,
            sample_rate_cached: 0.0,
            glide_coefficient: 1.0,
            ceiling_hz: HIGHEST_LINE_HZ,
        };
        // Every line starts at its level: the onset fade brings it in.
        source.line_gains = source.target_gains();
        source
    }

    fn retune(&mut self, sample_rate: f32) {
        self.glide_coefficient = 1.0 - (-1.0 / (SETTING_GLIDE_SECONDS * sample_rate)).exp();
        self.ceiling_hz = (0.95 * 0.5 * sample_rate).min(HIGHEST_LINE_HZ);
    }

    /// Weight of a line at `frequency_hz`: 1 below the fade, 0 at and above
    /// the ceiling, a raised cosine between.
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

    /// Where each line's gain is heading: the rotation and the master gain;
    /// 0 for a line not in the stack. The level for the line's frequency is
    /// not in it (see [`Self::line_amplitude`]).
    fn target_gains(&self) -> [f32; MOST_LINES] {
        let lines = line_count(self.settings.pitch_hz, self.settings.floor_hz);
        let half_depth_db = 0.5 * self.settings.rotation_depth_db;
        let mut targets = [0.0; MOST_LINES];
        for (index, target) in targets.iter_mut().enumerate().take(lines) {
            let rotation_db = half_depth_db
                * (std::f64::consts::TAU * (self.rotation_phase - index as f64 / lines as f64))
                    .cos() as f32;
            *target = 10.0_f32.powf(rotation_db / 20.0) * self.settings.master_gain;
        }
        targets
    }

    /// Line `index`'s amplitude now: the curve's level and the ceiling's
    /// weight at the frequency it has now, times its gliding gain. The level
    /// follows the frequency at once, not after a glide of its own: on every
    /// segment the curve falls faster than 6 dB per octave, so while the pitch
    /// glides a line's amplitude times its frequency (how steeply it moves)
    /// stays between its values at the two ends. A level that trailed its
    /// frequency would let a low, loud line rise in pitch before it falls in
    /// level, and step harder than either end.
    fn line_amplitude(&self, index: usize) -> f32 {
        let frequency_hz = self.pitch_hz / (1u32 << index) as f32;
        REFERENCE_AMPLITUDE
            * 10.0_f32.powf(line_level_db(frequency_hz) / 20.0)
            * self.ceiling_weight(frequency_hz)
            * self.line_gains[index]
    }

    /// How many lines the stack has at its settings.
    #[cfg(test)]
    fn line_count(&self) -> usize {
        line_count(self.settings.pitch_hz, self.settings.floor_hz)
    }

    /// Jump the pitch and every gain to where they are heading, as if the
    /// glide had finished.
    #[cfg(test)]
    fn settle(&mut self) {
        self.pitch_hz = self.settings.pitch_hz;
        self.line_gains = self.target_gains();
    }
}

impl SoundSource for FocusToneSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        self.next_mixed(sample_rate, &mut LayerMix::untouched())
    }

    /// One layer per line of the stack, the named pitch first, then each octave below it.
    fn layer_count(&self) -> usize {
        line_count(self.settings.pitch_hz, self.settings.floor_hz)
    }

    #[inline]
    fn next_mixed(&mut self, sample_rate: f32, mix: &mut LayerMix) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.retune(sample_rate);
        }
        let step = self.glide_coefficient;
        self.pitch_hz += (self.settings.pitch_hz - self.pitch_hz) * step;

        let targets = self.target_gains();
        let seconds = 1.0 / sample_rate as f64;
        let mut lines = [0.0; MOST_LINES];
        for (index, target) in targets.into_iter().enumerate() {
            let gain = &mut self.line_gains[index];
            *gain += (target - *gain) * step;
            if target == 0.0 && *gain < SILENT_GAIN {
                *gain = 0.0;
                continue;
            }
            let amplitude = self.line_amplitude(index);
            let phase = &mut self.line_phases[index];
            lines[index] = amplitude * (std::f64::consts::TAU * *phase).sin() as f32;
            let frequency_hz = self.pitch_hz as f64 / (1u32 << index) as f64;
            *phase = (*phase + frequency_hz * seconds).fract();
        }
        let sample = mix.sum(0, &lines);

        let onset = if self.onset_elapsed_seconds < ONSET_SECONDS {
            0.5 - 0.5 * (std::f64::consts::PI * self.onset_elapsed_seconds / ONSET_SECONDS).cos()
        } else {
            1.0
        } as f32;
        self.onset_elapsed_seconds = (self.onset_elapsed_seconds + seconds).min(ONSET_SECONDS);
        self.rotation_phase =
            (self.rotation_phase + seconds / self.settings.rotation_period_seconds as f64).fract();
        sample * onset
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    /// Heard everywhere, like a wind: a tone to sit in, not one to find. A
    /// scene's `shape.ambient: false` makes it a point.
    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }

    // All time-varying state advances on the audio sample clock above.
    fn tick(&mut self, _dt: f32) {}

    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through, and one NaN would turn every later
        // sample into NaN: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        let mut settings = self.settings;
        match param {
            SynthParam::Pitch => settings.pitch_hz = value,
            SynthParam::Floor => settings.floor_hz = value,
            SynthParam::RotationPeriod => settings.rotation_period_seconds = value,
            SynthParam::RotationDepth => settings.rotation_depth_db = value,
            SynthParam::MasterGain => settings.master_gain = value,
            _ => return,
        }
        self.settings = settings.clamped();
    }
}

#[cfg(test)]
mod tests;
