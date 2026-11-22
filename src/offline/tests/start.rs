//! `--start` lands where the timeline would: rendering from a moment is the
//! tail of a render from the beginning, as far as a seek allows.
//!
//! What a seek moves is the plan (`Plan::seek`): every voice decides again
//! from its beginning, so the takes after the moment are the takes a render
//! from 0 hears there, and a take sounding at the moment starts partway. What
//! it does not move is everything that plays without a plan: a generator
//! such as the meadow's wind plays on from its own beginning, the reverb and
//! the delay lines start empty, and the wavering of moving air starts from
//! its first state. So the tail of a whole render cannot be matched, and is
//! not tested; the birds alone can.

use atrium_core::probe::Probe;

use super::{on_headphones, peak, render, MEADOW};
use crate::config::SceneConfig;
use crate::offline::RenderSettings;

const START: f64 = 20.0;
const SECONDS: f64 = 10.0;
const CHANNELS: usize = 2;
const SEED: u64 = 1;

/// The birds alone: their direct sound and reflections, without the
/// meadow's wind (ambient), the late reverb, and the wavering of the air,
/// none of which a seek moves.
const BIRDS: Probe = Probe {
    ambient: false,
    reverb: false,
    moving_air: false,
    ..Probe::WHOLE
};

/// When the last take sounding at `START` ends.
fn takes_under_way_end() -> f64 {
    let mut scene = SceneConfig::load(MEADOW).expect("scene loads");
    scene.seed = Some(SEED);
    let plan = scene.build_unstarted().expect("scene builds").plan;
    plan.upcoming(START)
        .into_iter()
        .map(|(_, planned)| planned.at + planned.seconds)
        .filter(|end| *end > START)
        .fold(f64::NAN, f64::max)
}

/// The largest difference between `first` and `second` from `from` seconds
/// on, and the larger of their peaks there.
fn difference_from(first: &[f32], second: &[f32], from: f64) -> (f32, f32) {
    let skipped = (from * 48_000.0) as usize * CHANNELS;
    let (first, second) = (&first[skipped..], &second[skipped..]);
    let largest = first
        .iter()
        .zip(second)
        .fold(0f32, |largest, (first, second)| {
            largest.max((first - second).abs())
        });
    (largest, peak(first).max(peak(second)))
}

/// Ten seconds from 20 s are the last ten of thirty, for the birds: the
/// take under way at 20 s is heard from its middle, and once it has ended
/// (and its sound has crossed the meadow) every sample is the same within
/// float rounding.
///
/// While it sounds, the two differ: a seek starts the take at the sample the
/// moment falls on, but skips into it by the exact time, so it can sound up
/// to half a sample (10 µs) earlier or later than played through, and it
/// fades in over its first milliseconds so it does not click. Measured here:
/// up to 0.65 of the peak while it sounds, on the birds' treble, and 4e-7
/// once it has ended.
#[test]
fn a_render_from_a_moment_is_the_tail_of_one_from_the_beginning() {
    let under_way_until = takes_under_way_end();
    assert!(
        under_way_until.is_finite(),
        "no take is sounding at {START} s; pick a moment where one is"
    );
    let whole = render(
        MEADOW,
        &RenderSettings {
            probe: BIRDS,
            seed: Some(SEED),
            ..on_headphones(START + SECONDS)
        },
    );
    let from_start = render(
        MEADOW,
        &RenderSettings {
            probe: BIRDS,
            seed: Some(SEED),
            start: START,
            ..on_headphones(SECONDS)
        },
    );
    let tail = &whole.frames()[(START * 48_000.0) as usize * CHANNELS..];
    assert_eq!(tail.len(), from_start.frames().len());

    // The take under way is heard from its middle, as loud as played through.
    let first_second = 48_000 * CHANNELS;
    let energy = |samples: &[f32]| samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
    let level_db = 10.0
        * (energy(&from_start.frames()[..first_second]) / energy(&tail[..first_second])).log10();
    assert!(
        level_db.abs() < 0.5,
        "the take under way is {level_db:.2} dB off"
    );

    // Its sound has 15 m of air to cross, and the filters a moment to ring.
    let settled = under_way_until - START + 0.2;
    let (largest, loudest) = difference_from(tail, from_start.frames(), settled);
    let (while_under_way, _) = difference_from(tail, from_start.frames(), 0.0);
    eprintln!(
        "the take under way: {level_db:+.3} dB, differs by {:.2} of the peak until {settled:.2} s; \
         then by {:.1e}",
        while_under_way / loudest,
        largest / loudest
    );
    assert!(loudest > 1e-4, "no bird sings after {settled:.1} s");
    assert!(
        largest <= 1e-5 * loudest,
        "from {settled:.1} s the renders differ by {largest} (peak {loudest})"
    );
}
