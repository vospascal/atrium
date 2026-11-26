//! The `recipe` voice: plays a recipe's chords as its planner decides them
//! (`atrium_recipe`), each note a pair of voices a few cents apart whose
//! pitch drifts and whose level wanders on its own, each chord swelling in
//! and out over the next. Measured from Astral Ambience "Calm"
//! (docs/ambient-recipes.md, docs/recipe-design.md).
//!
//! What plays when is the planner's; this file only makes the sound. All
//! storage is made in [`RecipeSource::new`]: the chords sounding at once, a
//! wavetable for the timbre, and the notes' voices. The audio thread then
//! asks the planner for the next chord when its time comes, which allocates
//! nothing.

use atrium_core::layers::LayerMix;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::types::Vec3;
use atrium_recipe::{
    ChordEvent, PingEvent, PingPlanner, Pings, Planner, Recipe, Span, Swoosh, SwooshEvent,
    SwooshPlanner, Voice, MOST_NOTES, MOST_VOICES,
};

use super::noise::Rng;

/// The layers it hands its sound over in, as a scene names them: the chords
/// and their colour always, then pings and swoosh when the recipe has them,
/// in that order (see [`layers`]).
const CHORDS_LAYER: usize = 0;
const COLOUR_LAYER: usize = 1;

/// The layers of a recipe, by the keys a scene names them by.
pub fn layers(recipe: &Recipe) -> Vec<&'static str> {
    let mut keys = vec!["chords", "colour"];
    if !recipe.pings().is_empty() {
        keys.push("pings");
    }
    if recipe.swoosh().is_some() {
        keys.push("swoosh");
    }
    keys
}

/// Pings ringing at once, of every kind. One rings for four `ring`s (80 dB
/// down), so two kinds ringing 1–4 s, each every 5 s or more, keep about six
/// going; past twelve the quietest gives way.
const MOST_PINGS: usize = 12;
/// A ping's strike (s): instant to the ear, no click.
const PING_ATTACK: f64 = 0.003;

/// Chords sounding at once. A turn's fade out may outlast the next turn's
/// hold (Calm: holds of 10–14 s, fades of up to 24 s), so up to three overlap.
const MOST_SOUNDING: usize = 3;
/// One cycle of the timbre, read with linear interpolation.
const TABLE_SIZE: usize = 2048;
/// The harmonics in the timbre's wavetable.
const HARMONICS: usize = 8;
/// Drift and wander are updated every this many samples, then held.
const CONTROL_SAMPLES: u32 = 32;
/// A voice's peak amplitude before calibration; a pair shares it.
const VOICE_AMPLITUDE: f32 = 0.05;
/// The piece's fade-in at the very start (s): it swells in, it does not
/// strike. (It costs the 20 s calibration preview about 0.6 dB, played that
/// much louder: a raised cosine squared averages 3/8 over its 4 s.)
const ONSET_SECONDS: f64 = 4.0;
/// The shortest rise of a note (s). The first chord has no swell of its own,
/// and a note staggered into it would otherwise begin at full level.
const SHORTEST_NOTE_RISE: f64 = 1.0;

/// A value moving smoothly between random targets in [−1, 1], each move
/// taking a time drawn from a span, eased in and out (raised cosine).
#[derive(Clone, Copy, Debug, Default)]
struct Wander {
    from: f32,
    to: f32,
    elapsed: f32,
    duration: f32,
}

impl Wander {
    fn new(random: &mut Rng, shortest: f32, longest: f32) -> Self {
        let mut wander = Self {
            from: random.next_bipolar(),
            to: random.next_bipolar(),
            elapsed: 0.0,
            duration: 1.0,
        };
        wander.duration = Self::draw_duration(random, shortest, longest);
        // Begin somewhere along the first move, not at its start.
        wander.elapsed = random.next_f32() * wander.duration;
        wander
    }

    fn draw_duration(random: &mut Rng, shortest: f32, longest: f32) -> f32 {
        (shortest + random.next_f32() * (longest - shortest)).max(0.05)
    }

    fn step(&mut self, seconds: f32, random: &mut Rng, shortest: f32, longest: f32) -> f32 {
        self.elapsed += seconds;
        if self.elapsed >= self.duration {
            self.elapsed -= self.duration;
            self.from = self.to;
            self.to = random.next_bipolar();
            self.duration = Self::draw_duration(random, shortest, longest);
            self.elapsed = self.elapsed.min(self.duration);
        }
        let share = self.elapsed / self.duration;
        let eased = 0.5 - 0.5 * (std::f32::consts::PI * share).cos();
        self.from + (self.to - self.from) * eased
    }
}

