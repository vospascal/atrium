//! A voice with a repertoire: it utters one take, rests, and utters another.
//!
//! This is the "rests" rule of a kind. A take is one recording played from
//! start to end; between takes the voice is silent for a gap that is never
//! the same twice. Everything is decided here on the audio thread from a
//! seeded generator, with no allocation: the takes are decoded beforehand.

use std::sync::Arc;

use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::SoundSource;
use atrium_core::telemetry::SourceSignal;
use atrium_core::types::Vec3;

use crate::audio::decode::AudioBuffer;
use crate::audio::signal_meter::SignalMeter;

/// Seconds over which a take fades in and out, so its edges never click.
const EDGE_FADE_SECONDS: f32 = 0.005;

/// One thing the voice can utter, and how often it is chosen next to the
/// others: a take with chance 3 is uttered three times as often as one with
/// chance 1.
pub struct Take {
    pub audio: Arc<AudioBuffer>,
    pub chance: f32,
}

/// How a voice rests: how long it rests between takes, and how much one
/// utterance may differ from the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rests {
    /// Shortest and longest rest between two takes (s).
    pub gap_seconds: (f32, f32),
    /// Each take is up to this much louder or softer (dB).
    pub level_variation_db: f32,
    /// Each take is up to this much higher or lower (cents).
    pub pitch_variation_cents: f32,
    /// How many times one take is uttered before another is chosen, fewest
    /// and most: `(1, 1)` changes every time, `(2, 5)` holds on to a take
    /// for two to five utterances, the way a great tit keeps one song a while.
    pub bout: (u32, u32),
    /// Utter the first take at once instead of after a random part of the
    /// longest gap. A voice heard on its own starts at once; voices in a
    /// scene wait, so that several started together do not open with a chord.
    pub starts_at_once: bool,
}

impl Rests {
    /// Rest between `shortest` and `longest` seconds, choosing a new take
    /// every time, with no variation.
    pub fn plain(shortest: f32, longest: f32) -> Self {
        Self {
            gap_seconds: (shortest, longest),
            level_variation_db: 0.0,
            pitch_variation_cents: 0.0,
            bout: (1, 1),
            starts_at_once: false,
        }
    }
}

enum State {
    /// Silent for this many more output samples.
    Resting { samples_left: f64 },
    /// Uttering `take` from `position` (in the take's own samples).
    Sounding {
        take: usize,
        position: f64,
        gain: f32,
        rate: f64,
    },
}

/// A source that utters its takes with rests between them.
pub struct VoiceNode {
    takes: Vec<Take>,
    rests: Rests,
    state: State,
    /// The take uttered last, so the next one differs from it.
    last_take: Option<usize>,
    /// How many more times the last take is uttered before another is chosen.
    left_in_bout: u32,
    random: u64,
    /// The output rate last seen, to count a changed rest in samples.
    sample_rate: f32,
    position: Vec3,
    pub amplitude: f32,
    /// Base amplitude at 0 dB SPL: `amplitude = unit_amplitude · 10^(spl/20)`.
    pub unit_amplitude: f32,
    pub pattern: DirectivityPattern,
    pub spread: f32,
    pub ref_dist: f32,
    facing: Option<f32>,
    muted: bool,
    signal_meter: Option<SignalMeter>,
}

impl VoiceNode {
    /// A voice at `position` that utters `takes`. `seed` decides every choice
    /// it makes, so two voices with different seeds never fall into step and
    /// one voice with the same seed does the same again.
    ///
    /// It starts resting, for a random part of its longest gap, so several
    /// voices that start together do not open with a chord.
    pub fn new(takes: Vec<Take>, rests: Rests, position: Vec3, seed: u64) -> Self {
        let mut node = Self {
            takes,
            rests,
            state: State::Resting { samples_left: 0.0 },
            last_take: None,
            left_in_bout: 0,
            // A zero state would stay zero in the generator below.
            random: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            sample_rate: 0.0,
            position,
            amplitude: 0.5,
            unit_amplitude: 0.0,
            pattern: DirectivityPattern::OMNI,
            spread: 0.0,
            ref_dist: 1.0,
            facing: None,
            muted: false,
            signal_meter: None,
        };
        // Until the first sample tells it the output rate, count in seconds.
        let opening = if rests.starts_at_once {
            0.0
        } else {
            node.unit_random() * rests.gap_seconds.1
        };
        node.state = State::Resting {
            samples_left: -opening as f64,
        };
        node
    }

