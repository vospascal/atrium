// Procedural thunder: a strike (crack + tail) followed by a rolling rumble.
//
// Port of ThunderStrikeProcessor.ts and ThunderRumbleProcessor.ts, joined into
// one clap. Algorithm per clap:
//   • Crack (30–50 ms): half-sine gated white noise plus three quickly
//     decaying resonators near 500 Hz, 1.7 kHz and 3 kHz. Lightning usually
//     has several return strokes, so a clap may crack up to three times.
//   • Tail (preset `tail_seconds`): 0.7·brown body + 0.3·hiss under a
//     quadratic fade. Crack and tail pass a one-pole lowpass whose corner
//     falls with distance: far strikes are dull, near strikes bright.
//   • Rumble (preset `rumble_seconds`): infra (< 25 Hz) ground shake + brown
//     body + hiss through a high shelf at 3 kHz (−12 dB at distance 1), under
//     a raised-cosine 10 % fade in and out, with slow random "rolls". It
//     starts halfway through the strike's tail and rises under it.
//
// Everything a clap varies (crack length, return strokes, resonator pitch,
// tail and rumble length, rolls) is drawn from the source's seeded generator
// at the moment the clap starts, so no two claps are alike and the same seed
// gives the same claps.
//
// The source is silent until told to strike: a plan sends it `Utterance`s
// (take = preset) and it keeps them on the session's sample clock, as
// `crate::audio::voice_node::VoiceNode` keeps its takes.

use std::f32::consts::{PI, TAU};

use atrium_core::commands::SynthParam;
use atrium_core::source::{SoundSource, Utterance};
use atrium_core::types::Vec3;

use super::noise::{white_noise_density_scale, BrownNoise, OnePoleHP, OnePoleLP, PinkNoise, Rng};
use crate::audio::filters::Biquad;

/// Preset numbers, as `Utterance::take` names them.
pub const PRESET_NEAR: u8 = 0;
pub const PRESET_MID: u8 = 1;
pub const PRESET_FAR: u8 = 2;
pub const PRESET_MASSIVE: u8 = 3;

/// How many claps may sound at once. A third steals the oldest.
const VOICES: usize = 2;
/// How many strikes the source holds ahead.
const APPOINTMENTS: usize = 8;
/// A stolen clap fades out over this long before the new one starts, so the
/// cut rumble never clicks.
const STEAL_RELEASE_SECONDS: f32 = 0.010;

/// Crack length is drawn between these (the TS used a fixed 40 ms).
const CRACK_MIN_SECONDS: f32 = 0.030;
const CRACK_MAX_SECONDS: f32 = 0.050;
/// Resonator time constant: the TS multiplied the amplitude by 0.995 every
/// sample at 48 kHz, which is −1 / (48 000 · ln 0.995) = 4.16 ms.
const RESONATOR_TIME_CONSTANT_SECONDS: f32 = 0.004_158;
/// The TS resonator frequencies; each clap moves each by up to ±20 %.
const RESONATOR_FREQUENCIES_HZ: [f32; 3] = [500.0, 1_700.0, 3_000.0];
/// Later return strokes arrive this long after the one before.
const RETURN_STROKE_GAP_MIN_SECONDS: f32 = 0.040;
const RETURN_STROKE_GAP_MAX_SECONDS: f32 = 0.160;

/// The rumble starts this far into the strike's tail.
const RUMBLE_ONSET_TAIL_FRACTION: f32 = 0.5;
/// Fraction of the rumble spent fading in, and again fading out.
const RUMBLE_FADE_FRACTION: f32 = 0.1;
/// Corner and depth of the rumble's distance shelf (the TS's −12 dB at
/// distance 1 above 3 kHz).
const RUMBLE_SHELF_HZ: f32 = 3_000.0;
const RUMBLE_SHELF_DB_AT_FULL_DISTANCE: f32 = -12.0;
/// Corner of the infra layer: two one-pole lowpasses in series.
const INFRA_CORNER_HZ: f32 = 25.0;
/// Highpass under the infra layer, so it shakes and does not drift.
const INFRA_FLOOR_HZ: f32 = 8.0;
/// Rolls: a new random level every 0.2–0.7 s, smoothed at 1.5 Hz, between
/// this floor and 1.
const ROLL_FLOOR: f32 = 0.45;
const ROLL_SMOOTHING_HZ: f32 = 1.5;
const ROLL_INTERVAL_MIN_SECONDS: f32 = 0.2;
const ROLL_INTERVAL_MAX_SECONDS: f32 = 0.7;

