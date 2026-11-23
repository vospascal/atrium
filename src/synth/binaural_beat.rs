//! Binaural beat: two pure sine tones, one for each ear, as harmonance.com's
//! binaural-beat pieces make them (`docs/binaural-beats.md`).
//!
//! - **Two tones.** The left ear hears the carrier (108 Hz in every piece),
//!   the right ear the carrier plus the beat. Nothing else: no music, noise
//!   or pulsing. The beat is not in either tone; hearing makes it from the
//!   difference between the ears.
//! - **One voice per ear.** A [`BinauralToneSource`] is one ear's tone. The
//!   scene plays the left one as the sound and the right one as its second
//!   voice (`SoundSource::second_voice`), each held beside the head
//!   ([`Ear::offset`], `SoundSource::beside_head`), so each ear gets as much
//!   of its own tone as the 3D render allows. The render still lets both ears
//!   hear both; how much leaks is measured in `docs/binaural-beats.md`.
//! - **Fixed or a session.** A fixed piece holds its beat; a session moves it
//!   linearly from `beat` to `end_beat` over the piece's duration
//!   ([`BinauralBeatSettings::beat_at`]).
//! - **It ends.** It fades in over [`FADE_IN_SECONDS`], plays until its
//!   duration (nine hours unless set shorter), having faded out over the last
//!   [`FADE_OUT_SECONDS`], and then stays silent. It never loops, and a
//!   longer duration set after it has ended does not start it again. Each
//!   fade is at most a quarter of the duration, so a shortened piece still
//!   fades.
//!
//! Nothing steps: a change of setting moves the carrier, the beat and the
//! gain to their new course with a 50 ms time constant, and the oscillator
//! keeps its phase. Only a change glides: a session's own sweep and the fades
//! are followed exactly, so the beat is on its line at every moment. Both
//! ears' tones are computed from the same settings and the same count of
//! samples, so the right ear's tone is always the left's plus the beat. There
//! is no randomness: the same settings give the same samples.

use std::f64::consts::{PI, TAU};

use atrium_core::commands::SynthParam;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

/// Nine hours (s): how long every harmonance.com piece plays.
pub const NINE_HOURS_SECONDS: f32 = 32_400.0;
/// How far each tone is held from the centre of the head (m): straight out to
/// its own side, 3 cm from the head's surface, the nearest Brungart and
/// Rabinowitz measured a head at (1999). This close the head shadows the far
/// ear even at 110 Hz: measured on headphones, each ear hears the other's
/// tone about 21 dB below its own, against about 3 dB from a metre away
/// (`docs/binaural-beats.md`). It is also some 32 dB louder at the near ear
/// than a metre away, which the presets' `reference_spl` allows for.
pub const EAR_TONE_DISTANCE_METRES: f32 = 0.12;
/// Length of the raised-cosine fade-in when it starts (s).
pub const FADE_IN_SECONDS: f64 = 2.0;
/// Length of the raised-cosine fade-out that ends at its duration (s).
pub const FADE_OUT_SECONDS: f64 = 20.0;
/// Longest share of the duration a fade may take.
const LONGEST_FADE_SHARE: f64 = 0.25;
/// Lowest and highest carrier (Hz). Hearing makes a binaural beat from
/// carriers below about 1.5 kHz; the pieces use 108 Hz.
const CARRIER_RANGE_HZ: (f32, f32) = (20.0, 1_500.0);
/// Lowest and highest beat (Hz). The pieces run from 1.5 to 40 Hz.
const BEAT_RANGE_HZ: (f32, f32) = (0.0, 100.0);
/// Shortest and longest duration (s): a second to a day.
const DURATION_RANGE_SECONDS: (f32, f32) = (1.0, 86_400.0);
/// Amplitude of each tone at full gain, before the scene's calibration.
const AMPLITUDE: f64 = 0.1;
/// Time constant the carrier, the beat and the gain move to a new value with
/// (s).
const SETTING_GLIDE_SECONDS: f64 = 0.05;
/// Below this gain (linear), once its duration is over, it has stopped.
const SILENT_GAIN: f64 = 1e-9;

/// Which ear a tone is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ear {
    Left,
    Right,
}

impl Ear {
    /// Where this ear's tone is held, in the head's frame (metres ahead, to
    /// the left, up): [`EAR_TONE_DISTANCE_METRES`] straight out to its side.
    pub fn offset(self) -> Vec3 {
        match self {
            Ear::Left => Vec3::new(0.0, EAR_TONE_DISTANCE_METRES, 0.0),
            Ear::Right => Vec3::new(0.0, -EAR_TONE_DISTANCE_METRES, 0.0),
        }
    }
}

