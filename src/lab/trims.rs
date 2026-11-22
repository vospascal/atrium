//! The audition's trims: how much each generator candidate is turned up or
//! down so it plays at the reference's level, and whether an edit since the
//! last match has made that trim stale.
//!
//! What makes a match stale: a mixer level, solo or mute, or any generator
//! setting (the working candidate's); moving or changing the loop, the seed
//! or the reference (both generators'). With the match held, a stale trim
//! stays as it was and is marked for a rematch; without it, it is dropped.

use super::candidate::Candidate;

/// An edit that can change what a candidate measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// A working layer's level, solo or mute.
    Mixer,
    /// A working generator setting.
    Setting,
    /// The loop moved or changed, or the seed.
    Loop,
    /// Another reference recording.
    Reference,
}

impl Edit {
    /// The candidates whose level the edit can change. The reference is the
    /// target and is never trimmed.
    fn reaches(self, candidate: Candidate) -> bool {
        match self {
            Edit::Mixer | Edit::Setting => candidate == Candidate::Working,
            Edit::Loop | Edit::Reference => candidate != Candidate::Reference,
        }
    }
}

/// Each candidate's trim (dB): 0 for the reference, and for a generator
/// what the last match measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trims {
    trims_db: [f32; 3],
    matched: [bool; 3],
    stale: [bool; 3],
    /// Keep the trims as they are through edits.
    pub hold: bool,
}

impl Default for Trims {
    fn default() -> Self {
        Self {
            trims_db: [0.0; 3],
            matched: [false; 3],
            stale: [false; 3],
            hold: true,
        }
    }
}

impl Trims {
    /// The trim `candidate` plays at (dB).
    pub fn trim_db(&self, candidate: Candidate) -> f32 {
        self.trims_db[candidate.index()]
    }

    /// Whether a match set `candidate`'s trim.
    pub fn matched(&self, candidate: Candidate) -> bool {
        self.matched[candidate.index()]
    }

    /// Whether an edit since the match may have changed `candidate`'s level:
    /// "Trim held · rematch needed".
    pub fn rematch_needed(&self, candidate: Candidate) -> bool {
        self.stale[candidate.index()]
    }

    /// Note `edit`: held, a matched trim reached by it stays and is marked
    /// stale; not held, it is dropped back to 0 dB, unmatched.
    pub fn after(&mut self, edit: Edit) {
        for candidate in Candidate::ALL {
            let index = candidate.index();
            if !edit.reaches(candidate) || !self.matched[index] {
                continue;
            }
            if self.hold {
                self.stale[index] = true;
            } else {
                self.trims_db[index] = 0.0;
                self.matched[index] = false;
                self.stale[index] = false;
            }
        }
    }

    /// Take the trims a match measured: fresh, none stale.
    pub fn matched_to(&mut self, trims_db: [f32; 3]) {
        self.trims_db = trims_db;
        self.trims_db[Candidate::Reference.index()] = 0.0;
        self.matched = [true; 3];
        self.stale = [false; 3];
    }

    /// Trims as a session saved them.
    pub fn restored(trims_db: [f32; 3], matched: [bool; 3], stale: [bool; 3], hold: bool) -> Self {
        Self {
            trims_db,
            matched,
            stale,
            hold,
        }
    }
}