/// Layer trims. The TS layers were unbalanced by its hot pink noise (RMS
/// ≈ 1.56, against ≈ 0.30 here) and its infra layer was ≈ 50 dB under the
/// brown body, so the ground shake was inaudible. These bring each layer to
/// a known RMS at gain 1 at 48 kHz: brown 0.09 → 0.25, infra (two poles at
/// 25 Hz of unit-density white, RMS 0.0165) → 0.25, rumble hiss 0.30 → 0.06.
const BROWN_TRIM: f32 = 2.8;
const INFRA_INPUT_GAIN: f32 = 15.0;
const RUMBLE_HISS_TRIM: f32 = 0.2;
/// Whole-clap trim so even a massive crack peaks under 1, the level every
/// generator keeps; loudness itself is set by SPL calibration.
const OUTPUT_TRIM: f32 = 0.4;
/// The TS's "gray" hiss: mostly pink with some white for bite.
const GRAY_PINK_SHARE: f32 = 0.7;

/// The shape of one preset: the TS strike and rumble presets side by side.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ThunderPreset {
    /// Peak of the crack (TS `crackGain`).
    crack_gain: f32,
    /// Starting level of the strike's tail (TS `tailGain`).
    tail_gain: f32,
    /// How long the tail lasts (TS `tailDecay`).
    tail_seconds: f32,
    /// 0 close … 1 far: darkens crack and tail.
    strike_distance: f32,
    /// 0 pink … 1 gray hiss in the tail.
    strike_colour: f32,
    /// The strike's own level (TS strike `masterGain`).
    strike_level: f32,
    /// How long the rumble lasts (TS `duration`).
    rumble_seconds: f32,
    /// Infra layer level (TS `infraGain`).
    infra_gain: f32,
    /// Brown body level (TS `brownGain`).
    brown_gain: f32,
    /// 0 pink … 1 gray hiss in the rumble.
    rumble_colour: f32,
    /// Rumble hiss level (TS `rumbleGain`).
    rumble_gain: f32,
    /// 0 close … 1 far: shelves the rumble hiss.
    rumble_distance: f32,
}

/// The TS presets near, mid, far and massive, unchanged.
const PRESETS: [ThunderPreset; 4] = [
    ThunderPreset {
        crack_gain: 1.2,
        tail_gain: 0.6,
        tail_seconds: 1.0,
        strike_distance: 0.2,
        strike_colour: 0.2,
        strike_level: 1.0,
        rumble_seconds: 7.0,
        infra_gain: 0.25,
        brown_gain: 0.5,
        rumble_colour: 0.15,
        rumble_gain: 1.0,
        rumble_distance: 0.2,
    },
    ThunderPreset {
        crack_gain: 1.0,
        tail_gain: 0.6,
        tail_seconds: 1.5,
        strike_distance: 0.4,
        strike_colour: 0.3,
        strike_level: 1.0,
        rumble_seconds: 10.0,
        infra_gain: 0.2,
        brown_gain: 0.4,
        rumble_colour: 0.3,
        rumble_gain: 1.0,
        rumble_distance: 0.5,
    },
    ThunderPreset {
        crack_gain: 0.7,
        tail_gain: 0.5,
        tail_seconds: 2.0,
        strike_distance: 0.7,
        strike_colour: 0.5,
        strike_level: 0.9,
        rumble_seconds: 14.0,
        infra_gain: 0.3,
        brown_gain: 0.6,
        rumble_colour: 0.4,
        rumble_gain: 0.9,
        rumble_distance: 0.85,
    },
    ThunderPreset {
        crack_gain: 1.5,
        tail_gain: 0.8,
        tail_seconds: 1.2,
        strike_distance: 0.15,
        strike_colour: 0.25,
        strike_level: 1.2,
        rumble_seconds: 12.0,
        infra_gain: 0.45,
        brown_gain: 0.7,
        rumble_colour: 0.25,
        rumble_gain: 1.2,
        rumble_distance: 0.25,
    },
];

/// The preset `take` names; an unknown take is `mid`.
fn preset(take: u8) -> &'static ThunderPreset {
    PRESETS
        .get(take as usize)
        .unwrap_or(&PRESETS[PRESET_MID as usize])
}

/// How long a clap of `preset` lasts at rate 1, in seconds: the longest crack,
/// the half of the tail before the rumble starts, and the whole rumble. Every
/// clap ends by then (its lengths are only ever drawn shorter), so a planner
/// can space claps by it. An unknown preset is `mid`.
pub fn clap_seconds(preset_number: u8) -> f32 {
    let shape = preset(preset_number);
    CRACK_MAX_SECONDS + RUMBLE_ONSET_TAIL_FRACTION * shape.tail_seconds + shape.rumble_seconds
}

/// Lowpass corner that darkens a strike at `distance`. The TS used a
/// one-pole with coefficient 0.3 + 0.4 · distance at 48 kHz; this is that
/// pole's corner in Hz (7.4 kHz near, 4.2 kHz far), so it holds at any rate.
fn strike_damping_hz(distance: f32) -> f32 {
    let pole = 0.3 + 0.4 * distance.clamp(0.0, 1.0);
    -pole.ln() * super::noise::REFERENCE_SAMPLE_RATE / TAU
}

