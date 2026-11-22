//! One voice's decisions, made ahead: which take, when, how loud, how high.
//!
//! Every choice comes from one seeded generator, so the same voice with the
//! same seed decides the same way again, and two seeds never fall into step.

use super::rhythm::{LevelSpread, Rhythm};

/// An utterance decided for one voice, in seconds on the session's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Planned {
    /// Which take, counted from 0.
    pub take: u8,
    /// When it starts (s).
    pub at: f64,
    /// How long it lasts at its rate (s).
    pub seconds: f64,
    /// Louder or softer than the take itself (linear).
    pub gain: f32,
    /// Faster and higher, or slower and lower (1 is as recorded).
    pub rate: f32,
    /// How far into the take it starts (s, as heard): more than 0 only for a
    /// take the scene was moved into the middle of.
    pub skip: f64,
}

/// How soon a voice that skipped an appointment looks again, at the least
/// (s): a voice without rests would otherwise look again at once, forever.
const MIN_RECHECK_SECONDS: f32 = 1.0;

/// What one voice has decided so far, and what it will decide next.
#[derive(Clone, Debug)]
pub(super) struct VoicePlan {
    rhythm: Rhythm,
    random: u64,
    /// The take uttered last, so the next one differs from it.
    last_take: Option<usize>,
    /// How many more times the last take is uttered before another is chosen.
    left_in_bout: u32,
    /// When the next utterance starts (s).
    next_at: f64,
    /// When the last utterance ended, which a changed rest counts from (s).
    last_end: f64,
    /// What its choices follow from, and when it began deciding (s).
    seed: u64,
    born: f64,
    /// How much louder or softer this one is than its kind's mean (dB),
    /// decided once from its seed.
    individual_db: f32,
    /// A generator of its own for the level spread, so a kind given a spread
    /// keeps every other choice it made without one.
    spread_random: u64,
}

impl VoicePlan {
    /// A voice that starts deciding at `from` seconds. It opens with a rest
    /// of a random part of its longest gap, so several voices started
    /// together do not open with a chord.
    pub(super) fn new(rhythm: Rhythm, seed: u64, from: f64) -> Self {
        let mut spread_random =
            (seed ^ 0xD1B5_4A32_D192_ED03).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let individual_db = match rhythm.rests.level_spread.individual_db {
            deviation if deviation > 0.0 => normal_random(&mut spread_random) * deviation,
            _ => 0.0,
        };
        let mut voice = Self {
            rhythm,
            // A zero state would stay zero in the generator below.
            random: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            last_take: None,
            left_in_bout: 0,
            next_at: from,
            last_end: from,
            seed,
            born: from,
            individual_db,
            spread_random,
        };
        voice.next_at = from + (voice.unit_random() * voice.rhythm.rests.gap_seconds.1) as f64;
        voice
    }

    /// The same voice as it was when it began, with its rhythm as it is now:
    /// deciding again from there makes the same choices it made, which is
    /// how the scene is moved to another moment.
    pub(super) fn restarted(&self) -> Self {
        Self::new(self.rhythm.clone(), self.seed, self.born)
    }

    /// Utter the next take at `now` instead of after the opening rest: a
    /// voice heard on its own, to judge it, should not keep anyone waiting.
    pub(super) fn start_at(&mut self, now: f64) {
        self.next_at = now;
        self.last_end = now;
    }

    /// When the next utterance starts (s).
    pub(super) fn next_at(&self) -> f64 {
        self.next_at
    }

    /// Decide the next utterance and the rest after it, as sounded under
    /// conditions that give it `activity` (0 to 1) and `level_db`. At its
    /// appointment it sounds with a chance of `activity`; otherwise it rests
    /// again, so less activity means longer silences and none means none.
    pub(super) fn next_under(&mut self, activity: f32, level_db: f32) -> Option<Planned> {
        if activity < 1.0 && self.unit_random() >= activity {
            self.last_end = self.next_at;
            self.next_at += self.rest().max(MIN_RECHECK_SECONDS) as f64;
            return None;
        }
        let mut planned = self.next();
        planned.gain *= 10f32.powf(level_db / 20.0);
        Some(planned)
    }

    /// Decide the next utterance and the rest after it.
    pub(super) fn next(&mut self) -> Planned {
        let take = self.next_take();
        self.last_take = Some(take);
        let rests = self.rhythm.rests;
        let gain = 10f32.powf(self.signed_random() * rests.level_variation_db / 20.0);
        let rate = 2f32.powf(self.signed_random() * rests.pitch_variation_cents / 1200.0);
        let gain = gain * 10f32.powf(self.spread_level(rests.level_spread) / 20.0);
        let seconds = self.rhythm.takes[take].seconds as f64 / rate as f64;
        let at = self.next_at;
        self.last_end = at + seconds;
        self.next_at = self.last_end + self.rest() as f64;
        Planned {
            take: take as u8,
            at,
            seconds,
            gain,
            rate,
            skip: 0.0,
        }
    }

