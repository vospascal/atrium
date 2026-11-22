//! Matching levels: each candidate measured alone at the output tap, by the
//! analyser's A-weighted level while it sounds, and the generators trimmed
//! to the reference.
//!
//! The live analysis gives the A-weighted level of every 50 ms column of
//! what the output played (`atrium_analysis::Live`). From those the meter
//! forms the 400 ms blocks, 100 ms apart, of the level while sounding
//! (`atrium_analysis::power_while_sounding`, ITU-R BS.1770's two gates):
//! blocks below the threshold of hearing are silence, and blocks 10 dB below
//! the mean of the rest are the quiet between gusts. A candidate is measured
//! until 10 s of it have sounded, across as many passes of the loop as that
//! takes, or for 60 s at most. The two halves of the passage are measured
//! apart as well, so a match on a passage whose level swings shows that it
//! does.

use super::candidate::Candidate;

/// The live analysis's column (s).
const COLUMN_SECONDS: f64 = atrium_analysis::Live::COLUMN_SECONDS;
/// A gating block is 8 columns (400 ms), and blocks start every 2 (100 ms).
const BLOCK_COLUMNS: usize = 8;
const STEP_COLUMNS: usize = 2;
/// Blocks below the threshold of hearing are silence (dB SPL).
const ABSOLUTE_GATE_DB: f64 = 0.0;
/// Blocks this far below the mean of the blocks above silence are the quiet
/// between events (dB).
const RELATIVE_GATE_DB: f64 = -10.0;
/// How much of a candidate must have sounded before its level is taken (s).
pub const SOUNDING_SECONDS: f64 = 10.0;
/// The longest a candidate is listened to, sounding or not (s).
pub const LONGEST_SECONDS: f64 = 60.0;

/// Whether the output at `clock` lay in the first half of the passage, for
/// a voice whose pass under way began at `origin_clock` and whose passes are
/// `loop_frames` long and follow on without a break.
pub fn in_first_half(clock: u64, origin_clock: u64, loop_frames: u64) -> bool {
    let loop_frames = loop_frames.max(1) as i64;
    let into = (clock as i64 - origin_clock as i64).rem_euclid(loop_frames);
    into < loop_frames / 2
}

/// One candidate's level, as measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// A-weighted, while sounding (dB SPL).
    pub level_db: f64,
    /// The same over the first and over the second half of the passage, when
    /// each had a block that sounded.
    pub halves_db: [Option<f64>; 2],
    /// How long it sounded (s).
    pub sounding_seconds: f64,
}

impl Reading {
    /// How far apart the two halves of the passage are (dB), when both sounded.
    pub fn halves_apart_db(&self) -> Option<f64> {
        match self.halves_db {
            [Some(first), Some(second)] => Some((first - second).abs()),
            _ => None,
        }
    }
}

/// The level of one candidate, built up column by column.
#[derive(Clone, Debug, Default)]
pub struct LevelMeter {
    /// Each column's A-weighted power (relative to 0 dB SPL), and whether it
    /// lay in the first half of the passage.
    columns: Vec<(f64, bool)>,
}

impl LevelMeter {
    /// Add a column whose A-weighted level was `level_db` (dB SPL).
    pub fn push(&mut self, level_db: f64, first_half: bool) {
        let power = if level_db.is_finite() {
            10f64.powf(level_db / 10.0)
        } else {
            0.0
        };
        self.columns.push((power, first_half));
    }

    /// How long it has been listened to (s).
    pub fn heard_seconds(&self) -> f64 {
        self.columns.len() as f64 * COLUMN_SECONDS
    }

    /// How long it has sounded (s): the blocks that pass both gates, each
    /// standing for the 100 ms step.
    pub fn sounding_seconds(&self) -> f64 {
        self.sounding_blocks().len() as f64 * STEP_COLUMNS as f64 * COLUMN_SECONDS
    }

    /// Whether it has been listened to for long enough.
    pub fn done(&self) -> bool {
        self.sounding_seconds() >= SOUNDING_SECONDS || self.heard_seconds() >= LONGEST_SECONDS
    }

    /// Its level, once anything has sounded.
    pub fn reading(&self) -> Option<Reading> {
        let sounding = self.sounding_blocks();
        let level = |blocks: &mut dyn Iterator<Item = f64>| {
            let (sum, count) = blocks.fold((0.0, 0usize), |(sum, count), power| {
                (sum + power, count + 1)
            });
            (count > 0).then(|| 10.0 * (sum / count as f64).log10())
        };
        let level_db = level(&mut sounding.iter().map(|(power, _)| *power))?;
        let half = |first: bool| {
            level(
                &mut sounding
                    .iter()
                    .filter(|(_, in_first)| *in_first == first)
                    .map(|(power, _)| *power),
            )
        };
        Some(Reading {
            level_db,
            halves_db: [half(true), half(false)],
            sounding_seconds: self.sounding_seconds(),
        })
    }

