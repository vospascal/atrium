// One clap sounding: its channel's pulses played as filtered N-waves, the
// roar read from its moments, and the colour over both.

use std::f32::consts::TAU;

use atrium_core::source::Utterance;

use super::channel::{build_channel, Pulse, MAX_PULSES};
use super::colour::Colour;
use super::kinds::{kind, KINDS};
use super::roar::{shape_the_roar, Moment, MAX_MOMENTS, MOMENT_SECONDS};
use super::KIND_CLAP;
use crate::synth::noise::{white_noise_density_scale, Rng, REFERENCE_SAMPLE_RATE};

/// The most pulses sounding at one moment of a clap; more are dropped, quietest first being likeliest.
const MAX_SOUNDING: usize = 1_024;
/// How long a clap fades when a newer one takes its place (s).
const STEAL_SECONDS: f32 = 0.010;

/// Each of a pulse's three filter stages sits this far above where the air
/// takes 3 dB off, so the three together lose 3 dB there: 1 / √(2^(1/3) − 1).
const STAGE_CUTOFF_FACTOR: f32 = 1.961;

/// The roar's noise falls 6 dB an octave above this (Hz), as thunder's
/// spectrum does above its peak.
const ROAR_TILT_HZ: f32 = 260.0;
/// Below this the roar is let go (Hz); the infrasonic swell lives under it.
const ROAR_FLOOR_HZ: f32 = 70.0;
/// The air's two filter stages sit this far above where it takes 3 dB off,
/// so the two together lose 3 dB there: 1 / √(√2 − 1).
const AIR_STAGE_FACTOR: f32 = 1.554;
/// White noise from −1 to 1 has power 1/3; this makes it 1.
const WHITE_TO_UNIT: f32 = 1.732;

/// A pulse being heard: which one, how far into it, and its three filter stages.
#[derive(Clone, Copy, Debug, Default)]
struct Sounding {
    pulse: u32,
    elapsed: f32,
    first: f32,
    second: f32,
    third: f32,
    coefficient: f32,
}

/// One clap: its channel's pulses in arrival order, and those sounding now.
pub(super) struct Clap {
    pulses: Vec<Pulse>,
    /// Which pulse is the infrasonic swell; `u32::MAX` when there is none.
    swell: u32,
    /// The channel's loudness and brightness, one [`Moment`] per [`MOMENT_SECONDS`].
    moments: Vec<Moment>,
    /// The roar's own noise, and its filters: a tilt, the air, and a floor.
    noise: Rng,
    tilt: f32,
    air_first: f32,
    air_second: f32,
    floor: f32,
    /// The clap's colour: less below the boom, the boom itself, and the
    /// cliff above, set when the output rate is known and the boom moving
    /// slowly through the clap.
    colour: Colour,
    next: usize,
    sounding: [Sounding; MAX_SOUNDING],
    sounding_count: usize,
    /// Seconds into the clap, at its own rate.
    elapsed: f32,
    rate: f32,
    gain: f32,
    pub(super) kind: u8,
    pub(super) active: bool,
    /// 1 while it plays; falls to 0 when a newer clap takes its place.
    steal_gain: f32,
    pub(super) releasing: bool,
    /// Order of starting, to find the oldest.
    pub(super) started: u64,
}

