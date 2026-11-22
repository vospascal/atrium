//! What can be heard in the lab: the reference recording, the previous
//! version of the generator and the working one.

/// One of the three things the lab switches between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Candidate {
    /// The recording the generator is tuned towards.
    Reference,
    /// The baseline for this round: the generator as it was.
    Previous,
    /// The generator with this round's changes, live.
    Working,
}

impl Candidate {
    pub const ALL: [Candidate; 3] = [
        Candidate::Reference,
        Candidate::Previous,
        Candidate::Working,
    ];

    /// Where it is counted in arrays of three.
    pub fn index(self) -> usize {
        match self {
            Candidate::Reference => 0,
            Candidate::Previous => 1,
            Candidate::Working => 2,
        }
    }

    /// The candidate counted at `index`, if there is one.
    pub fn at(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The name a session file uses.
    pub fn name(self) -> &'static str {
        match self {
            Candidate::Reference => "reference",
            Candidate::Previous => "previous",
            Candidate::Working => "working",
        }
    }

    /// The candidate a session file names.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.name() == name)
    }
}
