//! The lab's voice on the audio thread: the reference recording and the two
//! generator candidates playing in step, one heard at a time.
//!
//! All three advance every sample whichever is heard, so switching keeps
//! the place in the passage. A switch fades over 10 ms. Between the
//! reference and a generator, two independent noises, the fade keeps the
//! power constant (sine and cosine); between the two generators, which share
//! their seed and so their noise, it keeps the amplitude constant (straight
//! lines), as either law would bump or dip the other kind of pair by 3 dB.
//!
//! Every wrap of the loop, and every replay, puts back the state each
//! generator had at the cue, kept when the voice was built: a copy, which
//! costs no allocation, so the passage and its lead-in are the same samples
//! every time. The working generator's level and colour settings are applied
//! on top, so a change to them is heard at once. Loop edges fade over 5 ms.
//!
//! Nothing here allocates, frees or takes a lock.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::types::Vec3;

use super::candidate::Candidate;
use super::controls::{LabControls, Wanted};
use super::settings::apply_live;
use crate::audio::decode::AudioBuffer;
use crate::audio::synth_node::SynthNode;
use crate::config::BuiltSound;
use crate::synth::soft_wind::SoftWindSource;

/// How long a switch between candidates, or pausing and playing, fades (s).
pub const SWITCH_FADE_SECONDS: f32 = 0.010;
/// How long the loop's edges, and a replay, fade (s).
pub const EDGE_FADE_SECONDS: f32 = 0.005;

/// What a voice is made of, prepared off the audio thread.
pub struct VoiceParts {
    pub controls: Arc<LabControls>,
    /// The rate the generators' states were kept at, the output's.
    pub sample_rate: f32,
    /// The reference recording, mono, and the gain that brings it to the
    /// generator's level before any trim.
    pub reference: Arc<AudioBuffer>,
    pub reference_gain: f32,
    /// Where the reference's loop starts in the recording (s).
    pub reference_from_seconds: f64,
    /// How long the passage is (s), the same for all three.
    pub loop_seconds: f64,
    /// Each generator candidate as it is at its cue, set up with its
    /// settings.
    pub previous: SoftWindSource,
    pub working: SoftWindSource,
}

/// A generator candidate: as it plays, and its state at the cue.
struct Generated {
    live: SoftWindSource,
    kept: SoftWindSource,
}

impl Generated {
    fn new(kept: SoftWindSource) -> Self {
        // The live one shares the kept one's repertoire from the start, so
        // putting the kept state back never frees anything.
        Self {
            live: kept.clone(),
            kept,
        }
    }

    #[inline]
    fn rewind(&mut self) {
        self.live.clone_from(&self.kept);
    }
}

/// The lab's voice. Wrap it with [`placed_like`] to put it in the scene.
pub struct LabVoice {
    controls: Arc<LabControls>,
    reference: Arc<AudioBuffer>,
    reference_gain: f32,
    /// In the recording's own frames.
    reference_start: f64,
    reference_step: f64,
    reference_position: f64,
    loop_frames: usize,
    /// Frames into the pass.
    position: usize,
    previous: Generated,
    working: Generated,
    revision: u32,
    wanted: Wanted,
    replays_seen: u32,
    trims: [f32; 3],
    /// How far each candidate is faded in, 0 to 1, and whether the switch
    /// under way keeps power (one side is the reference) or amplitude.
    weights: [f32; 3],
    equal_power: bool,
    settled: bool,
    play_gain: f32,
    /// Samples left of the fade before a replay starts the passage again.
    restart_countdown: usize,
    fade_step: f32,
    edge_frames: f32,
    block_clock: u64,
    block_sample: u64,
    /// The clock sample the pass under way began at.
    origin_clock: u64,
}

impl LabVoice {
    pub fn new(parts: VoiceParts) -> Self {
        let sample_rate = parts.sample_rate.max(1.0);
        let (revision, wanted) = parts.controls.current();
        let mut voice = Self {
            reference_start: parts.reference_from_seconds.max(0.0)
                * parts.reference.sample_rate as f64,
            reference_step: parts.reference.sample_rate as f64 / sample_rate as f64,
            reference_position: 0.0,
            reference: parts.reference,
            reference_gain: parts.reference_gain,
            loop_frames: ((parts.loop_seconds * sample_rate as f64).round() as usize).max(1),
            position: 0,
            previous: Generated::new(parts.previous),
            working: Generated::new(parts.working),
            revision,
            wanted,
            replays_seen: wanted.replays,
            trims: [1.0; 3],
            weights: [0.0; 3],
            equal_power: false,
            settled: false,
            play_gain: 0.0,
            restart_countdown: 0,
            fade_step: 1.0 / (SWITCH_FADE_SECONDS * sample_rate),
            edge_frames: EDGE_FADE_SECONDS * sample_rate,
            block_clock: 0,
            block_sample: 0,
            origin_clock: 0,
            controls: parts.controls,
        };
        voice.weights[wanted.heard.index()] = 1.0;
        voice.trims = wanted.trims_db.map(|trim| 10.0_f32.powf(trim / 20.0));
        voice.restart(0);
        voice
    }