    /// The gating blocks that pass both gates: their power, and whether they
    /// start in the first half of the passage.
    fn sounding_blocks(&self) -> Vec<(f64, bool)> {
        if self.columns.len() < BLOCK_COLUMNS {
            return Vec::new();
        }
        let blocks: Vec<(f64, bool)> = (0..=(self.columns.len() - BLOCK_COLUMNS) / STEP_COLUMNS)
            .map(|block| {
                let start = block * STEP_COLUMNS;
                let span = &self.columns[start..start + BLOCK_COLUMNS];
                let power = span.iter().map(|(power, _)| power).sum::<f64>() / BLOCK_COLUMNS as f64;
                (power, span[0].1)
            })
            .collect();
        let absolute_gate = 10f64.powf(ABSOLUTE_GATE_DB / 10.0);
        let above_silence: Vec<(f64, bool)> = blocks
            .into_iter()
            .filter(|(power, _)| *power > absolute_gate)
            .collect();
        if above_silence.is_empty() {
            return above_silence;
        }
        let mean =
            above_silence.iter().map(|(power, _)| power).sum::<f64>() / above_silence.len() as f64;
        let relative_gate = mean * 10f64.powf(RELATIVE_GATE_DB / 10.0);
        above_silence
            .into_iter()
            .filter(|(power, _)| *power > relative_gate)
            .collect()
    }
}

/// What a match found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchOutcome {
    /// Each candidate's reading, by [`Candidate::index`]; none for one that
    /// never sounded.
    pub readings: [Option<Reading>; 3],
    /// The trims to play at from now on (dB): 0 for the reference; for a
    /// generator, the trim it was measured at plus what it lacked. A
    /// generator that never sounded keeps its trim.
    pub trims_db: [f32; 3],
}

impl MatchOutcome {
    /// How far `candidate` measured from the reference, at the trim it was
    /// measured at (dB).
    pub fn difference_db(&self, candidate: Candidate) -> Option<f64> {
        let reference = self.readings[Candidate::Reference.index()]?;
        let measured = self.readings[candidate.index()]?;
        Some(measured.level_db - reference.level_db)
    }
}

/// A match under way: the reference, then the previous generator, then the
/// working one, each alone until its meter is done.
#[derive(Clone, Debug)]
pub struct LevelMatch {
    meters: [LevelMeter; 3],
    /// The candidate being measured, by index; 3 once all are.
    measuring: usize,
    /// The trims each candidate is measured at (dB).
    trims_db: [f32; 3],
}

impl LevelMatch {
    /// A match of candidates playing at `trims_db`.
    pub fn new(trims_db: [f32; 3]) -> Self {
        Self {
            meters: Default::default(),
            measuring: 0,
            trims_db,
        }
    }

    /// The candidate to hear now, until all are measured.
    pub fn measuring(&self) -> Option<Candidate> {
        Candidate::at(self.measuring)
    }

    /// Add a column of the candidate being measured. Its meter done, the
    /// next candidate is measured.
    pub fn push(&mut self, level_db: f64, first_half: bool) {
        let Some(meter) = self.meters.get_mut(self.measuring) else {
            return;
        };
        meter.push(level_db, first_half);
        if meter.done() {
            self.measuring += 1;
        }
    }

    /// How far along the match is, 0 to 1.
    pub fn progress(&self) -> f32 {
        let within = self.meters.get(self.measuring).map_or(0.0, |meter| {
            (meter.sounding_seconds() / SOUNDING_SECONDS)
                .max(meter.heard_seconds() / LONGEST_SECONDS)
                .min(1.0)
        });
        ((self.measuring as f64 + within) / 3.0) as f32
    }

    /// What it found, once every candidate is measured.
    pub fn outcome(&self) -> Option<MatchOutcome> {
        if self.measuring < self.meters.len() {
            return None;
        }
        let readings = [0, 1, 2].map(|index| self.meters[index].reading());
        let mut trims_db = self.trims_db;
        trims_db[Candidate::Reference.index()] = 0.0;
        if let Some(reference) = readings[Candidate::Reference.index()] {
            for candidate in [Candidate::Previous, Candidate::Working] {
                if let Some(reading) = readings[candidate.index()] {
                    let index = candidate.index();
                    trims_db[index] += (reference.level_db - reading.level_db) as f32;
                }
            }
        }
        Some(MatchOutcome { readings, trims_db })
    }
}