/// A two-pole filter (RBJ, Q = 1/√2: Butterworth, 12 dB per octave): a
/// low-pass for the voice's `cutoff` (a warm pad, its harmonics above the
/// corner rolled off), a high-pass and a low-pass for the swoosh's band. In
/// f64: at a corner of a few hundred hertz and 96 kHz the poles sit close to 1.
#[derive(Clone, Copy, Debug, Default)]
struct LowPass {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl LowPass {
    fn tune(&mut self, corner_hertz: f64, sample_rate: f32) {
        let omega = std::f64::consts::TAU * corner_hertz.min(0.45 * sample_rate as f64)
            / sample_rate as f64;
        let alpha = omega.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let cosine = omega.cos();
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 - cosine) / 2.0 / a0;
        self.b1 = (1.0 - cosine) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cosine / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn tune_high_pass(&mut self, corner_hertz: f64, sample_rate: f32) {
        let omega = std::f64::consts::TAU * corner_hertz.min(0.45 * sample_rate as f64)
            / sample_rate as f64;
        let alpha = omega.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let cosine = omega.cos();
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 + cosine) / 2.0 / a0;
        self.b1 = -(1.0 + cosine) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cosine / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn process(&mut self, input: f32) -> f32 {
        let input = input as f64;
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        output as f32
    }
}

/// One note of a sounding chord: its voices, spread across the detune.
#[derive(Clone, Copy, Debug, Default)]
struct Note {
    frequency: f64,
    colour: bool,
    /// The timbre's table, or a pure sine for a note too high for it.
    pure: bool,
    /// How much later than its chord it begins (s): the chord's stagger.
    delay: f64,
    phases: [f64; MOST_VOICES],
    /// Each voice's frequency ratio to the note now: its detune and drift.
    ratios: [f64; MOST_VOICES],
    /// Each voice's own greatest drift (cents), drawn from the voice's span.
    drift_cents: [f32; MOST_VOICES],
    drift: [Wander; MOST_VOICES],
    wander: Wander,
    /// The level wander's gain now.
    gain: f32,
}

/// A chord's turn, as the planner decided it.
#[derive(Clone, Copy, Debug, Default)]
struct Turn {
    at: f64,
    hold: f64,
    swell_in: f64,
    swell_out: f64,
}

/// A ping ringing: its note, its phase, how long since its strike and how
/// fast it dies. A pure tone: Calm's tings add nothing above 700 Hz.
#[derive(Clone, Copy, Debug, Default)]
struct Ping {
    active: bool,
    /// Its kind's level.
    amplitude: f32,
    frequency: f64,
    phase: f64,
    age: f64,
    ring: f64,
}

impl Ping {
    /// Its level now: a raised-cosine strike, then 20 dB down every `ring`
    /// seconds.
    fn level(&self) -> f32 {
        let strike = if self.age < PING_ATTACK {
            0.5 - 0.5 * (std::f64::consts::PI * self.age / PING_ATTACK).cos()
        } else {
            1.0
        };
        let decay = (-std::f64::consts::LN_10 * self.age / self.ring).exp();
        (strike * decay) as f32
    }
}

/// A swoosh swelling: its turn.
#[derive(Clone, Copy, Debug, Default)]
struct Swelling {
    active: bool,
    at: f64,
    rise: f64,
    fall: f64,
}

impl Swelling {
    fn level(&self, time: f64) -> f64 {
        let since = time - self.at;
        if since < 0.0 || since >= self.rise + self.fall {
            0.0
        } else if since < self.rise {
            0.5 - 0.5 * (std::f64::consts::PI * since / self.rise).cos()
        } else {
            0.5 + 0.5 * (std::f64::consts::PI * (since - self.rise) / self.fall).cos()
        }
    }
}

/// A chord sounding: its turn and its notes.
#[derive(Clone, Copy, Debug, Default)]
struct Sounding {
    active: bool,
    turn: Turn,
    /// Its colour notes' gain: its own colour level.
    colour_gain: f32,
    notes: [Note; MOST_NOTES],
    count: usize,
}

impl Turn {
    /// The level at `time` (s) of a note beginning `delay` after the chord: 0
    /// before it, rising over the chord's swell in (at least
    /// [`SHORTEST_NOTE_RISE`]), 1 while the chord holds, falling over its swell
    /// out once the next chord arrives. Every note leaves together; they
    /// arrive one by one.
    fn envelope(&self, time: f64, delay: f64) -> f64 {
        let since = time - self.at;
        let rising_since = since - delay;
        if rising_since < 0.0 {
            return 0.0;
        }
        let rise = self.swell_in.max(SHORTEST_NOTE_RISE);
        let rising = if rising_since < rise {
            0.5 - 0.5 * (std::f64::consts::PI * rising_since / rise).cos()
        } else {
            1.0
        };
        let fading = since - self.hold;
        if fading <= 0.0 {
            return rising;
        }
        if self.swell_out <= 0.0 || fading >= self.swell_out {
            return 0.0;
        }
        rising * (0.5 + 0.5 * (std::f64::consts::PI * fading / self.swell_out).cos())
    }

