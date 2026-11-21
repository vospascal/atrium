//! Hearing a kind before it is placed, or one take of a sound on its own,
//! from the audition voice.

use atrium::config::read_preset;
use atrium::engine::edit::SceneEdit;
use atrium_world::{EntityId, Moves};

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
        edits.stop_audition()?;
        self.planner.remove_voice(AUDITION_SLOT);
        let Some(preset) = preset else {
            return Ok(());
        };
        let taken = lock(&self.scene)
            .description
            .sources
            .iter()
            .any(|source| source.slot as usize == AUDITION_SLOT);
        if taken {
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
    /// it sounds and nothing once it has ended.
    ///
    /// Building it decodes the recording, so this takes a moment. Keep it off
    /// a UI thread.
    pub fn audition_take(&self, slot: u16, take: Option<u16>) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        edits.stop_audition()?;
        self.planner.remove_voice(AUDITION_SLOT);
        let Some(take) = take else {
            return Ok(());
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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