    pub(crate) fn meter_signal(&mut self, rms: f32) {
        self.signal_meter = Some(SignalMeter::new(rms * self.amplitude));
    }

    /// A number from 0 up to, not including, 1.
    fn unit_random(&mut self) -> f32 {
        // xorshift64*: small, fast, and good enough to choose a take.
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        let bits = self.random.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40;
        bits as f32 / (1u64 << 24) as f32
    }

    /// A number from −1 to 1.
    fn signed_random(&mut self) -> f32 {
        self.unit_random() * 2.0 - 1.0
    }

    /// Choose the next take by its chance, never the one just uttered when
    /// there is another to choose.
    fn choose_take(&mut self) -> usize {
        let avoid = if self.takes.len() > 1 {
            self.last_take
        } else {
            None
        };
        let total: f32 = self
            .takes
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != avoid)
            .map(|(_, take)| take.chance.max(0.0))
            .sum();
        let mut pick = self.unit_random() * total;
        let mut chosen = 0;
        for (index, take) in self.takes.iter().enumerate() {
            if Some(index) == avoid {
                continue;
            }
            chosen = index;
            pick -= take.chance.max(0.0);
            if pick < 0.0 {
                break;
            }
        }
        chosen
    }

    /// The take for the next utterance: the same one again while a bout
    /// lasts, else a new one by chance, which starts a bout of its own.
    fn next_take(&mut self) -> usize {
        if let Some(last) = self.last_take.filter(|_| self.left_in_bout > 0) {
            self.left_in_bout -= 1;
            return last;
        }
        let take = self.choose_take();
        let (fewest, most) = self.rests.bout;
        // Only draw when there is a choice, so a voice without bouts makes
        // the same choices it always made from the same seed.
        let length = if most > fewest {
            fewest + (self.unit_random() * (most - fewest + 1) as f32) as u32
        } else {
            fewest
        };
        self.left_in_bout = length.max(1) - 1;
        take
    }

    fn start_take(&mut self) {
        let take = self.next_take();
        self.last_take = Some(take);
        let gain = 10.0_f32.powf(self.signed_random() * self.rests.level_variation_db / 20.0);
        let rate =
            2.0_f64.powf((self.signed_random() * self.rests.pitch_variation_cents) as f64 / 1200.0);
        self.state = State::Sounding {
            take,
            position: 0.0,
            gain,
            rate,
        };
    }

    fn start_rest(&mut self, sample_rate: f32) {
        let (shortest, longest) = self.rests.gap_seconds;
        let seconds = shortest + self.unit_random() * (longest - shortest).max(0.0);
        self.state = State::Resting {
            samples_left: (seconds * sample_rate) as f64,
        };
    }
}

impl SoundSource for VoiceNode {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.takes.is_empty() {
            return 0.0;
        }
        self.sample_rate = sample_rate;
        let sample = match &mut self.state {
            State::Resting { samples_left } => {
                // A negative count is the opening rest, still in seconds.
                if *samples_left < 0.0 {
                    *samples_left = -*samples_left * sample_rate as f64;
                }
                *samples_left -= 1.0;
                if *samples_left <= 0.0 {
                    self.start_take();
                }
                0.0
            }
            State::Sounding {
                take,
                position,
                gain,
                rate,
            } => {
                let audio = &self.takes[*take].audio;
                let length = audio.samples.len();
                let index = *position as usize;
                if index + 1 >= length {
                    self.start_rest(sample_rate);
                    0.0
                } else {
                    let fraction = (*position - index as f64) as f32;
                    let value = audio.samples[index]
                        + (audio.samples[index + 1] - audio.samples[index]) * fraction;
                    // Fade the first and last few milliseconds.
                    let edge = (EDGE_FADE_SECONDS * audio.sample_rate as f32).max(1.0);
                    let from_start = *position as f32 / edge;
                    let from_end = (length as f64 - 1.0 - *position) as f32 / edge;
                    let fade = from_start.min(from_end).clamp(0.0, 1.0);
                    let out = value * *gain * fade;
                    *position += audio.sample_rate as f64 / sample_rate as f64 * *rate;
                    out
                }
            }
        };