    fn ended(&self, time: f64) -> bool {
        time - self.at >= self.hold + self.swell_out
    }
}

pub struct RecipeSource {
    position: Vec3,
    /// Each chord's notes and colour notes (Hz) and its colour gain, copied
    /// from the recipe.
    chords: Vec<(Vec<f64>, Vec<f64>, f32)>,
    voice: Voice,
    stagger: Span,
    master_gain: f32,
    /// Each kind of ping, its planner and its next ping; made here, never
    /// grown.
    pings: Vec<(Pings, PingPlanner, PingEvent)>,
    ringing: [Ping; MOST_PINGS],
    swoosh: Option<(Swoosh, SwooshPlanner, SwooshEvent)>,
    swelling: Swelling,
    /// The swoosh's band: a high-pass and a low-pass, and the rate tuned for.
    band: [LowPass; 2],
    band_rate: f32,
    planner: Planner,
    upcoming: ChordEvent,
    sounding: [Sounding; MOST_SOUNDING],
    table: Vec<f32>,
    sine: Vec<f32>,
    random: Rng,
    /// The swoosh's noise, from its own stream, so a recipe given a swoosh
    /// keeps its chords' drift and wander exactly as before.
    noise: Rng,
    time: f64,
    control_countdown: u32,
    /// One low-pass per layer, when the voice has a cutoff, and the rate it
    /// was tuned for.
    filters: [LowPass; 2],
    filter_rate: f32,
}

impl RecipeSource {
    pub fn new(position: Vec3, recipe: &Recipe, master_gain: f32, seed: u64) -> Self {
        let mut planner = Planner::new(recipe, seed);
        let upcoming = planner.next().expect("a planner never ends");
        let voice = *recipe.voice();
        Self {
            position,
            chords: recipe
                .chords()
                .iter()
                .map(|chord| {
                    (
                        chord.notes().to_vec(),
                        chord.colour().to_vec(),
                        10f32.powf(chord.colour_level_db() as f32 / 20.0),
                    )
                })
                .collect(),
            voice,
            stagger: recipe.swell_stagger(),
            pings: recipe
                .pings()
                .iter()
                .map(|pings| {
                    let mut planner = PingPlanner::new(pings, seed);
                    let first = planner.next().expect("a planner never ends");
                    (*pings, planner, first)
                })
                .collect(),
            ringing: [Ping::default(); MOST_PINGS],
            swoosh: recipe.swoosh().map(|swoosh| {
                let mut planner = SwooshPlanner::new(swoosh, seed);
                let first = planner.next().expect("a planner never ends");
                (*swoosh, planner, first)
            }),
            swelling: Swelling::default(),
            band: [LowPass::default(); 2],
            band_rate: 0.0,
            master_gain: if master_gain.is_finite() {
                master_gain.max(0.0)
            } else {
                1.0
            },
            planner,
            upcoming,
            sounding: [Sounding::default(); MOST_SOUNDING],
            table: timbre(voice.brightness),
            sine: (0..TABLE_SIZE)
                .map(|index| {
                    (std::f64::consts::TAU * index as f64 / TABLE_SIZE as f64).sin() as f32
                })
                .collect(),
            random: Rng::new(seed ^ 0x5EED_C40D),
            noise: Rng::new(seed ^ 0x0000_5A5A_7E15),
            time: 0.0,
            control_countdown: 0,
            filters: [LowPass::default(); 2],
            filter_rate: 0.0,
        }
    }

