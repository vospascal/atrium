//! Hearing a kind before it is placed, or one take of a sound on its own,
//! from the audition voice.

use atrium::config::read_preset;
use atrium::engine::edit::SceneEdit;
use atrium_world::{EntityId, Moves};

use super::edits::Edits;
use super::shape::ALL_ROUND;
use super::{lock, Engine, AUDITION_SLOT};
use crate::api::EngineError;

/// The loudest a preview of a kind plays (dB SPL at 1 m), however loud the
/// kind itself is.
const PREVIEW_LEVEL_LIMIT: f32 = 80.0;

#[uniffi::export]
impl Engine {
    /// Play `preset` just ahead of the listener, to hear it before placing
    /// it, or stop with `None`. One preset is auditioned at a time, and it is
    /// not part of the scene: `scene()` does not list it.
    ///
    /// Building the sound decodes its recording or measures its generator, so
    /// this call takes a moment. Keep it off a UI thread.
    pub fn audition(&self, preset: Option<String>) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        self.end_audition(&mut edits)?;
        let Some(preset) = preset else {
            return Ok(());
        };
        if self.audition_voice_taken() {
            return Err(EngineError::NoFreeVoice);
        }
        let id = EntityId::unused("audition", []);
        // Heard two metres away, a kind as loud as thunder would hurt: a
        // preview is never louder than a raised voice.
        let level = read_preset(&preset)
            .map_err(|error| EngineError::Scene(error.to_string()))?
            .level;
        let mut previewed = self.sound_ahead(preset, None, None);
        previewed.sound.level = Some(level.min(PREVIEW_LEVEL_LIMIT));
        let built = self
            .sounds
            .build(&id, &previewed, AUDITION_SLOT)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        edits.send(SceneEdit::AddSource {
            slot: AUDITION_SLOT as u16,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        // A kind that rests is told by the plan when to speak; heard on its
        // own it speaks at once.
        self.planner.preview_voice(AUDITION_SLOT, built.rhythm);
        edits.auditioned = true;
        Ok(())
    }

    /// Play take `take` of the sound in `slot` once, just ahead of the
    /// listener and all round, to hear it on its own; or stop with `None`.
    /// It plays from the audition voice, like `audition`, and stops any
    /// audition in progress. Telemetry reports the audition voice's take while
    /// it sounds and nothing once it has ended. In a scene whose sounds take
    /// every voice there is none to hear it from, and it is refused.
    ///
    /// Building it decodes the recording, so this takes a moment. Keep it off
    /// a UI thread.
    pub fn audition_take(&self, slot: u16, take: Option<u16>) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        self.end_audition(&mut edits)?;
        let Some(take) = take else {
            return Ok(());
        };
        if self.audition_voice_taken() {
            return Err(EngineError::NoFreeVoice);
        }
        let scene = lock(&self.scene);
        let Some(source) = scene
            .description
            .sources
            .iter()
            .find(|source| source.slot == slot)
        else {
            return Ok(());
        };
        let id =
            EntityId::new(&source.id).map_err(|error| EngineError::Scene(error.to_string()))?;
        let Some(mut entity) = scene
            .document
            .sounds
            .iter()
            .find(|(own, _)| **own == id)
            .map(|(_, entity)| entity.clone())
        else {
            return Ok(());
        };
        drop(scene);
        entity.place = self.place_ahead();
        entity.shape.width = Some(ALL_ROUND);
        entity.moves = Moves::Still;
        let built = self
            .sounds
            .build_take(&id, &entity, take as usize, AUDITION_SLOT)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        edits.send(SceneEdit::AddSource {
            slot: AUDITION_SLOT as u16,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        edits.auditioned = true;
        Ok(())
    }
}

impl Engine {
    /// Stop the audition in progress, if there is one, and the plan for its
    /// voice. With none, nothing is touched: the last voice may then hold one
    /// of the scene's own sounds, whose plan must stay.
    pub(super) fn end_audition(&self, edits: &mut Edits) -> Result<(), EngineError> {
        if edits.auditioned {
            edits.stop_audition()?;
            self.planner.remove_voice(AUDITION_SLOT);
        }
        Ok(())
    }

    /// Whether one of the scene's own sounds plays from the audition voice.
    pub(super) fn audition_voice_taken(&self) -> bool {
        lock(&self.scene)
            .description
            .sources
            .iter()
            .any(|source| source.slot as usize == AUDITION_SLOT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// In a scene whose sounds take all sixteen voices, the last one is the
    /// scene's: hearing a kind or a take is refused, and stopping an
    /// audition that never started leaves that sound's plan alone. Before,
    /// the play button on a take replaced the sixteenth sound outright.
    #[test]
    fn a_full_scene_keeps_its_last_voice() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let mut scene = atrium::config::SceneConfig::load("scenes/frog.yaml").expect("reads");
        let (id, frog) = scene.sounds.iter().next().expect("a frog");
        let (id, frog) = (id.to_string(), frog.clone());
        // Robins fill the other voices: their phrases last a second or two and
        // come every few seconds, so the plan always has one coming. A frog's
        // chorus lasts over a minute and is rarely in the plan's window.
        for copy in 2..=atrium_core::telemetry::MAX_SOURCES {
            let mut placed = frog.clone();
            placed.sound.preset = Some("sources/robin.yaml".into());
            placed.place.x += copy as f32;
            scene
                .sounds
                .push(
                    EntityId::new(format!("{id}-{copy}")).expect("an id"),
                    placed,
                )
                .expect("added");
        }
        let path = std::env::temp_dir().join(format!("atrium-full-{}.yaml", std::process::id()));
        let path = path.to_str().expect("utf-8").to_owned();
        scene.save(&path).expect("saved");
        let engine = Engine::new(path.clone()).expect("scene should play");
        let _ = std::fs::remove_file(&path);
        engine.set_volume(0.0).expect("volume");
        let last_is_planned = || {
            engine
                .planned(60.0)
                .iter()
                .any(|take| take.slot as usize == AUDITION_SLOT)
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !last_is_planned() {
            assert!(
                Instant::now() < deadline,
                "the last robin was never planned"
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        engine.audition(None).expect("nothing to stop");
        engine.audition_take(0, None).expect("nothing to stop");
        assert!(
            last_is_planned(),
            "stopping nothing dropped the last robin's plan"
        );
        assert!(matches!(
            engine.audition(Some("sources/frog.yaml".into())),
            Err(EngineError::NoFreeVoice)
        ));
        assert!(matches!(
            engine.audition_take(0, Some(0)),
            Err(EngineError::NoFreeVoice)
        ));
        assert!(
            last_is_planned(),
            "a refused audition dropped the last robin's plan"
        );
        let last = engine.scene().sources.into_iter().last().expect("sixteen");
        assert_eq!(last.slot as usize, AUDITION_SLOT);
        assert!(
            last.id.ends_with("-16"),
            "the sixteenth sound is still the scene's"
        );
    }

    /// Hearing a kind before placing it, and one of a sound's takes on its
    /// own: both are heard within a moment, even for a kind that rests
    /// between takes. At zero volume:
    /// `cargo test -p atrium-ffi -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "opens the default output device"]
    fn a_kind_and_a_take_are_heard_at_once_on_their_own() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/meadow-birds.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let heard_within = |seconds: u64| {
            let deadline = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < deadline {
                if let Some(telemetry) = engine.latest_telemetry() {
                    if telemetry.sources.iter().any(|source| {
                        source.slot as usize == AUDITION_SLOT && source.take.is_some()
                    }) {
                        return true;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        engine
            .audition(Some("sources/robin.yaml".into()))
            .expect("the robin builds");
        assert!(
            heard_within(2),
            "a kind that rests is heard at once when previewed"
        );
        engine.audition(None).expect("stops");

        let robin = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.id == "robin")
            .expect("a robin")
            .slot;
        engine.audition_take(robin, Some(2)).expect("one take");
        assert!(heard_within(2), "one take on its own is heard at once");
    }
}
