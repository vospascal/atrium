//! The dry space a scene without an environment plays in: an impulse renders
//! as its direct sound alone, with no reflection and no reverb tail, in
//! every render mode; the same impulse in the glass atrium does not.
//!
//! What is compared is the whole render against the direct sound alone (the
//! probe with reflections and reverb off): a filter on the direct path (the
//! loudspeakers' bass management, the headphone equalisation) rings in both
//! alike, while a reflection or a tail is in the whole render only.

use atrium_core::commands::Command;
use atrium_core::probe::Probe;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

use crate::config::{SceneConfig, SceneKind};

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK_FRAMES: usize = 480;
/// The impulse sounds this far into the render, once every ramp has settled.
const FIRED_AT_SECONDS: f32 = 0.25;

/// One sample of 1 at a set moment, then silence, from a point.
struct Impulse {
    position: Vec3,
    at_sample: usize,
    played: usize,
}

impl SoundSource for Impulse {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        self.played += 1;
        if self.played - 1 == self.at_sample {
            1.0
        } else {
            0.0
        }
    }
    fn position(&self) -> Vec3 {
        self.position
    }
    fn tick(&mut self, _dt: f32) {}
}

/// Two seconds of the impulse two metres ahead of the listener in a
/// listening session in `environment` (none for the dry space), rendered in
/// `mode` with only what `probe` lets through.
fn response(environment: Option<&str>, mode: RenderMode, probe: Probe) -> Vec<f32> {
    let mut config = SceneConfig::empty(SceneKind::Listening);
    config.environment = environment.map(str::to_owned);
    let mut scene = config.build().expect("it builds").scene;
    let ahead = scene.listener.position + Vec3::new(0.0, 2.0, 0.0);
    scene.sources[0] = Box::new(Impulse {
        position: ahead,
        at_sample: (FIRED_AT_SECONDS * SAMPLE_RATE) as usize,
        played: 0,
    });
    let channels = scene.speaker_layout.total_channels();
    scene.sample_rate = SAMPLE_RATE;
    scene.active_pipeline = mode;
    scene.init_pipelines();
    let (mut commands, mut drained) = rtrb::RingBuffer::new(1);
    commands
        .push(Command::SetProbe { probe })
        .expect("room for the probe");
    scene.process_commands(&mut drained);
    let mut block = vec![0.0f32; BLOCK_FRAMES * channels];
    let mut response = Vec::new();
    for _ in 0..(2.0 * SAMPLE_RATE) as usize / BLOCK_FRAMES {
        scene.render(&mut block, channels);
        response.extend_from_slice(&block);
    }
    response
}

/// What reflections and reverb add to the impulse's direct sound in
/// `environment` and `mode`: the energy of the whole render less the direct
/// sound alone, in dB against the direct sound's; `None` when they add
/// nothing at all, the two renders being the same samples.
fn reflections_and_tail_db(environment: Option<&str>, mode: RenderMode) -> Option<f64> {
    let whole = response(environment, mode, Probe::WHOLE);
    let direct = response(
        environment,
        mode,
        Probe {
            reflections: false,
            reverb: false,
            ..Probe::WHOLE
        },
    );
    let direct_energy: f64 = direct.iter().map(|&sample| (sample as f64).powi(2)).sum();
    assert!(direct_energy > 0.0, "{mode:?}: the impulse is heard");
    let added: f64 = whole
        .iter()
        .zip(&direct)
        .map(|(&all, &alone)| (all as f64 - alone as f64).powi(2))
        .sum();
    (added > 0.0).then(|| 10.0 * (added / direct_energy).log10())
}

#[test]
fn an_impulse_in_the_dry_space_is_its_direct_sound_alone() {
    for mode in RenderMode::ALL {
        let dry = reflections_and_tail_db(None, mode);
        let atrium = reflections_and_tail_db(Some("environments/atrium.yaml"), mode);
        println!(
            "{mode:?}: reflections and tail against the direct sound, dry {dry:?}, glass atrium {atrium:?} dB"
        );
        assert_eq!(dry, None, "{mode:?}: the dry space adds something");
        let atrium = atrium.expect("the atrium adds reflections and a tail");
        assert!(
            atrium > -30.0,
            "{mode:?}: the atrium adds only {atrium:.1} dB"
        );
    }
}
