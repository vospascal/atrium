//! A listening session rendered as `Rendering` renders it: no weather story
//! runs, whatever its file keeps, so for ten minutes every condition stays at
//! its default, the air stays still, and a sound that follows the rain holds
//! the setting the default gives it.

use atrium_core::commands::SynthParam;

use crate::config::{SceneConfig, SceneKind};
use crate::plan::{Scheduled, SteppedPlanner, Weather};

const SCENE: &str = "scenes/listening-session-test.yaml";
const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK_FRAMES: usize = 512;
const SECONDS: f64 = 600.0;
/// An hour of weather a minute: ten hours of it in the render, were any to
/// run.
const WEATHER_SPEED: f64 = 60.0;

/// What the plan handed the audio thread over the render.
#[derive(Debug, Default)]
struct Handed {
    /// Every `Air`: wind, daylight and bearing.
    air: Vec<(f32, f32, f32)>,
    /// Every setting sent to a generator.
    settings: Vec<(u16, SynthParam, f32)>,
}

#[test]
fn a_listening_session_runs_no_weather_for_ten_minutes() {
    let mut config = SceneConfig::load(SCENE).unwrap_or_else(|error| panic!("{SCENE}: {error}"));
    config.seed = Some(1);
    assert_eq!(config.kind, SceneKind::Listening);
    assert!(config.weather.is_some(), "its file keeps a passing storm");
    let built = config.build_unstarted().expect("it builds");
    assert!(built.plan.weather().is_none(), "no weather story");

    // The plan as the stepped planner runs it, to read its conditions.
    let mut watched = built.plan.clone();
    watched.set_weather_speed(0.0, WEATHER_SPEED);
    let defaults = Weather::calm(0).at(0.0);

    // As `Rendering::prepare` does, with the plan's ring passed through here
    // on its way to the scene, to see what it hands over.
    let mut scene = built.scene;
    let channels = scene.speaker_layout.total_channels();
    scene.sample_rate = SAMPLE_RATE;
    scene.fit_to_output_device(channels);
    scene.init_pipelines();
    let (to_scene, from_here) = rtrb::RingBuffer::new(256);
    let mut to_scene = to_scene;
    scene.plan_in = Some(from_here);
    let (planned, mut handed_over) = rtrb::RingBuffer::new(256);
    let mut planner = SteppedPlanner::new(built.plan, planned, 0.0, WEATHER_SPEED);

    let total_frames = (SECONDS * SAMPLE_RATE as f64) as usize;
    let mut block = vec![0.0f32; BLOCK_FRAMES * channels];
    let mut handed = Handed::default();
    let mut moments = 0;
    let mut power = 0.0f64;
    let mut rendered = 0;
    while rendered < total_frames {
        let seconds = (rendered + BLOCK_FRAMES) as f64 / SAMPLE_RATE as f64;
        planner.hand_over(seconds, SAMPLE_RATE);
        while let Ok(scheduled) = handed_over.pop() {
            match scheduled {
                Scheduled::Air {
                    wind,
                    daylight,
                    wind_from,
                } => handed.air.push((wind, daylight, wind_from)),
                Scheduled::Set { slot, param, value } => handed.settings.push((slot, param, value)),
                Scheduled::Utter { .. } | Scheduled::Cancel { .. } => {}
            }
            to_scene.push(scheduled).expect("drained every block");
        }
        assert_eq!(
            watched.conditions_at(seconds),
            defaults,
            "the conditions moved at {seconds} s"
        );
        moments += 1;
        scene.render(&mut block, channels);
        power += block
            .iter()
            .map(|&sample| (sample as f64).powi(2))
            .sum::<f64>();
        rendered += BLOCK_FRAMES;
    }
    println!("listening session, {SECONDS} s at weather speed {WEATHER_SPEED}: {moments} blocks, conditions at their defaults in every one; handed over {handed:?}");
    assert_eq!(
        handed.air,
        [(0.0, 0.0, crate::plan::PREVAILING_WIND_FROM)],
        "the air is still, sent once"
    );
    // The rain follows the rain, from 0 to 1: at the default, none.
    assert_eq!(handed.settings, [(1, SynthParam::Intensity, 0.0)]);
    let rms = (power / (total_frames * channels) as f64).sqrt();
    assert!(rms > 1e-4, "the tone sounds throughout: RMS {rms}");
}
