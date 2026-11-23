//! The place a scene plays in: an environment file, or the dry space.
//!
//! A place cannot change while a scene plays: its room's pipelines, its
//! loudspeakers and where its sounds stand in the world are all built with
//! it. So a new place builds the scene again, from its scene file, and plays
//! that in place of what played: the same sounds with every setting and
//! layer mix, the same seed, at the same moment and the same point of the
//! weather story, with what the listener chose (mutes, probe, volume, output,
//! pause and head tracking) carried over. The sound stops for the moment the
//! new scene takes to build and the device to open.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use atrium::config::SceneConfig;
use atrium_core::commands::Command;

use super::device::output_commands;
use super::edits::Edits;
use super::{lock, Engine, Live, Playing};
use crate::api::EngineError;

#[uniffi::export]
impl Engine {
    /// Play the scene in `space`, an environment file such as
    /// `environments/cathedral.yaml`, or with none in the dry space, where
    /// nothing reflects or rings. The scene file keeps it. The scene is built
    /// again there, which takes a moment, like `add_sound`, and the sound
    /// stops meanwhile; `scene()` reports the new place, and its sounds may
    /// play from other voices than before. A place that does not build is
    /// refused, and the scene plays on where it was.
    pub fn set_place(&self, space: Option<String>) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        let mut scene = lock(&self.scene);
        if scene.document.environment == space {
            return Ok(());
        }
        let mut document = scene.document.clone();
        document.environment = space;
        self.play_place(&mut edits, &mut scene, document)
    }
}

impl Engine {
    /// Build `document` and play it in place of the scene playing, carrying
    /// over the moment, the weather's and what belongs to the listener. It
    /// becomes the scene file only once it plays.
    pub(super) fn play_place(
        &self,
        edits: &mut Edits,
        scene: &mut Live,
        document: SceneConfig,
    ) -> Result<(), EngineError> {
        self.end_audition(edits)?;
        let earlier = self.planner();
        // The same seed, so every choice and the weather are what they would
        // have been; the scene file keeps whatever it said of it.
        let mut seeded = document.clone();
        seeded.seed = Some(earlier.seed());
        let mut built = seeded
            .build()
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        built.planner.carry_on_from(&earlier);

        // Mutes are kept by voice, and a sound may take another voice now.
        let muted_ids: BTreeSet<&str> = scene
            .description
            .sources
            .iter()
            .filter(|source| scene.muted.contains(&source.slot))
            .map(|source| source.id.as_str())
            .collect();
        let muted: BTreeSet<u16> = built
            .description
            .sources
            .iter()
            .filter(|source| muted_ids.contains(source.id.as_str()))
            .map(|source| source.slot)
            .collect();
        for &slot in &muted {
            built.scene.sources[slot as usize].set_muted(true);
        }
        let mut first = vec![Command::SetProbe { probe: scene.probe }];
        if let Some(gain) = scene.volume {
            first.push(Command::SetMasterGain { gain });
        }
        if self.paused.load(Ordering::Relaxed) {
            // Already faded out: it starts silent, its clock standing still.
            built.scene.paused = true;
            built.scene.pause_gain = 0.0;
        }

        let mut playing = Playing::start(built, &document, &first)?;
        // The listener's output, if this device still plays it.
        let output = scene
            .output
            .filter(|output| playing.outputs.contains(output));
        // The new ring holds only the first few commands: there is room.
        for command in output.into_iter().flat_map(output_commands) {
            let _ = playing.commands.push(command);
        }
        // From here nothing can fail, so the scene is never half moved.
        *lock(&self.commands) = playing.commands;
        *lock(&self.telemetry) = playing.telemetry;
        *lock(&self.analysis) = playing.analysis;
        *edits = playing.edits;
        *lock(&self.outputs) = playing.outputs;
        *lock(&self.sounds) = playing.sounds;
        *lock(&self.planner) = Arc::new(playing.planner);
        self.placement.set(playing.listener);
        // Dropping the old stream stops it, now that the new one plays.
        drop(std::mem::replace(&mut *lock(&self.output), playing.output));
        scene.description = playing.description;
        scene.document = document;
        scene.muted = muted;
        scene.output = output;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::rounded;
    use super::*;
    use crate::api::{SceneDescription, SceneKind, Story};

    fn at_the_repository_root() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
    }

