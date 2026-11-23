//! A sound's layers: how each is mixed, set while it plays.

use std::collections::BTreeMap;

use atrium::config::layer_setting;
use atrium_core::commands::Command;
use atrium_core::layers::LAYER_LEVEL_RANGE_DB;
use atrium_world::Layer;

use super::{lock, rounded, sound_in, Engine, Live};
use crate::api::EngineError;

#[uniffi::export]
impl Engine {
    /// Mix the layer `key` of the sound in `slot` (from its
    /// `SourceDescription::layers`): `level_db` against the layer as the
    /// generator makes it, from −60 to +12 dB, to a tenth of a dB; `solo` to
    /// hear it alone with any other soloed layer of the sound; `mute` to
    /// leave it out unless soloed. It glides there in about 10 ms, so it
    /// never clicks. `scene()` reports it, the scene file keeps it, and undo
    /// takes it back. A slot that holds no sound, or a level that is not a
    /// number, changes nothing; a key the sound has no layer for is an
    /// error.
    pub fn set_source_layer(
        &self,
        slot: u16,
        key: String,
        level_db: f32,
        solo: bool,
        mute: bool,
    ) -> Result<(), EngineError> {
        if !level_db.is_finite() {
            return Ok(());
        }
        let layer = Layer {
            level: rounded(
                level_db.clamp(LAYER_LEVEL_RANGE_DB.0, LAYER_LEVEL_RANGE_DB.1),
                10.0,
            ),
            solo,
            mute,
        };
        let mut scene = lock(&self.scene);
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        let Some(index) = source.layers.iter().position(|known| known.key == key) else {
            return Err(EngineError::Scene(format!(
                "{} has no layer `{key}`",
                source.name
            )));
        };
        if let Some(entity) = scene.entity_in(slot) {
            if layer.is_as_made() {
                entity.sound.layers.remove(&key);
            } else {
                entity.sound.layers.insert(key, layer);
            }
        }
        self.play_layer(&mut scene, slot, index, layer)
    }
}

impl Engine {
    /// Mix layer `index` of the sound in `slot` as `layer`: heard at once,
    /// and reported by `scene()`.
    fn play_layer(
        &self,
        scene: &mut Live,
        slot: u16,
        index: usize,
        layer: Layer,
    ) -> Result<(), EngineError> {
        self.command(Command::SetSourceLayer {
            index: slot,
            layer: index as u8,
            setting: layer_setting(&layer),
        })?;
        if let Some(shown) =
            sound_in(&mut scene.description, slot).and_then(|source| source.layers.get_mut(index))
        {
            let setting = layer_setting(&layer);
            shown.level_db = setting.level_db;
            shown.solo = setting.solo;
            shown.mute = setting.mute;
        }
        Ok(())
    }

    /// Mix every layer of the sound in `slot` as `layers` has it, and one it
    /// leaves out as made: what undo puts back.
    pub(super) fn play_layers(
        &self,
        scene: &mut Live,
        slot: u16,
        layers: &BTreeMap<String, Layer>,
    ) -> Result<(), EngineError> {
        let keys: Vec<String> = sound_in(&mut scene.description, slot)
            .map(|source| {
                source
                    .layers
                    .iter()
                    .map(|layer| layer.key.clone())
                    .collect()
            })
            .unwrap_or_default();
        for (index, key) in keys.iter().enumerate() {
            let wanted = layers.get(key).copied().unwrap_or_default();
            self.play_layer(scene, slot, index, wanted)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layers the scene file keeps for its first sound.
    fn kept_layers(engine: &Engine) -> BTreeMap<String, Layer> {
        let scene = lock(&engine.scene);
        let (_, beat) = scene.document.sounds.iter().next().expect("the beat");
        beat.sound.layers.clone()
    }

    /// A layer set while it plays is reported, kept in the scene file, and
    /// undone and redone; an unknown key is an error; a level is clamped
    /// and rounded to a tenth of a dB.
    #[test]
    fn a_layer_change_is_kept_undone_and_redone() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine =
            Engine::new("scenes/binaural-beat-test.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let beat = engine.scene().sources[0].clone();
        let shown = |engine: &Engine| {
            engine.scene().sources[0]
                .layers
                .iter()
                .map(|layer| (layer.name.clone(), layer.level_db, layer.solo, layer.mute))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown(&engine),
            [
                ("left ear 108 Hz".to_string(), 0.0, false, false),
                ("right ear 118 Hz".to_string(), 0.0, false, false),
            ]
        );
        assert!(engine
            .set_source_layer(beat.slot, "middle".into(), 0.0, false, false)
            .is_err());

        engine.remember("Mute right".into());
        engine
            .set_source_layer(beat.slot, "right".into(), 0.0, false, true)
            .expect("mute");
        engine.remember("Left".into());
        engine
            .set_source_layer(beat.slot, "left".into(), -6.04, false, false)
            .expect("level");
        let after = shown(&engine);
        assert_eq!(after[0].1, -6.0, "a tenth of a dB");
        assert!(after[1].3, "muted");
        let kept = kept_layers(&engine);
        assert_eq!(kept.len(), 2, "both kept: {kept:?}");

        assert!(engine.undo().expect("undo"));
        assert_eq!(shown(&engine)[0].1, 0.0);
        assert!(shown(&engine)[1].3);
        assert!(engine.undo().expect("undo"));
        assert!(!shown(&engine)[1].3, "as made again");
        assert!(engine.redo().expect("redo"));
        assert!(engine.redo().expect("redo"));
        assert_eq!(shown(&engine), after);

        // Brought back to as made, it leaves the file.
        engine
            .set_source_layer(beat.slot, "left".into(), 0.0, false, false)
            .expect("as made");
        let kept = kept_layers(&engine);
        assert_eq!(kept.keys().collect::<Vec<_>>(), ["right"]);
    }
}
