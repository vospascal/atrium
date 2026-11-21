// The rumble body: how loud and how bright a clap's roar is, moment by
// moment, gathered from its channel's arrivals.

use std::f32::consts::TAU;

use super::channel::{absorbed_above, Pulse};
use super::kinds::Kind;

/// How long one moment of a clap's roar is (s): short enough to keep the
/// flicker of a rumble, long enough to hold many pulses.
pub(super) const MOMENT_SECONDS: f32 = 0.010;
/// The most moments a clap has: its longest kind, with room to spare.
pub(super) const MAX_MOMENTS: usize = 2_600;

/// The roar's loudest moment, as a share of the kind's level, before the
/// limit rounds the peaks of the noise.
const ROAR_LEVEL: f32 = 0.28;

/// One stretch of a clap: how loud its arriving channel is, and how bright the
/// air leaves it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Moment {
    pub(super) loudness: f32,
    pub(super) cutoff: f32,
}

/// How the channel's arrivals shape the roar: their energy and brightness,
/// gathered per [`MOMENT_SECONDS`] and gently smoothed, at the kind's level.
/// The swells, the holding and the darkening of a rumble come from here.
pub(super) fn shape_the_roar(kind: &Kind, pulses: &[Pulse], moments: &mut Vec<Moment>) {
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
