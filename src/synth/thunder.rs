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

use std::f32::consts::TAU;

use atrium_core::commands::SynthParam;
use atrium_core::source::{SoundSource, Utterance};
use atrium_core::types::Vec3;

use super::noise::Rng;
use crate::audio::filters::Biquad;

/// The kinds of thunder, as `Utterance::take` names them.
pub const KIND_CRACK: u8 = 0;
pub const KIND_CLAP: u8 = 1;
pub const KIND_ROLLING: u8 = 2;
pub const KIND_DISTANT: u8 = 3;

/// Metres per second, at about 20 °C.
const SPEED_OF_SOUND: f32 = 343.0;
/// Claps that can sound at once; a third fades the oldest out.
const CLAPS: usize = 2;
const APPOINTMENTS: usize = 8;
/// The most pulses one clap is made of: segments of channel and branches.
const MAX_PULSES: usize = 16_000;
/// The most pulses sounding at one moment of a clap; more are dropped, quietest first being likeliest.
const MAX_SOUNDING: usize = 1_024;
/// How long a clap fades when a newer one takes its place (s).
const STEAL_SECONDS: f32 = 0.010;
/// Mean turn from one segment of channel to the next (radians): about 16°,
/// the tortuosity measured on real lightning.
const MEAN_TURN: f32 = 0.28;
/// A pulse's length 1 km from its segment (s); it lengthens a little with distance.
const PULSE_SECONDS_AT_A_KILOMETRE: f32 = 0.0025;
/// Pulses each segment sounds as, for its crookedness below a segment's length.
const MICRO_PULSES: usize = 10;
/// How far those pulses spread in time beyond the segment's own length (s).
const MICRO_JITTER_SECONDS: f32 = 0.004;
/// The most the ground takes off sound that grazes it the whole way (dB).
const GROUND_LOSS_DB: f32 = 14.0;
/// Sound arriving at this elevation or steeper loses nothing to the ground (radians, about 15°).
const GROUND_LOSS_BELOW: f32 = 0.26;
/// From this far the ground's loss is full; nearer, it is less (m).
const GROUND_LOSS_FULL_FROM: f32 = 2_000.0;
/// Air absorption at 1 kHz (dB per metre); it grows with the square of the frequency.
const ABSORPTION_AT_A_KILOHERTZ: f32 = 0.005;
/// How much louder a segment side-on to the listener is than one end-on: the
/// claps in a rumble.
const SIDE_ON: f32 = 0.6;
/// Each of a pulse's three filter stages sits this far above where the air
/// takes 3 dB off, so the three together lose 3 dB there: 1 / √(2^(1/3) − 1).
const STAGE_CUTOFF_FACTOR: f32 = 1.961;
/// Window over which pulses that arrive together are added up to find a
/// clap's loudest moment (s).
const LOUDEST_WINDOW_SECONDS: f32 = 0.015;
/// Share of a clap's own span over which its last arrivals fade.
const FADE_SHARE: f32 = 0.4;
/// How far a cloud flash's course turns from running straight away from the
/// listener (radians): up to 60° either way.
const MAX_FLASH_ANGLE: f32 = 1.05;
/// How loud the run inside the cloud is next to the channel to the ground:
/// as loud, since a flash keeps its energy along its whole run, which is
/// what holds a rumble up for seconds instead of letting it sag at once.
const IN_CLOUD_STRENGTH: f32 = 1.0;
/// Below this a clap thins out (Hz).
const LOW_CUT_HZ: f32 = 90.0;
/// Where a flash's boom can sit (Hz), and how much it lifts (dB) and how broad it is.
const BOOM_HZ: (f32, f32) = (170.0, 230.0);
const BOOM_DB: f32 = 4.0;
const BOOM_Q: f32 = 1.0;
/// How far the boom drifts either way, as a share of its pitch, and how fast (Hz).
const BOOM_DRIFT: f32 = 0.15;
const BOOM_DRIFT_HZ: (f32, f32) = (0.15, 0.4);
/// How long one moment of a clap's roar is (s): short enough to keep the
/// flicker of a rumble, long enough to hold many pulses.
const MOMENT_SECONDS: f32 = 0.010;
/// The most moments a clap has: its longest kind, with room to spare.
const MAX_MOMENTS: usize = 2_600;
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
/// The roar's loudest moment, as a share of the kind's level, before the
/// limit rounds the peaks of the noise.
const ROAR_LEVEL: f32 = 0.28;
/// How loud a branch is next to the main channel: it carries less current.
const BRANCH_STRENGTH: f32 = 0.5;
/// Output trim, so every kind peaks under full scale at every rate.
const OUTPUT_TRIM: f32 = 0.9;

