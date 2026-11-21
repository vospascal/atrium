// Thunder, worked out from a lightning channel.
//
// A lightning channel is kilometres of jagged segments. When the current
// passes, each segment bursts outward and sends out a short N-shaped pressure
// pulse. Thunder is every segment's pulse arriving at the listener at its own
// travel time (Ribner and Roy, 1982): side-on stretches of channel arrive
// together as claps, the rest smears into a rumble over seconds, and the
// farther parts of a flash arrive later and darker because air absorbs high
// frequencies with distance. `docs/thunder-research.md` has the physics and
// the sources.
//
// Each clap builds a channel fresh from the seed: a ground strike climbs from
// its ground point to the cloud with a few branches, a cloud flash runs
// horizontally for kilometres. Every segment becomes one pulse with an arrival
// time, a level and a low-pass from its distance; the claps, the rolls and the
// darkening tail all come from that geometry. One slow pressure swell under the
// clap carries the infrasonic part for the LFE channel.
//
// The source is silent until told to strike: the session's plan sends it
// `Utterance`s (take = kind of thunder) and it keeps them on the session's
// sample clock, as `crate::audio::voice_node::VoiceNode` keeps its takes.
// Building a channel happens when a strike starts, on the audio thread, into
// storage made beforehand: nothing allocates there.

mod channel;
mod clap;
mod colour;
mod kinds;
mod roar;

use atrium_core::commands::SynthParam;
use atrium_core::source::{SoundSource, Utterance};
use atrium_core::types::Vec3;

use super::noise::Rng;
use clap::Clap;
pub use kinds::clap_seconds;

/// The kinds of thunder, as `Utterance::take` names them.
pub const KIND_CRACK: u8 = 0;
pub const KIND_CLAP: u8 = 1;
pub const KIND_ROLLING: u8 = 2;
pub const KIND_DISTANT: u8 = 3;

/// Claps that can sound at once; a third fades the oldest out.
const CLAPS: usize = 2;
const APPOINTMENTS: usize = 8;

/// Output trim, so every kind peaks under full scale at every rate.
const OUTPUT_TRIM: f32 = 0.9;

/// Procedural thunder, silent until told to strike.
///
/// Each strike is an [`Utterance`]: `take` is the kind ([`KIND_CRACK`],
/// [`KIND_CLAP`], [`KIND_ROLLING`], [`KIND_DISTANT`]), `at_sample` when on the
/// session's clock, `gain` and `rate` its level and speed. Up to two claps
/// sound at once; a third takes the place of the oldest after a 10 ms fade.
pub struct ThunderSource {
    claps: [Clap; CLAPS],
    /// Upcoming strikes, earliest first; `None` is a free place.
    appointments: [Option<Utterance>; APPOINTMENTS],
    /// The session's clock, in output samples.
    clock: u64,
    claps_started: u64,
    /// Output trim.
    pub master_gain: f32,
    /// World-space position: the direction the storm is in.
    pub position: Vec3,
    random: Rng,
}

impl ThunderSource {
    /// A silent thunder source at `position`; `seed` makes its flashes.
    pub fn new(position: Vec3, seed: u64) -> Self {
        Self {
            claps: [
                Clap::new(seed.wrapping_add(11)),
                Clap::new(seed.wrapping_add(23)),
            ],
            appointments: [None; APPOINTMENTS],
            clock: 0,
            claps_started: 0,
            master_gain: 1.0,
            position,
            random: Rng::new(seed),
        }
    }

    /// Start every due strike a clap is free for; when none is free, fade the
    /// oldest so the strike starts a few milliseconds later.
    fn keep_appointments(&mut self) {
        while let Some(utterance) = self.appointments[0].filter(|due| due.at_sample <= self.clock) {
            if let Some(free) = self.claps.iter_mut().find(|clap| !clap.active) {
                self.claps_started += 1;
                free.start(&utterance, self.claps_started, &mut self.random);
                self.appointments.rotate_left(1);
                self.appointments[APPOINTMENTS - 1] = None;
            } else {
                if !self.claps.iter().any(|clap| clap.releasing) {
                    if let Some(oldest) = self.claps.iter_mut().min_by_key(|clap| clap.started) {
                        oldest.release();
                    }
                }
                return;
            }
        }
    }

    #[cfg(test)]
    fn sounding_claps(&self) -> usize {
        self.claps.iter().filter(|clap| clap.active).count()
    }
}

impl SoundSource for ThunderSource {
    /// Only its level changes while it plays.
    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        if !value.is_finite() {
            return;
        }
        if matches!(param, SynthParam::MasterGain) {
            self.master_gain = value.clamp(0.0, 2.0);
        }
    }

    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        self.keep_appointments();
        self.clock += 1;
        let mut sample = 0.0;
        for clap in &mut self.claps {
            if clap.active {
                sample += clap.next_sample(sample_rate);
            }
        }
        sample * self.master_gain * OUTPUT_TRIM
    }

    fn set_clock(&mut self, sample: u64) {
        self.clock = sample;
    }

    /// Kept in time order. With every place taken, the latest appointment
    /// gives way to an earlier one.
    fn schedule(&mut self, utterance: Utterance) {
        let at = self
            .appointments
            .iter()
            .position(|slot| slot.is_none_or(|held| held.at_sample > utterance.at_sample));
        if let Some(at) = at {
            self.appointments[at..].rotate_right(1);
            self.appointments[at] = Some(utterance);
        }
    }

    /// Claps already rolling roll on; only the ones still to come are dropped.
    fn cancel(&mut self) {
        self.appointments = [None; APPOINTMENTS];
    }

    /// The kind of the newest clap sounding, `None` when silent.
    fn sounding_take(&self) -> Option<u8> {
        self.claps
            .iter()
            .filter(|clap| clap.active)
            .max_by_key(|clap| clap.started)
            .map(|clap| clap.kind)
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn tick(&mut self, _dt: f32) {}
}

#[cfg(test)]
mod tests;

/// Claps of every kind as WAV files, to listen to while tuning:
/// `THUNDER_OUT=target/thunder-samples cargo test --release --lib write_listening_samples`.
/// Without `THUNDER_OUT` it writes nothing.
#[cfg(test)]
mod listening_samples;