/// Raised-cosine fade in over the first and out over the last 10 % of `x`
/// in 0..1.
fn rumble_envelope(x: f32) -> f32 {
    if x < RUMBLE_FADE_FRACTION {
        0.5 * (1.0 - (PI * x / RUMBLE_FADE_FRACTION).cos())
    } else if x > 1.0 - RUMBLE_FADE_FRACTION {
        0.5 * (1.0 - (PI * (1.0 - x) / RUMBLE_FADE_FRACTION).cos())
    } else {
        1.0
    }
}

/// One decaying sine streak in the crack.
#[derive(Clone, Copy)]
struct Resonator {
    frequency_hz: f32,
    /// Level it is struck to at each stroke.
    strike_amplitude: f32,
    amplitude: f32,
    phase: f32,
    phase_increment: f32,
    decay: f32,
}

impl Resonator {
    const SILENT: Self = Self {
        frequency_hz: 0.0,
        strike_amplitude: 0.0,
        amplitude: 0.0,
        phase: 0.0,
        phase_increment: 0.0,
        decay: 0.0,
    };

    fn retune(&mut self, sample_rate: f32) {
        self.phase_increment = TAU * self.frequency_hz / sample_rate;
        self.decay = (-1.0 / (RESONATOR_TIME_CONSTANT_SECONDS * sample_rate)).exp();
    }

    #[inline]
    fn next_sample(&mut self) -> f32 {
        let output = self.phase.sin() * self.amplitude;
        self.phase += self.phase_increment;
        if self.phase >= TAU {
            self.phase -= TAU;
        }
        self.amplitude *= self.decay;
        output
    }
}

/// One clap sounding, and everything it was drawn with.
struct ClapVoice {
    sounding: bool,
    preset_number: u8,
    shape: ThunderPreset,
    /// Order it started in, so the newest and oldest are known.
    started: u64,
    gain: f32,
    rate: f32,
    /// Host samples left of a steal's fade-out; 0 when not being stolen.
    release_remaining: u32,
    release_length: u32,
    /// Seconds of the clap's own time since it started.
    elapsed: f64,
    /// The rate its own time runs at: the host rate divided by its `rate`.
    voice_sample_rate: f32,
    white_noise_scale: f32,

    crack_seconds: f32,
    stroke_onsets: [f32; 3],
    stroke_gains: [f32; 3],
    stroke_count: usize,
    next_stroke: usize,
    resonators: [Resonator; 3],
    tail_seconds: f32,
    rumble_onset: f32,
    rumble_seconds: f32,
    length_seconds: f32,

    white: Rng,
    pink: PinkNoise,
    brown: BrownNoise,
    strike_lowpass: OnePoleLP,
    infra_lowpasses: [OnePoleLP; 2],
    infra_highpass: OnePoleHP,
    rumble_shelf: Biquad,
    roll_target: f32,
    roll_next_change: f32,
    roll_smoother: OnePoleLP,
}

impl ClapVoice {
    fn new(seed: u64) -> Self {
        let rate = super::noise::REFERENCE_SAMPLE_RATE;
        Self {
            sounding: false,
            preset_number: PRESET_MID,
            shape: PRESETS[PRESET_MID as usize],
            started: 0,
            gain: 1.0,
            rate: 1.0,
            release_remaining: 0,
            release_length: 1,
            elapsed: 0.0,
            voice_sample_rate: rate,
            white_noise_scale: 1.0,
            crack_seconds: CRACK_MAX_SECONDS,
            stroke_onsets: [0.0; 3],
            stroke_gains: [0.0; 3],
            stroke_count: 0,
            next_stroke: 0,
            resonators: [Resonator::SILENT; 3],
            tail_seconds: 0.0,
            rumble_onset: 0.0,
            rumble_seconds: 0.0,
            length_seconds: 0.0,
            white: Rng::new(seed),
            pink: PinkNoise::new(seed.wrapping_add(1)),
            brown: BrownNoise::new(seed.wrapping_add(2)),
            strike_lowpass: OnePoleLP::new(strike_damping_hz(0.4), rate),
            infra_lowpasses: [
                OnePoleLP::new(INFRA_CORNER_HZ, rate),
                OnePoleLP::new(INFRA_CORNER_HZ, rate),
            ],
            infra_highpass: OnePoleHP::new(INFRA_FLOOR_HZ, rate),
            rumble_shelf: Biquad::unity(),
            roll_target: 1.0,
            roll_next_change: 0.0,
            roll_smoother: OnePoleLP::new(ROLL_SMOOTHING_HZ, rate),
        }
    }