/// What makes one kind of thunder: the flash and where it is.
struct Kind {
    /// A ground strike climbs from the ground to the cloud; otherwise a flash
    /// runs horizontally inside the cloud.
    ground: bool,
    /// Distance to the strike's ground point, or to the nearest point of a
    /// cloud flash (m).
    distance: (f32, f32),
    /// Length of the main channel (m).
    length: (f32, f32),
    /// Cloud base for a ground strike, height of a cloud flash (m).
    height: (f32, f32),
    /// How far a ground strike's channel runs on inside the cloud once it
    /// reaches it (m): the long, darker tail of a rumble.
    in_cloud: (f32, f32),
    branches: u32,
    /// Length of one segment of channel (m).
    step: f32,
    /// Its loudest moment, before the output trim.
    level: f32,
    /// How much of the pulses themselves is heard over the roar: the edge of
    /// a crack close by, little of it from far away.
    edge: f32,
    /// Above this the sound falls steeply (Hz): thunder heard from a few
    /// kilometres keeps almost nothing above about 1 kHz.
    cliff_hz: f32,
    /// The infrasonic swell's level, as a share of `level`, and its length (s).
    infra: f32,
    infra_seconds: (f32, f32),
    /// The longest a clap of this kind lasts (s).
    seconds: f32,
}

const KINDS: [Kind; 4] = [
    // A ground strike a few hundred metres away: a sharp crack and a short roll.
    Kind {
        ground: true,
        distance: (250.0, 600.0),
        length: (2_000.0, 3_000.0),
        height: (1_500.0, 2_500.0),
        in_cloud: (800.0, 2_000.0),
        branches: 4,
        step: 3.0,
        level: 1.0,
        cliff_hz: 2_500.0,
        edge: 0.9,
        infra: 0.01,
        infra_seconds: (0.10, 0.20),
        seconds: 9.0,
    },
    // A ground strike 1.5–3 km off: an onset, a body, a darkening tail.
    Kind {
        ground: true,
        distance: (1_500.0, 3_000.0),
        length: (2_000.0, 3_000.0),
        height: (2_000.0, 3_000.0),
        in_cloud: (500.0, 1_500.0),
        branches: 5,
        step: 4.0,
        level: 0.7,
        cliff_hz: 1_100.0,
        edge: 0.3,
        infra: 0.02,
        infra_seconds: (0.15, 0.30),
        seconds: 12.0,
    },
    // A long flash inside the cloud: rolling thunder that swells and fades.
    Kind {
        ground: false,
        distance: (1_500.0, 4_000.0),
        length: (5_000.0, 9_000.0),
        height: (3_000.0, 5_000.0),
        in_cloud: (0.0, 0.0),
        branches: 3,
        step: 5.0,
        level: 0.6,
        cliff_hz: 950.0,
        edge: 0.25,
        infra: 0.025,
        infra_seconds: (0.20, 0.40),
        seconds: 22.0,
    },
    // A ground strike 8–14 km off: a low grumble, the highs long gone.
    Kind {
        ground: true,
        distance: (8_000.0, 14_000.0),
        length: (3_000.0, 5_000.0),
        height: (2_500.0, 3_500.0),
        in_cloud: (2_000.0, 4_000.0),
        branches: 4,
        step: 6.0,
        level: 0.35,
        cliff_hz: 650.0,
        edge: 0.15,
        infra: 0.03,
        infra_seconds: (0.25, 0.45),
        seconds: 16.0,
    },
];

fn kind(take: u8) -> &'static Kind {
    &KINDS[(take as usize).min(KINDS.len() - 1)]
}

/// The longest a clap of `kind` lasts at rate 1 (s): a plan spaces claps
/// with it.
pub fn clap_seconds(kind_number: u8) -> f32 {
    kind(kind_number).seconds
}

/// What the ground and the air just above it leave of sound that comes in
/// low, as a share: sound from low on a channel grazes the ground the whole
/// way and loses up to [`GROUND_LOSS_DB`] over kilometres, while sound from
/// high up comes down steeply and keeps it all. That is why a strike a few
/// kilometres off opens without a bang, and its rumble holds.
fn ground_effect(from: Vec3, distance: f32) -> f32 {
    let elevation = (from.z / distance).clamp(0.0, 1.0).asin();
    let grazing = 1.0 - (elevation / GROUND_LOSS_BELOW).min(1.0);
    let far = (distance / GROUND_LOSS_FULL_FROM).min(1.0);
    10f32.powf(-GROUND_LOSS_DB * grazing * far / 20.0)
}

/// The frequency at which air on `distance` metres takes 3 dB off (Hz).
fn absorbed_above(distance: f32) -> f32 {
    1_000.0 * (3.0 / (ABSORPTION_AT_A_KILOHERTZ * distance.max(1.0))).sqrt()
}