        if self.muted {
            // It keeps going while muted, so it comes back where it would be.
            return 0.0;
        }
        let output = sample * self.amplitude;
        if let Some(meter) = &mut self.signal_meter {
            meter.push(output, sample_rate);
        }
        output
    }

    fn signal(&self) -> Option<SourceSignal> {
        self.signal_meter.as_ref().map(SignalMeter::signal)
    }

    fn sounding_take(&self) -> Option<u8> {
        match self.state {
            State::Sounding { take, .. } => Some(take as u8),
            State::Resting { .. } => None,
        }
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn tick(&mut self, _dt: f32) {}

    fn orientation(&self) -> Vec3 {
        match self.facing {
            Some(yaw) => Vec3::new(yaw.cos(), yaw.sin(), 0.0),
            None => Vec3::new(1.0, 0.0, 0.0),
        }
    }

    fn directivity(&self) -> DirectivityPattern {
        self.pattern
    }

    fn is_muted(&self) -> bool {
        self.muted
    }

    fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    fn set_position(&mut self, position: Vec3) {
        self.position = position;
    }

    fn spread(&self) -> f32 {
        self.spread
    }

    fn set_spread(&mut self, spread: f32) {
        self.spread = spread;
    }

    fn set_reference_spl(&mut self, spl: f32) -> f32 {
        if self.unit_amplitude > 0.0 {
            self.amplitude = self.unit_amplitude * 10.0_f32.powf(spl / 20.0);
        }
        self.amplitude
    }

    fn set_directivity(&mut self, pattern: DirectivityPattern) {
        self.pattern = pattern;
    }

    fn set_facing(&mut self, yaw: f32) {
        self.facing = Some(yaw);
    }

    /// A rest already longer than the new longest is cut to a fresh one, so
    /// a shorter gap is heard at once instead of after a long silence.
    fn set_gap(&mut self, shortest: f32, longest: f32) {
        if !(shortest >= 0.0 && longest >= shortest) {
            return;
        }
        self.rests.gap_seconds = (shortest, longest);
        if let State::Resting { samples_left } = self.state {
            let longest_samples = (longest * self.sample_rate) as f64;
            if self.sample_rate > 0.0 && samples_left > longest_samples {
                self.start_rest(self.sample_rate);
            }
        }
    }

    fn set_bout(&mut self, fewest: u32, most: u32) {
        if fewest >= 1 && most >= fewest {
            self.rests.bout = (fewest, most);
            self.left_in_bout = self.left_in_bout.min(most - 1);
        }
    }

    fn set_take_chance(&mut self, take: u8, chance: f32) {
        if let Some(take) = self.takes.get_mut(take as usize) {
            if chance >= 0.0 {
                take.chance = chance;
            }
        }
    }

    fn ref_distance(&self) -> f32 {
        self.ref_dist
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::spectral_profile::SpectralProfile;

    const RATE: u32 = 1_000;

    /// A take of `length` samples that all hold `value`, so the take being
    /// uttered can be read from the output.
    fn take(value: f32, length: usize, chance: f32) -> Take {
        Take {
            audio: Arc::new(AudioBuffer {
                samples: vec![value; length],
                sample_rate: RATE,
                rms: value.abs(),
                spectral_profile: SpectralProfile::default(),
            }),
            chance,
        }
    }

    fn steady(gap_seconds: (f32, f32)) -> Rests {
        Rests::plain(gap_seconds.0, gap_seconds.1)
    }

    fn voice(takes: Vec<Take>, rests: Rests, seed: u64) -> VoiceNode {
        let mut node = VoiceNode::new(takes, rests, Vec3::ZERO, seed);
        node.amplitude = 1.0;
        node
    }

    /// The takes uttered over `seconds`, in order, read from `sounding_take`.
    fn takes_uttered(node: &mut VoiceNode, seconds: usize) -> Vec<u8> {
        let mut uttered = Vec::new();
        let mut before = None;
        for _ in 0..seconds * RATE as usize {
            node.next_sample(RATE as f32);
            let now = node.sounding_take();
            if now != before {
                if let Some(take) = now {
                    uttered.push(take);
                }
                before = now;
            }
        }
        uttered
    }

    /// The point of a repertoire: with more than one take, the same one is
    /// never uttered twice running, and over time every take is heard.
    #[test]
    fn it_never_utters_the_same_take_twice_running() {
        let takes = vec![
            take(0.1, 100, 1.0),
            take(0.2, 100, 1.0),
            take(0.3, 100, 1.0),
        ];
        let mut node = voice(takes, steady((0.05, 0.2)), 7);
        let uttered = takes_uttered(&mut node, 60);
        assert!(
            uttered.len() > 100,
            "only {} takes in a minute",
            uttered.len()
        );
        assert!(uttered.windows(2).all(|pair| pair[0] != pair[1]));
        for wanted in 0..3u8 {
            assert!(uttered.contains(&wanted), "take {wanted} was never uttered");
        }
    }

    /// In bouts a take is uttered a few times running before another is
    /// chosen, and the bout lengths stay inside their range. A voice heard on
    /// its own starts at once.
    #[test]
    fn in_bouts_a_take_is_held_for_a_few_utterances() {
        let rests = Rests {
            bout: (2, 4),
            starts_at_once: true,
            ..Rests::plain(0.01, 0.02)
        };
        let mut node = voice(
            vec![take(0.1, 30, 1.0), take(0.2, 30, 1.0), take(0.3, 30, 1.0)],
            rests,
            11,
        );
        node.next_sample(RATE as f32);
        assert!(node.sounding_take().is_some(), "it starts at once");
        let uttered = takes_uttered(&mut node, 30);
        let mut runs = Vec::new();
        let mut length = 1;
        for pair in uttered.windows(2) {
            if pair[0] == pair[1] {
                length += 1;
            } else {
                runs.push(length);
                length = 1;
            }
        }
        // The first run may have begun before counting; the rest are whole bouts.
        let whole = &runs[1..];
        assert!(whole.len() > 20, "only {} bouts", whole.len());
        assert!(whole.iter().all(|run| (2..=4).contains(run)), "{whole:?}");
        assert!(
            whole.contains(&2) && whole.contains(&4),
            "every length occurs: {whole:?}"
        );
    }

    /// Edited while it plays: a take given chance 0 is no longer chosen, and
    /// a long rest is cut short when the gap is made shorter.
    #[test]
    fn its_rests_and_chances_change_while_it_plays() {
        let mut node = voice(
            vec![take(0.1, 50, 1.0), take(0.2, 50, 1.0), take(0.3, 50, 1.0)],
            steady((100.0, 100.0)),
            5,
        );
        node.set_take_chance(1, 0.0);
        // Its opening rest is up to 100 s; a gap of at most 0.1 s ends it now.
        node.next_sample(RATE as f32);
        node.set_gap(0.05, 0.1);
        let uttered = takes_uttered(&mut node, 20);
        assert!(
            uttered.len() > 50,
            "only {} takes after the gap was shortened",
            uttered.len()
        );
        assert!(
            !uttered.contains(&1),
            "a take with chance 0 is never chosen"
        );
        node.set_gap(5.0, 1.0);
        assert_eq!(
            node.rests.gap_seconds,
            (0.05, 0.1),
            "a gap from long to short is ignored"
        );
    }

    /// It is silent between takes, and every rest lies inside the gap it was
    /// given: here 0.2 to 0.5 s, which is 200 to 500 samples at 1 kHz.
    #[test]
    fn it_rests_between_takes_for_a_gap_inside_its_range() {
        let mut node = voice(
            vec![take(0.5, 100, 1.0), take(0.5, 100, 1.0)],
            steady((0.2, 0.5)),
            3,
        );
        let mut rests = Vec::new();
        let mut resting = 0usize;
        let mut started = false;
        for _ in 0..40 * RATE as usize {
            let sample = node.next_sample(RATE as f32);
            if node.sounding_take().is_some() {
                if started && resting > 0 {
                    rests.push(resting);
                }
                started = true;
                resting = 0;
            } else {
                assert_eq!(sample, 0.0, "a resting voice is silent");
                resting += 1;
            }
        }
        assert!(rests.len() > 20);
        let (shortest, longest) = (*rests.iter().min().unwrap(), *rests.iter().max().unwrap());
        assert!(
            shortest >= 199 && longest <= 502,
            "rests ran from {shortest} to {longest}"
        );
        assert!(
            longest - shortest > 100,
            "the gaps barely differ: {shortest} to {longest}"
        );
    }

    /// A take with a higher chance is uttered more often. It cannot follow
    /// itself, so with four takes the favourite is heard at most every other
    /// time: here close to half of all utterances, against about a sixth for
    /// each of the others.
    #[test]
    fn a_take_with_a_higher_chance_is_uttered_more_often() {
        let takes = vec![
            take(0.1, 20, 6.0),
            take(0.2, 20, 1.0),
            take(0.3, 20, 1.0),
            take(0.4, 20, 1.0),
        ];
        let mut node = voice(takes, steady((0.01, 0.02)), 11);
        let uttered = takes_uttered(&mut node, 60);
        let count = |wanted: u8| uttered.iter().filter(|take| **take == wanted).count();
        for other in 1..4u8 {
            assert!(count(other) > 0, "take {other} was never uttered");
            assert!(
                count(0) > count(other) * 2,
                "the favourite came {} times, take {other} {} times",
                count(0),
                count(other)
            );
        }
    }

    /// Two voices with different seeds do not fall into step, and the same
    /// seed does the same again.
    #[test]
    fn a_seed_decides_every_choice() {
        let build = |seed| {
            voice(
                vec![take(0.1, 50, 1.0), take(0.2, 50, 1.0), take(0.3, 50, 1.0)],
                steady((0.1, 0.4)),
                seed,
            )
        };
        let first = takes_uttered(&mut build(1), 20);
        assert_eq!(first, takes_uttered(&mut build(1), 20));
        assert_ne!(first, takes_uttered(&mut build(2), 20));
    }

    /// One take can only repeat, and it still rests between utterances.
    #[test]
    fn a_single_take_repeats_with_rests() {
        let mut node = voice(vec![take(0.5, 100, 1.0)], steady((0.1, 0.2)), 5);
        let uttered = takes_uttered(&mut node, 5);
        assert!(uttered.len() > 5 && uttered.iter().all(|take| *take == 0));
    }

    /// Level varies by at most the given decibels: with 6 dB, the loudest
    /// utterance of a constant take is at most twice the plain value and the
    /// softest at least half, and they are not all the same.
    #[test]
    fn each_utterance_is_a_little_louder_or_softer() {
        let rests = Rests {
            level_variation_db: 6.0,
            ..Rests::plain(0.01, 0.02)
        };
        let mut node = voice(vec![take(0.5, 100, 1.0), take(0.5, 100, 1.0)], rests, 9);
        let mut peaks = Vec::new();
        let mut peak = 0.0f32;
        for _ in 0..30 * RATE as usize {
            let sample = node.next_sample(RATE as f32).abs();
            if node.sounding_take().is_some() {
                peak = peak.max(sample);
            } else if peak > 0.0 {
                peaks.push(peak);
                peak = 0.0;
            }
        }
        let loudest = peaks.iter().cloned().fold(0.0, f32::max);
        let softest = peaks.iter().cloned().fold(f32::MAX, f32::min);
        assert!(loudest <= 0.5 * 2.0 + 1e-3 && softest >= 0.5 / 2.0 - 1e-3);
        assert!(
            loudest / softest > 1.5,
            "from {softest} to {loudest} is too alike"
        );
    }

    #[test]
    fn a_muted_voice_is_silent_and_keeps_going() {
        let mut node = voice(
            vec![take(0.5, 100, 1.0), take(0.5, 100, 1.0)],
            steady((0.05, 0.1)),
            2,
        );
        node.set_muted(true);
        let mut sang = false;
        for _ in 0..5 * RATE as usize {
            assert_eq!(node.next_sample(RATE as f32), 0.0);
            sang |= node.sounding_take().is_some();
        }
        assert!(sang, "it went through its takes while muted");
    }
}