    /// Start a fresh clap of `utterance`'s preset, drawing everything that
    /// makes it this clap from `rng`.
    fn start(&mut self, utterance: &Utterance, started: u64, sample_rate: f32, rng: &mut Rng) {
        let shape = *preset(utterance.take);
        self.sounding = true;
        self.preset_number = if (utterance.take as usize) < PRESETS.len() {
            utterance.take
        } else {
            PRESET_MID
        };
        self.shape = shape;
        self.started = started;
        self.gain = if utterance.gain.is_finite() {
            utterance.gain.clamp(0.0, 4.0)
        } else {
            1.0
        };
        self.rate = if utterance.rate.is_finite() && utterance.rate > 0.0 {
            utterance.rate.clamp(0.25, 4.0)
        } else {
            1.0
        };
        self.release_remaining = 0;
        self.elapsed = 0.0;

        // Crack: its length, then one to three return strokes.
        self.crack_seconds =
            CRACK_MIN_SECONDS + (CRACK_MAX_SECONDS - CRACK_MIN_SECONDS) * rng.next_f32();
        self.stroke_count = 1 + (rng.next_f32() * 3.0) as usize % 3;
        self.stroke_onsets[0] = 0.0;
        self.stroke_gains[0] = 1.0;
        for stroke in 1..self.stroke_count {
            let gap = RETURN_STROKE_GAP_MIN_SECONDS
                + (RETURN_STROKE_GAP_MAX_SECONDS - RETURN_STROKE_GAP_MIN_SECONDS) * rng.next_f32();
            self.stroke_onsets[stroke] = self.stroke_onsets[stroke - 1] + gap;
            self.stroke_gains[stroke] = 0.35 + 0.4 * rng.next_f32();
        }
        self.next_stroke = 0;
        for (resonator, frequency_hz) in self.resonators.iter_mut().zip(RESONATOR_FREQUENCIES_HZ) {
            resonator.frequency_hz = frequency_hz * (0.8 + 0.4 * rng.next_f32());
            resonator.strike_amplitude = 0.7 + 0.3 * rng.next_f32();
            resonator.amplitude = 0.0;
            resonator.phase = rng.next_f32() * TAU;
        }

        // Tail and rumble are drawn only shorter than the preset, so a clap
        // never outlasts `clap_seconds`.
        self.tail_seconds = shape.tail_seconds * (0.85 + 0.15 * rng.next_f32());
        self.rumble_onset = self.crack_seconds + RUMBLE_ONSET_TAIL_FRACTION * self.tail_seconds;
        self.rumble_seconds = shape.rumble_seconds * (0.8 + 0.2 * rng.next_f32());
        self.length_seconds =
            (self.crack_seconds + self.tail_seconds).max(self.rumble_onset + self.rumble_seconds);

        self.roll_target = 1.0;
        self.roll_next_change = 0.0;
        self.retune(sample_rate);
    }

    /// Set every coefficient for the host rate; the clap's own time runs at
    /// `sample_rate / rate`, which moves its pitch and length together.
    fn retune(&mut self, sample_rate: f32) {
        let voice_sample_rate = sample_rate / self.rate;
        self.voice_sample_rate = voice_sample_rate;
        self.white_noise_scale = white_noise_density_scale(voice_sample_rate);
        for resonator in &mut self.resonators {
            resonator.retune(voice_sample_rate);
        }
        self.strike_lowpass.set_cutoff(
            strike_damping_hz(self.shape.strike_distance),
            voice_sample_rate,
        );
        for lowpass in &mut self.infra_lowpasses {
            lowpass.set_cutoff(INFRA_CORNER_HZ, voice_sample_rate);
        }
        self.infra_highpass
            .set_cutoff(INFRA_FLOOR_HZ, voice_sample_rate);
        self.rumble_shelf.set_high_shelf(
            RUMBLE_SHELF_HZ,
            RUMBLE_SHELF_DB_AT_FULL_DISTANCE * self.shape.rumble_distance.clamp(0.0, 1.0),
            voice_sample_rate,
        );
        self.roll_smoother
            .set_cutoff(ROLL_SMOOTHING_HZ, voice_sample_rate);
        self.release_length = ((STEAL_RELEASE_SECONDS * sample_rate) as u32).max(1);
    }

    /// Begin fading out so another clap can take this voice.
    fn release(&mut self) {
        if self.release_remaining == 0 {
            self.release_remaining = self.release_length;
        }
    }

    fn releasing(&self) -> bool {
        self.sounding && self.release_remaining > 0
    }