/// One segment's pulse, as the listener receives it.
#[derive(Clone, Copy, Debug, Default)]
struct Pulse {
    /// When it arrives, from the clap's first arrival (s).
    at: f32,
    /// Its peak, + then −, as an N-wave.
    amplitude: f32,
    /// How long the N lasts (s).
    seconds: f32,
    /// Where the air on its way starts taking its highs (Hz).
    cutoff: f32,
}

impl Pulse {
    /// How long it is still heard after it arrives: the N, then its filter
    /// ringing out (s).
    fn lifetime(&self) -> f32 {
        self.seconds + 6.0 / (TAU * self.cutoff)
    }
}

/// A pulse being heard: which one, how far into it, and its two filter stages.
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
struct Clap {
    pulses: Vec<Pulse>,
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
    kind: u8,
    active: bool,
    /// 1 while it plays; falls to 0 when a newer clap takes its place.
    steal_gain: f32,
    releasing: bool,
    /// Order of starting, to find the oldest.
    started: u64,
}

impl Clap {
    fn new(seed: u64) -> Self {
        Self {
            pulses: Vec::with_capacity(MAX_PULSES),
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

    fn start(&mut self, utterance: &Utterance, started: u64, random: &mut Rng) {
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
        build_channel(kind(self.kind), random, &mut self.pulses);
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

    fn release(&mut self) {
        self.releasing = true;
    }

    fn next_sample(&mut self, sample_rate: f32) -> f32 {
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
            sum += sounding.third;
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
        let coloured =
            self.colour
                .process(roar + kind(self.kind).edge * sum, self.elapsed, sample_rate);
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
        let white = self.noise.next_bipolar();
        self.tilt += tilt * (white - self.tilt);
        self.air_first += air * (self.tilt - self.air_first);
        self.air_second += air * (self.air_first - self.air_second);
        self.floor += floor * (self.air_second - self.floor);
        // White noise through the tilt keeps a share tilt / (2 − tilt) of its
        // power: scale it back to steady power, so `loudness` is its level.
        let steady = ((2.0 - tilt) / tilt).sqrt() * WHITE_TO_UNIT;
        (self.air_second - self.floor) * steady * loudness
    }
}

/// The colour every clap of thunder has, measured on a recording a few
/// kilometres off: little below about 90 Hz, a boom around 200 Hz where most of
/// its energy sits, and a cliff above which almost nothing is left. The boom's
/// pitch differs from flash to flash and drifts slowly through each.
struct Colour {
    low_cut: Biquad,
    boom: Biquad,
    cliff_first: Biquad,
    cliff_second: Biquad,
    boom_hz: f32,
    drift_rate: f32,
    drift_phase: f32,
    cliff_hz: f32,
    /// The output rate the filters are set for; 0 until a clap starts.
    sample_rate: f32,
    /// When the boom was last moved (s into the clap).
    moved_at: f32,
}

impl Colour {
    fn new() -> Self {
        Self {
            low_cut: Biquad::unity(),
            boom: Biquad::unity(),
            cliff_first: Biquad::unity(),
            cliff_second: Biquad::unity(),
            boom_hz: BOOM_HZ.0,
            drift_rate: 0.0,
            drift_phase: 0.0,
            cliff_hz: 1_000.0,
            sample_rate: 0.0,
            moved_at: 0.0,
        }
    }

    fn start(&mut self, kind: &Kind, random: &mut Rng) {
        self.boom_hz = BOOM_HZ.0 + (BOOM_HZ.1 - BOOM_HZ.0) * random.next_f32();
        self.drift_rate = BOOM_DRIFT_HZ.0 + (BOOM_DRIFT_HZ.1 - BOOM_DRIFT_HZ.0) * random.next_f32();
        self.drift_phase = random.next_f32() * TAU;
        self.cliff_hz = kind.cliff_hz;
        // Set afresh on the first sample, when the output rate is known.
        self.sample_rate = 0.0;
        self.moved_at = f32::NEG_INFINITY;
        self.low_cut.reset();
        self.boom.reset();
        self.cliff_first.reset();
        self.cliff_second.reset();
    }

    fn process(&mut self, sample: f32, elapsed: f32, sample_rate: f32) -> f32 {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.low_cut = Biquad::highpass(LOW_CUT_HZ, sample_rate);
            self.cliff_first = Biquad::lowpass(self.cliff_hz, sample_rate);
            self.cliff_second = Biquad::lowpass(self.cliff_hz, sample_rate);
            self.moved_at = f32::NEG_INFINITY;
        }
        if elapsed - self.moved_at >= MOMENT_SECONDS {
            let drift =
                1.0 + BOOM_DRIFT * (TAU * self.drift_rate * elapsed + self.drift_phase).sin();
            self.boom
                .set_peak(self.boom_hz * drift, BOOM_DB, BOOM_Q, sample_rate);
            self.moved_at = elapsed;
        }
        let shaped = self.boom.process(self.low_cut.process(sample));
        self.cliff_second.process(self.cliff_first.process(shaped))
    }
}

/// One stretch of a clap: how loud its arriving channel is, and how bright the
/// air leaves it.
#[derive(Clone, Copy, Debug)]
struct Moment {
    loudness: f32,
    cutoff: f32,
}

/// How the channel's arrivals shape the roar: their energy and brightness,
/// gathered per [`MOMENT_SECONDS`] and gently smoothed, at the kind's level.
/// The swells, the holding and the darkening of a rumble come from here.
fn shape_the_roar(kind: &Kind, pulses: &[Pulse], moments: &mut Vec<Moment>) {
    moments.clear();
    let Some(last) = pulses.last() else {
        return;
    };
    let count = ((last.at / MOMENT_SECONDS) as usize + 2).min(MAX_MOMENTS);
    for _ in 0..count {
        moments.push(Moment {
            loudness: 0.0,
            cutoff: 0.0,
        });
    }
    // Energy and an energy-weighted brightness per moment.
    for pulse in pulses {
        let index = (pulse.at / MOMENT_SECONDS) as usize;
        if let Some(moment) = moments.get_mut(index) {
            let heard = pulse.amplitude * (TAU * pulse.cutoff * pulse.seconds).min(1.0);
            let power = heard * heard;
            moment.loudness += power;
            moment.cutoff += power * pulse.cutoff;
        }
    }
    let mut last_cutoff = absorbed_above(kind.distance.0);
    for moment in moments.iter_mut() {
        if moment.loudness > 0.0 {
            moment.cutoff /= moment.loudness;
            last_cutoff = moment.cutoff;
        } else {
            moment.cutoff = last_cutoff;
        }
    }
    // Smoothed over a few moments, then as loudness.
    let mut previous = 0.0;
    for index in 0..moments.len() {
        let here = moments[index].loudness;
        let next = moments.get(index + 1).map_or(0.0, |moment| moment.loudness);
        moments[index].loudness = ((previous + 2.0 * here + next) / 4.0).sqrt();
        previous = here;
    }
    let loudest = moments
        .iter()
        .fold(0.0f32, |loudest, moment| loudest.max(moment.loudness));
    let scale = kind.level * ROAR_LEVEL / loudest.max(1e-9);
    for moment in moments.iter_mut() {
        moment.loudness *= scale;
    }
}

/// A direction turned at random by about [`MEAN_TURN`], then pulled toward
/// `pull` by `pull_strength`, so a channel wanders but keeps its course.
fn turned(direction: Vec3, pull: Vec3, pull_strength: f32, random: &mut Rng) -> Vec3 {
    // A normal deviate from three uniform ones, scaled so the mean turn is MEAN_TURN.
    let spread = |random: &mut Rng| {
        (random.next_bipolar() + random.next_bipolar() + random.next_bipolar()) * MEAN_TURN * 0.72
    };
    let jitter = Vec3::new(spread(random), spread(random), spread(random));
    (direction + jitter + pull * pull_strength).normalize()
}

/// Walk `length` metres of channel from `start` along `direction`, adding a
/// pulse for each segment, with `strength` for its loudness. Returns where it
/// ended up and which way it was heading.
#[allow(clippy::too_many_arguments)]
fn walk(
    start: Vec3,
    mut direction: Vec3,
    length: f32,
    step: f32,
    pull: Vec3,
    pull_strength: f32,
    strength: f32,
    random: &mut Rng,
    pulses: &mut Vec<Pulse>,
) -> (Vec3, Vec3) {
    let mut point = start;
    let mut walked = 0.0;
    while walked < length && pulses.len() + MICRO_PULSES < MAX_PULSES - 1 {
        direction = turned(direction, pull, pull_strength, random);
        let next = point + direction * step;
        let next = Vec3::new(next.x, next.y, next.z.max(0.0));
        let middle = (point + next) * 0.5;
        let distance = middle.length().max(1.0);
        // Side-on to the listener, a segment's pulse is strongest.
        let towards_listener = middle * (-1.0 / distance);
        let along = direction.dot(towards_listener).clamp(-1.0, 1.0);
        let side_on = 1.0 - along * along;
        let loudness = strength * step * ((1.0 - SIDE_ON) + SIDE_ON * side_on * side_on) / distance
            * ground_effect(middle, distance);
        // Pulses lengthen with distance, and vary with the segment's energy.
        let seconds = PULSE_SECONDS_AT_A_KILOMETRE
            * (distance / 1_000.0).powf(0.25)
            * (0.6 + 0.8 * random.next_f32());
        // A channel is crooked at every scale, down to centimetres: each
        // segment sounds as several pulses spread over the time its own
        // length takes to pass, which together make a roar, not a click.
        let spread = step * along.abs() / SPEED_OF_SOUND + MICRO_JITTER_SECONDS;
        for _ in 0..MICRO_PULSES {
            pulses.push(Pulse {
                at: distance / SPEED_OF_SOUND + spread * random.next_bipolar(),
                amplitude: loudness * (0.5 + random.next_f32()) / (MICRO_PULSES as f32).sqrt(),
                seconds: seconds * (0.7 + 0.6 * random.next_f32()),
                cutoff: absorbed_above(distance),
            });
        }
        point = next;
        walked += step;
    }
    (point, direction)
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

fn between((low, high): (f32, f32), random: &mut Rng) -> f32 {
    low + (high - low) * random.next_f32()
}

/// Build one flash of `kind` with the listener at the origin, as pulses in
/// arrival order, the first at 0, its loudest moment at the kind's level.
fn build_channel(kind: &Kind, random: &mut Rng, pulses: &mut Vec<Pulse>) {
    pulses.clear();
    let bearing = random.next_f32() * TAU;
    let distance = between(kind.distance, random);
    let length = between(kind.length, random);
    let height = between(kind.height, random);
    let toward = Vec3::new(bearing.cos(), bearing.sin(), 0.0);
    let up = Vec3::new(0.0, 0.0, 1.0);
    let mut branch_starts = [Vec3::ZERO; 8];
    if kind.ground {
        // From the ground point up to the cloud, climbing as it wanders. A
        // channel that reaches the cloud early leaves its last branch points
        // at the top.
        let ground = toward * distance;
        let mut direction = turned(up, up, 0.0, random);
        let mut point = ground;
        let pieces = branch_starts.len();
        let mut filled = 0;
        while filled < pieces && point.z < height {
            (point, direction) = walk(
                point,
                direction,
                length / pieces as f32,
                kind.step,
                up,
                0.35,
                1.0,
                random,
                pulses,
            );
            branch_starts[filled] = point;
            filled += 1;
        }
        for start in branch_starts.iter_mut().skip(filled) {
            *start = point;
        }
        // On inside the cloud, level and wandering, away from the listener
        // more often than not.
        let in_cloud = between(kind.in_cloud, random);
        if in_cloud > 0.0 {
            let off = bearing + (random.next_f32() - 0.5) * 2.0 * MAX_FLASH_ANGLE;
            let course = Vec3::new(off.cos(), off.sin(), 0.0);
            walk(
                point,
                course,
                in_cloud,
                kind.step,
                course,
                0.08,
                IN_CLOUD_STRENGTH,
                random,
                pulses,
            );
        }
    } else {
        // A cloud flash starts nearest at `distance` and runs off through the
        // cloud, mostly away from the listener: its far end arrives many
        // seconds after its near one, which is what makes it roll.
        let nearest = toward * distance + up * height;
        let turn_off_radial = (random.next_f32() - 0.5) * 2.0 * MAX_FLASH_ANGLE;
        let course = Vec3::new(
            (bearing + turn_off_radial).cos(),
            (bearing + turn_off_radial).sin(),
            0.0,
        );
        let mut direction = course;
        let mut point = nearest;
        let pieces = branch_starts.len();
        for start in branch_starts.iter_mut() {
            (point, direction) = walk(
                point,
                direction,
                length / pieces as f32,
                kind.step,
                course,
                0.08,
                1.0,
                random,
                pulses,
            );
            *start = point;
        }
    }
    // Branches: thinner, shorter, reaching down and out from points on the channel.
    for _ in 0..kind.branches {
        let start = branch_starts
            [(random.next_f32() * branch_starts.len() as f32) as usize % branch_starts.len()];
        let out = Vec3::new(random.next_bipolar(), random.next_bipolar(), -0.6).normalize();
        let branch_length = 150.0 + 300.0 * random.next_f32();
        walk(
            start,
            out,
            branch_length,
            kind.step,
            out,
            0.2,
            BRANCH_STRENGTH,
            random,
            pulses,
        );
    }

    if pulses.is_empty() {
        return;
    }
    pulses.sort_unstable_by(|left, right| left.at.total_cmp(&right.at));
    let first = pulses[0].at;
    for pulse in pulses.iter_mut() {
        pulse.at -= first;
    }

    // What arrives after the kind's length is not kept, and the clap's own
    // last stretch fades, as the far end of a channel thins out.
    pulses.retain(|pulse| pulse.at + pulse.lifetime() < kind.seconds);
    let last = pulses.last().map_or(0.0, |pulse| pulse.at);
    let fade_from = last * (1.0 - FADE_SHARE);
    let fade_length = (last * FADE_SHARE).max(1e-3);
    for pulse in pulses.iter_mut() {
        if pulse.at > fade_from {
            pulse.amplitude *= 1.0 - ((pulse.at - fade_from) / fade_length).min(1.0);
        }
    }

    // Its loudest moment, from the energy of what arrives together: pulses
    // arriving within a few milliseconds of each other mostly do not line up,
    // so their + and − halves partly cancel, and adding their heights would
    // overstate it many times. Each counts as loud as the air leaves it: a
    // pulse much shorter than its filter's time is smoothed down.
    let heard = |pulse: &Pulse| {
        let height = pulse.amplitude * (TAU * pulse.cutoff * pulse.seconds).min(1.0);
        height * height
    };
    let mut loudest = 0.0f32;
    let mut window_start = 0;
    let mut window_energy = 0.0f32;
    for index in 0..pulses.len() {
        window_energy += heard(&pulses[index]);
        while pulses[index].at - pulses[window_start].at > LOUDEST_WINDOW_SECONDS {
            window_energy -= heard(&pulses[window_start]);
            window_start += 1;
        }
        loudest = loudest.max(window_energy.max(0.0).sqrt());
    }
    let scale = kind.level / loudest.max(1e-9);
    for pulse in pulses.iter_mut() {
        pulse.amplitude *= scale;
    }

    // The infrasonic swell, just after the first arrival, under the whole clap.
    if pulses.len() < MAX_PULSES {
        let swell = Pulse {
            at: 0.05,
            amplitude: kind.level * kind.infra,
            seconds: between(kind.infra_seconds, random),
            cutoff: 20.0,
        };
        if swell.at + swell.lifetime() < kind.seconds {
            let at = pulses.partition_point(|pulse| pulse.at <= swell.at);
            pulses.insert(at, swell);
        }
    }
}

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
mod tests {
    use super::*;
    use crate::synth::test_support::{averaged_power_spectrum, band_power, render};

    const SAMPLE_RATE: f32 = 48_000.0;
    const KIND_NUMBERS: [u8; 4] = [KIND_CRACK, KIND_CLAP, KIND_ROLLING, KIND_DISTANT];

    fn strike(take: u8, at_sample: u64) -> Utterance {
        Utterance {
            take,
            at_sample,
            gain: 1.0,
            rate: 1.0,
            skip_seconds: 0.0,
        }
    }

    fn one_clap(take: u8, seed: u64) -> Vec<f32> {
        let mut thunder = ThunderSource::new(Vec3::ZERO, seed);
        thunder.schedule(strike(take, 0));
        render(clap_seconds(take) + 1.0, SAMPLE_RATE, |rate| {
            thunder.next_sample(rate)
        })
    }

    fn energy(samples: &[f32]) -> f64 {
        samples.iter().map(|sample| (*sample as f64).powi(2)).sum()
    }

    /// The share of a stretch's energy above `hertz`.
    fn share_above(samples: &[f32], hertz: f32) -> f64 {
        let spectrum = averaged_power_spectrum(samples);
        band_power(&spectrum, SAMPLE_RATE, hertz, 20_000.0)
            / band_power(&spectrum, SAMPLE_RATE, 1.0, 20_000.0)
    }

    #[test]
    fn it_is_silent_until_told_to_strike() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 1);
        let samples = render(2.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert!(samples.iter().all(|sample| *sample == 0.0));
        assert_eq!(thunder.sounding_take(), None);
    }

    #[test]
    fn a_strike_sounds_at_its_sample_and_ends_within_its_clap_length() {
        for take in KIND_NUMBERS {
            let mut thunder = ThunderSource::new(Vec3::ZERO, 5 + take as u64);
            let at = 4_800;
            thunder.schedule(strike(take, at));
            let samples = render(clap_seconds(take) + 0.3, SAMPLE_RATE, |rate| {
                thunder.next_sample(rate)
            });
            assert!(
                samples[..at as usize].iter().all(|sample| *sample == 0.0),
                "kind {take}: silent before"
            );
            assert!(
                energy(&samples[at as usize..at as usize + 9_600]) > 0.0,
                "kind {take}: heard at once"
            );
            assert_eq!(
                thunder.sounding_take(),
                None,
                "kind {take}: over within its length"
            );
        }
    }

    #[test]
    fn rate_two_ends_in_half_the_time() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 3);
        thunder.schedule(Utterance {
            rate: 2.0,
            ..strike(KIND_ROLLING, 0)
        });
        render(
            clap_seconds(KIND_ROLLING) / 2.0 + 0.2,
            SAMPLE_RATE,
            |rate| thunder.next_sample(rate),
        );
        assert_eq!(thunder.sounding_take(), None);
    }