    /// The chord whose turn has come starts sounding, in a free place or in
    /// the place of the one furthest into its fade.
    fn begin(&mut self, turn: ChordEvent, sample_rate: f32) {
        let slot = self
            .sounding
            .iter()
            .position(|sounding| !sounding.active)
            .unwrap_or_else(|| {
                (0..MOST_SOUNDING)
                    .min_by(|&a, &b| {
                        let end = |s: &Sounding| s.turn.at + s.turn.hold;
                        end(&self.sounding[a]).total_cmp(&end(&self.sounding[b]))
                    })
                    .unwrap_or(0)
            });
        let (notes, colour, colour_gain) = &self.chords[turn.chord];
        let colour_gain = *colour_gain;
        let ceiling = 0.45 * sample_rate as f64;
        let voice = self.voice;
        let stagger = self.stagger;
        let random = &mut self.random;
        let (drift_shortest, drift_longest) = (
            voice.drift_seconds.low() as f32,
            voice.drift_seconds.high() as f32,
        );
        let mut count = 0;
        let mut placed = [Note::default(); MOST_NOTES];
        for (frequency, is_colour) in notes
            .iter()
            .map(|frequency| (*frequency, false))
            .chain(colour.iter().map(|frequency| (*frequency, true)))
            .take(MOST_NOTES)
        {
            let mut drift_cents = [0.0; MOST_VOICES];
            let mut drift = [Wander::default(); MOST_VOICES];
            let mut phases = [0.0; MOST_VOICES];
            for side in 0..voice.voices {
                drift_cents[side] = voice.drift_cents.at(random.next_f32() as f64) as f32;
                drift[side] = Wander::new(random, drift_shortest, drift_longest);
                // Voices out of step, so a chord does not start with every
                // wave at its peak together.
                phases[side] = random.next_f32() as f64;
            }
            placed[count] = Note {
                frequency,
                colour: is_colour,
                pure: frequency * HARMONICS as f64 > ceiling,
                delay: stagger.at(random.next_f32() as f64),
                phases,
                ratios: [1.0; MOST_VOICES],
                drift_cents,
                drift,
                wander: Wander::new(
                    random,
                    voice.wander_seconds.low() as f32,
                    voice.wander_seconds.high() as f32,
                ),
                gain: 1.0,
            };
            count += 1;
        }
        self.sounding[slot] = Sounding {
            active: true,
            colour_gain,
            turn: Turn {
                at: turn.at,
                hold: turn.hold,
                swell_in: turn.swell_in,
                swell_out: turn.swell_out,
            },
            notes: placed,
            count,
        };
        // Its drift and wander from the first sample, not after a control step.
        self.control(slot, 0.0);
    }

    /// Move each voice's drift and each note's wander on by `seconds`.
    fn control(&mut self, slot: usize, seconds: f32) {
        let voice = self.voice;
        let (drift_shortest, drift_longest) = (
            voice.drift_seconds.low() as f32,
            voice.drift_seconds.high() as f32,
        );
        let (wander_shortest, wander_longest) = (
            voice.wander_seconds.low() as f32,
            voice.wander_seconds.high() as f32,
        );
        let random = &mut self.random;
        let sounding = &mut self.sounding[slot];
        for note in sounding.notes.iter_mut().take(sounding.count) {
            for side in 0..voice.voices {
                // Spread evenly from −detune to +detune: two voices at the
                // two ends, three with one on the note, and so on.
                let detune = if voice.voices == 1 {
                    0.0
                } else {
                    voice.detune_cents * (2.0 * side as f64 / (voice.voices - 1) as f64 - 1.0)
                };
                let drift = note.drift[side].step(seconds, random, drift_shortest, drift_longest)
                    * note.drift_cents[side];
                note.ratios[side] = 2f64.powf((detune + drift as f64) / 1200.0);
            }
            let wander = note
                .wander
                .step(seconds, random, wander_shortest, wander_longest);
            note.gain = 10f32.powf(voice.wander_db as f32 * wander / 20.0);
        }
    }
}

impl RecipeSource {
    /// The place of the chord most recently arrived and still sounding,
    /// whose notes and colour notes a ping chooses from.
    fn latest_chord(&self) -> Option<usize> {
        (0..MOST_SOUNDING)
            .filter(|&slot| {
                let sounding = &self.sounding[slot];
                sounding.active && sounding.turn.at <= self.time
            })
            .max_by(|&a, &b| {
                self.sounding[a]
                    .turn
                    .at
                    .total_cmp(&self.sounding[b].turn.at)
            })
    }