    fn next_sample(&mut self, rng: &mut Rng) -> f32 {
        let time = self.elapsed as f32;
        let shape = self.shape;
        let voice_sample_rate = self.voice_sample_rate;

        let white = self.white.next_bipolar() * self.white_noise_scale;
        let pink = self.pink.next_sample(voice_sample_rate);
        let body = self.brown.next_sample(voice_sample_rate) * BROWN_TRIM;
        let gray = GRAY_PINK_SHARE * pink + (1.0 - GRAY_PINK_SHARE) * white;

        // Crack: each stroke re-strikes the resonators and opens its own
        // half-sine gate.
        if self.next_stroke < self.stroke_count && time >= self.stroke_onsets[self.next_stroke] {
            let stroke_gain = self.stroke_gains[self.next_stroke];
            for resonator in &mut self.resonators {
                resonator.amplitude = resonator.strike_amplitude * stroke_gain;
            }
            self.next_stroke += 1;
        }
        let tonal: f32 = self.resonators.iter_mut().map(Resonator::next_sample).sum();
        let mut crack = 0.0;
        for stroke in 0..self.stroke_count {
            let since = time - self.stroke_onsets[stroke];
            if (0.0..self.crack_seconds).contains(&since) {
                let gate = (PI * since / self.crack_seconds).sin() * self.stroke_gains[stroke];
                crack += (white + tonal) * gate;
            }
        }

        // Tail: brown body and hiss under a quadratic fade.
        let mut tail = 0.0;
        let tail_time = time - self.crack_seconds;
        if (0.0..self.tail_seconds).contains(&tail_time) {
            let remaining = 1.0 - tail_time / self.tail_seconds;
            let hiss = pink + (gray - pink) * shape.strike_colour;
            tail = (0.7 * body + 0.3 * hiss) * shape.tail_gain * remaining * remaining;
        }
        let strike =
            self.strike_lowpass.process(crack * shape.crack_gain + tail) * shape.strike_level;

        // Rumble: every filter runs all clap long so none starts from a jump.
        let infra_white = self.white.next_bipolar() * self.white_noise_scale * INFRA_INPUT_GAIN;
        let [first_lowpass, second_lowpass] = &mut self.infra_lowpasses;
        let infra = self
            .infra_highpass
            .process(second_lowpass.process(first_lowpass.process(infra_white)));
        let rumble_hiss = self
            .rumble_shelf
            .process(pink + (gray - pink) * shape.rumble_colour)
            * RUMBLE_HISS_TRIM;
        if time >= self.roll_next_change {
            self.roll_target = ROLL_FLOOR + (1.0 - ROLL_FLOOR) * rng.next_f32();
            self.roll_next_change = time
                + ROLL_INTERVAL_MIN_SECONDS
                + (ROLL_INTERVAL_MAX_SECONDS - ROLL_INTERVAL_MIN_SECONDS) * rng.next_f32();
        }
        let roll = self.roll_smoother.process(self.roll_target);
        let mut rumble = 0.0;
        let rumble_time = time - self.rumble_onset;
        if (0.0..self.rumble_seconds).contains(&rumble_time) {
            let envelope = rumble_envelope(rumble_time / self.rumble_seconds);
            rumble = (shape.infra_gain * infra
                + shape.brown_gain * body
                + shape.rumble_gain * rumble_hiss)
                * envelope
                * roll;
        }

        let mut output = (strike + rumble) * OUTPUT_TRIM * self.gain;
        if self.release_remaining > 0 {
            output *= self.release_remaining as f32 / self.release_length as f32;
            self.release_remaining -= 1;
            if self.release_remaining == 0 {
                self.sounding = false;
            }
        }
        self.elapsed += 1.0 / voice_sample_rate as f64;
        if self.elapsed >= self.length_seconds as f64 {
            self.sounding = false;
            self.release_remaining = 0;
        }
        output
    }
}

/// Procedural thunder, silent until told to strike.
///
/// Each strike is an [`Utterance`]: `take` is the preset ([`PRESET_NEAR`],
/// [`PRESET_MID`], [`PRESET_FAR`], [`PRESET_MASSIVE`]), `at_sample` when on the
/// session's clock, `gain` and `rate` its level and speed. Up to two claps
/// sound at once; a third steals the oldest after a 10 ms fade. Heard from a
/// direction but wide: it is a localized object, widened by the scene's spread.
pub struct ThunderSource {
    voices: [ClapVoice; VOICES],
    /// Upcoming strikes, earliest first; `None` is a free place.
    appointments: [Option<Utterance>; APPOINTMENTS],
    /// The session's clock, in output samples.
    clock: u64,
    /// How many claps have started, to order the voices.
    claps_started: u64,
    sample_rate_cached: f32,
    /// Output trim.
    pub master_gain: f32,
    /// World-space position: the direction the storm is in.
    pub position: Vec3,
    rng: Rng,
}

impl ThunderSource {
    /// A silent thunder source at `position`; `seed` makes its claps.
    pub fn new(position: Vec3, seed: u64) -> Self {
        Self {
            voices: [
                ClapVoice::new(seed.wrapping_add(11)),
                ClapVoice::new(seed.wrapping_add(23)),
            ],
            appointments: [None; APPOINTMENTS],
            clock: 0,
            claps_started: 0,
            sample_rate_cached: 0.0,
            master_gain: 1.0,
            position,
            rng: Rng::new(seed),
        }
    }

    /// Start every due strike a voice is free for; when none is free, fade
    /// out the oldest clap so the strike starts a few milliseconds later.
    fn keep_appointments(&mut self, sample_rate: f32) {
        while let Some(utterance) = self.appointments[0].filter(|due| due.at_sample <= self.clock) {
            if let Some(free) = self.voices.iter_mut().find(|voice| !voice.sounding) {
                self.claps_started += 1;
                free.start(&utterance, self.claps_started, sample_rate, &mut self.rng);
                self.appointments.rotate_left(1);
                self.appointments[APPOINTMENTS - 1] = None;
            } else {
                if !self.voices.iter().any(ClapVoice::releasing) {
                    if let Some(oldest) = self.voices.iter_mut().min_by_key(|voice| voice.started) {
                        oldest.release();
                    }
                }
                return;
            }
        }
    }

