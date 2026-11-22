//! What the control thread asks of the lab's voice, and what the voice
//! reports back: shared numbers, read and written without a lock.
//!
//! The control thread writes what it wants and then counts a revision; the
//! voice reads everything again at the start of a block when the revision
//! has moved. The voice reports where it is in the passage, which candidate
//! is heard alone and since when, on the same audio clock the output tap's
//! blocks are stamped with.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use super::candidate::Candidate;
use super::settings::LIVE;

/// No candidate: a switch is under way, or nothing plays.
const NONE: u8 = u8::MAX;

/// Shared between the lab on the control thread and its voice.
#[derive(Debug)]
pub struct LabControls {
    revision: AtomicU32,
    heard: AtomicU8,
    trims_db: [AtomicU32; 3],
    working: [AtomicU32; LIVE.len()],
    playing: AtomicBool,
    looping: AtomicBool,
    replays: AtomicU32,
    // Reported by the voice.
    position_frames: AtomicU64,
    sounding: AtomicU8,
    settled_clock: AtomicU64,
    origin_clock: AtomicU64,
}

/// What the voice reads at the start of a block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Wanted {
    pub(crate) heard: Candidate,
    pub(crate) trims_db: [f32; 3],
    pub(crate) working: [f32; LIVE.len()],
    pub(crate) playing: bool,
    pub(crate) looping: bool,
    pub(crate) replays: u32,
}

impl LabControls {
    /// Controls that ask for `heard`, paused, looping, untrimmed, with the
    /// working generator's live values at `working`.
    pub fn new(heard: Candidate, working: [f32; LIVE.len()]) -> Self {
        Self {
            revision: AtomicU32::new(0),
            heard: AtomicU8::new(heard.index() as u8),
            trims_db: Default::default(),
            working: working.map(|value| AtomicU32::new(value.to_bits())),
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(true),
            replays: AtomicU32::new(0),
            position_frames: AtomicU64::new(0),
            sounding: AtomicU8::new(NONE),
            settled_clock: AtomicU64::new(0),
            origin_clock: AtomicU64::new(0),
        }
    }

    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::Release);
    }

    /// Switch to `candidate`, over the voice's fade.
    pub fn hear(&self, candidate: Candidate) {
        self.heard.store(candidate.index() as u8, Ordering::Relaxed);
        self.changed();
    }

    /// Play `candidate` `trim_db` louder (negative: softer).
    pub fn set_trim(&self, candidate: Candidate, trim_db: f32) {
        self.trims_db[candidate.index()].store(trim_db.to_bits(), Ordering::Relaxed);
        self.changed();
    }

    /// Give the working generator these values of [`LIVE`], heard at once.
    pub fn set_working(&self, values: [f32; LIVE.len()]) {
        for (slot, value) in self.working.iter().zip(values) {
            slot.store(value.to_bits(), Ordering::Relaxed);
        }
        self.changed();
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::Relaxed);
        self.changed();
    }

    pub fn set_looping(&self, looping: bool) {
        self.looping.store(looping, Ordering::Relaxed);
        self.changed();
    }

    /// Start the passage again from its cues, playing, even mid-pass.
    pub fn replay(&self) {
        self.playing.store(true, Ordering::Relaxed);
        self.replays.fetch_add(1, Ordering::Relaxed);
        self.changed();
    }

    /// Whether play was asked for. The voice clears it when a pass ends with
    /// the loop off.
    pub fn asked_to_play(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// The candidate asked for.
    pub fn asked_to_hear(&self) -> Candidate {
        Candidate::at(self.heard.load(Ordering::Relaxed) as usize).unwrap_or(Candidate::Working)
    }

    pub fn looping(&self) -> bool {
        self.looping.load(Ordering::Relaxed)
    }

    /// What is asked now, and its revision.
    pub(crate) fn current(&self) -> (u32, Wanted) {
        let revision = self.revision.load(Ordering::Acquire);
        let float = |slot: &AtomicU32| f32::from_bits(slot.load(Ordering::Relaxed));
        (
            revision,
            Wanted {
                heard: self.asked_to_hear(),
                trims_db: std::array::from_fn(|index| float(&self.trims_db[index])),
                working: std::array::from_fn(|index| float(&self.working[index])),
                playing: self.playing.load(Ordering::Relaxed),
                looping: self.looping(),
                replays: self.replays.load(Ordering::Relaxed),
            },
        )
    }

    /// What is asked, if it changed since `last_revision`.
    pub(crate) fn wanted(&self, last_revision: u32) -> Option<(u32, Wanted)> {
        if self.revision.load(Ordering::Acquire) == last_revision {
            return None;
        }
        Some(self.current())
    }

    // ── What the voice reports ──

    /// How far into the passage the voice is (output frames).
    pub fn position_frames(&self) -> u64 {
        self.position_frames.load(Ordering::Relaxed)
    }

    /// The candidate heard alone, its switch over, and the audio clock sample
    /// from which it was; none while a switch is under way.
    pub fn sounding(&self) -> Option<(Candidate, u64)> {
        let candidate = Candidate::at(self.sounding.load(Ordering::Acquire) as usize)?;
        Some((candidate, self.settled_clock.load(Ordering::Relaxed)))
    }

    /// The audio clock sample at which the pass under way began, had the
    /// voice played without a break since.
    pub fn origin_clock(&self) -> u64 {
        self.origin_clock.load(Ordering::Relaxed)
    }

    pub(crate) fn report_position(&self, frames: u64, origin_clock: u64) {
        self.position_frames.store(frames, Ordering::Relaxed);
        self.origin_clock.store(origin_clock, Ordering::Relaxed);
    }

    pub(crate) fn report_sounding(&self, sounding: Option<(Candidate, u64)>) {
        match sounding {
            Some((candidate, clock)) => {
                self.settled_clock.store(clock, Ordering::Relaxed);
                self.sounding
                    .store(candidate.index() as u8, Ordering::Release);
            }
            None => self.sounding.store(NONE, Ordering::Release),
        }
    }

    /// A pass ended with the loop off: play is no longer asked for.
    pub(crate) fn report_stopped(&self) {
        self.playing.store(false, Ordering::Relaxed);
        self.changed();
    }
}