    /// A ping of this kind strikes a note of the latest chord, moved by
    /// octaves into the kind's range, in the quietest (or a free) place.
    fn strike(&mut self, pings: &Pings, due: &PingEvent) {
        let Some(slot) = self.latest_chord() else {
            return;
        };
        let sounding = &self.sounding[slot];
        if sounding.count == 0 {
            return;
        }
        let pick = ((due.choice * sounding.count as f64) as usize).min(sounding.count - 1);
        let mut frequency = sounding.notes[pick].frequency;
        while frequency < pings.low_hertz {
            frequency *= 2.0;
        }
        while frequency > pings.high_hertz {
            frequency *= 0.5;
        }
        let place = (0..MOST_PINGS)
            .min_by(|&a, &b| {
                let level = |ping: &Ping| if ping.active { ping.level() } else { -1.0 };
                level(&self.ringing[a]).total_cmp(&level(&self.ringing[b]))
            })
            .unwrap_or(0);
        self.ringing[place] = Ping {
            active: true,
            amplitude: VOICE_AMPLITUDE * 10f32.powf(pings.level_db as f32 / 20.0),
            frequency,
            phase: 0.0,
            age: 0.0,
            ring: due.ring,
        };
    }

    /// The pings due, of every kind, strike; then the ringing ones sound.
    fn ping_sample(&mut self, step: f64) -> f32 {
        for kind in 0..self.pings.len() {
            while self.pings[kind].2.at <= self.time {
                let (pings, planner, upcoming) = &mut self.pings[kind];
                let (pings, due) = (*pings, *upcoming);
                *upcoming = planner.next().expect("a planner never ends");
                self.strike(&pings, &due);
            }
        }
        let mut sum = 0.0;
        for ping in self.ringing.iter_mut().filter(|ping| ping.active) {
            sum += ping.amplitude * ping.level() * read(&self.sine, ping.phase);
            ping.phase = (ping.phase + ping.frequency * step).fract();
            ping.age += step;
            // 80 dB down: gone.
            if ping.age > 4.0 * ping.ring {
                ping.active = false;
            }
        }
        sum
    }