    /// Rest between `shortest` and `longest` from now on. A rest already
    /// longer than that, counted from the last utterance's end, is drawn
    /// again, so a shorter gap is heard at once and not after a long silence.
    pub(super) fn set_gap(&mut self, now: f64, shortest: f32, longest: f32) {
        if !(shortest >= 0.0 && longest >= shortest) {
            return;
        }
        self.rhythm.rests.gap_seconds = (shortest, longest);
        if self.next_at - self.last_end.max(now) > longest as f64 {
            self.next_at = self.last_end.max(now) + self.rest() as f64;
        }
    }

    pub(super) fn set_bout(&mut self, fewest: u32, most: u32) {
        if fewest >= 1 && most >= fewest {
            self.rhythm.rests.bout = (fewest, most);
            self.left_in_bout = self.left_in_bout.min(most - 1);
        }
    }

    pub(super) fn set_chance(&mut self, take: usize, chance: f32) {
        if let Some(take) = self.rhythm.takes.get_mut(take) {
            if chance >= 0.0 {
                take.chance = chance;
            }
        }
    }

    /// A rest drawn from the gap range (s).
    fn rest(&mut self) -> f32 {
        let (shortest, longest) = self.rhythm.rests.gap_seconds;
        shortest + self.unit_random() * (longest - shortest).max(0.0)
    }

    /// The same take again while a bout lasts, else a new one by chance,
    /// which starts a bout of its own.
    fn next_take(&mut self) -> usize {
        if let Some(last) = self.last_take.filter(|_| self.left_in_bout > 0) {
            self.left_in_bout -= 1;
            return last;
        }
        let take = self.choose_take();
        let (fewest, most) = self.rhythm.rests.bout;
        // Only draw when there is a choice, so a voice without bouts spends
        // its random numbers on takes and rests alone.
        let length = if most > fewest {
            fewest + (self.unit_random() * (most - fewest + 1) as f32) as u32
        } else {
            fewest
        };
        self.left_in_bout = length.clamp(fewest.max(1), most.max(1)) - 1;
        take
    }

    /// A take by its chance, never the one just uttered when there is
    /// another that can be chosen.
    fn choose_take(&mut self) -> usize {
        let takes = &self.rhythm.takes;
        let others = takes
            .iter()
            .enumerate()
            .any(|(index, take)| Some(index) != self.last_take && take.chance > 0.0);
        let avoid = if others { self.last_take } else { None };
        let total: f32 = takes
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != avoid)
            .map(|(_, take)| take.chance.max(0.0))
            .sum();
        let mut pick = self.unit_random() * total;
        let mut chosen = 0;
        for (index, take) in self.rhythm.takes.iter().enumerate() {
            if Some(index) == avoid || take.chance <= 0.0 {
                continue;
            }
            chosen = index;
            pick -= take.chance;
            if pick < 0.0 {
                break;
            }
        }
        chosen
    }

    /// This utterance's level from its kind's mean (dB): this one's own
    /// level and the utterance's, kept inside the measured range.
    fn spread_level(&mut self, spread: LevelSpread) -> f32 {
        if spread.is_none() {
            return 0.0;
        }
        let utterance_db = if spread.utterance_db > 0.0 {
            normal_random(&mut self.spread_random) * spread.utterance_db
        } else {
            0.0
        };
        let (softest, loudest) = spread.range_db;
        (self.individual_db + utterance_db).clamp(softest, loudest)
    }

    /// A number from 0 up to, not including, 1.
    fn unit_random(&mut self) -> f32 {
        unit_random(&mut self.random)
    }

    /// A number from −1 to 1.
    fn signed_random(&mut self) -> f32 {
        self.unit_random() * 2.0 - 1.0
    }
}

/// A number from 0 up to, not including, 1, from the generator `state`.
fn unit_random(state: &mut u64) -> f32 {
    // xorshift64*: small, fast, and good enough to choose a take.
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    let bits = state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40;
    bits as f32 / (1u64 << 24) as f32
}

