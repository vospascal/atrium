//! What a plan needs to know about a voice: its takes and how it rests.

/// How a voice rests between takes, and how much one utterance may differ
/// from the next.
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
        }
    }
}

/// One take as a plan sees it: how long it lasts and how often it is chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TakeRhythm {
    /// Its length as recorded (s).
    pub seconds: f32,
    /// How often it is chosen next to the others: 3 three times as often as
    /// 1, and 0 never.
    pub chance: f32,
}

/// Everything a plan needs to decide when a voice sounds and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Rhythm {
    pub takes: Vec<TakeRhythm>,
    pub rests: Rests,
}