    /// The swoosh due begins to swell; then its noise, in its band, sounds.
    fn swoosh_sample(&mut self, sample_rate: f32) -> f32 {
        let Some((swoosh, planner, upcoming)) = &mut self.swoosh else {
            return 0.0;
        };
        let swoosh = *swoosh;
        if upcoming.at <= self.time {
            self.swelling = Swelling {
                active: true,
                at: upcoming.at,
                rise: upcoming.rise,
                fall: upcoming.fall,
            };
            *upcoming = planner.next().expect("a planner never ends");
        }
        if self.band_rate != sample_rate {
            self.band_rate = sample_rate;
            self.band[0].tune_high_pass(swoosh.low_hertz, sample_rate);
            self.band[1].tune(swoosh.high_hertz, sample_rate);
        }
        // Uniform noise of RMS 1/√3, made RMS 1 within the band whatever the
        // rate: the band holds (high − low) / (rate / 2) of its power.
        let scale =
            (3.0 * 0.5 * sample_rate as f64 / (swoosh.high_hertz - swoosh.low_hertz)).sqrt() as f32;
        let noise = self.noise.next_bipolar() * scale;
        let above_low = self.band[0].process(noise);
        let banded = self.band[1].process(above_low);
        if !self.swelling.active {
            return 0.0;
        }
        let level = self.swelling.level(self.time) as f32;
        if self.time - self.swelling.at >= self.swelling.rise + self.swelling.fall {
            self.swelling.active = false;
        }
        banded * level * VOICE_AMPLITUDE * 10f32.powf(swoosh.level_db as f32 / 20.0)
    }
}

/// One cycle of the timbre: harmonics 1 to 8 at k^−(2 − brightness), so 0 is
/// soft (1/k², nearly a triangle's balance) and 1 bright (1/k, a saw's),
/// scaled to a peak of 1.
fn timbre(brightness: f64) -> Vec<f32> {
    let exponent = 2.0 - brightness.clamp(0.0, 1.0);
    let mut table: Vec<f64> = (0..TABLE_SIZE)
        .map(|index| {
            let phase = std::f64::consts::TAU * index as f64 / TABLE_SIZE as f64;
            (1..=HARMONICS)
                .map(|harmonic| (harmonic as f64 * phase).sin() / (harmonic as f64).powf(exponent))
                .sum()
        })
        .collect();
    let peak = table
        .iter()
        .fold(0.0f64, |peak, value| peak.max(value.abs()));
    if peak > 0.0 {
        table.iter_mut().for_each(|value| *value /= peak);
    }
    table.into_iter().map(|value| value as f32).collect()
}

fn read(table: &[f32], phase: f64) -> f32 {
    let position = phase * TABLE_SIZE as f64;
    let index = position as usize % TABLE_SIZE;
    let next = (index + 1) % TABLE_SIZE;
    let share = (position - position.floor()) as f32;
    table[index] + (table[next] - table[index]) * share
}

impl SoundSource for RecipeSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        self.next_mixed(sample_rate, &mut LayerMix::untouched())
    }

    fn layer_count(&self) -> usize {
        2 + usize::from(!self.pings.is_empty()) + usize::from(self.swoosh.is_some())
    }

    fn next_mixed(&mut self, sample_rate: f32, mix: &mut LayerMix) -> f32 {
        while self.time >= self.upcoming.at {
            let turn = self.upcoming;
            self.upcoming = self.planner.next().expect("a planner never ends");
            self.begin(turn, sample_rate);
        }
        if self.control_countdown == 0 {
            self.control_countdown = CONTROL_SAMPLES;
            let seconds = CONTROL_SAMPLES as f32 / sample_rate;
            for slot in 0..MOST_SOUNDING {
                if self.sounding[slot].active {
                    self.control(slot, seconds);
                }
            }
        }
        self.control_countdown -= 1;

        let step = 1.0 / sample_rate as f64;
        let voices = self.voice.voices;
        let voice_amplitude = VOICE_AMPLITUDE / voices as f32;
        let mut layers = [0.0f32; 2];
        for sounding in self.sounding.iter_mut().filter(|sounding| sounding.active) {
            if sounding.turn.ended(self.time) {
                sounding.active = false;
                continue;
            }
            let (time, turn) = (self.time, sounding.turn);
            for note in sounding.notes.iter_mut().take(sounding.count) {
                let envelope = turn.envelope(time, note.delay) as f32;
                if envelope <= 0.0 {
                    // Not yet begun, or gone: its phases wait.
                    continue;
                }
                let table = if note.pure { &self.sine } else { &self.table };
                let mut sample = 0.0;
                for side in 0..voices {
                    sample += read(table, note.phases[side]);
                    note.phases[side] =
                        (note.phases[side] + note.frequency * note.ratios[side] * step).fract();
                }
                let level = envelope * note.gain * voice_amplitude;
                if note.colour {
                    layers[COLOUR_LAYER] += sample * level * sounding.colour_gain;
                } else {
                    layers[CHORDS_LAYER] += sample * level;
                }
            }
        }
        let onset = if self.time < ONSET_SECONDS {
            (0.5 - 0.5 * (std::f64::consts::PI * self.time / ONSET_SECONDS).cos()) as f32
        } else {
            1.0
        };
        if let Some(corner) = self.voice.cutoff_hertz {
            if self.filter_rate != sample_rate {
                self.filter_rate = sample_rate;
                for filter in &mut self.filters {
                    filter.tune(corner, sample_rate);
                }
            }
            for (layer, filter) in layers.iter_mut().zip(&mut self.filters) {
                *layer = filter.process(*layer);
            }
        }
        let mut all = [layers[CHORDS_LAYER], layers[COLOUR_LAYER], 0.0, 0.0];
        let mut count = 2;
        if !self.pings.is_empty() {
            all[count] = self.ping_sample(step);
            count += 1;
        }
        if self.swoosh.is_some() {
            all[count] = self.swoosh_sample(sample_rate);
            count += 1;
        }
        self.time += step;
        mix.sum(0, &all[..count]) * onset * self.master_gain
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    /// Heard all round, like the focus tone: music to sit in. A scene's
    /// `shape.ambient: false` makes it a point.
    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }

    // Everything advances on the sample clock above.
    fn tick(&mut self, _dt: f32) {}
}

#[cfg(test)]
mod tests;