    fn scratch(name: &str) -> String {
        let directory = std::env::temp_dir().join(format!("atrium-place-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory.join(name).to_str().expect("a path").to_owned()
    }

    /// The sounds as a UI shows them, by id and without their voices, which
    /// a new place may hand out again, each where it stands from the
    /// listener: the scene file places both in the place, and a place puts
    /// the scene where its own middle is.
    fn sounds_by_id(scene: &SceneDescription) -> Vec<crate::api::SourceDescription> {
        let mut sources = scene.sources.clone();
        for source in &mut sources {
            source.slot = 0;
            source.color = String::new();
            source.position.x = rounded(source.position.x - scene.listener.x, 1000.0);
            source.position.y = rounded(source.position.y - scene.listener.y, 1000.0);
        }
        sources.sort_by(|left, right| left.id.cmp(&right.id));
        sources
    }

    /// A passing storm with its sounds changed (a level, a layer's mix) moved
    /// from its place to the cathedral and then to the dry space: each time
    /// the description reports the new place, every sound comes back with
    /// the same settings and layer mixes, the weather is the same story at
    /// the same point, the paused clock is where it was, a muted sound stays
    /// muted and the scene file keeps the place.
    #[test]
    fn a_new_place_rebuilds_the_scene_with_the_same_sounds() {
        at_the_repository_root();
        let engine = Engine::new("scenes/passing-storm.yaml".into()).expect("it plays");
        engine.set_volume(0.0).expect("volume");
        let first = engine.scene().sources[0].clone();
        engine.set_source_level(first.slot, 61.5).expect("level");
        let layered = engine
            .scene()
            .sources
            .into_iter()
            .find(|source| source.layers.len() > 1)
            .expect("a sound with layers");
        let layer = layered.layers[0].key.clone();
        engine
            .set_source_layer(layered.slot, layer.clone(), -6.0, false, true)
            .expect("a layer's mix");
        engine.set_source_muted(first.slot, true).expect("mute");
        engine.set_weather_speed(60.0);
        engine.seek(1800.0);
        // Paused, the clock stands still, so the moment and the weather can be
        // compared exactly across the gap.
        engine.set_paused(true).expect("pause");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let before = engine.scene();
        let weather = engine.weather().expect("a storm");
        assert_eq!(weather.story, Story::PassingStorm);
        println!("from {:?}", before.space);

        for space in [Some("environments/cathedral.yaml".to_string()), None] {
            let time_before = engine.scene_time();
            let conditions_before = engine.conditions(0.0, 0.0, 1)[0];
            engine.remember("Place".into());
            engine.set_place(space.clone()).expect("it moves");
            let after = engine.scene();
            assert_eq!(after.space, space);
            assert_eq!(after.kind, SceneKind::Nature);
            assert_eq!(sounds_by_id(&after), sounds_by_id(&before));
            let moved = after
                .sources
                .iter()
                .find(|source| source.id == layered.id)
                .expect("still there");
            assert!(moved
                .layers
                .iter()
                .any(|kept| kept.key == layer && kept.level_db == -6.0 && kept.mute));
            assert_eq!(engine.weather(), Some(weather.clone()));
            assert_eq!(engine.weather_speed(), 60.0);
            let time_after = engine.scene_time();
            let conditions_after = engine.conditions(0.0, 0.0, 1)[0];
            println!(
                "to {space:?}: clock {time_before:.3} s → {time_after:.3} s; wind {:.5} → {:.5}, rain {:.5} → {:.5}",
                conditions_before.wind,
                conditions_after.wind,
                conditions_before.rain,
                conditions_after.rain,
            );
            assert!(engine.paused(), "still paused");
            assert!(
                (time_after - time_before).abs() < 0.01,
                "the clock carried on from where it stood"
            );
            for (was, is) in [
                (conditions_before.wind, conditions_after.wind),
                (conditions_before.rain, conditions_after.rain),
                (conditions_before.storm, conditions_after.storm),
                (conditions_before.daylight, conditions_after.daylight),
            ] {
                assert!((was - is).abs() < 1e-6, "the weather carried on");
            }
            let muted_slot = after
                .sources
                .iter()
                .find(|source| source.id == first.id)
                .expect("the muted sound")
                .slot;
            assert!(lock(&engine.scene).muted.contains(&muted_slot));
        }

        let path = scratch("storm-dry.yaml");
        engine
            .save_scene(path.clone(), String::new())
            .expect("it saves");
        let text = std::fs::read_to_string(&path).expect("it reads");
        assert!(!text.contains("environment:"), "{text}");
        assert!(!text.contains("seed:"), "the file keeps its own seed");
        drop(engine);
        let reopened = Engine::new(path).expect("it opens");
        reopened.set_volume(0.0).expect("volume");
        assert_eq!(reopened.scene().space, None);
        assert_eq!(sounds_by_id(&reopened.scene()), sounds_by_id(&before));
    }

    /// The place is a step: saved in the scene file, undone back to the
    /// place before, redone to the new one, as any edit is.
    #[test]
    fn a_place_is_saved_undone_and_redone() {
        at_the_repository_root();
        let engine = Engine::new_scene(SceneKind::Listening).expect("it plays");
        engine.set_volume(0.0).expect("volume");
        engine
            .add_sound("sources/creatures/frog.yaml".into(), None)
            .expect("a frog");
        assert_eq!(engine.scene().space, None, "a listening session starts dry");
        engine.remember("Place".into());
        engine
            .set_place(Some("environments/small_atrium.yaml".into()))
            .expect("into a room");
        let path = scratch("session-in-a-room.yaml");
        engine
            .save_scene(path.clone(), "In a room".into())
            .expect("it saves");
        let text = std::fs::read_to_string(&path).expect("it reads");
        assert!(
            text.contains("environment: environments/small_atrium.yaml"),
            "{text}"
        );
        let reopened = Engine::new(path).expect("it opens");
        reopened.set_volume(0.0).expect("volume");
        assert_eq!(
            reopened.scene().space.as_deref(),
            Some("environments/small_atrium.yaml")
        );
        assert_eq!(reopened.scene().kind, SceneKind::Listening);
        drop(reopened);

        assert_eq!(engine.history().undoable, ["Place"]);
        assert!(engine.undo().expect("undo"));
        assert_eq!(engine.scene().space, None);
        assert_eq!(engine.scene().sources.len(), 1, "the frog stays");
        assert!(engine.redo().expect("redo"));
        assert_eq!(
            engine.scene().space.as_deref(),
            Some("environments/small_atrium.yaml")
        );
        assert!(engine
            .set_place(Some("environments/nowhere.yaml".into()))
            .is_err());
        assert_eq!(
            engine.scene().space.as_deref(),
            Some("environments/small_atrium.yaml"),
            "a place that does not build leaves the scene where it was"
        );
    }

    /// One sample of 1 at a set moment, then silence, from a point: the
    /// impulse of `atrium`'s dry-space check (`config/build_tests/dry.rs`).
    struct Impulse {
        position: atrium_core::types::Vec3,
        at_sample: usize,
        played: usize,
    }

    impl atrium_core::source::SoundSource for Impulse {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            self.played += 1;
            if self.played - 1 == self.at_sample {
                1.0
            } else {
                0.0
            }
        }
        fn position(&self) -> atrium_core::types::Vec3 {
            self.position
        }
        fn tick(&mut self, _dt: f32) {}
    }

    /// What reflections and reverb add to an impulse 2 m ahead of the
    /// listener in the scene `document`, rendered in `mode`, as the dry-space
    /// check measures it: the energy of the whole render less the direct
    /// sound alone (the probe with reflections and reverb off), in dB against
    /// the direct sound's; `None` when the two are the same samples.
    fn reflections_and_tail_db(
        document: &SceneConfig,
        mode: atrium_core::speaker::RenderMode,
    ) -> Option<f64> {
        const SAMPLE_RATE: f32 = 48_000.0;
        const BLOCK_FRAMES: usize = 480;
        let response = |probe: atrium_core::probe::Probe| {
            let mut scene = document.clone().build().expect("it builds").scene;
            let ahead = scene.listener.position + atrium_core::types::Vec3::new(0.0, 2.0, 0.0);
            scene.sources[0] = Box::new(Impulse {
                position: ahead,
                at_sample: (0.25 * SAMPLE_RATE) as usize,
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
        };
        let whole = response(atrium_core::probe::Probe::WHOLE);
        let direct = response(atrium_core::probe::Probe {
            reflections: false,
            reverb: false,
            ..atrium_core::probe::Probe::WHOLE
        });
        let direct_energy: f64 = direct.iter().map(|&sample| (sample as f64).powi(2)).sum();
        assert!(direct_energy > 0.0, "{mode:?}: the impulse is heard");
        let added: f64 = whole
            .iter()
            .zip(&direct)
            .map(|(&all, &alone)| (all as f64 - alone as f64).powi(2))
            .sum();
        (added > 0.0).then(|| 10.0 * (added / direct_energy).log10())
    }

    /// A listening session moved into the glass atrium gains reflections and
    /// a tail; moved to Dry, the scene it keeps renders an impulse as its
    /// direct sound alone, in every render mode.
    #[test]
    fn switching_to_dry_renders_the_direct_sound_only() {
        at_the_repository_root();
        let engine = Engine::new_scene(SceneKind::Listening).expect("it plays");
        engine.set_volume(0.0).expect("volume");
        engine
            .set_place(Some("environments/atrium.yaml".into()))
            .expect("into the atrium");
        let in_the_atrium = lock(&engine.scene).document.clone();
        engine.set_place(None).expect("to Dry");
        let dry = lock(&engine.scene).document.clone();
        assert_eq!(dry.environment, None);
        for mode in atrium_core::speaker::RenderMode::ALL {
            let atrium = reflections_and_tail_db(&in_the_atrium, mode);
            let after = reflections_and_tail_db(&dry, mode);
            println!(
                "{mode:?}: reflections and tail against the direct sound, glass atrium {atrium:?} dB, then Dry {after:?}"
            );
            assert!(
                atrium.expect("the atrium adds reflections and a tail") > -30.0,
                "{mode:?}"
            );
            assert_eq!(after, None, "{mode:?}: Dry adds something");
        }
    }
}