/// What a binaural beat is set to. Every value is kept in range by
/// [`BinauralToneSource`]; out-of-range values are clamped, not refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BinauralBeatSettings {
    /// The left ear's tone (Hz), 20 to 1500.
    pub carrier_hz: f32,
    /// The beat (Hz), 0 to 100: the right ear's tone is this much above the
    /// left's. For a session, the beat it starts at.
    pub beat_hz: f32,
    /// The beat a session ends at (Hz), 0 to 100; `None` for a fixed beat.
    pub end_beat_hz: Option<f32>,
    /// How long it plays, fades included (s), 1 to 86 400.
    pub duration_seconds: f32,
    /// Output trim, linear, 0 to 2.
    pub master_gain: f32,
}

impl Default for BinauralBeatSettings {
    /// The alpha piece: 108 Hz left, 118 Hz right, for nine hours.
    fn default() -> Self {
        Self {
            carrier_hz: 108.0,
            beat_hz: 10.0,
            end_beat_hz: None,
            duration_seconds: NINE_HOURS_SECONDS,
            master_gain: 1.0,
        }
    }
}

impl BinauralBeatSettings {
    /// The settings kept in range, as the tones play them.
    pub fn clamped(self) -> Self {
        let beat = |hz: f32| hz.clamp(BEAT_RANGE_HZ.0, BEAT_RANGE_HZ.1);
        Self {
            carrier_hz: self
                .carrier_hz
                .clamp(CARRIER_RANGE_HZ.0, CARRIER_RANGE_HZ.1),
            beat_hz: beat(self.beat_hz),
            end_beat_hz: self.end_beat_hz.map(beat),
            duration_seconds: self
                .duration_seconds
                .clamp(DURATION_RANGE_SECONDS.0, DURATION_RANGE_SECONDS.1),
            master_gain: self.master_gain.clamp(0.0, 2.0),
        }
    }

    /// The beat `elapsed_seconds` into the piece (Hz): a fixed beat
    /// throughout; a session's moved linearly from its start to its end over
    /// the duration, and held at the end after it.
    pub fn beat_at(&self, elapsed_seconds: f64) -> f64 {
        let start = self.beat_hz as f64;
        match self.end_beat_hz {
            None => start,
            Some(end) => {
                let progress = (elapsed_seconds / self.duration_seconds as f64).clamp(0.0, 1.0);
                start + (end as f64 - start) * progress
            }
        }
    }

    /// The frequency of `ear`'s tone `elapsed_seconds` into the piece (Hz):
    /// the carrier on the left, the carrier plus the beat on the right.
    pub fn ear_frequency(&self, ear: Ear, elapsed_seconds: f64) -> f64 {
        match ear {
            Ear::Left => self.carrier_hz as f64,
            Ear::Right => self.carrier_hz as f64 + self.beat_at(elapsed_seconds),
        }
    }

    /// How loud it is `elapsed_seconds` into the piece, 0 to 1, before the
    /// master gain: a raised cosine up over the fade-in, 1, a raised cosine
    /// down over the fade-out that ends at the duration, and 0 from then on.
    /// Each fade takes at most a quarter of the duration.
    pub fn envelope_at(&self, elapsed_seconds: f64) -> f64 {
        let duration = self.duration_seconds as f64;
        if !(0.0..duration).contains(&elapsed_seconds) {
            return 0.0;
        }
        let raised = |share: f64| 0.5 - 0.5 * (PI * share).cos();
        let fade_in = FADE_IN_SECONDS.min(LONGEST_FADE_SHARE * duration);
        let fade_out = FADE_OUT_SECONDS.min(LONGEST_FADE_SHARE * duration);
        let remaining = duration - elapsed_seconds;
        let rise = if elapsed_seconds < fade_in {
            raised(elapsed_seconds / fade_in)
        } else {
            1.0
        };
        let fall = if remaining < fade_out {
            raised(remaining / fade_out)
        } else {
            1.0
        };
        rise * fall
    }
}

/// Where a tone is: its carrier (Hz), its beat (Hz) and its gain (linear,
/// the envelope times the master gain).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Course {
    carrier_hz: f64,
    beat_hz: f64,
    gain: f64,
}

impl BinauralBeatSettings {
    /// Where these settings put a tone `elapsed_seconds` into the piece.
    fn course_at(&self, elapsed_seconds: f64) -> Course {
        Course {
            carrier_hz: self.carrier_hz as f64,
            beat_hz: self.beat_at(elapsed_seconds),
            gain: self.envelope_at(elapsed_seconds) * self.master_gain as f64,
        }
    }
}

