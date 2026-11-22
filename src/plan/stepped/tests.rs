//! The plan driven in step hands the audio thread the takes the live planner
//! hands it, for the same seed, from the start and from a moment moved to.

use std::sync::Arc;
use std::time::Duration;

use atrium_core::source::Utterance;

use super::SteppedPlanner;
use crate::config::SceneConfig;
use crate::plan::{Clock, Plan, Planner, Scheduled};

const SAMPLE_RATE: f32 = 48_000.0;
const SECONDS: f64 = 20.0;

/// The meadow's plan for `seed`, as a scene built from its file starts it.
fn meadow_plan(seed: u64) -> Plan {
    let mut scene = SceneConfig::load("scenes/meadow-birds.yaml").expect("scene loads");
    scene.seed = Some(seed);
    scene.build_unstarted().expect("scene builds").plan
}

/// Every utterance handed over, in order, for each slot. Handed over in a
/// different grouping, slots interleave differently; each slot's own order
/// is what a voice hears.
fn utterances_by_slot(
    handed_over: &mut rtrb::Consumer<Scheduled>,
    before_sample: u64,
) -> Vec<Vec<Utterance>> {
    let mut by_slot = vec![Vec::new(); atrium_core::telemetry::MAX_SOURCES];
    while let Ok(scheduled) = handed_over.pop() {
        if let Scheduled::Utter { slot, utterance } = scheduled {
            if utterance.at_sample < before_sample {
                by_slot[slot as usize].push(utterance);
            }
        }
    }
    by_slot
}

/// The live planner on its own thread, its clock moved forward by hand
/// `step` seconds at a time with a wake-up between steps, as an audio thread
/// moves it. `seek` moves the scene first, as the timeline does.
fn live(plan: Plan, seek: Option<f64>, step: f64) -> Vec<Vec<Utterance>> {
    let clock = Arc::new(Clock::default());
    let (to_audio, mut handed_over) = rtrb::RingBuffer::new(4096);
    let planner = Planner::start(plan, clock.clone(), to_audio);
    if let Some(seconds) = seek {
        planner.seek(seconds);
    }
    let step_frames = (step * SAMPLE_RATE as f64).round() as u64;
    let end_sample = (SECONDS * SAMPLE_RATE as f64) as u64;
    while clock.samples() < end_sample {
        clock.advance(step_frames, SAMPLE_RATE);
        // Long enough for the planner to wake at least once. A wake missed
        // under load only makes it take more at the next: what it decides
        // does not depend on when it looks.
        std::thread::sleep(Duration::from_millis(70));
    }
    std::thread::sleep(Duration::from_millis(150));
    drop(planner);
    utterances_by_slot(&mut handed_over, end_sample)
}

/// The same plan driven in step with `block`-frame blocks.
fn stepped(plan: Plan, start: f64, block: usize) -> Vec<Vec<Utterance>> {
    let (to_audio, mut handed_over) = rtrb::RingBuffer::new(4096);
    let mut planner = SteppedPlanner::new(plan, to_audio, start, 1.0);
    let end_sample = (SECONDS * SAMPLE_RATE as f64) as usize;
    let mut rendered = 0;
    while rendered < end_sample {
        rendered += block.min(end_sample - rendered);
        planner.hand_over(rendered as f64 / SAMPLE_RATE as f64, SAMPLE_RATE);
    }
    utterances_by_slot(&mut handed_over, end_sample as u64)
}

/// Stepped with a clock moved by hand, the live planner hands over exactly
/// the takes the stepped one does: the same take, sample, gain, rate and
/// skip, for every bird, whatever the block size. Moved to 20 s first, both
/// start the take under way there partway and go on alike.
#[test]
fn the_stepped_plan_utters_what_the_live_planner_utters() {
    let cases = [(3, None), (11, None), (3, Some(20.0)), (11, Some(20.0))];
    // The live planners wait on the wall clock; run them side by side.
    let live_runs: Vec<_> = cases
        .iter()
        .map(|&(seed, seek)| std::thread::spawn(move || live(meadow_plan(seed), seek, 0.3)))
        .collect();
    for ((seed, seek), live_run) in cases.into_iter().zip(live_runs) {
        let start = seek.unwrap_or(0.0);
        let in_step = stepped(meadow_plan(seed), start, 512);
        let uttered: usize = in_step.iter().map(Vec::len).sum();
        assert!(
            uttered >= 8,
            "seed {seed} from {start} s: only {uttered} takes in 20 s"
        );
        assert_eq!(
            live_run.join().expect("the live run"),
            in_step,
            "seed {seed} from {start} s: the live planner uttered otherwise"
        );
        assert_eq!(
            stepped(meadow_plan(seed), start, 37),
            in_step,
            "seed {seed} from {start} s: the block size changed what is uttered"
        );
    }
}