impl Clap {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            pulses: Vec::with_capacity(MAX_PULSES),
            swell: u32::MAX,
            moments: Vec::with_capacity(MAX_MOMENTS),
            noise: Rng::new(seed),
            tilt: 0.0,
            air_first: 0.0,
            air_second: 0.0,
            floor: 0.0,
            colour: Colour::new(),
            next: 0,
            sounding: [Sounding::default(); MAX_SOUNDING],
            sounding_count: 0,
            elapsed: 0.0,
            rate: 1.0,
            gain: 1.0,
            kind: KIND_CLAP,
            active: false,
            steal_gain: 1.0,
            releasing: false,
            started: 0,
        }
    }

    pub(super) fn start(&mut self, utterance: &Utterance, started: u64, random: &mut Rng) {
        self.kind = utterance.take.min(KINDS.len() as u8 - 1);
        let rate = if utterance.rate.is_finite() {
            utterance.rate.clamp(0.25, 4.0)
        } else {
            1.0
        };
        let gain = if utterance.gain.is_finite() {
            utterance.gain.clamp(0.0, 4.0)
        } else {
            1.0
        };
        self.rate = rate;
        self.gain = gain;
        self.swell = build_channel(kind(self.kind), random, &mut self.pulses)
            .map_or(u32::MAX, |index| index as u32);
        shape_the_roar(kind(self.kind), &self.pulses, &mut self.moments);
        self.tilt = 0.0;
        self.air_first = 0.0;
        self.air_second = 0.0;
        self.floor = 0.0;
        self.colour.start(kind(self.kind), random);
        // Started partway: skip what has long been heard.
        let skip = utterance.skip_seconds.max(0.0) * rate;
        self.next = self
            .pulses
            .partition_point(|pulse| pulse.at + pulse.lifetime() < skip);
        self.elapsed = skip;
        self.sounding_count = 0;
        self.active = true;
        self.steal_gain = 1.0;
        self.releasing = false;
        self.started = started;
    }

    pub(super) fn release(&mut self) {
        self.releasing = true;
    }

    pub(super) fn next_sample(&mut self, sample_rate: f32) -> f32 {
        let step = self.rate / sample_rate;
        while self.next < self.pulses.len() && self.pulses[self.next].at <= self.elapsed {
            let pulse = self.pulses[self.next];
            if self.sounding_count < MAX_SOUNDING {
                self.sounding[self.sounding_count] = Sounding {
                    pulse: self.next as u32,
                    elapsed: self.elapsed - pulse.at,
                    first: 0.0,
                    second: 0.0,
                    third: 0.0,
                    coefficient: 1.0
                        - (-TAU * pulse.cutoff * STAGE_CUTOFF_FACTOR * self.rate / sample_rate)
                            .exp(),
                };
                self.sounding_count += 1;
            }
            self.next += 1;
        }

        let mut sum = 0.0;
        let mut swell = 0.0;
        let mut index = 0;
        while index < self.sounding_count {
            let sounding = &mut self.sounding[index];
            let pulse = self.pulses[sounding.pulse as usize];
            // An N-wave: from + down through zero to −, then nothing.
            let value = if sounding.elapsed < pulse.seconds {
                pulse.amplitude * (1.0 - 2.0 * sounding.elapsed / pulse.seconds)
            } else {
                0.0
            };
            sounding.first += sounding.coefficient * (value - sounding.first);
            sounding.second += sounding.coefficient * (sounding.first - sounding.second);
            sounding.third += sounding.coefficient * (sounding.second - sounding.third);
            if sounding.pulse == self.swell {
                swell += sounding.third;
            } else {
                sum += sounding.third;
            }
            sounding.elapsed += step;
            if sounding.elapsed > pulse.lifetime() {
                self.sounding_count -= 1;
                self.sounding[index] = self.sounding[self.sounding_count];
            } else {
                index += 1;
            }
        }
        let roar = self.roar(sample_rate);
        self.elapsed += step;
        let moments_over = self.elapsed / MOMENT_SECONDS >= self.moments.len() as f32;

        if self.releasing {
            self.steal_gain -= 1.0 / (STEAL_SECONDS * sample_rate);
        }
        if self.steal_gain <= 0.0
            || (self.next >= self.pulses.len() && self.sounding_count == 0 && moments_over)
        {
            self.active = false;
            self.steal_gain = 0.0;
        }
        // Where pulses do line up, a gentle limit rounds the peak, as the air
        // near a strike and any microphone do; quieter than a fifth of full
        // scale it changes nothing.
        let coloured = self.colour.process(
            roar,
            kind(self.kind).edge * sum + swell,
            self.elapsed,
            sample_rate,
        );
        soft_limit(coloured * self.gain) * self.steal_gain.max(0.0)
    }

    /// The roar at this moment: noise, tilted down like thunder's spectrum,
    /// darkened by the air as the channel's arriving parts are, and as loud
    /// as they are, both read from the moments and blended between them.
    fn roar(&mut self, sample_rate: f32) -> f32 {
        let position = self.elapsed / MOMENT_SECONDS;
        let index = position as usize;
        let Some(here) = self.moments.get(index).copied() else {
            return 0.0;
        };
        let next = self.moments.get(index + 1).copied().unwrap_or(Moment {
            loudness: 0.0,
            ..here
        });
        let along = position - index as f32;
        let loudness = here.loudness + (next.loudness - here.loudness) * along;
        let cutoff = here.cutoff + (next.cutoff - here.cutoff) * along;

        let coefficient = |hertz: f32| 1.0 - (-TAU * hertz * self.rate / sample_rate).exp();
        let tilt = coefficient(ROAR_TILT_HZ);
        let air = coefficient(cutoff * AIR_STAGE_FACTOR);
        let floor = coefficient(ROAR_FLOOR_HZ);
        // Scaled like every generator's white noise, so its density per Hz
        // is the same at any output rate.
        let white = self.noise.next_bipolar() * white_noise_density_scale(sample_rate);
        self.tilt += tilt * (white - self.tilt);
        self.air_first += air * (self.tilt - self.air_first);
        self.air_second += air * (self.air_first - self.air_second);
        self.floor += floor * (self.air_second - self.floor);
        // White noise through the tilt keeps a share tilt / (2 − tilt) of its
        // power at the reference rate: scale it back to steady power, so
        // `loudness` is its level.
        let reference_tilt = 1.0 - (-TAU * ROAR_TILT_HZ * self.rate / REFERENCE_SAMPLE_RATE).exp();
        let steady = ((2.0 - reference_tilt) / reference_tilt).sqrt() * WHITE_TO_UNIT;
        (self.air_second - self.floor) * steady * loudness
    }
}

/// Unchanged below a fifth of full scale, then bending smoothly toward 1.
fn soft_limit(value: f32) -> f32 {
    const KNEE: f32 = 0.2;
    let size = value.abs();
    if size <= KNEE {
        return value;
    }
    let over = (size - KNEE) / (1.0 - KNEE);
    value.signum() * (KNEE + (1.0 - KNEE) * over / (1.0 + over))
}
