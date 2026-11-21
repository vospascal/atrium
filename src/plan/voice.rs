//! One voice's decisions, made ahead: which take, when, how loud, how high.
//!
//! Every choice comes from one seeded generator, so the same voice with the
//! same seed decides the same way again, and two seeds never fall into step.

use super::rhythm::Rhythm;

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
}

impl VoicePlan {
    /// A voice that starts deciding at `from` seconds. It opens with a rest
    /// of a random part of its longest gap, so several voices started
    /// together do not open with a chord.
    pub(super) fn new(rhythm: Rhythm, seed: u64, from: f64) -> Self {
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
}