    #[test]
    fn appointments_follow_the_sessions_clock_in_time_order() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 4);
        thunder.set_clock(100_000);
        thunder.schedule(strike(KIND_DISTANT, 100_480));
        thunder.schedule(strike(KIND_CRACK, 0));
        thunder.next_sample(SAMPLE_RATE);
        assert_eq!(
            thunder.sounding_take(),
            Some(KIND_CRACK),
            "a moment already past is kept at once"
        );
        render(0.02, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert_eq!(
            thunder.sounding_take(),
            Some(KIND_DISTANT),
            "then the later one"
        );
    }

    #[test]
    fn at_most_two_claps_sound_at_once() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 8);
        for at in [0, 10, 20] {
            thunder.schedule(strike(KIND_ROLLING, at));
        }
        render(0.05, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert_eq!(
            thunder.sounding_claps(),
            2,
            "the third took the oldest's place"
        );
    }

    /// Near thunder keeps its crack, far thunder is a low grumble, and every
    /// clap darkens as it goes on: its later parts come from farther away.
    #[test]
    fn near_is_bright_far_is_dull_and_a_clap_darkens() {
        let crack = one_clap(KIND_CRACK, 2);
        let distant = one_clap(KIND_DISTANT, 2);
        assert!(share_above(&crack, 1_000.0) > 3.0 * share_above(&distant, 1_000.0));
        for seed in [3, 4, 5] {
            let clap = one_clap(KIND_CLAP, seed);
            let second = SAMPLE_RATE as usize;
            let early = &clap[..2 * second];
            let late = &clap[3 * second..7 * second];
            assert!(
                share_above(early, 400.0) > share_above(late, 400.0),
                "seed {seed}: the tail is darker"
            );
        }
    }

    /// Like the reference recording, a clap a few kilometres off has most of
    /// its energy low: centred well under 500 Hz, little above 2 kHz.
    #[test]
    fn a_clap_sounds_low_like_the_recording() {
        for seed in [6, 7] {
            let clap = one_clap(KIND_CLAP, seed);
            let spectrum = averaged_power_spectrum(&clap);
            let total = band_power(&spectrum, SAMPLE_RATE, 20.0, 20_000.0);
            let low = band_power(&spectrum, SAMPLE_RATE, 20.0, 500.0);
            assert!(
                low / total > 0.75,
                "seed {seed}: {:.2} of it under 500 Hz",
                low / total
            );
            assert!(share_above(&clap, 2_000.0) < 0.03);
        }
    }

    /// A cloud flash rolls on long after a crack is over.
    #[test]
    fn rolling_thunder_rolls_on_long_after_a_crack() {
        let second = SAMPLE_RATE as usize;
        for seed in [1, 2] {
            let crack = one_clap(KIND_CRACK, seed);
            let rolling = one_clap(KIND_ROLLING, seed);
            let tail = |samples: &[f32]| energy(&samples[6 * second..]) / energy(samples);
            assert!(tail(&rolling) > 4.0 * tail(&crack), "seed {seed}");
        }
    }

    /// The farther, the more of what arrives is the infrasonic swell.
    #[test]
    fn distant_thunder_is_more_infrasound_than_a_crack() {
        let infra = |samples: &[f32]| {
            let spectrum = averaged_power_spectrum(samples);
            band_power(&spectrum, SAMPLE_RATE, 1.0, 25.0)
                / band_power(&spectrum, SAMPLE_RATE, 1.0, 20_000.0)
        };
        assert!(infra(&one_clap(KIND_DISTANT, 3)) > 2.0 * infra(&one_clap(KIND_CRACK, 3)));
    }

    #[test]
    fn claps_come_from_the_seed_and_never_repeat() {
        assert_eq!(one_clap(KIND_CLAP, 11), one_clap(KIND_CLAP, 11));
        assert_ne!(one_clap(KIND_CLAP, 11), one_clap(KIND_CLAP, 12));
        let mut thunder = ThunderSource::new(Vec3::ZERO, 13);
        thunder.schedule(strike(KIND_CRACK, 0));
        let first = render(clap_seconds(KIND_CRACK), SAMPLE_RATE, |rate| {
            thunder.next_sample(rate)
        });
        thunder.schedule(strike(KIND_CRACK, 0));
        let second = render(clap_seconds(KIND_CRACK), SAMPLE_RATE, |rate| {
            thunder.next_sample(rate)
        });
        assert_ne!(first, second, "a new flash every time");
    }

    #[test]
    fn a_minute_of_thunder_is_finite() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 21);
        let mut at = 0;
        for (index, take) in KIND_NUMBERS.iter().cycle().take(12).enumerate() {
            thunder.schedule(Utterance {
                gain: 0.5 + index as f32 * 0.3,
                rate: 0.6 + (index % 4) as f32 * 0.5,
                ..strike(*take, at)
            });
            at += 220_000;
        }
        let samples = render(60.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert!(samples.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn every_kind_peaks_under_one_at_every_sample_rate() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            for take in KIND_NUMBERS {
                for seed in [1, 2, 3] {
                    let mut thunder = ThunderSource::new(Vec3::ZERO, seed);
                    thunder.schedule(strike(take, 0));
                    let samples = render(clap_seconds(take), sample_rate, |rate| {
                        thunder.next_sample(rate)
                    });
                    let peak = samples
                        .iter()
                        .fold(0f32, |peak, sample| peak.max(sample.abs()));
                    assert!(
                        peak < 1.0,
                        "kind {take} at {sample_rate} Hz, seed {seed}: {peak}"
                    );
                    assert!(peak > 0.05, "kind {take}: heard at all");
                }
            }
        }
    }

    #[test]
    fn master_gain_zero_silences_and_nan_is_ignored() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 2);
        thunder.set_synth_param(SynthParam::MasterGain, f32::NAN);
        assert_eq!(thunder.master_gain, 1.0);
        thunder.set_synth_param(SynthParam::MasterGain, 0.0);
        thunder.schedule(strike(KIND_CRACK, 0));
        let samples = render(1.0, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert!(samples.iter().all(|sample| *sample == 0.0));
    }

    /// Moved into the middle of a clap, it carries on from there instead of
    /// starting over.
    #[test]
    fn a_clap_can_start_partway() {
        let mut thunder = ThunderSource::new(Vec3::ZERO, 9);
        thunder.schedule(Utterance {
            skip_seconds: clap_seconds(KIND_ROLLING) - 0.5,
            ..strike(KIND_ROLLING, 0)
        });
        render(0.6, SAMPLE_RATE, |rate| thunder.next_sample(rate));
        assert_eq!(
            thunder.sounding_take(),
            None,
            "only its last half second was left"
        );
    }

    #[test]
    fn a_channel_fits_its_storage_and_its_length() {
        let mut random = Rng::new(3);
        let mut pulses = Vec::with_capacity(MAX_PULSES);
        for take in KIND_NUMBERS {
            for _ in 0..5 {
                build_channel(kind(take), &mut random, &mut pulses);
                assert!(
                    pulses.len() > 300 && pulses.len() <= MAX_PULSES,
                    "kind {take}: {} pulses",
                    pulses.len()
                );
                assert_eq!(pulses.capacity(), MAX_PULSES, "nothing grew");
                assert!(
                    pulses.windows(2).all(|pair| pair[0].at <= pair[1].at),
                    "in arrival order"
                );
                assert!(pulses
                    .iter()
                    .all(|pulse| pulse.at + pulse.lifetime() < clap_seconds(take)));
            }
        }
    }
}
/// Claps of every kind as WAV files, to listen to while tuning:
/// `THUNDER_OUT=target/thunder-samples cargo test --release --lib write_listening_samples`.
/// Without `THUNDER_OUT` it writes nothing.
#[cfg(test)]
mod listening_samples {
    use super::*;
    fn write_wav(path: &str, samples: &[f32], sample_rate: u32) {
        let mut bytes = Vec::new();
        let data_length = (samples.len() * 2) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_length.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes());
        }
        std::fs::write(path, bytes).expect("write");
    }
    #[test]
    fn write_listening_samples() {
        let Ok(directory) = std::env::var("THUNDER_OUT") else {
            return;
        };
        std::fs::create_dir_all(&directory).expect("directory");
        let names = ["1-crack", "2-clap", "3-rolling", "4-distant"];
        for take in 0..4u8 {
            for seed in 1..=3u64 {
                let mut thunder = ThunderSource::new(Vec3::ZERO, seed * 7);
                thunder.schedule(Utterance {
                    take,
                    at_sample: 4_800,
                    gain: 1.0,
                    rate: 1.0,
                    skip_seconds: 0.0,
                });
                let samples: Vec<f32> = (0..((clap_seconds(take) + 0.5) * 48_000.0) as usize)
                    .map(|_| thunder.next_sample(48_000.0))
                    .collect();
                // Each at the same peak, so they can be compared by ear.
                let peak = samples
                    .iter()
                    .fold(0f32, |peak, sample| peak.max(sample.abs()))
                    .max(1e-6);
                // Up to where it has died away to 50 dB under its peak.
                let end = samples
                    .iter()
                    .rposition(|sample| sample.abs() > peak * 0.003)
                    .map_or(samples.len(), |last| last + 4_800);
                let scaled: Vec<f32> = samples[..end.min(samples.len())]
                    .iter()
                    .map(|sample| sample * 0.8 / peak)
                    .collect();
                write_wav(
                    &format!("{directory}/{}-{seed}.wav", names[take as usize]),
                    &scaled,
                    48_000,
                );
            }
        }
    }
}