/// A number from the standard normal distribution (Box–Muller).
fn normal_random(state: &mut u64) -> f32 {
    // 1 − u lies in (0, 1], so the logarithm is finite.
    let radius = (-2.0 * (1.0 - unit_random(state)).ln()).sqrt();
    radius * (std::f32::consts::TAU * unit_random(state)).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::rhythm::{Rests, TakeRhythm};

    fn rhythm(lengths: &[f32], rests: Rests) -> Rhythm {
        Rhythm {
            takes: lengths
                .iter()
                .map(|&seconds| TakeRhythm {
                    seconds,
                    chance: 1.0,
                })
                .collect(),
            rests,
        }
    }

    fn decide(voice: &mut VoicePlan, count: usize) -> Vec<Planned> {
        (0..count).map(|_| voice.next()).collect()
    }

    /// The point of a repertoire: the same take is never uttered twice
    /// running, and over time every take is heard.
    #[test]
    fn it_never_utters_the_same_take_twice_running() {
        let mut voice = VoicePlan::new(rhythm(&[1.0, 1.0, 1.0], Rests::plain(0.1, 0.5)), 7, 0.0);
        let takes: Vec<u8> = decide(&mut voice, 200)
            .iter()
            .map(|planned| planned.take)
            .collect();
        assert!(takes.windows(2).all(|pair| pair[0] != pair[1]));
        for wanted in 0..3 {
            assert!(takes.contains(&wanted), "take {wanted} was never uttered");
        }
    }

    /// Each rest lies inside the gap range, measured from the end of one
    /// utterance to the start of the next, and the opening rest inside the
    /// longest gap.
    #[test]
    fn every_rest_lies_inside_its_range() {
        let mut voice = VoicePlan::new(rhythm(&[0.8, 1.3], Rests::plain(2.0, 5.0)), 3, 10.0);
        let planned = decide(&mut voice, 300);
        assert!(planned[0].at >= 10.0 && planned[0].at <= 15.0);
        for pair in planned.windows(2) {
            let rest = pair[1].at - (pair[0].at + pair[0].seconds);
            assert!((2.0 - 1e-6..=5.0 + 1e-6).contains(&rest), "rest {rest}");
        }
    }

    /// Two seeds do not fall into step, and the same seed decides the same.
    #[test]
    fn a_seed_decides_every_choice() {
        let plan = |seed| {
            decide(
                &mut VoicePlan::new(
                    rhythm(&[1.0, 1.0, 1.0, 1.0], Rests::plain(1.0, 4.0)),
                    seed,
                    0.0,
                ),
                40,
            )
        };
        assert_eq!(plan(5), plan(5));
        assert_ne!(plan(5), plan(6));
    }

    /// Level and pitch vary inside their ranges, and the length follows the
    /// pitch: higher is shorter.
    #[test]
    fn level_and_pitch_vary_inside_their_ranges() {
        let rests = Rests {
            level_variation_db: 6.0,
            pitch_variation_cents: 100.0,
            ..Rests::plain(0.0, 0.0)
        };
        let mut voice = VoicePlan::new(rhythm(&[2.0, 2.0], rests), 9, 0.0);
        let planned = decide(&mut voice, 300);
        let half_tone = 2f32.powf(100.0 / 1200.0);
        assert!(planned.iter().all(|p| (0.5..=2.0).contains(&p.gain)));
        assert!(planned
            .iter()
            .all(|p| p.rate >= 1.0 / half_tone - 1e-4 && p.rate <= half_tone + 1e-4));
        assert!(planned.iter().any(|p| p.gain > 1.2) && planned.iter().any(|p| p.gain < 0.8));
        assert!(planned
            .iter()
            .all(|p| (p.seconds * p.rate as f64 - 2.0).abs() < 1e-4));
    }

    /// In bouts a take is held for a few utterances, every bout length
    /// inside its range.
    #[test]
    fn in_bouts_a_take_is_held_for_a_few_utterances() {
        let rests = Rests {
            bout: (2, 4),
            ..Rests::plain(0.1, 0.2)
        };
        let mut voice = VoicePlan::new(rhythm(&[1.0, 1.0, 1.0], rests), 11, 0.0);
        let takes: Vec<u8> = decide(&mut voice, 400)
            .iter()
            .map(|planned| planned.take)
            .collect();
        let mut runs = Vec::new();
        let mut length = 1;
        for pair in takes.windows(2) {
            if pair[0] == pair[1] {
                length += 1;
            } else {
                runs.push(length);
                length = 1;
            }
        }
        assert!(runs.iter().all(|run| (2..=4).contains(run)), "{runs:?}");
        assert!(
            runs.contains(&2) && runs.contains(&4),
            "every length occurs"
        );
    }

    /// Changed while it runs: a take with chance 0 is no longer chosen, and
    /// a long rest is drawn again when the gap is made shorter.
    #[test]
    fn its_rests_and_chances_change_while_it_runs() {
        let mut voice =
            VoicePlan::new(rhythm(&[0.5, 0.5, 0.5], Rests::plain(100.0, 100.0)), 5, 0.0);
        voice.set_chance(1, 0.0);
        voice.set_gap(1.0, 0.1, 0.3);
        assert!(
            voice.next_at() <= 1.3 + 1e-6,
            "the opening rest was cut to the new gap"
        );
        let takes: Vec<u8> = decide(&mut voice, 100)
            .iter()
            .map(|planned| planned.take)
            .collect();
        assert!(!takes.contains(&1));
        voice.set_gap(2.0, 5.0, 1.0);
        let before = voice.next_at();
        voice.set_gap(2.0, 5.0, 1.0);
        assert_eq!(
            voice.next_at(),
            before,
            "a gap from long to short is ignored"
        );
    }

    /// With only one take that can be chosen, it is chosen every time, even
    /// twice running.
    #[test]
    fn one_take_left_is_uttered_every_time() {
        let mut voice = VoicePlan::new(rhythm(&[0.5, 0.5], Rests::plain(0.1, 0.2)), 2, 0.0);
        voice.set_chance(0, 0.0);
        assert!(decide(&mut voice, 20)
            .iter()
            .all(|planned| planned.take == 1));
    }

    const BLACKBIRD: LevelSpread = LevelSpread {
        individual_db: 3.8,
        utterance_db: 2.5,
        range_db: (-15.4, 12.3),
    };

    fn with_spread(spread: LevelSpread) -> Rhythm {
        rhythm(
            &[1.0, 1.0, 1.0],
            Rests {
                level_spread: spread,
                ..Rests::plain(0.5, 3.0)
            },
        )
    }

    fn decibels(gain: f32) -> f64 {
        20.0 * (gain as f64).log10()
    }

    fn standard_deviation(values: &[f64]) -> f64 {
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        (values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / values.len() as f64)
            .sqrt()
    }

    /// A spread changes how loud, and nothing else: the takes, the moments
    /// and the pitch are those of the same voice without one.
    #[test]
    fn a_spread_changes_the_level_and_no_other_choice() {
        let plain = decide(
            &mut VoicePlan::new(with_spread(LevelSpread::NONE), 21, 0.0),
            200,
        );
        let spread = decide(&mut VoicePlan::new(with_spread(BLACKBIRD), 21, 0.0), 200);
        for (plain, spread) in plain.iter().zip(&spread) {
            assert_eq!(
                (plain.take, plain.at, plain.rate),
                (spread.take, spread.at, spread.rate)
            );
        }
        assert!(plain.iter().all(|planned| planned.gain == 1.0));
        assert!(spread.iter().any(|planned| planned.gain != 1.0));
    }

    /// Across many birds of a kind, their own levels spread by the measured
    /// deviation between birds; across one bird's songs, by the deviation
    /// between songs. 4000 draws put a sample deviation within about 3 % of
    /// the true one (1.96 / sqrt(2 · 4000) ≈ 2.2 %).
    #[test]
    fn birds_and_songs_spread_by_their_measured_deviations() {
        let birds: Vec<f64> = (0..4000u64)
            .map(|seed| {
                let spread = LevelSpread {
                    utterance_db: 0.0,
                    range_db: (-100.0, 100.0),
                    ..BLACKBIRD
                };
                let gain = VoicePlan::new(with_spread(spread), seed * 7919 + 1, 0.0)
                    .next()
                    .gain;
                decibels(gain)
            })
            .collect();
        let between = standard_deviation(&birds);
        assert!(
            (between / 3.8 - 1.0).abs() < 0.03,
            "between birds {between:.3} dB"
        );

        let mut one_bird = VoicePlan::new(
            with_spread(LevelSpread {
                range_db: (-100.0, 100.0),
                ..BLACKBIRD
            }),
            5,
            0.0,
        );
        let songs: Vec<f64> = decide(&mut one_bird, 4000)
            .iter()
            .map(|planned| decibels(planned.gain))
            .collect();
        let within = standard_deviation(&songs);
        assert!(
            (within / 2.5 - 1.0).abs() < 0.03,
            "between songs {within:.3} dB"
        );
    }

    /// No song is softer or louder than the softest and loudest measured,
    /// however far a bird's own level lies out.
    #[test]
    fn no_song_leaves_the_measured_range() {
        let narrow = LevelSpread {
            individual_db: 10.0,
            utterance_db: 10.0,
            range_db: (-4.0, 3.1),
        };
        let levels: Vec<f64> = (0..200u64)
            .flat_map(|seed| decide(&mut VoicePlan::new(with_spread(narrow), seed, 0.0), 20))
            .map(|planned| decibels(planned.gain))
            .collect();
        assert!(levels
            .iter()
            .all(|level| (-4.0 - 1e-4..=3.1 + 1e-4).contains(level)));
        assert!(levels.iter().any(|level| *level < -3.99));
        assert!(levels.iter().any(|level| *level > 3.09));
    }

    /// Moved to another moment, a bird is the same bird: the same level of
    /// its own and the same songs.
    #[test]
    fn a_restarted_voice_is_the_same_bird() {
        let mut voice = VoicePlan::new(with_spread(BLACKBIRD), 13, 0.0);
        let first = decide(&mut voice, 30);
        assert_eq!(decide(&mut voice.restarted(), 30), first);
    }
}
