// The lightning channel: a jagged walk of segments from the seed, each one a
// pulse with its own arrival time, level and darkening, plus the infrasonic
// swell under the clap.

use std::f32::consts::TAU;

use atrium_core::types::Vec3;

use super::kinds::Kind;
use crate::synth::noise::Rng;

/// Metres per second, at about 20 °C.
const SPEED_OF_SOUND: f32 = 343.0;

/// The most pulses one clap is made of: segments of channel and branches.
pub(super) const MAX_PULSES: usize = 16_000;

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

/// How loud a branch is next to the main channel: it carries less current.
const BRANCH_STRENGTH: f32 = 0.5;

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
pub(super) fn absorbed_above(distance: f32) -> f32 {
    1_000.0 * (3.0 / (ABSORPTION_AT_A_KILOHERTZ * distance.max(1.0))).sqrt()
}

/// One segment's pulse, as the listener receives it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Pulse {
    /// When it arrives, from the clap's first arrival (s).
    pub(super) at: f32,
    /// Its peak, + then −, as an N-wave.
    pub(super) amplitude: f32,
    /// How long the N lasts (s).
    pub(super) seconds: f32,
    /// Where the air on its way starts taking its highs (Hz).
    pub(super) cutoff: f32,
}

impl Pulse {
    /// How long it is still heard after it arrives: the N, then its filter
    /// ringing out (s).
    pub(super) fn lifetime(&self) -> f32 {
        self.seconds + 6.0 / (TAU * self.cutoff)
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

fn between((low, high): (f32, f32), random: &mut Rng) -> f32 {
    low + (high - low) * random.next_f32()
}

/// Build one flash of `kind` with the listener at the origin, as pulses in
/// arrival order, the first at 0, its loudest moment at the kind's level.
/// Returns which pulse is the infrasonic swell, which is heard in full
/// whatever a kind's edge is.
pub(super) fn build_channel(
    kind: &Kind,
    random: &mut Rng,
    pulses: &mut Vec<Pulse>,
) -> Option<usize> {
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
        return None;
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
            return Some(at);
        }
    }
    None
}