/// One ear's tone of a binaural beat; see the module documentation for what
/// it makes. The other ear's tone is a second one with the same settings.
pub struct BinauralToneSource {
    ear: Ear,
    /// What it is set to.
    settings: BinauralBeatSettings,
    /// What a change of setting left to glide away, falling to 0 with a
    /// 50 ms time constant: the carrier (Hz), the beat (Hz) and the gain
    /// (linear) now are where the settings put them plus this.
    glide: Course,
    /// The carrier, the beat and the gain the last sample was made with.
    now: Course,
    /// The oscillator's phase, in cycles, in [0, 1).
    phase: f64,
    /// Seconds played before the last change of sample rate.
    elapsed_before_seconds: f64,
    /// Samples played since the last change of sample rate.
    elapsed_samples: u64,
    /// Whether it has played to its end and fallen silent, for good.
    stopped: bool,
    sample_rate_cached: f32,
    /// One-pole step per sample for the glide at the cached rate.
    glide_coefficient: f64,
}

impl BinauralToneSource {
    /// `ear`'s tone, set to `settings` from its first sample.
    pub fn new(ear: Ear, settings: BinauralBeatSettings) -> Self {
        let settings = settings.clamped();
        Self {
            ear,
            settings,
            glide: Course::default(),
            // The fade-in brings it in from silence.
            now: settings.course_at(0.0),
            phase: 0.0,
            elapsed_before_seconds: 0.0,
            elapsed_samples: 0,
            stopped: false,
            sample_rate_cached: 0.0,
            glide_coefficient: 1.0,
        }
    }

    /// Both ears' tones, set to `settings`: the left, then the right.
    pub fn pair(settings: BinauralBeatSettings) -> (Self, Self) {
        (
            Self::new(Ear::Left, settings),
            Self::new(Ear::Right, settings),
        )
    }

    fn elapsed_seconds(&self) -> f64 {
        self.elapsed_before_seconds
            + self.elapsed_samples as f64 / self.sample_rate_cached.max(1.0) as f64
    }

    /// The frequency it plays now (Hz).
    fn frequency_hz(&self) -> f64 {
        match self.ear {
            Ear::Left => self.now.carrier_hz,
            Ear::Right => self.now.carrier_hz + self.now.beat_hz,
        }
    }
}

impl SoundSource for BinauralToneSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.elapsed_before_seconds = self.elapsed_seconds();
            self.elapsed_samples = 0;
            self.sample_rate_cached = sample_rate;
            self.glide_coefficient =
                1.0 - (-1.0 / (SETTING_GLIDE_SECONDS * sample_rate as f64)).exp();
        }
        if self.stopped {
            return 0.0;
        }
        let elapsed = self.elapsed_seconds();
        let remaining = 1.0 - self.glide_coefficient;
        self.glide.carrier_hz *= remaining;
        self.glide.beat_hz *= remaining;
        self.glide.gain *= remaining;
        let course = self.settings.course_at(elapsed);
        self.now = Course {
            carrier_hz: course.carrier_hz + self.glide.carrier_hz,
            beat_hz: course.beat_hz + self.glide.beat_hz,
            gain: course.gain + self.glide.gain,
        };

        let sample = AMPLITUDE * self.now.gain * (TAU * self.phase).sin();
        self.phase = (self.phase + self.frequency_hz() / sample_rate as f64).fract();
        self.elapsed_samples += 1;
        if elapsed >= self.settings.duration_seconds as f64 && self.now.gain.abs() < SILENT_GAIN {
            self.stopped = true;
        }
        sample as f32
    }

    /// Not where it is heard: it is held beside the head
    /// ([`Self::beside_head`]), wherever the scene places it.
    fn position(&self) -> Vec3 {
        Vec3::ZERO
    }

    fn beside_head(&self) -> Option<Vec3> {
        Some(self.ear.offset())
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
            SynthParam::Pitch => settings.carrier_hz = value,
            SynthParam::Beat => settings.beat_hz = value,
            SynthParam::EndBeat => settings.end_beat_hz = Some(value),
            SynthParam::Duration => settings.duration_seconds = value,
            SynthParam::MasterGain => settings.master_gain = value,
            _ => return,
        }
        // Start the new course from where the tone is, and glide the
        // difference away.
        let settings = settings.clamped();
        let elapsed = self.elapsed_seconds();
        let course = settings.course_at(elapsed);
        self.glide = Course {
            carrier_hz: self.now.carrier_hz - course.carrier_hz,
            beat_hz: self.now.beat_hz - course.beat_hz,
            gain: self.now.gain - course.gain,
        };
        self.settings = settings;
    }
}

#[cfg(test)]
mod tests;