    #[cfg(test)]
    fn sounding_claps(&self) -> usize {
        self.voices.iter().filter(|voice| voice.sounding).count()
    }
}

impl SoundSource for ThunderSource {
    /// Only its level changes while it plays; any other setting is not one
    /// it has.
    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        if matches!(param, SynthParam::MasterGain) {
            self.master_gain = value.clamp(0.0, 2.0);
        }
    }

    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            for voice in &mut self.voices {
                if voice.sounding {
                    voice.retune(sample_rate);
                }
            }
        }
        self.keep_appointments(sample_rate);
        self.clock += 1;
        let mut sample = 0.0;
        for voice in &mut self.voices {
            if voice.sounding {
                sample += voice.next_sample(&mut self.rng);
            }
        }
        sample * self.master_gain
    }

    fn set_clock(&mut self, sample: u64) {
        self.clock = sample;
    }

    /// Kept in time order. With every place taken, the latest appointment
    /// gives way to an earlier one.
    fn schedule(&mut self, utterance: Utterance) {
        let at = self
            .appointments
            .iter()
            .position(|slot| slot.is_none_or(|held| held.at_sample > utterance.at_sample));
        if let Some(at) = at {
            self.appointments[at..].rotate_right(1);
            self.appointments[at] = Some(utterance);
        }
    }

    /// Claps already rolling roll on; only the ones still to come are dropped.
    fn cancel(&mut self) {
        self.appointments = [None; APPOINTMENTS];
    }

    /// The preset of the newest clap sounding, `None` when silent.
    fn sounding_take(&self) -> Option<u8> {
        self.voices
            .iter()
            .filter(|voice| voice.sounding)
            .max_by_key(|voice| voice.started)
            .map(|voice| voice.preset_number)
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn tick(&mut self, _dt: f32) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::test_support::{averaged_power_spectrum, band_power, render};

    const SAMPLE_RATE: f32 = 48_000.0;

    fn strike(take: u8, at_sample: u64) -> Utterance {
        Utterance {
            take,
            at_sample,
            gain: 1.0,
            rate: 1.0,
            skip_seconds: 0.0,
        }
    }

    /// One clap of `take` at sample 0, rendered for its whole length.
    fn one_clap(take: u8, seed: u64) -> Vec<f32> {
        let mut thunder = ThunderSource::new(Vec3::ZERO, seed);
        thunder.schedule(strike(take, 0));
        render(clap_seconds(take) + 0.5, SAMPLE_RATE, |rate| {
            thunder.next_sample(rate)
        })
    }

    fn energy(samples: &[f32]) -> f64 {
        samples.iter().map(|&value| (value as f64).powi(2)).sum()
    }

    #[test]
    fn it_is_silent_until_told_to_strike() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 1);
        assert_eq!(thunder.sounding_take(), None);
        let samples = render(10.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert!(samples.iter().all(|&value| value == 0.0));
        assert_eq!(thunder.sounding_take(), None);
    }

    /// A strike sounds from its sample on, reports its preset while it
    /// sounds, and is over within `clap_seconds`.
    #[test]
    fn a_strike_sounds_at_its_sample_and_ends_within_its_clap_length() {
        for take in [PRESET_NEAR, PRESET_MID, PRESET_FAR, PRESET_MASSIVE] {
            let mut thunder = ThunderSource::new(Vec3::ZERO, 3 + take as u64);
            let start = 4_800usize;
            thunder.schedule(strike(take, start as u64));
            let length = (clap_seconds(take) * SAMPLE_RATE) as usize;
            let mut samples = Vec::new();
            let mut takes = Vec::new();
            for _ in 0..start + length + 4_800 {
                samples.push(thunder.next_sample(SAMPLE_RATE));
                takes.push(thunder.sounding_take());
            }
            assert!(
                samples[..start].iter().all(|&value| value == 0.0),
                "{take}: early"
            );
            assert!(takes[..start].iter().all(Option::is_none));
            assert_eq!(takes[start], Some(take));
            let crack = &samples[start..start + (0.03 * SAMPLE_RATE) as usize];
            assert!(energy(crack) > 1e-3, "{take}: no crack");
            assert!(
                samples[start + length..].iter().all(|&value| value == 0.0),
                "{take}: still sounding after clap_seconds"
            );
            assert_eq!(thunder.sounding_take(), None);
            // It sounds for most of its length, not just the crack.
            let late = &samples[start + length / 2..start + length / 2 + 4_800];
            assert!(energy(late) > 0.0, "{take}: no rumble");
        }
    }

    /// Twice the rate is twice as fast: the clap ends within half its length.
    #[test]
    fn rate_two_ends_in_half_the_time() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 9);
        thunder.schedule(Utterance {
            take: PRESET_FAR,
            at_sample: 0,
            gain: 1.0,
            rate: 2.0,
            skip_seconds: 0.0,
        });
        let half = (clap_seconds(PRESET_FAR) / 2.0 * SAMPLE_RATE) as usize;
        for _ in 0..half {
            thunder.next_sample(SAMPLE_RATE);
        }
        assert_eq!(thunder.sounding_take(), None);
    }

    /// A moment already past is kept at once, and strikes are kept in time
    /// order whatever order they came in.
    #[test]
    fn appointments_follow_the_sessions_clock_in_time_order() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 4);
        thunder.set_clock(100_000);
        thunder.schedule(strike(PRESET_FAR, 100_000 + 2_000));
        thunder.schedule(strike(PRESET_NEAR, 10));
        thunder.next_sample(SAMPLE_RATE);
        assert_eq!(thunder.sounding_take(), Some(PRESET_NEAR), "past is now");
        for _ in 0..2_000 {
            thunder.next_sample(SAMPLE_RATE);
        }
        assert_eq!(thunder.sounding_take(), Some(PRESET_FAR));
        assert_eq!(thunder.sounding_claps(), 2);
    }

    /// Claps overlap, two at most: a third fades the oldest out and takes
    /// its place 10 ms later.
    #[test]
    fn at_most_two_claps_sound_at_once() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 5);
        thunder.schedule(strike(PRESET_NEAR, 0));
        thunder.schedule(strike(PRESET_MID, 1_000));
        thunder.schedule(strike(PRESET_MASSIVE, 2_000));
        let mut most = 0;
        for index in 0..4_000 {
            let sample = thunder.next_sample(SAMPLE_RATE);
            assert!(sample.is_finite());
            most = most.max(thunder.sounding_claps());
            if index == 2_000 {
                assert_eq!(thunder.sounding_take(), Some(PRESET_MID), "still fading");
            }
        }
        assert_eq!(most, 2);
        assert_eq!(thunder.sounding_take(), Some(PRESET_MASSIVE));
        assert!(thunder
            .voices
            .iter()
            .all(|voice| voice.preset_number != PRESET_NEAR));
    }

    /// Fraction of a signal's energy above 2 kHz.
    fn bright_fraction(samples: &[f32]) -> f64 {
        let spectrum = averaged_power_spectrum(samples);
        band_power(&spectrum, SAMPLE_RATE, 2_000.0, 20_000.0)
            / band_power(&spectrum, SAMPLE_RATE, 0.0, 24_000.0)
    }

    /// A near strike is brighter than a far one: distance darkens the tail
    /// and shelves the rumble. Compared on the strike and on the whole clap.
    #[test]
    fn near_is_brighter_than_far() {
        for seed in [1, 2, 3] {
            let near = one_clap(PRESET_NEAR, seed);
            let far = one_clap(PRESET_FAR, seed);
            let near_fraction = bright_fraction(&near);
            let far_fraction = bright_fraction(&far);
            assert!(
                near_fraction > 2.0 * far_fraction,
                "seed {seed}: near {near_fraction:.2e}, far {far_fraction:.2e}"
            );
            let strike_length = (0.6 * SAMPLE_RATE) as usize;
            assert!(
                bright_fraction(&near[..strike_length]) > bright_fraction(&far[..strike_length]),
                "seed {seed}: the strike alone"
            );
        }
    }

    /// The massive preset shakes the ground more: more energy below 25 Hz
    /// than a near strike, which is what reaches the LFE channel.
    #[test]
    fn massive_has_more_infra_than_near() {
        for seed in [1, 2] {
            // Welch averages per frame; times the length it is total energy.
            let infra = |samples: &[f32]| {
                let spectrum = averaged_power_spectrum(samples);
                band_power(&spectrum, SAMPLE_RATE, 0.0, 25.0) * samples.len() as f64
            };
            let massive = infra(&one_clap(PRESET_MASSIVE, seed));
            let near = infra(&one_clap(PRESET_NEAR, seed));
            assert!(
                massive > 2.0 * near,
                "seed {seed}: massive {massive:.3e}, near {near:.3e}"
            );
        }
    }

    /// The infra layer alone is real program material: it holds its level
    /// below 25 Hz and has little above 100 Hz.
    #[test]
    fn the_infra_layer_lives_below_25_hz() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 8);
        let mut shape = PRESETS[PRESET_MASSIVE as usize];
        shape.crack_gain = 0.0;
        shape.tail_gain = 0.0;
        shape.brown_gain = 0.0;
        shape.rumble_gain = 0.0;
        thunder.schedule(strike(PRESET_MASSIVE, 0));
        thunder.next_sample(SAMPLE_RATE);
        thunder
            .voices
            .iter_mut()
            .for_each(|voice| voice.shape = shape);
        let samples = render(10.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        let below = band_power(&spectrum, SAMPLE_RATE, 0.0, 25.0);
        let above = band_power(&spectrum, SAMPLE_RATE, 100.0, 24_000.0);
        assert!(below > 10.0 * above, "below {below:.3e}, above {above:.3e}");
        let rms = (energy(&samples) / samples.len() as f64).sqrt();
        // Nominal 0.25 (layer) x 0.45 (massive) x 0.4 (trim) = 0.045, less
        // the rolls (mean about 0.7), the fades and the 8 Hz floor: about
        // half of it. The TS infra layer sat about 50 dB under this.
        assert!(rms > 0.015, "infra RMS {rms}");
    }

    /// The same seed makes the same clap; another seed, another clap; and one
    /// source never strikes the same clap twice.
    #[test]
    fn claps_come_from_the_seed_and_never_repeat() {
        let first = one_clap(PRESET_MID, 7);
        assert_eq!(first, one_clap(PRESET_MID, 7));
        let other = one_clap(PRESET_MID, 8);
        let length = (2.0 * SAMPLE_RATE) as usize;
        let difference: f64 = first[..length]
            .iter()
            .zip(&other[..length])
            .map(|(a, b)| ((a - b) as f64).powi(2))
            .sum();
        assert!(difference > 0.1 * energy(&first[..length]));

        let mut thunder = ThunderSource::new(Vec3::ZERO, 7);
        thunder.schedule(strike(PRESET_NEAR, 0));
        let gap = (clap_seconds(PRESET_NEAR) * SAMPLE_RATE) as u64 + 10;
        thunder.schedule(strike(PRESET_NEAR, gap));
        let both = render(2.0 * gap as f32 / SAMPLE_RATE, SAMPLE_RATE, |rate| {
            thunder.next_sample(rate)
        });
        let once = &both[..length];
        let again = &both[gap as usize..gap as usize + length];
        assert_ne!(once, again);
    }

    /// A minute of overlapping claps of every preset, rate and gain (and a
    /// non-finite one) stays finite and bounded.
    #[test]
    fn a_minute_of_thunder_is_finite() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 21);
        let rates = [1.0, 0.5, 2.0, f32::NAN, 1.3];
        for index in 0..8u64 {
            thunder.schedule(Utterance {
                take: (index % 5) as u8,
                at_sample: index * (7.5 * SAMPLE_RATE) as u64,
                gain: if index == 3 { f32::INFINITY } else { 1.0 },
                rate: rates[index as usize % rates.len()],
                skip_seconds: 0.0,
            });
        }
        let samples = render(60.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        let peak = samples
            .iter()
            .fold(0.0_f32, |most, value| most.max(value.abs()));
        assert!(peak < 2.0, "peak {peak}");
        assert!(energy(&samples) > 0.0);
    }

    /// A single clap of any preset peaks under 1 at every host rate, as
    /// every generator does; the crack is the peak, and it is loudest at
    /// 96 kHz where the white noise carries the most bandwidth.
    #[test]
    fn every_preset_peaks_under_one_at_every_sample_rate() {
        for take in [PRESET_NEAR, PRESET_MID, PRESET_FAR, PRESET_MASSIVE] {
            for seed in 0..4 {
                for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
                    let mut thunder = ThunderSource::new(Vec3::ZERO, seed);
                    thunder.schedule(strike(take, 0));
                    let samples = render(1.0, sample_rate, |rate| thunder.next_sample(rate));
                    let peak = samples
                        .iter()
                        .fold(0.0_f32, |most, value| most.max(value.abs()));
                    assert!(
                        peak < 1.0,
                        "{take} seed {seed} @ {sample_rate}: peak {peak}"
                    );
                }
            }
        }
    }

    #[test]
    fn master_gain_zero_silences_and_nan_is_ignored() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 2);
        thunder.set_synth_param(SynthParam::MasterGain, f32::NAN);
        assert_eq!(thunder.master_gain, 1.0);
        thunder.set_synth_param(SynthParam::MasterGain, 5.0);
        assert_eq!(thunder.master_gain, 2.0);
        thunder.set_synth_param(SynthParam::MasterGain, 0.0);
        thunder.schedule(strike(PRESET_MASSIVE, 0));
        let samples = render(3.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert!(samples.iter().all(|&value| value == 0.0));
        assert_eq!(
            thunder.sounding_take(),
            Some(PRESET_MASSIVE),
            "it still plays"
        );
    }

    /// `clap_seconds` is the longest crack, half the tail, and the rumble.
    #[test]
    fn clap_lengths_follow_the_presets() {
        assert!((clap_seconds(PRESET_NEAR) - 7.55).abs() < 1e-4);
        assert!((clap_seconds(PRESET_MID) - 10.8).abs() < 1e-4);
        assert!((clap_seconds(PRESET_FAR) - 15.05).abs() < 1e-4);
        assert!((clap_seconds(PRESET_MASSIVE) - 12.65).abs() < 1e-4);
        assert_eq!(clap_seconds(9), clap_seconds(PRESET_MID));
    }

    /// The strike's damping corner is the TS one-pole's: 7.4 kHz near,
    /// 4.2 kHz far.
    #[test]
    fn strike_damping_matches_the_typescript_pole() {
        assert!((strike_damping_hz(0.2) - 7_391.0).abs() < 5.0);
        assert!((strike_damping_hz(0.7) - 4_161.0).abs() < 5.0);
    }
}