    /// Take what the control thread asks for, if anything changed.
    fn read_controls(&mut self) {
        let Some((revision, wanted)) = self.controls.wanted(self.revision) else {
            return;
        };
        self.revision = revision;
        if wanted.heard != self.wanted.heard {
            let reference = Candidate::Reference;
            self.equal_power = wanted.heard == reference || self.wanted.heard == reference;
            self.settled = false;
            self.controls.report_sounding(None);
        }
        if wanted.replays != self.replays_seen {
            self.replays_seen = wanted.replays;
            // Playing: fade out, then start again. Silent: start again now.
            if self.play_gain > 0.0 {
                // The fade's samples, then the one the passage starts on.
                self.restart_countdown = self.edge_frames.max(1.0) as usize + 1;
            } else {
                let clock = self.block_clock + self.block_sample;
                self.restart(clock);
            }
        }
        if wanted.working != self.wanted.working {
            apply_live(&mut self.working.live, &wanted.working);
        }
        self.trims = wanted.trims_db.map(|trim| 10.0_f32.powf(trim / 20.0));
        self.wanted = wanted;
    }

    /// Start the passage again: every candidate at its cue.
    #[inline]
    fn restart(&mut self, clock: u64) {
        self.previous.rewind();
        self.working.rewind();
        apply_live(&mut self.working.live, &self.wanted.working);
        self.reference_position = self.reference_start;
        self.position = 0;
        self.restart_countdown = 0;
        self.origin_clock = clock;
    }

    /// The reference recording at the voice's place in it, between samples
    /// by a straight line; silence past its end.
    #[inline]
    fn reference_sample(&mut self) -> f32 {
        let samples = &self.reference.samples;
        let index = self.reference_position as usize;
        let fraction = (self.reference_position - index as f64) as f32;
        let first = samples.get(index).copied().unwrap_or(0.0);
        let second = samples.get(index + 1).copied().unwrap_or(0.0);
        self.reference_position += self.reference_step;
        first + (second - first) * fraction
    }

    /// Move each candidate's fade a step toward the one asked for; once only
    /// it sounds, report it, from `clock` on.
    #[inline]
    fn fade_between_candidates(&mut self, clock: u64) {
        if self.settled {
            return;
        }
        let heard = self.wanted.heard.index();
        let mut arrived = true;
        for (index, weight) in self.weights.iter_mut().enumerate() {
            let target = if index == heard { 1.0 } else { 0.0 };
            *weight = approach(*weight, target, self.fade_step);
            arrived &= *weight == target;
        }
        if arrived {
            self.settled = true;
            self.controls
                .report_sounding(Some((self.wanted.heard, clock)));
        }
    }

    #[inline]
    fn candidate_gain(&self, index: usize) -> f32 {
        let weight = self.weights[index];
        let shaped = if self.equal_power {
            (weight * FRAC_PI_2).sin()
        } else {
            weight
        };
        shaped * self.trims[index]
    }

    /// The fade at the passage's edges and before a replay.
    #[inline]
    fn edge_gain(&self) -> f32 {
        let into = (self.position + 1) as f32 / self.edge_frames;
        let left = (self.loop_frames - self.position) as f32 / self.edge_frames;
        let restarting = if self.restart_countdown > 0 {
            self.restart_countdown as f32 / self.edge_frames
        } else {
            1.0
        };
        into.min(left).min(restarting).min(1.0)
    }
}

impl SoundSource for LabVoice {
    #[inline]
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        let clock = self.block_clock + self.block_sample;
        self.block_sample += 1;
        let playing = if self.wanted.playing { 1.0 } else { 0.0 };
        if playing == 0.0 && self.play_gain == 0.0 {
            // Paused: everything holds where it is, and a switch is instant;
            // the candidate is reported from the first sample it plays.
            self.weights = [0.0; 3];
            self.weights[self.wanted.heard.index()] = 1.0;
            return 0.0;
        }
        self.play_gain = approach(self.play_gain, playing, self.fade_step);

        if self.position >= self.loop_frames {
            self.restart(clock);
            if !self.wanted.looping {
                // Once through: stop at the start, ready to play it again.
                self.wanted.playing = false;
                self.play_gain = 0.0;
                self.controls.report_stopped();
                return 0.0;
            }
        }
        if self.restart_countdown > 0 {
            self.restart_countdown -= 1;
            if self.restart_countdown == 0 {
                self.restart(clock);
            }
        }
        self.fade_between_candidates(clock);
        let edge = self.edge_gain();
        let reference = self.reference_sample() * self.reference_gain;
        let previous = self.previous.live.next_sample(sample_rate);
        let working = self.working.live.next_sample(sample_rate);
        self.position += 1;
        let mixed = self.candidate_gain(0) * reference
            + self.candidate_gain(1) * previous
            + self.candidate_gain(2) * working;
        mixed * edge * self.play_gain
    }

    fn position(&self) -> Vec3 {
        Vec3::ZERO
    }

    fn emitter_kind(&self) -> EmitterKind {
        self.working.live.emitter_kind()
    }

    fn set_clock(&mut self, sample: u64) {
        self.block_clock = sample;
        self.block_sample = 0;
    }

    fn tick(&mut self, _dt: f32) {
        self.read_controls();
        self.controls
            .report_position(self.position as u64, self.origin_clock);
    }
}

/// `voice` in the scene as `built` would have been: its level, its shape and
/// its place, so the lab's candidates go through the same render as the
/// sound they stand for.
pub fn placed_like(voice: LabVoice, built: &BuiltSound) -> Box<dyn SoundSource> {
    let mut node = SynthNode::new(Box::new(voice), built.description.position);
    node.amplitude = built.amplitude;
    node.ref_dist = built.source.ref_distance();
    node.pattern = built.source.directivity();
    node.spread = built.source.spread();
    node.set_arc_degrees(built.source.arc_degrees());
    node.set_emitter_kind(built.source.emitter_kind());
    Box::new(node)
}

/// `value` moved `step` toward `target`, not past it.
#[inline]
fn approach(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}
